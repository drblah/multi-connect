use std::collections::{BTreeMap, VecDeque};
use std::collections::btree_map::OccupiedEntry;
use tokio::time::Instant;
use crate::messages::Packet;

struct OrderInfo {
    seq: u64,
    timestamp: Instant
}

struct PacketQueue {
    seq_ordered: BTreeMap<u64, Packet>,
    insert_order: VecDeque<OrderInfo>
}

impl PacketQueue {
    pub fn new() -> Self {
        Self {
            seq_ordered: BTreeMap::new(),
            insert_order: VecDeque::new()
        }
    }
    
    pub fn insert(&mut self, packet: Packet) {
        self.seq_ordered.insert(packet.seq, packet.clone());
        self.insert_order.push_back(
            OrderInfo {
                seq: packet.seq,
                timestamp: Instant::now()
            }
        )
    }
    
    pub fn get_oldest_timestamp(&self) -> Option<Instant> {
        self.insert_order.front().map(|x| x.timestamp)
    }
    
    pub fn first_entry(&mut self) -> Option<OccupiedEntry<'_, u64, Packet>> {
        let entry = self.seq_ordered.first_entry();
        
        entry
    }

    pub fn last_entry(&mut self) -> Option<OccupiedEntry<'_, u64, Packet>> {
        let entry = self.seq_ordered.last_entry();

        entry
    }
    
    pub fn delete(&mut self, seq: u64) -> Option<Packet> {
        if let Some(packet) = self.seq_ordered.remove(&seq) {
            self.insert_order.retain(|x| x.seq != seq);
            
            Some(packet)
        } else {
            None
        }
    }
    
    pub fn pop_first(&mut self) -> Option<Packet> {
        if let Some(packet) = self.seq_ordered.pop_first() {
            self.insert_order.retain(|x| x.seq != packet.0);
            
            Some(packet.1)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn insert_ordering() {
        let mut queue = PacketQueue::new();
        
        let packet0 = Packet {
            seq: 0,
            id: 0,
            bytes: vec![],
        };

        let packet1 = Packet {
            seq: 1,
            id: 0,
            bytes: vec![],
        };

        let packet2 = Packet {
            seq: 2,
            id: 0,
            bytes: vec![],
        };
        
        queue.insert(packet1.clone());
        queue.insert(packet2.clone());
        queue.insert(packet0.clone());
        
        let expected_seq_order = vec![packet0.seq, packet1.seq, packet2.seq];
        let expected_insert_order = vec![packet1.seq, packet2.seq, packet0.seq];
        
        let seq_order: Vec<u64> = queue.seq_ordered.iter().map(|x| *x.0).collect();
        let insert_order: Vec<u64> = queue.insert_order.iter().map(|x| x.seq).collect();
        
        assert_eq!(expected_seq_order, seq_order);
        assert_eq!(expected_insert_order, insert_order);
    }
    
    #[tokio::test]
    async fn get_oldest_timestamp() {
        let mut queue = PacketQueue::new();

        let packet0 = Packet {
            seq: 0,
            id: 0,
            bytes: vec![],
        };

        let packet1 = Packet {
            seq: 1,
            id: 0,
            bytes: vec![],
        };

        let packet2 = Packet {
            seq: 2,
            id: 0,
            bytes: vec![],
        };
        
        // 1 is inserted first and has therefore waited longest in the queue. It should be the
        // oldest timestamp
        queue.insert(packet1.clone());
        queue.insert(packet2.clone());
        queue.insert(packet0.clone());
        
        let oldest_timestamp = queue.get_oldest_timestamp().unwrap();
        let packet1_timestamp = queue.insert_order.iter().filter(|x| x.seq == 1).next().unwrap();
        
        assert_eq!(oldest_timestamp, packet1_timestamp.timestamp);
        
    }
}