use std::net::{IpAddr, SocketAddr};
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

#[derive(Debug)]
pub struct Endpoint {
    pub id: EndpointId,
    pub session_id: Uuid,
    pub connections: Vec<(SocketAddr, Connection)>,
    pub tx_counter: u64,
    pub packet_sorter: PacketSorter
}

impl Endpoint {
    pub fn new(id: EndpointId, session_id: Uuid) -> Self {
        Endpoint {
            id,
            connections: Vec::new(),
            session_id,
            tx_counter: 0,
            // TODO: Expose packet_sorter timeout in settings
            packet_sorter: PacketSorter::new(Duration::from_millis(5)),
        }
    }

    pub async fn add_connection(
        &mut self,
        source_address: SocketAddr,
        local_address: SocketAddr,
    ) -> Result<(), std::io::Error> {
        // We already know the connection, so we update the last seen time
        if let Some((_address, connection)) = self.connections.iter_mut().find(|(address, _)| *address == source_address) {
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

            self.connections.push((source_address, new_connection));
        }

        Ok(())
    }

    pub async fn acknowledge(&mut self, own_id: EndpointId, session_id: Uuid, tun_address: std::net::IpAddr) {
        // Return ACK
        let ack = HelloAck { id: own_id, session_id, tun_address };
        let ack_message = Messages::HelloAck(ack);

        let serialized = bincode::serialize(&ack_message).unwrap();

        for (address, connection) in &mut self.connections {
            if connection.state == ConnectionState::Startup || connection.state == ConnectionState::Connected {
                match connection.write(serialized.clone()).await {
                    Ok(_) => {
                        connection.state = ConnectionState::Connected;
                    }
                    Err(_) => {
                        error!("Failed to send ACK to {}. Setting as Disconnected", address);
                        connection.state = ConnectionState::Disconnected;
                    }
                }
            }
        }
    }

    pub async fn await_connections(&self) -> Result<(EndpointId, Vec<u8>, SocketAddr, Option<(String, IpAddr)>)> {
        let mut futures = Vec::new();

        for (_, connection) in self.connections.iter() {
            futures.push(connection.read().boxed())
        }

        let (item_resolved, _ready_future_index, _remaining_futures) = select_all(futures).await;

        let item_resolved = item_resolved?;

        Ok((self.id, item_resolved.0, item_resolved.1, item_resolved.2))
    }

    pub async fn await_connection_timeouts(&self) -> (EndpointId, SocketAddr) {
        let mut futures = Vec::new();

        for (_, connection) in self.connections.iter() {
            futures.push(connection.await_connection_timeout().boxed())
        }

        let (item_resolved, _ready_future_index, _remaining_futures) = select_all(futures).await;

        (self.id, item_resolved)
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

    pub fn get_alive_connections(&self) -> Vec<Option<(String, IpAddr)>> {
        let mut alive_connections = Vec::new();

        for connection in &self.connections {
            alive_connections.push(connection.1.get_name_address_touple())
        }

        alive_connections
    }
}