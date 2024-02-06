use std::net::{SocketAddr};
use std::ops::AddAssign;
use std::time::Duration;
use smol::Async;
use smol::net::UdpSocket;
use socket2::SockAddr;
use uuid::Uuid;
use anyhow::Result;
use smol::future::FutureExt;
use futures::future::select_all;
use log::{error, info};
use crate::connection::{Connection, ConnectionState};
use crate::messages::{EndpointId, HelloAck, Messages, Packet};
use crate::packet_sorter::PacketSorter;
use crate::path_latency::PathLatency;

#[derive(Debug)]
pub struct Endpoint {
    pub id: EndpointId,
    pub session_id: Uuid,
    pub connections: Vec<((String, SocketAddr), Connection)>,
    pub tx_counter: u64,
    pub hello_counter: u64,
    pub hello_ack_counter:u64,
    pub packet_sorter: PacketSorter,
    pub hello_path_latency: PathLatency,
    pub hello_ack_path_latency: PathLatency
}

impl Endpoint {
    pub fn new(id: EndpointId, session_id: Uuid) -> Self {
        Endpoint {
            id,
            connections: Vec::new(),
            session_id,
            tx_counter: 0,
            hello_counter: 0,
            hello_ack_counter: 0,
            // TODO: Expose packet_sorter timeout in settings
            packet_sorter: PacketSorter::new(Duration::from_millis(100)),
            hello_path_latency: PathLatency::new(),
            hello_ack_path_latency: PathLatency::new()
        }
    }

    pub async fn add_connection(
        &mut self,
        source_address: SocketAddr,
        interface_name: String,
        local_address: SocketAddr,
    ) -> Result<(), std::io::Error> {
        // We already know the connection, so we update the last seen time
        // TODO: Also check on interface name
        if let Some((_address, connection)) = self.connections.iter_mut().find(|((name, addr), _)| *addr == source_address && *name == interface_name) {
            info!("Reset hello timeout for {}", source_address);
            connection.reset_hello_timeout().await;
        } else {
            let socket = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, None)?;
            socket.set_reuse_address(true)?;

            socket.bind(&SockAddr::from(local_address))?;

            let socket = std::net::UdpSocket::from(socket);
            socket.set_nonblocking(true)?;

            let socket = UdpSocket::from(Async::try_from(socket)?);

            socket.connect(source_address).await?;

            // We don't know the interface_name when we handle dynamically incoming connection
            // TODO: Figure out if we need to handle this case
            let new_connection = Connection::new(socket, None);

            self.connections.push(((interface_name, source_address), new_connection));
        }

        Ok(())
    }

    pub async fn acknowledge(&mut self, own_id: EndpointId, session_id: Uuid, tun_address: std::net::IpAddr) {
        // Return ACK
        let ack = HelloAck { id: own_id, session_id, tun_address, hello_ack_seq: self.hello_ack_counter };
        self.hello_ack_counter.add_assign(1);
        let ack_message = Messages::HelloAck(ack);

        let serialized = bincode::serialize(&ack_message).unwrap();

        for (key, connection) in &mut self.connections {
            if connection.state == ConnectionState::Startup || connection.state == ConnectionState::Connected {
                match connection.write(serialized.clone()).await {
                    Ok(_) => {
                        connection.state = ConnectionState::Connected;
                    }
                    Err(_) => {
                        error!("Failed to send ACK to ({}, {}). Setting as Disconnected", key.0, key.1);
                        connection.state = ConnectionState::Disconnected;
                    }
                }
            }
        }
    }

    pub async fn await_connections(&self) -> Result<(EndpointId, Vec<u8>, SocketAddr, (String, SocketAddr))> {
        let mut futures = Vec::new();

        for (_, connection) in self.connections.iter() {
            futures.push(connection.read().boxed())
        }

        let (item_resolved, _ready_future_index, _remaining_futures) = select_all(futures).await;

        let item_resolved = item_resolved?;

        Ok((self.id, item_resolved.0, item_resolved.1, item_resolved.2))
    }

    pub async fn await_connection_timeouts(&self) -> (EndpointId, String, SocketAddr) {
        let mut futures = Vec::new();

        for (_, connection) in self.connections.iter() {
            futures.push(connection.await_connection_timeout().boxed())
        }

        let (item_resolved, _ready_future_index, _remaining_futures) = select_all(futures).await;

        (self.id, item_resolved.1, item_resolved.0)
    }

    pub async fn await_packet_sorter_deadline(&self) -> EndpointId {
        self.packet_sorter.await_deadline().await;

        self.id
    }

    pub async fn await_sorted_packet(&self) -> (EndpointId, Option<Packet>) {
        (
            self.id,
            self.packet_sorter.await_have_next_packet().await
            )
    }

    pub fn has_connections(&self) -> bool {
        self.connections.len() != 0
    }

    pub fn get_alive_connections(&self) -> Vec<(String, SocketAddr)> {
        let mut alive_connections = Vec::new();

        for connection in &self.connections {
            alive_connections.push(connection.1.get_name_address_touple())
        }

        alive_connections
    }
}