use anyhow::Result;
use common::messages::{EndpointId, HelloAck, Messages, Packet};
use futures::future::select_all;
use futures::StreamExt;
use smol::future::FutureExt;
use smol::lock::Mutex;
use smol::net::UdpSocket;
use smol::Async;
use socket2::SockAddr;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::ops::AddAssign;
use std::time::{Duration, SystemTime};
use etherparse::{InternetSlice, SlicedPacket};
use log::{error, info, warn};
use uuid::Uuid;
use common::sequencer::Sequencer;

#[derive(Debug, PartialEq)]
enum ConnectionState {
    Startup,
    Connected,
    Disconnected,
}

#[derive(Debug)]
struct Connection {
    socket: UdpSocket,

    // TODO: Expose this connection timeout as a user configuration
    connection_timeout: Mutex<smol::Timer>,
    state: ConnectionState,
    buffer: Mutex<[u8; 65535]>,
}

impl Connection {
    fn new(socket: UdpSocket) -> Connection {
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

struct Endpoint {
    id: EndpointId,
    session_id: Uuid,
    connections: HashMap<SocketAddr, Connection>,
    tx_counter: u64,
    packet_sorter: Sequencer
}

impl Endpoint {
    fn new(id: EndpointId, session_id: Uuid) -> Self {
        Endpoint {
            id,
            connections: HashMap::new(),
            session_id,
            tx_counter: 0,
            packet_sorter: Sequencer::new(Duration::from_secs(1)),
        }
    }

    async fn add_connection(
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

    async fn acknowledge(&mut self, own_id: EndpointId) {
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

    async fn await_connections(&self) -> Result<(EndpointId, Vec<u8>, SocketAddr)> {
        let mut futures = Vec::new();

        for (_, connection) in self.connections.iter() {
            futures.push(connection.read().boxed())
        }

        let (item_resolved, _ready_future_index, _remaining_futures) = select_all(futures).await;

        let item_resolved = item_resolved?;

        Ok((self.id, item_resolved.0, item_resolved.1))
    }

    async fn await_connection_timeouts(&self) -> (EndpointId, SocketAddr) {
        let mut futures = Vec::new();

        for (_, connection) in self.connections.iter() {
            futures.push(connection.await_connection_timeout().boxed())
        }

        let (item_resolved, _ready_future_index, _remaining_futures) = select_all(futures).await;

        (self.id, item_resolved)
    }

    async fn await_sequencer_deadline(&self) -> EndpointId {
        self.packet_sorter.await_deadline().await;

        self.id
    }

    pub fn has_connections(&self) -> bool {
        self.connections.len() != 0
    }
}

pub struct ConnectionManager {
    endpoints: HashMap<EndpointId, Endpoint>,
    local_address: SocketAddr,
    pub own_id: EndpointId,
    session_history: Vec<Uuid>,
    routes: HashMap<IpAddr, EndpointId>
}

impl ConnectionManager {
    pub fn new(local_address: SocketAddr, own_id: EndpointId) -> ConnectionManager {
        ConnectionManager {
            endpoints: HashMap::new(),
            local_address,
            own_id,
            session_history: Vec::new(),
            routes: HashMap::new(),
        }
    }

    pub async fn handle_hello(&mut self, message: Vec<u8>, source_address: SocketAddr) {
        if let Ok(decoded) = bincode::deserialize::<Messages>(&message) {
            info!("Hello decoded!");
            info!("{:?}", decoded);

            if let Messages::Hello(decoded) = decoded {

                // Ensure endpoint exists
                if !self.endpoints.contains_key(&decoded.id) {
                    let new_endpoint = Endpoint::new(decoded.id.clone(), decoded.session_id.clone());
                    self.endpoints.insert(decoded.id.clone(), new_endpoint);
                    self.session_history.push(decoded.session_id.clone());
                }


                if let Some(endpoint) = self.endpoints.get_mut(&decoded.id) {
                    // If the session id has changed. Overwrite the endpoint
                    if endpoint.session_id != decoded.session_id {
                        if self.session_history.contains(&decoded.session_id) {
                            error!("Session ID {} for endpoint {} has already been used. Refusing to accept new connection.", decoded.session_id, decoded.id);
                            return;
                        } else {
                            info!("Session ID has changed. Overwriting endpoint");
                            *endpoint = Endpoint::new(decoded.id.clone(), decoded.session_id.clone());
                            self.session_history.push(decoded.session_id.clone());
                        }
                    }

                    // Add connections
                    match endpoint
                        .add_connection(source_address, self.local_address)
                        .await
                    {
                        Ok(()) => {
                            info!(
                                "Added {} as a new connection to EP: {}",
                                source_address, decoded.id
                            );

                            endpoint.acknowledge(self.own_id).await
                        }
                        Err(e) => {
                            error!(
                                "Failed to add {} as new connection to EP: {} due to error: {}",
                                source_address,
                                decoded.id,
                                e.to_string()
                            )
                        }
                    }

                    endpoint.acknowledge(self.own_id).await;

                    // Add route
                    self.routes.insert(decoded.tun_address, decoded.id);
                }
            }
        } else {
            error!("Failed to decode. But lets say hi anyways :^)");
        }
    }

    pub async fn handle_established_message(&mut self, message: Vec<u8>, endpoint_id: EndpointId, source_address: SocketAddr) {
        if let Ok(decoded) = bincode::deserialize::<Messages>(&message) {
            match decoded {
                Messages::Packet(packet) => {
                    self.handle_tunnel_message(packet, endpoint_id).await;
                }
                Messages::Hello(hello) => {
                    if let Some(endpoint) = self.endpoints.get_mut(&hello.id) {
                        endpoint.add_connection(source_address, self.local_address).await.unwrap();
                    } else {
                        error!("Received hello from unknown endpoint: {}", hello.id);
                    }
                }
                Messages::HelloAck(_) => {
                    unimplemented!()
                }
                Messages::KeepAlive(_) => {
                    todo!()
                }
            }
        }
    }

    pub async fn handle_tunnel_message(&mut self, packet: Packet, endpoint_id: EndpointId) {
        todo!()
    }

    fn get_route(&self, packet_bytes: &[u8]) -> Option<EndpointId> {
        match SlicedPacket::from_ip(packet_bytes) {
            Err(e) => {
                error!("Error learning tun_ip: {}", e);
                None
            }

            Ok(ip_packet) => {
                match ip_packet.ip {
                    Some(InternetSlice::Ipv4(ipheader, ..)) => {
                        let ipv4 = IpAddr::V4(ipheader.destination_addr());
                        return self.routes.get(&ipv4).cloned();

                    }
                    Some(InternetSlice::Ipv6(_, _)) => {
                        warn!("TODO: Handle learning IPv6 route");
                        None
                    }
                    None => {
                        warn!("No IP header detected. Cannot learn route!");
                        None
                    }
                }
            }
        }
    }

    pub async fn handle_packet_from_tun(&mut self, packet: &[u8]) {
        // Look up route based on packet destination IP
        if let Some(endpoint_id) = self.get_route(packet) {
            // get endpoint
            let endpoint = self.endpoints.get_mut(&endpoint_id).unwrap();

            // Encapsulate packet in messages::Packet
            let packet = Packet {
                seq: endpoint.tx_counter,
                id: endpoint.id,
                bytes: packet.to_vec(),
            };

            let serialized_pakcet = bincode::serialize(&Messages::Packet(packet)).unwrap();

            endpoint.tx_counter.add_assign(1);

            // Send to endpoint
            for (_address, connection) in &mut endpoint.connections {
                connection.write(serialized_pakcet.clone()).await;
            }
        } else {
            warn!("No route found for packet: {:?}. Dropping packet", packet);
        }
    }

    pub fn has_endpoints(&self) -> bool {
        self.endpoints.len() != 0
    }

    pub async fn await_incoming(&self) -> Result<(EndpointId, Vec<u8>, SocketAddr)> {
        let mut futures = Vec::new();

        for (_, endpoint) in self.endpoints.iter() {
            if endpoint.has_connections() {
                futures.push(endpoint.await_connections().boxed())
            }
        }

        let (item_resolved, _ready_future_index, _remaining_futures) = select_all(futures).await;

        Ok(item_resolved?)
    }

    pub async fn await_timeout(&self) -> (EndpointId, SocketAddr) {
        let mut futures = Vec::new();

        for (_, endpoint) in self.endpoints.iter() {
            if endpoint.has_connections() {
                futures.push(endpoint.await_connection_timeouts().boxed())
            }
        }

        let (item_resolved, _ready_future_index, _remaining_futures) = select_all(futures).await;

        item_resolved
    }

    pub fn remove_connection(&mut self, id: EndpointId, address: SocketAddr) {
        if let Some(endpoint) = self.endpoints.get_mut(&id) {
            info!(
                "Deadline exceeded. Removing Connection: {} from Endpoint: {}",
                id, address
            );
            endpoint.connections.remove(&address).unwrap();
        }

        let mut to_be_removed = Vec::new();

        for (id, endpoint) in self.endpoints.iter() {
            if !endpoint.has_connections() {
                // All connections has been removed on this endpoint and it should therefore
                // be removed
                to_be_removed.push(id.clone())
            }
        }

        for id in to_be_removed {
            self.endpoints.remove(&id).unwrap();

            // Iterate self.routes and remove where the key is equal to id
            self.routes.retain(|_key, value| value != &id)
        }
    }

    pub async fn await_packet_sorters(&self) -> EndpointId {
        let mut futures = Vec::new();

        for (_, endpoint) in self.endpoints.iter() {
            if endpoint.has_connections() {
                futures.push(endpoint.await_sequencer_deadline().boxed())
            }
        }

        let (item_resolved, _ready_future_index, _remaining_futures) = select_all(futures).await;

        item_resolved
    }

    pub async fn handle_packet_sorter_deadline(&mut self, endpoint_id: EndpointId) {
        info!("Handling sorter deadline for {}", endpoint_id);
        if let Some(endpoint) = self.endpoints.get_mut(&endpoint_id) {
            endpoint.packet_sorter.advance_queue().await
        }
    }
}


#[cfg(test)]
mod tests {
    use uuid::Uuid;
    use common::messages;
    use crate::connection_manager::ConnectionManager;

    #[test]
    fn handle_hello_from_empty() {
        smol::block_on(async {
            let mut conman = ConnectionManager::new("127.0.0.1:0".parse().unwrap(), 1);

            let hello = messages::Hello { id: 154, session_id: Uuid::parse_str("47ce9f06-a692-4463-8075-d0033d1b7229").unwrap() };

            let hello_message = messages::Messages::Hello(hello);
            let serialized = bincode::serialize(&hello_message).unwrap();

            conman.handle_hello(serialized, "127.0.0.2:123".parse().unwrap()).await;

            assert!(conman.has_endpoints());
            assert_eq!(conman.endpoints.len(), 1);

            let endpoints = conman.endpoints.get(&154).unwrap();

            assert_eq!(endpoints.session_id, Uuid::parse_str("47ce9f06-a692-4463-8075-d0033d1b7229").unwrap());
        });
    }
    #[test]
    fn handle_hello_from_overwrite() {
        smol::block_on(async {
            let uuids = vec![
                Uuid::parse_str("47ce9f06-a692-4463-8075-d0033d1b7229").unwrap(),
                Uuid::parse_str("deadbeef-a692-4463-8075-d0033d1b7229").unwrap()
            ];

            let mut conman = ConnectionManager::new("127.0.0.1:0".parse().unwrap(), 1);

            for uuid in uuids {
                let hello = messages::Hello { id: 154, session_id: uuid };

                let hello_message = messages::Messages::Hello(hello);
                let serialized = bincode::serialize(&hello_message).unwrap();

                conman.handle_hello(serialized, "127.0.0.2:123".parse().unwrap()).await;
            }

            assert!(conman.has_endpoints());
            assert_eq!(conman.endpoints.len(), 1);

            let endpoints = conman.endpoints.get(&154).unwrap();

            assert_eq!(endpoints.session_id, Uuid::parse_str("deadbeef-a692-4463-8075-d0033d1b7229").unwrap());
        });
    }

    #[test]
    fn handle_hello_refuse_overwrite_session_reuse() {
        smol::block_on(async {
            let uuids = vec![
                Uuid::parse_str("47ce9f06-a692-4463-8075-d0033d1b7229").unwrap(),
                Uuid::parse_str("deadbeef-a692-4463-8075-d0033d1b7229").unwrap(),
                Uuid::parse_str("47ce9f06-a692-4463-8075-d0033d1b7229").unwrap(),
            ];

            let mut conman = ConnectionManager::new("127.0.0.1:0".parse().unwrap(), 1);

            for uuid in uuids {
                let hello = messages::Hello { id: 154, session_id: uuid };

                let hello_message = messages::Messages::Hello(hello);
                let serialized = bincode::serialize(&hello_message).unwrap();

                conman.handle_hello(serialized, "127.0.0.2:123".parse().unwrap()).await;
            }

            assert!(conman.has_endpoints());
            assert_eq!(conman.endpoints.len(), 1);

            let endpoints = conman.endpoints.get(&154).unwrap();

            assert_eq!(endpoints.session_id, Uuid::parse_str("deadbeef-a692-4463-8075-d0033d1b7229").unwrap());
        });
    }
}