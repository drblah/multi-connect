use std::io::Error;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
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
use crate::threaded_receiver::ThreadedReceiver;
use crate::threaded_sender::ThreadedSender;
use crate::UdpSocketInfo;

#[derive(Debug, PartialEq)]
pub enum ConnectionState {
    Startup,
    Connected,
    Disconnected,
}

#[derive(Debug)]
pub struct Connection {
    name_address_touple: Option<(String, IpAddr)>,

    threaded_sender: ThreadedSender,
    threaded_receiver: ThreadedReceiver,

    // TODO: Expose this connection timeout as a user configuration
    connection_timeout: Mutex<smol::Timer>,
    pub state: ConnectionState,
    pub udpsocket_info: Arc<UdpSocketInfo>
}

impl Connection {
    pub fn new(socket: std::net::UdpSocket, interface_name: Option<&str>) -> Connection {

        let if_name = if interface_name.is_some() {
            interface_name.unwrap().to_string()
        } else {
            "DYNAMIC".to_string()
        };

        let local_address = socket.local_addr().unwrap();
        let destination_address = socket.peer_addr().unwrap();

        let udpsocket_info = Arc::new(UdpSocketInfo{
            socket_state: quinn_udp::UdpSocketState::new(quinn_udp::UdpSockRef::from(&socket)).unwrap(),
            socket,
            interface_name: if_name,
            local_address,
            destination_address,
        });

        let threaded_sender = ThreadedSender::new(udpsocket_info.clone());
        let threaded_receiver = ThreadedReceiver::new(udpsocket_info.clone());

        if let Some(interface_name) = interface_name {
            let interface_name = interface_name.to_string();
            Connection {
                name_address_touple: Some((interface_name, destination_address.ip())),

                threaded_sender,
                threaded_receiver,

                connection_timeout: Mutex::new(smol::Timer::after(Duration::from_secs(10))),
                state: ConnectionState::Startup,
                udpsocket_info
            }
        } else {
            Connection {
                name_address_touple: None,

                threaded_sender,
                threaded_receiver,

                connection_timeout: Mutex::new(smol::Timer::after(Duration::from_secs(10))),
                state: ConnectionState::Startup,
                udpsocket_info
            }
        }
    }

    pub async fn reset_hello_timeout(&mut self) {
        let mut deadline_lock = self.connection_timeout.lock().await;
        deadline_lock.set_after(Duration::from_secs(10));
    }

    pub async fn read(&self) -> Result<(Vec<u8>, SocketAddr)> {

        let packet = self.threaded_receiver.pop_next_packet().await?;

        let peer_address = self.udpsocket_info.destination_address;

        Ok((packet, peer_address))
    }

    pub async fn write(&self, packet: Vec<u8>) -> std::io::Result<usize> {
        //self.socket.send(&packet).await
        self.threaded_sender.try_send(packet).await;

        // Check result_channel for socket status
        // Be aware that we are only doing this to propagate std::io::ErrorKind::ConnectionRefused
        // Due to async and threading, the result we read here is not necessarily the result
        // For the packet we just submitted.
        Ok(self.threaded_sender.try_get_result().await?)
    }

    pub async fn await_connection_timeout(&self) -> SocketAddr {
        let mut deadline_lock = self.connection_timeout.lock().await;

        deadline_lock.next().await;
        self.udpsocket_info.destination_address
    }

    pub fn get_name_address_touple(& self) -> Option<(String, IpAddr)> {
        self.name_address_touple.clone()
    }
}