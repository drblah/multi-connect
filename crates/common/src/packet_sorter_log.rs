use smol::io::AsyncWriteExt;

pub struct PacketSorterLogger {
    log_file_writer: smol::io::BufWriter<smol::fs::File>
}

impl PacketSorterLogger {
    pub async fn new(log_path: String) -> Self {

        let file = smol::fs::File::create(log_path).await.unwrap();
        let mut log_file_writer = smol::io::BufWriter::new(file);

        // Add header:
        log_file_writer.write_all(
            "timestamp,packet_index\n".as_ref()
        ).await.unwrap();

        PacketSorterLogger {
            log_file_writer
        }
    }

    pub async fn add_log_line(&mut self, packet_index: u64) {
        let timestamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_micros();
        let log_line = format!("{timestamp},{packet_index}\n");
        self.log_file_writer.write(
            log_line.as_str().as_ref()
        ).await.unwrap();
    }

    pub async fn flush(&mut self) {
        self.log_file_writer.flush().await.unwrap();
    }
}