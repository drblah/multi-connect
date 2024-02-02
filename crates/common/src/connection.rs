use std::net::{IpAddr, SocketAddr};
use std::os::fd::{AsRawFd, FromRawFd};
use std::time::Duration;
use smol::lock::Mutex;
use smol::net::UdpSocket;
use smol::stream::StreamExt;
use anyhow::Result;
use log::{debug};

#[derive(Debug, PartialEq)]
pub enum ConnectionState {
    Startup,
    Connected,
    Disconnected,
}

#[derive(Debug)]
pub struct Connection {
    socket: UdpSocket,
    std_socket: std::net::UdpSocket,
    name_address_touple: Option<(String, IpAddr)>,

    // TODO: Expose this connection timeout as a user configuration
    connection_timeout: Mutex<smol::Timer>,
    pub state: ConnectionState,
    buffer: Mutex<[u8; 65535]>,
    peer_addr: SocketAddr
}

impl Connection {
    pub fn new(socket: UdpSocket, interface_name: Option<&str>) -> Connection {
        let destination_socket_addr = socket.peer_addr().unwrap();
        let std_socket = unsafe { std::net::UdpSocket::from_raw_fd(socket.clone().as_raw_fd()) };
        if let Some(interface_name) = interface_name {
            let interface_name = interface_name.to_string();
            Connection {
                socket,
                std_socket,
                name_address_touple: Some((interface_name, destination_socket_addr.ip())),
                connection_timeout: Mutex::new(smol::Timer::after(Duration::from_secs(10))),
                state: ConnectionState::Startup,
                buffer: Mutex::new([0; 65535]),
                peer_addr: destination_socket_addr
            }
        } else {
            Connection {
                socket,
                std_socket,
                name_address_touple: None,
                connection_timeout: Mutex::new(smol::Timer::after(Duration::from_secs(10))),
                state: ConnectionState::Startup,
                buffer: Mutex::new([0; 65535]),
                peer_addr: destination_socket_addr
            }
        }
    }

    pub async fn reset_hello_timeout(&mut self) {
        let mut deadline_lock = self.connection_timeout.lock().await;
        deadline_lock.set_after(Duration::from_secs(10));
    }

    pub async fn read(&self) -> Result<(Vec<u8>, SocketAddr, Option<(String, IpAddr)>)> {
        let mut buffer_lock = self.buffer.lock().await;
        let message_length = self.socket.recv(buffer_lock.as_mut_slice()).await?;

        Ok((buffer_lock[..message_length].to_vec(), self.peer_addr, self.name_address_touple.clone()))
    }

    pub async fn write(&self, packet: Vec<u8>) -> std::io::Result<usize> {
        match self.std_socket.send(&packet) {
            Ok(send_bytes) => { Ok(send_bytes)}
            Err(e) => {
                match e.kind() {
                    std::io::ErrorKind::WouldBlock => {
                        debug!("Write call on {:?} would have blocked", self.name_address_touple);
                        Ok(0)
                    }
                    _ => {
                        Err(e)
                    }
                }
            }
        }
    }

    pub async fn await_connection_timeout(&self) -> SocketAddr {
        let mut deadline_lock = self.connection_timeout.lock().await;

        deadline_lock.next().await;
        self.peer_addr
    }

    pub fn get_name_address_touple(& self) -> Option<(String, IpAddr)> {
        self.name_address_touple.clone()
    }
}