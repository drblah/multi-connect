use anyhow::Result;
use crate::messages::{EndpointId, Messages, Packet};
use futures::future::select_all;
use smol::future::FutureExt;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::ops::AddAssign;
use etherparse::{InternetSlice, SlicedPacket};
use log::{error, info, warn};
use tokio_tun::Tun;
use uuid::Uuid;
use crate::endpoint::Endpoint;
use crate::{ConnectionInfo, make_socket, messages};


#[derive(Debug)]
pub struct ConnectionManager {
    endpoints: HashMap<EndpointId, Endpoint>,
    local_address: SocketAddr,
    pub own_id: EndpointId,
    session_history: Vec<Uuid>,
    routes: HashMap<IpAddr, EndpointId>,
    pub tun_address: IpAddr
}

impl ConnectionManager {
    pub fn new(local_address: SocketAddr, own_id: EndpointId, tun_address: IpAddr) -> ConnectionManager {
        ConnectionManager {
            endpoints: HashMap::new(),
            local_address,
            own_id,
            session_history: Vec::new(),
            routes: HashMap::new(),
            tun_address
        }
    }

    /// Handles new incomming connections and keeps existing connections alive
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

                            endpoint.acknowledge(self.own_id, endpoint.session_id, self.tun_address).await
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

                    endpoint.acknowledge(self.own_id, endpoint.session_id, self.tun_address).await;

