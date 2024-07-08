use std::collections::BTreeMap;

const MAX_TIMESTAMPS: usize = 100;

#[derive(Debug)]
pub struct PathLatency {
    ack_timestamps: BTreeMap<u64, Vec<std::time::SystemTime>>
}

impl PathLatency {
    pub fn new() -> Self {
        PathLatency {
            ack_timestamps: BTreeMap::new()
        }
    }

    pub fn insert_new_timestamp(&mut self, seq: u64) {
        if self.ack_timestamps.len() < MAX_TIMESTAMPS {
            if self.ack_timestamps.contains_key(&seq) {
                self.ack_timestamps.entry(seq).and_modify(|a| a.push(std::time::SystemTime::now()));
            } else {
                self.ack_timestamps.insert(seq, vec![std::time::SystemTime::now()]);
            }


        } else {
            self.ack_timestamps.pop_first();
            if self.ack_timestamps.contains_key(&seq) {
                self.ack_timestamps.entry(seq).and_modify(|a| a.push(std::time::SystemTime::now()));
            } else {
                self.ack_timestamps.insert(seq, vec![std::time::SystemTime::now()]);
            }
        }
    }

    pub fn estimate_path_delay_difference(&self) -> std::time::Duration {
        let mut sum = 0.0;
        let mut count = 0.0;

        for (_seq, timestamps) in &self.ack_timestamps {
            if timestamps.len() >= 2 {
                let first_timestamp = timestamps.first().unwrap();
                let last_timestamp = timestamps.last().unwrap();

                let difference = last_timestamp.duration_since(*first_timestamp).unwrap();

                sum += difference.as_secs_f64();
                count += 1.0;
            }
        }

        if count == 0.0 {
            return std::time::Duration::from_secs(0)
        }

        let alpha = 2.0 / (count + 2.0);

        std::time::Duration::from_secs_f64(sum * alpha)
    }
}

#[cfg(test)]
mod tests {
    use super::PathLatency;

    #[test]
    fn test_insert_new_timestamp_empty() {
        let mut path_latency = PathLatency::new();
        path_latency.insert_new_timestamp(1);
        assert_eq!(path_latency.ack_timestamps.len(), 1);
    }

    #[test]
    fn test_insert_new_timestamp_not_empty() {
        let mut path_latency = PathLatency::new();
        path_latency.insert_new_timestamp(1);
        path_latency.insert_new_timestamp(2);
        assert_eq!(path_latency.ack_timestamps.len(), 2);
    }

    #[test]
    fn test_insert_new_timestamp_max_size() {
        let mut path_latency = PathLatency::new();
        for i in 1..=101 {
            path_latency.insert_new_timestamp(i);
        }
        assert_eq!(path_latency.ack_timestamps.len(), 100);
    }

    #[test]
    fn test_insert_new_timestamp_after_max_size() {
        let mut path_latency = PathLatency::new();
        for i in 1..=101 {
            path_latency.insert_new_timestamp(i);
        }
        path_latency.insert_new_timestamp(102);
        assert_eq!(path_latency.ack_timestamps.len(), 100);
        assert_eq!(path_latency.ack_timestamps.keys().last().unwrap(), &102);
    }
}
