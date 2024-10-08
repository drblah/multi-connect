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
use crate::router::Route;

#[derive(Debug)]
pub struct ConnectionEntry {
    pub interface_name: String,
    pub interface_address: SocketAddr,
    pub connection: Connection
}

/// Endpoint represents a remote peer instance of multi-connect. It consists mainly of a list of
/// Connections, an EndpointId (which must be globally unique amongst all connected Endpoints), and
/// a session ID, which is randomly generated every time an Endpoint is created.
#[derive(Debug)]
pub struct Endpoint {
    pub id: EndpointId,
    pub session_id: Uuid,
    pub connections: Vec<ConnectionEntry>,
    pub tx_counter: u64,
    pub hello_counter: u64,
    pub hello_ack_counter:u64,
    pub packet_sorter: PacketSorter,
    pub hello_path_latency: PathLatency,
    pub hello_ack_path_latency: PathLatency
}

pub struct ReadInfo {
    pub connection_read_info: crate::connection::ReadInfo,
    pub endpoint_id: EndpointId
}

impl Endpoint {
    pub fn new(id: EndpointId, session_id: Uuid, packet_sorter_deadline: u64) -> Self {
        Endpoint {
            id,
            connections: Vec::new(),
            session_id,
            tx_counter: 0,
            hello_counter: 0,
            hello_ack_counter: 0,
            packet_sorter: PacketSorter::new(Duration::from_millis(packet_sorter_deadline)),
            hello_path_latency: PathLatency::new(),
            hello_ack_path_latency: PathLatency::new()
        }
    }

    pub async fn add_connection(
        &mut self,
        source_address: SocketAddr,
        interface_name: String,
        local_address: SocketAddr,
        connection_timeout: u64
    ) -> Result<(), std::io::Error> {
        // We already know the connection, so we update the last seen time
        if let Some(connection_entry) = self.connections.iter_mut().find(|connection_entry| connection_entry.interface_address == source_address && *connection_entry.interface_name == interface_name) {
            info!("Reset hello timeout for {}", source_address);
            connection_entry.connection.reset_hello_timeout().await;
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
            let new_connection = Connection::new(socket, None, connection_timeout);

            let connection_entry = ConnectionEntry {
                interface_name,
                interface_address: source_address,
                connection: new_connection,
            };

            self.connections.push(connection_entry);
        }

        Ok(())
    }

    pub async fn acknowledge(&mut self, own_id: EndpointId, session_id: Uuid, own_static_routes: &Option<Vec<Route>>) {
        // Return ACK
        let ack = HelloAck { id: own_id, session_id, static_routes: own_static_routes.clone(), hello_ack_seq: self.hello_ack_counter };
        self.hello_ack_counter.add_assign(1);
        let ack_message = Messages::HelloAck(ack);

        let serialized = bincode::serialize(&ack_message).unwrap();

        for connection_entry in &mut self.connections {
            if connection_entry.connection.state == ConnectionState::Startup || connection_entry.connection.state == ConnectionState::Connected {
                match connection_entry.connection.write(serialized.clone()).await {
                    Ok(_) => {
                        connection_entry.connection.state = ConnectionState::Connected;
                    }
                    Err(_) => {
                        error!("Failed to send ACK to ({}, {}). Setting as Disconnected", connection_entry.interface_name, connection_entry.interface_address);
                        connection_entry.connection.state = ConnectionState::Disconnected;
                    }
                }
            }
        }
    }

    pub async fn await_connections(&self) -> Result<ReadInfo> {
        let mut futures = Vec::new();

        for connection_entry in self.connections.iter() {
            futures.push(connection_entry.connection.read().boxed())
        }

        let (item_resolved, _ready_future_index, _remaining_futures) = select_all(futures).await;

        let item_resolved = item_resolved?;

        let read_info = ReadInfo {
            connection_read_info: item_resolved,
            endpoint_id: self.id
        };

        Ok(read_info)
    }

    pub async fn await_connection_timeouts(&self) -> (EndpointId, String, SocketAddr) {
        let mut futures = Vec::new();

        for connection_entry in self.connections.iter() {
            futures.push(connection_entry.connection.await_connection_timeout().boxed())
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

        for connection_entry in &self.connections {
            alive_connections.push((
                                       connection_entry.interface_name.clone(), connection_entry.interface_address.clone()
                                       )
            )
        }

        alive_connections
    }

    pub fn update_deadline(&mut self) {
        let hello_path_latency_diff = self.hello_path_latency.estimate_path_delay_difference();
        let hello_ack_path_latency_diff = self.hello_ack_path_latency.estimate_path_delay_difference();


        let deadline = hello_path_latency_diff.max(hello_ack_path_latency_diff) * 2;

        self.packet_sorter.set_deadline(deadline)
    }

    pub fn disable_interface(&mut self, interface_name: &str) {
        for connection in &mut self.connections {
            if connection.interface_name == interface_name {
                connection.connection.disable();
            }
        }
    }

    pub fn enable_interface(&mut self, interface_name: &str) {
        for connection in &mut self.connections {
            if connection.interface_name == interface_name {
                connection.connection.enable();
            }
        }
    }
}