                    // Add route
                    self.routes.insert(decoded.tun_address, decoded.id);
                }
            }
        } else {
            error!("Failed to decode. But lets say hi anyways :^)");
        }
    }

    pub async fn handle_established_message(&mut self, message: Vec<u8>, endpoint_id: EndpointId, source_address: SocketAddr, tun_dev: &mut Tun) {
        if let Ok(decoded) = bincode::deserialize::<Messages>(&message) {
            match decoded {
                Messages::Packet(packet) => {
                    if let Some(endpoint) = self.endpoints.get_mut(&endpoint_id) {
                        endpoint.packet_sorter.insert_packet(packet).await;

                        if endpoint.packet_sorter.have_next_packet() {
                            let packet = endpoint.packet_sorter.get_next_packet().await.unwrap();
                            self.handle_tunnel_message(packet, endpoint_id, tun_dev).await;
                        }

                    }
                }
                Messages::Hello(hello) => {
                    if let Some(endpoint) = self.endpoints.get_mut(&hello.id) {
                        endpoint.add_connection(source_address, self.local_address).await.unwrap();
                        endpoint.acknowledge( self.own_id, endpoint.session_id, self.tun_address).await;
                    } else {
                        error!("Received hello from unknown endpoint: {}", hello.id);
                    }
                }
                Messages::HelloAck(hello_ack) => {
                    info!("Received HelloAck: {:?}", hello_ack);
                    info!("Endpoints: {:?}", self.endpoints.keys());
                    if self.endpoints.contains_key(&hello_ack.id) {
                        info!("Learning route from HelloAck: {}, {}", hello_ack.id, hello_ack.tun_address);
                        self.routes.insert(hello_ack.tun_address, hello_ack.id);
                        for (_, connection) in self.endpoints.get_mut(&hello_ack.id).unwrap().connections.iter_mut() {
                            if connection.state == crate::connection::ConnectionState::Startup {
                                connection.state = crate::connection::ConnectionState::Connected;
                            }

                            if connection.state == crate::connection::ConnectionState::Connected {
                                connection.reset_hello_timeout().await;
                            }
                        }
                    } else {
                        error!("Received HelloAck from unknown endpoint: {}", hello_ack.id);
                    }

                }
                Messages::KeepAlive(_) => {
                    todo!()
                }
            }
        }
    }

    pub async fn handle_tunnel_message(&mut self, packet: Packet, _endpoint_id: EndpointId, tun_dev: &mut Tun) {
        tun_dev.send(
            packet.bytes.as_slice()
        ).await.unwrap();
    }

    pub async fn create_new_connection(&mut self, interface_name: String, local_address: SocketAddr, destination_address: SocketAddr, destination_endpoint_id: EndpointId) -> Result<()> {

        let own_ipv4 = match local_address {
            SocketAddr::V4(addr) => addr.ip().clone(),
            SocketAddr::V6(_) => panic!("IPv6 not supported")
        };

        let new_socket = make_socket(&interface_name, Some(own_ipv4), None, true)?;

        new_socket.connect(destination_address).await?;

        let new_endpoint = match self.endpoints.get_mut(&destination_endpoint_id) {
            Some(endpoint) => endpoint,
            None => {
                let new_endpoint = Endpoint::new(destination_endpoint_id, Uuid::new_v4());
                self.endpoints.insert(destination_endpoint_id, new_endpoint);
                self.endpoints.get_mut(&destination_endpoint_id).unwrap()
            }
        };

        let hello = Messages::Hello(messages::Hello { id: self.own_id, session_id: new_endpoint.session_id, tun_address: self.tun_address });
        let encoded = bincode::serialize(&hello).unwrap();

        new_socket.send(&encoded).await?;
        let mut new_connection = crate::connection::Connection::new(new_socket, Some(&interface_name));
        new_connection.state = crate::connection::ConnectionState::Startup;
        new_endpoint.connections.push((destination_address, new_connection));

        Ok(())
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
        if self.routes.len() == 0 { return () }

        if let Some(endpoint_id) = Some(self.routes.iter().last().unwrap().1) { // self.get_route(packet) {
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
            for (address, connection) in &mut endpoint.connections {
                match connection.write(serialized_pakcet.clone()).await {
                    Ok(_len) => {
                        continue
                    }
                    Err(e) => {
                        match e.kind() {
                            std::io::ErrorKind::ConnectionRefused => {
                                error!("Connection refused. Removing connection: {}", address);
                                connection.state = crate::connection::ConnectionState::Disconnected;
                            }
                            _ => {
                                error!("Error while writing to socket: {}", e.to_string());
                            }

                        }
                    }
                }
            }

        } else {
            warn!("No route found for packet. Dropping packet");
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
            if let Some(index) = endpoint.connections.iter().position(|(addr, _)| addr == &address) {
                endpoint.connections.remove(index);
            }
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

    pub fn remove_disconnected(&mut self) {
        let mut to_be_removed = Vec::new();

        for (endpoint_id, endpoint) in &self.endpoints {
            for (address, connection) in &endpoint.connections {
                if connection.state == crate::connection::ConnectionState::Disconnected {
                    to_be_removed.push((endpoint_id.clone(), address.clone()));
                }
            }
        }

        for (endpoint_id, address) in to_be_removed {
            self.remove_connection(endpoint_id, address)
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

    pub async fn greet_all_endpoints(&mut self) {
        for (_endpoint_id, endpoint) in &mut self.endpoints {
            let hello = Messages::Hello(messages::Hello { id: self.own_id, session_id: endpoint.session_id, tun_address: self.tun_address });
            let encoded = bincode::serialize(&hello).unwrap();

            for (_address, connection) in &mut endpoint.connections {
                if connection.state == crate::connection::ConnectionState::Connected {


                    match connection.write(encoded.clone()).await {
                        Ok(_) => {}
                        Err(e) => {
                            match e.kind() {
                                std::io::ErrorKind::NetworkUnreachable => {
                                    error!("Network unreachable. Removing connection: {}", _address);
                                    connection.state = crate::connection::ConnectionState::Disconnected;
                                }
                                _ => { error!("Connection encountered unhandled error: {}", e) }
                            }
                        }
                    }
                }
            }
        }
    }


    pub async fn ensure_endpoint_connections(&mut self, endpoint_id: EndpointId, connection_infos: &Vec<ConnectionInfo>) {
        if let Some(endpoint) = self.endpoints.get_mut(&endpoint_id) {
            let endpoint_alive_connections = endpoint.get_alive_connections();

            for connection_info in connection_infos {
                let connection_touple = Some((connection_info.interface_name.clone(), connection_info.destination_address.ip()));

                if !endpoint_alive_connections.contains(&connection_touple) {
                    match self.create_new_connection(
                        connection_info.interface_name.clone(),
                        connection_info.local_address,
                        connection_info.destination_address,
                        connection_info.destination_endpoint_id
                    ).await {
                        Ok(_) => {}
                        Err(e) => {
                            error!("Failed to re-open connection to EP: {} - {} - {}\n{}", connection_info.destination_endpoint_id, connection_info.interface_name, connection_info.destination_address, e)
                        }
                    }
                }
            }
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