use std::net::{SocketAddr};
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

/// Connection represents the current connection state between a certain network interface and an endpoint
#[derive(Debug)]
pub struct Connection {
    socket: UdpSocket,
    std_socket: std::net::UdpSocket,
    interface_name: Option<String>,
    #[allow(dead_code)]
    local_address: SocketAddr,

    connection_timeout: Mutex<smol::Timer>,
    pub state: ConnectionState,
    buffer: Mutex<[u8; 65535]>,
    peer_addr: SocketAddr
}

pub struct ReadInfo {
    pub packet_bytes: Vec<u8>,
    pub source_address: SocketAddr,
    pub interface_name: String
}


impl Connection {
    pub fn new(socket: UdpSocket, interface_name: Option<String>, connection_timeout: u64) -> Connection {
        let destination_socket_addr = socket.peer_addr().unwrap();
        let std_socket = unsafe { std::net::UdpSocket::from_raw_fd(socket.clone().as_raw_fd()) };
        let local_address = std_socket.local_addr().unwrap();

        Connection {
            socket,
            std_socket,
            interface_name,
            local_address,
            connection_timeout: Mutex::new(smol::Timer::after(Duration::from_millis(connection_timeout))),
            state: ConnectionState::Startup,
            buffer: Mutex::new([0; 65535]),
            peer_addr: destination_socket_addr
        }
    }

    pub async fn reset_hello_timeout(&mut self) {
        let mut deadline_lock = self.connection_timeout.lock().await;
        deadline_lock.set_after(Duration::from_secs(10));
    }

    pub async fn read(&self) -> Result<ReadInfo> {
        let mut buffer_lock = self.buffer.lock().await;
        let message_length = self.socket.recv(buffer_lock.as_mut_slice()).await?;

        let read_info = ReadInfo {
            packet_bytes: buffer_lock[..message_length].to_vec(),
            source_address: self.peer_addr,
            interface_name: self.get_interface_name().to_string()
        };

        Ok(read_info)
    }

    /// write attempts to send a packet to the connected endpoint over the Connection's network interface.
    /// Note: write is best-effort and will drop packets if the network interface is too busy. This is
    /// necessary to ensure that write never blocks. A block here would cause the whole event loop to block
    /// as well.
    pub async fn write(&self, packet: Vec<u8>) -> std::io::Result<usize> {
        match self.std_socket.send(&packet) {
            Ok(send_bytes) => { Ok(send_bytes)}
            Err(e) => {
                match e.kind() {
                    std::io::ErrorKind::WouldBlock => {
                        debug!("Write call on {:?} would have blocked", self.get_name_address_touple());
                        Ok(0)
                    }
                    _ => {
                        Err(e)
                    }
                }
            }
        }
    }

    pub async fn await_connection_timeout(&self) -> (SocketAddr, String) {
        let mut deadline_lock = self.connection_timeout.lock().await;

        deadline_lock.next().await;

        let interface_name = if self.interface_name.is_some() {
            self.interface_name.clone().unwrap()
        } else {
            "DYN-interface".to_string()
        };

        (self.peer_addr, interface_name)
    }

    pub fn get_name_address_touple(& self) -> (String, SocketAddr) {
        if self.interface_name.is_some() {
            (self.interface_name.clone().unwrap(), self.peer_addr)
        } else {
            ("DYN-interface".to_string(), self.peer_addr)
        }
    }

    pub fn get_interface_name(&self) -> String {
        if self.interface_name.is_some() {
            self.interface_name.clone().unwrap()
        } else {
            "DYN-interface".to_string()
        }
    }
}