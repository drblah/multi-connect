use tokio::sync::Mutex;
use std::time::Duration;
use log::{debug, error};
use tokio::sync::mpsc::error::TrySendError;
use crate::messages::Packet;
use crate::packet_queue::PacketQueue;

const YEAR: Duration = Duration::from_secs(31536000);


#[derive(Debug)]
pub struct PacketSorter {
    packet_queue: PacketQueue,
    pub next_seq: u64,

    deadline: Duration,
    deadline_timer: Mutex<tokio::time::Interval>,
    deadline_active: bool,
    sorted_packet_queue_tx: tokio::sync::mpsc::Sender<Packet>,
    sorted_packet_queue_rx: Mutex<tokio::sync::mpsc::Receiver<Packet>>
}

impl PacketSorter {
    pub fn new(deadline: Duration) -> Self {

        let (sorted_packet_queue_tx, sorted_packet_queue_rx) = tokio::sync::mpsc::channel(1000);

        let sorted_packet_queue_rx = Mutex::new(sorted_packet_queue_rx);

        PacketSorter {
            packet_queue: PacketQueue::new(),
            next_seq: 0,
            deadline,
            deadline_timer: Mutex::new( tokio::time::interval_at( tokio::time::Instant::now() + deadline, deadline) ),
            deadline_active: true,
            sorted_packet_queue_tx,
            sorted_packet_queue_rx
        }
    }

    pub async fn get_next_packet(&mut self) -> Option<Packet> {
        if let Some(entry) = self.packet_queue.first_entry() {
            if *entry.key() == self.next_seq {
                self.next_seq += 1;
                let pkt = self.packet_queue.pop_first().unwrap();

                let mut deadline_lock = self.deadline_timer.lock().await;


                if let Some(oldest_timestamp) = self.packet_queue.get_oldest_timestamp() {
                    let time_waited_in_queue = oldest_timestamp.elapsed();
                    *deadline_lock = tokio::time::interval_at( tokio::time::Instant::now() + self.deadline - time_waited_in_queue, self.deadline);
                    self.deadline_active = true;
                } else {
                    //*deadline_lock = tokio::time::interval_at( tokio::time::Instant::now() + self.deadline, self.deadline);
                    *deadline_lock = tokio::time::interval_at(tokio::time::Instant::now(), self.deadline); //
                    self.deadline_active = true;
                }

                return Some(pkt)
            }
        }

        None
    }

    pub async fn await_have_next_packet(&self) -> Option<Packet> {
        let mut sorted_packet_queue_rx_lock = self.sorted_packet_queue_rx.lock().await;

        sorted_packet_queue_rx_lock.recv().await
    }

    pub async fn insert_packet(&mut self, pkt: Packet) {
        let sequence_number = pkt.seq;
        if sequence_number >= self.next_seq {
            match self.packet_queue.last_entry() {
                Some(tail) => {
                    match sequence_number.checked_sub(*tail.key()) {
                        Some(diff) if diff > 100 => {
                            debug!("Large sequence jump detected. Clear packet queue and insert packet: from {} to {} - {}", *tail.key(), sequence_number, sequence_number - *tail.key());
                            self.packet_queue.insert(pkt);
                            self.advance_queue().await;
                        },
                        Some(_) => {
                            self.packet_queue.insert(pkt);
                        },
                        None => {
                            self.packet_queue.insert(pkt);
                        },
                    }
                },
                None => {
                    self.packet_queue.insert(pkt);
                },
            }

            // Check if we have one or more packets and move them to the sorted packet queue
            self.enqueue_sorted_packets().await;

            // Start deadline timer when we have new packets
            let mut deadline_timer_lock = self.deadline_timer.lock().await;
            if !self.deadline_active {
                *deadline_timer_lock = tokio::time::interval_at( tokio::time::Instant::now() + self.deadline, self.deadline);
                self.deadline_active = true;
            }
        }
    }

    async fn enqueue_sorted_packets(&mut self) {
        while let Some(packet) = self.get_next_packet().await {
            match self.sorted_packet_queue_tx.try_send(packet) {
                Ok(_) => {}
                Err(e) => {
                    match e {
                        TrySendError::Full(_) => {
                            error!("Packet sorter queue is full! Dropping packets!")
                        }
                        TrySendError::Closed(_) => {
                            error!("Packet sorter queue is closed!")
                        }
                    }
                }
            }
        }
    }

