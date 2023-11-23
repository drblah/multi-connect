use std::net::{IpAddr, SocketAddr};
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;
use smol::lock::Mutex;
use smol::net::UdpSocket;
use smol::stream::StreamExt;
use anyhow::Result;
use log::warn;
use smol::{Executor, future};
use smol::channel::{Receiver, Sender, TryRecvError, TrySendError};

#[derive(Debug, PartialEq)]
pub enum ConnectionState {
    Startup,
    Connected,
    Disconnected,
}

#[derive(Debug)]
pub struct Connection {
    socket: UdpSocket,
    name_address_touple: Option<(String, IpAddr)>,
    sender_channel: Sender<Vec<u8>>,
    result_channel: Receiver<std::io::Result<usize>>,
    sender_thread: JoinHandle<()>,

    // TODO: Expose this connection timeout as a user configuration
    connection_timeout: Mutex<smol::Timer>,
    pub state: ConnectionState,
    buffer: Mutex<[u8; 65535]>,
}

impl Connection {
    pub fn new(socket: UdpSocket, interface_name: Option<&str>) -> Connection {

        let (packet_sender, packet_receiver) = smol::channel::bounded::<Vec<u8>>(1000);
        let (result_sender, result_receiver) = smol::channel::bounded::<std::io::Result<usize>>(1000);

        let sender_socket = socket.clone();
        let sender_thread = thread::spawn(move || {
            let ex = Executor::new();

            ex.spawn(async {
                loop {
                    let bytes_to_send = packet_receiver.recv().await.unwrap();
                    let result =  sender_socket.send(bytes_to_send.as_slice()).await;
                    match result_sender.try_send(result) {
                        Ok(_) => {}
                        Err(e) => {
                            match e {
                                TrySendError::Full(_) => {}
                                TrySendError::Closed(_) => { panic!("Result channel closed!") }
                            }
                        }
                    }
                }
            }).detach();

            future::block_on(ex.run(future::pending::<()>()));
        });

        let destination_ip = socket.peer_addr().unwrap().ip();

        if let Some(interface_name) = interface_name {
            let interface_name = interface_name.to_string();
            Connection {
                socket,
                name_address_touple: Some((interface_name, destination_ip)),
                sender_channel: packet_sender,
                result_channel: result_receiver,
                sender_thread,
                connection_timeout: Mutex::new(smol::Timer::after(Duration::from_secs(10))),
                state: ConnectionState::Startup,
                buffer: Mutex::new([0; 65535]),
            }
        } else {
            Connection {
                socket,
                name_address_touple: None,
                sender_channel: packet_sender,
                result_channel: result_receiver,
                sender_thread,
                connection_timeout: Mutex::new(smol::Timer::after(Duration::from_secs(10))),
                state: ConnectionState::Startup,
                buffer: Mutex::new([0; 65535]),
            }
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
        match self.sender_channel.try_send(packet) {
            Ok(()) => {

            }
            Err(e) => {
                match e {
                    TrySendError::Full(_) => {
                        warn!("Send channel is full! Dropping packets on device: {}", self.socket.local_addr().unwrap());
                    }
                    TrySendError::Closed(_) => {
                        panic!("Sender channel closed!")
                    }
                }
            }
        };

        // Check result_channel for socket status
        // Be aware that we are only doing this to propagate std::io::ErrorKind::ConnectionRefused
        // Due to async and threading, the result we read here is not necessarily the result
        // For the packet we just submitted.
        match self.result_channel.try_recv() {
            Ok(socket_result) => {
                match socket_result {
                    Ok(bytes_sent) => {
                        Ok(bytes_sent)
                    }
                    Err(e) => {
                        Err(e)
                    }
                }
            }
            Err(e) => {
                match e {
                    TryRecvError::Empty => {Ok(0)}
                    TryRecvError::Closed => {
                        panic!("Result channel closed!")
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

    pub fn get_name_address_touple(& self) -> Option<(String, IpAddr)> {
        self.name_address_touple.clone()
    }
}