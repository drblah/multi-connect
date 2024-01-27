use std::collections::BTreeMap;
use smol::lock::Mutex;
use std::time::{Duration};
use log::{debug, error};
use smol::channel::TrySendError;
use smol::stream::StreamExt;
use crate::messages::Packet;

#[derive(Debug)]
pub struct Sequencer {
    packet_queue: BTreeMap<u64, Packet>,
    pub next_seq: u64,

    // TODO: Expose this deadline as a user configuration
    deadline: Duration,
    deadline_timer: Mutex<smol::Timer>,
    sorted_packet_queue_tx: smol::channel::Sender<Packet>,
    sorted_packet_queue_rx: smol::channel::Receiver<Packet>
}

impl Sequencer {
    pub fn new(deadline: Duration) -> Self {

        let (sorted_packet_queue_tx, sorted_packet_queue_rx) = smol::channel::bounded(1000);

        Sequencer {
            packet_queue: BTreeMap::new(),
            next_seq: 0,
            deadline,
            deadline_timer: Mutex::new(smol::Timer::after(deadline)),
            sorted_packet_queue_tx,
            sorted_packet_queue_rx
        }
    }

    pub async fn get_next_packet(&mut self) -> Option<Packet> {
        if let Some(entry) = self.packet_queue.first_entry() {
            if *entry.key() == self.next_seq {
                self.next_seq += 1;
                let pkt = self.packet_queue.pop_first().unwrap().1;

                let mut deadline_lock = self.deadline_timer.lock().await;
                deadline_lock.set_after(self.deadline);

                return Some(pkt)
            }
        }

        None
    }

    pub async fn await_have_next_packet(&self) -> Option<Packet> {
        match self.sorted_packet_queue_rx.recv().await {
            Ok(packet) => Some(packet),
            Err(e) => {
                error!("Sequencer sorted packet queue closed!");
                None
            }
        }
    }

    pub async fn insert_packet(&mut self, pkt: Packet) {
        if pkt.seq >= self.next_seq {
            match self.packet_queue.last_entry() {
                Some(tail) => {
                    match pkt.seq.checked_sub(*tail.key()) {
                        Some(diff) if diff > 10 => {
                            debug!("Large sequence jump detected. Clear packet queue and insert packet: from {} to {} - {}", *tail.key(), pkt.seq, pkt.seq - *tail.key());
                            self.packet_queue.clear();
                            self.packet_queue.entry(pkt.seq)
                                .or_insert(pkt);
                            self.advance_queue().await;
                        },
                        Some(_) => {
                            self.packet_queue.entry(pkt.seq)
                                .or_insert(pkt);
                        },
                        None => {
                            self.packet_queue.entry(pkt.seq)
                                .or_insert(pkt);
                        },
                    }
                },
                None => {
                    self.packet_queue.entry(pkt.seq)
                        .or_insert(pkt);
                },
            }

            // Check if we have one or more packets and move them to the sorted packet queue
            self.enqueue_sorted_packets().await;

            // Start deadline timer when we have new packets
            let mut deadline_timer_lock = self.deadline_timer.lock().await;
            if !deadline_timer_lock.will_fire() {
                deadline_timer_lock.set_after(self.deadline)
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
                            error!("Sequencer sorted queue is full! Dropping packets!")
                        }
                        TrySendError::Closed(_) => {
                            error!("Sequencer sorted queue is closed!")
                        }
                    }
                }
            }
        }
    }

    pub async fn advance_queue(&mut self) {
        if !self.packet_queue.is_empty() {
            self.next_seq = *self.packet_queue.first_entry().unwrap().key();
        }

        self.enqueue_sorted_packets().await;

        // Disable deadline timer until we get the next packet
        let mut deadline_timer_lock = self.deadline_timer.lock().await;
        *deadline_timer_lock = smol::Timer::never();
    }

    pub fn get_queue_length(&self) -> usize {
        self.packet_queue.len()
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

        deadline_lock.next().await;
    }
}
/*
#[cfg(test)]
mod tests {
    use std::time::Duration;
    use crate::messages::Packet;
    use crate::sequencer::Sequencer;

    #[test]
    fn unordered_insert() {
        let mut sequencer = Sequencer::new(Duration::from_millis(1));
        let packets = vec![
            Packet{
                seq: 0,
                id: 0,
                bytes: Vec::new()
            },
            Packet {
                seq: 3,
                id: 0,
                bytes: Vec::new()
            },
            Packet{
                seq: 1,
                id: 0,
                bytes: Vec::new()
            },
            Packet{
                seq: 2,
                id: 0,
                bytes: Vec::new()
            }
        ];
        let expected = [0, 1, 2, 3];

        for packet in packets{
            sequencer.insert_packet(packet)
        }

        for expected_seq in expected {
            let pkt = sequencer.get_next_packet().unwrap();

            assert_eq!(pkt.seq, expected_seq)
        }
    }

    #[test]
    fn unordered_insert_duplicates() {
        let mut sequencer = Sequencer::new(Duration::from_millis(1));
        let packets = vec![
            Packet {
                seq: 0,
                id: 0,
                bytes: Vec::new(),
            },
            Packet {
                seq: 3,
                id: 0,
                bytes: Vec::new(),
            },
            Packet {
                seq: 3,
                id: 0,
                bytes: Vec::new(),
            },
            Packet {
                seq: 1,
                id: 0,
                bytes: Vec::new(),
            },
            Packet {
                seq: 2,
                id: 0,
                bytes: Vec::new(),
            },
        ];
        let expected = [0, 1, 2, 3];

        for packet in packets {
            sequencer.insert_packet(packet)
        }

        for expected_seq in expected {
            let pkt = sequencer.get_next_packet().unwrap();

            assert_eq!(pkt.seq, expected_seq)
        }

        assert_eq!(sequencer.get_queue_length(), 0);
    }
}

 */