    pub async fn advance_queue(&mut self) {
        if !self.packet_queue.is_empty() {
            self.next_seq = *self.packet_queue.first_entry().unwrap().key();

            self.enqueue_sorted_packets().await;

            if self.packet_queue.is_empty() {
                let mut deadline_timer_lock = self.deadline_timer.lock().await;
                *deadline_timer_lock = tokio::time::interval_at( tokio::time::Instant::now() + YEAR, self.deadline); // TODO - This is a hack to disable the timer
                self.deadline_active = false;
            } else {
                // We have already checked if something is in the queue, so it is safe to unwrap here.
                let next_timestamp = self.packet_queue
                    .get_oldest_timestamp().unwrap();

                let time_waited_in_queue = next_timestamp.elapsed();
                // Fire instantly because the next packet has already waited long enough
                if time_waited_in_queue >= self.deadline {
                    let mut deadline_timer_lock = self.deadline_timer.lock().await;
                    *deadline_timer_lock = tokio::time::interval(self.deadline);
                    self.deadline_active = true;
                } else {


                    let mut deadline_timer_lock = self.deadline_timer.lock().await;
                    //deadline_timer_lock.set_after(self.deadline - time_waited_in_queue)
                    *deadline_timer_lock = tokio::time::interval_at( tokio::time::Instant::now() + self.deadline - time_waited_in_queue, self.deadline);
                    self.deadline_active = true;
                }

            }

        } else {
            // Disable deadline timer until we get the next packet
            let mut deadline_timer_lock = self.deadline_timer.lock().await;
            *deadline_timer_lock = tokio::time::interval_at( tokio::time::Instant::now() + YEAR, self.deadline); // TODO - This is a hack to disable the timer
        }
    }

    pub fn have_next_packet(&mut self) -> bool {
        match self.packet_queue.first_entry() {
            Some(pkt) => {
                *pkt.key() == self.next_seq
            }
            None => false
        }
    }

    pub async fn await_deadline(&self) {
        let mut deadline_lock = self.deadline_timer.lock().await;

        deadline_lock.tick().await;
    }

    pub fn set_deadline(&mut self, new_deadline: Duration) {
        self.deadline = new_deadline
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn sorter_handles_out_of_order_packets() {
        let mut sorter = PacketSorter::new(Duration::from_secs(1));
        let packet1 = Packet { seq: 0, id: 0, bytes: Vec::new() };
        let packet2 = Packet { seq: 1, id: 0, bytes: Vec::new() };

        sorter.insert_packet(packet2.clone()).await;
        sorter.insert_packet(packet1.clone()).await;

        let mut soter_rx_lock = sorter.sorted_packet_queue_rx.lock().await;

        let ordered1 = soter_rx_lock.recv().await.unwrap();
        let ordered2 = soter_rx_lock.recv().await.unwrap();

        assert_eq!(ordered1, packet1);
        assert_eq!(ordered2, packet2);
    }

    #[tokio::test]
    async fn sorter_clears_queue_on_large_sequence_jump() {
        let mut sorter = PacketSorter::new(Duration::from_secs(1));
        let packet1 = Packet { seq: 0, id: 0, bytes: Vec::new() };
        let packet11 = Packet { seq: 11, id: 0, bytes: Vec::new() };

        sorter.insert_packet(packet1.clone()).await;
        sorter.insert_packet(packet11.clone()).await;

        {
            let mut sorter_rx_lock = sorter.sorted_packet_queue_rx.lock().await;
            let first_packet = sorter_rx_lock.recv().await.unwrap();
            assert_eq!(first_packet, packet1);
        }

        // seq 11 should be in the btreehashmap but not yet considered "sorted". Therefore, we should not have next packet
        let should_be_false = sorter.have_next_packet();
        assert_eq!(should_be_false, false);

        // Wait for our deadline and advance queue
        sorter.await_deadline().await;
        sorter.advance_queue().await;


        // We should now have seq11
        let mut sorter_rx_lock = sorter.sorted_packet_queue_rx.lock().await;
        let eleventh_packet = sorter_rx_lock.recv().await.unwrap();
        assert_eq!(eleventh_packet, packet11);
        //assert_eq!(sorter.get_next_packet().await, Some(packet11));
        //assert_eq!(sorter.get_queue_length(), 0);
    }
}