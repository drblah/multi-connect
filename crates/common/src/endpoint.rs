use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;
use smol::Async;
use smol::net::UdpSocket;
use socket2::SockAddr;
use uuid::Uuid;
use anyhow::Result;
use smol::future::FutureExt;
use futures::future::select_all;
use crate::connection::{Connection, ConnectionState};
use crate::messages::{EndpointId, HelloAck, Messages};
use crate::sequencer::Sequencer;

pub struct Endpoint {
    pub id: EndpointId,
    pub session_id: Uuid,
    pub connections: HashMap<SocketAddr, Connection>,
    pub tx_counter: u64,
    pub packet_sorter: Sequencer
}

impl Endpoint {
    pub fn new(id: EndpointId, session_id: Uuid) -> Self {
        Endpoint {
            id,
            connections: HashMap::new(),
            session_id,
            tx_counter: 0,
            packet_sorter: Sequencer::new(Duration::from_secs(1)),
        }
    }

    pub async fn add_connection(
        &mut self,
        source_address: SocketAddr,
        local_address: SocketAddr,
    ) -> Result<(), std::io::Error> {
        // We already know the connection, so we update the last seen time
        if let Some(connection) = self.connections.get_mut(&source_address) {
            connection.reset_hello_timeout().await;
        } else {
            let socket = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, None)?;
            socket.set_reuse_address(true)?;

            socket.bind(&SockAddr::from(local_address))?;

            let socket = std::net::UdpSocket::from(socket);
            socket.set_nonblocking(true)?;

            let socket = UdpSocket::from(Async::try_from(socket)?);

            socket.connect(source_address).await?;

            let new_connection = Connection::new(socket);

            self.connections.insert(source_address, new_connection);
        }

        Ok(())
    }

    pub async fn acknowledge(&mut self, own_id: EndpointId) {
        // Return ACK
        let ack = HelloAck { id: own_id };
        let ack_message = Messages::HelloAck(ack);

        let serialized = bincode::serialize(&ack_message).unwrap();

        for (_address, connection) in &mut self.connections {
            if connection.state == ConnectionState::Startup {
                connection.write(serialized.clone()).await;
                connection.state = ConnectionState::Connected;
            }
        }
    }

    pub async fn await_connections(&self) -> Result<(EndpointId, Vec<u8>, SocketAddr)> {
        let mut futures = Vec::new();

        for (_, connection) in self.connections.iter() {
            futures.push(connection.read().boxed())
        }

        let (item_resolved, _ready_future_index, _remaining_futures) = select_all(futures).await;

        let item_resolved = item_resolved?;

        Ok((self.id, item_resolved.0, item_resolved.1))
    }

    pub async fn await_connection_timeouts(&self) -> (EndpointId, SocketAddr) {
        let mut futures = Vec::new();

        for (_, connection) in self.connections.iter() {
            futures.push(connection.await_connection_timeout().boxed())
        }

        let (item_resolved, _ready_future_index, _remaining_futures) = select_all(futures).await;

        (self.id, item_resolved)
    }

    pub async fn await_sequencer_deadline(&self) -> EndpointId {
        self.packet_sorter.await_deadline().await;

        self.id
    }

    pub fn has_connections(&self) -> bool {
        self.connections.len() != 0
    }
}