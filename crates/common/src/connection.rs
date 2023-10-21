use std::net::SocketAddr;
use std::time::Duration;
use smol::lock::Mutex;
use smol::net::UdpSocket;
use smol::stream::StreamExt;
use anyhow::Result;

#[derive(Debug, PartialEq)]
pub enum ConnectionState {
    Startup,
    Connected,
    Disconnected,
}

#[derive(Debug)]
pub struct Connection {
    socket: UdpSocket,

    // TODO: Expose this connection timeout as a user configuration
    connection_timeout: Mutex<smol::Timer>,
    pub state: ConnectionState,
    buffer: Mutex<[u8; 65535]>,
}

impl Connection {
    pub fn new(socket: UdpSocket) -> Connection {
        Connection {
            socket,
            connection_timeout: Mutex::new(smol::Timer::after(Duration::from_secs(10))),
            state: ConnectionState::Startup,
            buffer: Mutex::new([0; 65535]),
        }
    }

    pub async fn reset_hello_timeout(&mut self) {
        let mut deadline_lock = self.connection_timeout.lock().await;
        deadline_lock.set_after(Duration::from_secs(10));
    }

    pub async fn read(&self) -> Result<(Vec<u8>, SocketAddr)> {
        let mut buffer_lock = self.buffer.lock().await;
        let message_length = self.socket.recv(buffer_lock.as_mut_slice()).await?;

        Ok((buffer_lock[..message_length].to_vec(), self.socket.peer_addr().unwrap()))
    }

    pub async fn write(&self, packet: Vec<u8>) {
        let _len = self.socket.send(&packet).await.unwrap();
    }

    pub async fn await_connection_timeout(&self) -> SocketAddr {
        let mut deadline_lock = self.connection_timeout.lock().await;

        deadline_lock.next().await;
        self.socket.peer_addr().unwrap()
    }
}