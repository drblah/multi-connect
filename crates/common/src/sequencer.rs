use std::collections::BTreeMap;
use smol::lock::Mutex;
use std::time::{Duration};
use smol::stream::StreamExt;
use crate::messages::Packet;

#[derive(Debug)]
pub struct Sequencer {
    packet_queue: BTreeMap<u64, Packet>,
    pub next_seq: u64,
    deadline: Duration,
    deadline_timer: Mutex<smol::Timer>,
}

impl Sequencer {
    pub fn new(deadline: Duration) -> Self {
        Sequencer {
            packet_queue: BTreeMap::new(),
            next_seq: 0,
            deadline,
            deadline_timer: Mutex::new(smol::Timer::after(deadline)),
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

    pub fn insert_packet(&mut self, pkt: Packet) {
        if pkt.seq >= self.next_seq {
            self.packet_queue.entry(pkt.seq)
                .or_insert(pkt);
        }
    }

    pub fn advance_queue(&mut self) {
        if !self.packet_queue.is_empty() {
            self.next_seq = *self.packet_queue.first_entry().unwrap().key();
        }
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