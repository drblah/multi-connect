use std::net::SocketAddr;
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;
use smol::lock::Mutex;
use smol::net::UdpSocket;
use smol::stream::StreamExt;
use anyhow::Result;
use log::warn;
use smol::{Executor, future};
use smol::channel::{Sender, TrySendError};

#[derive(Debug, PartialEq)]
pub enum ConnectionState {
    Startup,
    Connected,
    Disconnected,
}

#[derive(Debug)]
pub struct Connection {
    socket: UdpSocket,
    sender_channel: Sender<Vec<u8>>,
    sender_thread: JoinHandle<()>,

    // TODO: Expose this connection timeout as a user configuration
    connection_timeout: Mutex<smol::Timer>,
    pub state: ConnectionState,
    buffer: Mutex<[u8; 65535]>,
}

impl Connection {
    pub fn new(socket: UdpSocket) -> Connection {

        let (sender, receiver) = smol::channel::bounded::<Vec<u8>>(100);

        let sender_socket = socket.clone();
        let sender_thread = thread::spawn(move || {
            let ex = Executor::new();

            ex.spawn(async {

                loop {
                    let bytes_to_send = receiver.recv().await.unwrap();
                    sender_socket.send(bytes_to_send.as_slice()).await.unwrap();
                }
            }).detach();

            future::block_on(ex.run(future::pending::<()>()));
        });


        Connection {
            socket,
            sender_channel: sender,
            sender_thread,
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

    pub async fn write(&self, packet: Vec<u8>) -> std::io::Result<usize> {
        //self.socket.send(&packet).await
        let packet_length = packet.len();
        match self.sender_channel.try_send(packet) {
            Ok(()) => {
                Ok(packet_length)
            }
            Err(e) => {
                match e {
                    TrySendError::Full(_) => {
                        warn!("Send channel is full! Dropping packets on device: {}", self.socket.local_addr().unwrap());
                        Ok(0)
                    }
                    TrySendError::Closed(_) => {
                        panic!("Sender channel closed!")
                    }
                }
            }
        }
    }

    pub async fn await_connection_timeout(&self) -> SocketAddr {
        let mut deadline_lock = self.connection_timeout.lock().await;

        deadline_lock.next().await;
        self.socket.peer_addr().unwrap()
    }
}