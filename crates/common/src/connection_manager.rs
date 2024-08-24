use anyhow::Result;
use crate::messages::{DuplicationCommands, EndpointId, Messages, Packet};
use futures::future::select_all;
use smol::future::FutureExt;
use std::collections::{HashMap};
use std::net::{IpAddr, SocketAddr};
use std::ops::AddAssign;
use aes_gcm_siv::{Aes256GcmSiv, KeyInit};
use aes_gcm_siv::aead::Aead;
use etherparse::{InternetSlice, SlicedPacket};
use log::{debug, error, info, warn};
use tokio_tun::Tun;
use uuid::Uuid;
use crate::endpoint::{ConnectionEntry, Endpoint};
use crate::{ConnectionInfo, endpoint, make_socket, messages};
use crate::interface_logger::InterfaceLogger;
use crate::router::{Route, Router};



pub struct ConnectionManager {
    endpoints: HashMap<EndpointId, Endpoint>,
    local_address: SocketAddr,
    pub own_id: EndpointId,
    session_history: Vec<Uuid>,
    routes: Router,
    pub tun_address: IpAddr,
    own_static_routes: Option<Vec<Route>>,
    connection_timeout: u64,
    packet_sorter_deadline: u64,
    encrpytion_key: [u8; 32]
}

impl ConnectionManager {
    pub fn new(local_address: SocketAddr, own_id: EndpointId, tun_address: IpAddr, own_static_routes: Option<Vec<Route>>, connection_timeout: u64, packet_sorter_deadline: u64, encrpytion_key: [u8; 32]) -> ConnectionManager {
        ConnectionManager {
            endpoints: HashMap::new(),
            local_address,
            own_id,
            session_history: Vec::new(),
            routes: Router::new(),
            tun_address,
            own_static_routes,
            connection_timeout,
            packet_sorter_deadline,
            encrpytion_key
        }
    }

    /// Handles new incomming connections and keeps existing connections alive
    pub async fn handle_hello(&mut self, message: Vec<u8>, source_address: SocketAddr, interface_name: String) {
        let cipher = Aes256GcmSiv::new_from_slice(&self.encrpytion_key).unwrap();
        let maybe_decrypted_message = match bincode::deserialize::<messages::EncryptedMessage>(&message) {
            Ok(encrypted_msg) => {
                match cipher.decrypt((&encrypted_msg.nonce).into(), encrypted_msg.message.as_ref()) {
                    Ok(decrypted) => {
                        Some(decrypted)
                    }
                    Err(e) => {
                        error!("Failed to decrypt EncryptedMessage from: {}: {}", source_address, e);
                        None
                    }
                }
            }
            Err(e) => {
                error!("Failed to decode EncryptedMessage from: {}: {}", source_address, e);
                None
            }
        };

        if let Some(decrypted_message) = maybe_decrypted_message {
            if let Ok(decoded) = bincode::deserialize::<Messages>(&decrypted_message) {
                info!("Hello decoded!");
                info!("{:?}", decoded);

                if let Messages::Hello(decoded) = decoded {

                    // Ensure endpoint exists
                    if !self.endpoints.contains_key(&decoded.id) {
                        let new_endpoint = Endpoint::new(decoded.id.clone(), decoded.session_id.clone(), self.packet_sorter_deadline);
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
                                *endpoint = Endpoint::new(decoded.id.clone(), decoded.session_id.clone(), self.packet_sorter_deadline);
                                self.session_history.push(decoded.session_id.clone());
                            }
                        }

                        // Add connections
                        match endpoint
                            .add_connection(source_address, interface_name, self.local_address, self.connection_timeout, &self.encrpytion_key)
                            .await
                        {
                            Ok(()) => {
                                info!(
                                    "Added {} as a new connection to EP: {}",
                                    source_address, decoded.id
                                );

                                endpoint.acknowledge(self.own_id, endpoint.session_id, &self.own_static_routes).await
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

                        endpoint.acknowledge(self.own_id, endpoint.session_id, &self.own_static_routes).await;

                        // Add route
                        if let Some(new_static_routes) = decoded.static_routes {
                            for route in new_static_routes {
                                self.routes.insert_route(route, decoded.id)
                            }
                        }
                    }
                }
            } else {
                error!("Failed to decode. But lets say hi anyways :^)");
            }
        }
    }

    pub async fn handle_established_message(&mut self, read_info: endpoint::ReadInfo, interface_logger: &mut Option<InterfaceLogger>) {
        if let Ok(decoded) = bincode::deserialize::<Messages>(&read_info.connection_read_info.packet_bytes) {
            match decoded {
                Messages::Packet(packet) => {
                    if let Some(endpoint) = self.endpoints.get_mut(&read_info.endpoint_id) {
                        if let Some(interface_logger) = interface_logger {
                            interface_logger.add_log_line(read_info.endpoint_id, packet.seq, read_info.connection_read_info.interface_name).await;
                        }

                        endpoint.packet_sorter.insert_packet(packet).await;
                    }
                }
                Messages::Hello(hello) => {
                    if let Some(endpoint) = self.endpoints.get_mut(&hello.id) {
                        endpoint.hello_path_latency.insert_new_timestamp(hello.hello_seq);
                        endpoint.add_connection(read_info.connection_read_info.source_address, read_info.connection_read_info.interface_name, self.local_address, self.connection_timeout, &self.encrpytion_key).await.unwrap();
                        endpoint.acknowledge( self.own_id, endpoint.session_id, &self.own_static_routes).await;
                        debug!("Hello latency-diff: {:.2} - Hello-ack latency-diff: {:.2}", endpoint.hello_path_latency.estimate_path_delay_difference().as_millis(), endpoint.hello_ack_path_latency.estimate_path_delay_difference().as_millis())
                    } else {
                        error!("Received hello from unknown endpoint: {}", hello.id);
                    }
                }
                Messages::HelloAck(hello_ack) => {
                    info!("Received HelloAck: {:?}", hello_ack);
                    info!("Endpoints: {:?}", self.endpoints.keys());
                    if self.endpoints.contains_key(&hello_ack.id) {
                        {
                            let endpoint = self.endpoints.get_mut(&hello_ack.id).unwrap();
                            endpoint.hello_ack_path_latency.insert_new_timestamp(hello_ack.hello_ack_seq);
                            debug!("Hello latency-diff: {:.2} - Hello-ack latency-diff: {:.2}", endpoint.hello_path_latency.estimate_path_delay_difference().as_millis(), endpoint.hello_ack_path_latency.estimate_path_delay_difference().as_millis())
                        }

                        if let Some(new_routes) = hello_ack.static_routes {
                            for route in new_routes {
                                self.routes.insert_route(route, hello_ack.id)
                            }
                        }

                        for connection_entry in self.endpoints.get_mut(&hello_ack.id).unwrap().connections.iter_mut() {
                            if connection_entry.interface_address == read_info.connection_read_info.source_address && connection_entry.interface_name == read_info.connection_read_info.interface_name {
                                if connection_entry.connection.state == crate::connection::ConnectionState::Startup {
                                    connection_entry.connection.state = crate::connection::ConnectionState::Connected;
                                }

                                if connection_entry.connection.state == crate::connection::ConnectionState::Connected {
                                    connection_entry.connection.reset_hello_timeout().await;
                                }
                            }
                        }

                    } else {
                        error!("Received HelloAck from unknown endpoint: {}", hello_ack.id);
                    }

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
                let new_endpoint = Endpoint::new(destination_endpoint_id, Uuid::new_v4(), self.packet_sorter_deadline);
                self.endpoints.insert(destination_endpoint_id, new_endpoint);
                self.endpoints.get_mut(&destination_endpoint_id).unwrap()
            }
        };

        let hello = Messages::Hello(messages::Hello { id: self.own_id, session_id: new_endpoint.session_id, static_routes: self.own_static_routes.clone(), hello_seq: new_endpoint.hello_counter });
        new_endpoint.hello_counter.add_assign(1);
        let encoded = bincode::serialize(&hello).unwrap();

        //new_socket.send(&encoded).await?;
        let mut new_connection = crate::connection::Connection::new(new_socket, Some(interface_name.clone()), self.connection_timeout, &self.encrpytion_key);
        new_connection.write(encoded).await?;
        new_connection.state = crate::connection::ConnectionState::Startup;

        let connection_entry = ConnectionEntry {
            interface_name,
            interface_address: destination_address,
            connection: new_connection,
        };

        new_endpoint.connections.push(connection_entry);

        Ok(())
    }


    fn get_route(&self, packet_bytes: &[u8]) -> Option<Vec<EndpointId>> {
        match SlicedPacket::from_ip(packet_bytes) {
            Err(e) => {
                error!("Error learning tun_ip: {}", e);
                None
            }

            Ok(ip_packet) => {
                match ip_packet.ip {
                    Some(InternetSlice::Ipv4(ipheader, ..)) => {
                        let ipv4 =  ipheader.destination_addr();
                        return Some(self.routes.lookup(&ipv4))

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
        if let Some(endpoint_ids) = self.get_route(packet) {
            for endpoint_id in endpoint_ids {

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
                for connection_entry in &mut endpoint.connections {
                    if connection_entry.connection.is_enabled() {
                        match connection_entry.connection.write(serialized_pakcet.clone()).await {
                            Ok(_len) => {
                                continue
                            }
                            Err(e) => {
                                match e.kind() {
                                    std::io::ErrorKind::ConnectionRefused => {
                                        error!("Connection refused. Removing connection: {}, {}", connection_entry.interface_name, connection_entry.interface_address);
                                        connection_entry.connection.state = crate::connection::ConnectionState::Disconnected;
                                    }
                                    _ => {
                                        error!("Error while writing to socket: {}", e.to_string());
                                    }
                                }
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

    pub async fn await_incoming(&self) -> Result<endpoint::ReadInfo> {
        let mut futures = Vec::new();

        for (_, endpoint) in self.endpoints.iter() {
            if endpoint.has_connections() {
                futures.push(endpoint.await_connections().boxed())
            }
        }

        let (item_resolved, _ready_future_index, _remaining_futures) = select_all(futures).await;

        Ok(item_resolved?)
    }

    pub async fn await_timeout(&self) -> (EndpointId, String, SocketAddr) {
        let mut futures = Vec::new();

        for (_, endpoint) in self.endpoints.iter() {
            if endpoint.has_connections() {
                futures.push(endpoint.await_connection_timeouts().boxed())
            }
        }

        let (item_resolved, _ready_future_index, _remaining_futures) = select_all(futures).await;

        item_resolved
    }

    pub async fn await_endpoint_sorted_packets(&self) -> (EndpointId, Option<Packet>) {
        let mut futures = Vec::new();

        for (_, endpoint) in self.endpoints.iter() {
            if endpoint.has_connections() {
                futures.push(endpoint.await_sorted_packet().boxed())
            }
        }

        let (item_resolved, _ready_future_index, _remaining_futures) = select_all(futures).await;

        item_resolved
    }

    pub fn remove_connection(&mut self, id: EndpointId, interface_name: String, address: SocketAddr) {
        if let Some(endpoint) = self.endpoints.get_mut(&id) {
            info!(
                "Removing Connection: {} from Endpoint: {}",
                id, address
            );
            if let Some(index) = endpoint.connections.iter().position(|connection_entry| connection_entry.interface_name == interface_name && connection_entry.interface_address == address ) {
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
            self.routes.remove_route_to_endpoint(id)
        }
    }

    pub fn remove_disconnected(&mut self) {
        let mut to_be_removed = Vec::new();

        for (endpoint_id, endpoint) in &self.endpoints {
            for connection_entry in &endpoint.connections {
                if connection_entry.connection.state == crate::connection::ConnectionState::Disconnected {
                    to_be_removed.push((endpoint_id.clone(), (connection_entry.interface_name.clone(), connection_entry.interface_address.clone())));
                }
            }
        }

        for (endpoint_id, (interface_name, address)) in to_be_removed {
            self.remove_connection(endpoint_id, interface_name, address)
        }
    }

    pub async fn await_packet_sorters(&self) -> EndpointId {
        let mut futures = Vec::new();

        for (_, endpoint) in self.endpoints.iter() {
            if endpoint.has_connections() {
                futures.push(endpoint.await_packet_sorter_deadline().boxed())
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
            let hello = Messages::Hello(messages::Hello { id: self.own_id, session_id: endpoint.session_id, static_routes: self.own_static_routes.clone(), hello_seq: endpoint.hello_counter });
            endpoint.hello_counter.add_assign(1);
            let encoded = bincode::serialize(&hello).unwrap();

            for connection_entry in &mut endpoint.connections {
                if connection_entry.connection.state == crate::connection::ConnectionState::Connected || connection_entry.connection.state == crate::connection::ConnectionState::Startup {
                    info!("Greeting on connection: {:?}", connection_entry.connection.get_name_address_touple());

                    match connection_entry.connection.write(encoded.clone()).await {
                        Ok(_) => {}
                        Err(e) => {
                            match e.kind() {
                                std::io::ErrorKind::NetworkUnreachable => {
                                    error!("Network unreachable. Removing connection: {}, {}", connection_entry.interface_name, connection_entry.interface_address);
                                    connection_entry.connection.state = crate::connection::ConnectionState::Disconnected;
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
                let connection_touple = (connection_info.interface_name.clone(), connection_info.destination_address);

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

    pub fn handle_selective_duplication_command(&mut self, duplication_command_bytes: &[u8]) {

        match serde_json::from_slice(duplication_command_bytes) {
            Ok(duplication_command) => {
                match duplication_command {
                    DuplicationCommands::InterfaceState(state) => {
                        info!("Received duplication command: {:?}", state);
                        for (_, ep) in &mut self.endpoints {
                            if state.enabled {
                                info!("Enabling interface: {}", state.interface_name);
                                ep.enable_interface(&state.interface_name)
                            } else {
                                info!("Disabling interface: {}", state.interface_name);
                                ep.disable_interface(&state.interface_name)
                            }
                        }
                    }
                }
            }
            Err(e) => {
                error!("Got a duplication command, but failed to decode: {}", e)
            }
        }
    }
}





#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use uuid::Uuid;
    use crate::connection::ConnectionState;
    use crate::{connection, endpoint, messages};
    use crate::connection_manager::ConnectionManager;
    use crate::messages::{HelloAck, Messages};

    #[test]
    fn handle_hello_from_empty() {
        smol::block_on(async {
            let conman_tun_address = "127.0.0.1".parse().unwrap();
            let mut conman = ConnectionManager::new("127.0.0.1:0".parse().unwrap(), 1, conman_tun_address, None, 10000, 100);


            //let hello_tun_address = "127.0.0.2".parse().unwrap();
            let hello = messages::Hello { id: 154, session_id: Uuid::parse_str("47ce9f06-a692-4463-8075-d0033d1b7229").unwrap(), static_routes: conman.own_static_routes.clone(), hello_seq: 0 };

            let hello_message = messages::Messages::Hello(hello);
            let serialized = bincode::serialize(&hello_message).unwrap();
            let server_interface_name = "DYN-interface".to_string();

            conman.handle_hello(serialized, "127.0.0.2:123".parse().unwrap(), server_interface_name.clone()).await;

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
            let conman_tun_address = "127.0.0.1".parse().unwrap();
            let mut conman = ConnectionManager::new("127.0.0.1:0".parse().unwrap(), 1, conman_tun_address, None, 10000, 100);

            let server_interface_name = "DYN-interface".to_string();

            for uuid in uuids {
                //let hello_tun_address = "127.0.0.2".parse().unwrap();
                let hello = messages::Hello { id: 154, session_id: uuid, static_routes: conman.own_static_routes.clone(), hello_seq: 0 };

                let hello_message = messages::Messages::Hello(hello);
                let serialized = bincode::serialize(&hello_message).unwrap();

                conman.handle_hello(serialized, "127.0.0.2:123".parse().unwrap(), server_interface_name.clone()).await;
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
            let conman_tun_address = "127.0.0.1".parse().unwrap();
            let mut conman = ConnectionManager::new("127.0.0.1:0".parse().unwrap(), 1, conman_tun_address, None, 10000, 100);
            let server_interface_name = "DYN-interface".to_string();

            for uuid in uuids {
                //let hello_tun_address = "127.0.0.2".parse().unwrap();
                let hello = messages::Hello { id: 154, session_id: uuid, static_routes: conman.own_static_routes.clone(), hello_seq: 0 };

                let hello_message = messages::Messages::Hello(hello);
                let serialized = bincode::serialize(&hello_message).unwrap();

                conman.handle_hello(serialized, "127.0.0.2:123".parse().unwrap(), server_interface_name.clone()).await;
            }

            assert!(conman.has_endpoints());
            assert_eq!(conman.endpoints.len(), 1);

            let endpoints = conman.endpoints.get(&154).unwrap();

            assert_eq!(endpoints.session_id, Uuid::parse_str("deadbeef-a692-4463-8075-d0033d1b7229").unwrap());
        });
    }

    #[test]
    fn connection_manager_client_single_connection() {
        smol::block_on(async {
            let conman_tun_address = "127.0.0.1".parse().unwrap();
            let mut conman = ConnectionManager::new("127.0.0.1:0".parse().unwrap(), 1, conman_tun_address, None, 10000, 100);

            let own_address: SocketAddr = "127.0.0.1:0".parse().unwrap();
            let server_id = 1337;
            let server_address = "127.0.0.2:1337".parse().unwrap();

            conman.create_new_connection(
                "lo".to_string(),
                own_address.clone(),
                server_address,
                server_id
            ).await.unwrap();

            let session_id = conman.endpoints.iter().next().unwrap().1.session_id;
            //let server_tun_address = "127.0.100.1".parse().unwrap();

            // Make fake HelloAck messages from the server
            let ack = HelloAck { id: server_id, session_id, static_routes: conman.own_static_routes.clone(), hello_ack_seq: 0 };
            let ack_message = Messages::HelloAck(ack);

            let serialized = bincode::serialize(&ack_message).unwrap();

            let read_info = endpoint::ReadInfo {
                connection_read_info: connection::ReadInfo {
                    packet_bytes: serialized,
                    source_address: server_address,
                    interface_name: "lo".to_string(),
                },
                endpoint_id: server_id,
            };


            conman.handle_established_message(
                read_info,
                &mut None
            ).await;

            // The connection should now be in connected state
            let connection_status = &conman.endpoints.iter().next().unwrap().1.connections.first().unwrap().connection.state;
            assert_eq!(*connection_status, ConnectionState::Connected);

        });
    }

    #[test]
    fn connection_manager_client_multiple_connections() {
        smol::block_on(async {
            let conman_tun_address = "127.0.0.1".parse().unwrap();
            let mut conman = ConnectionManager::new("127.0.0.1:0".parse().unwrap(), 1, conman_tun_address, None, 10000, 100);

            let own_address: SocketAddr = "127.0.0.1:0".parse().unwrap();
            let server_id = 1337;
            let server_address = "127.0.0.2:1337".parse().unwrap();

            conman.create_new_connection(
                "lo".to_string(),
                own_address.clone(),
                server_address,
                server_id
            ).await.unwrap();

            conman.create_new_connection(
                "lo".to_string(),
                "127.0.0.5:0".parse().unwrap(),
                server_address,
                server_id
            ).await.unwrap();

            let session_id = conman.endpoints.iter().next().unwrap().1.session_id;
            //let server_tun_address = "127.0.100.1".parse().unwrap();

            // Make fake HelloAck messages from the server
            let ack = HelloAck { id: server_id, session_id, static_routes: conman.own_static_routes.clone(), hello_ack_seq: 0 };
            let ack_message = Messages::HelloAck(ack);

            let serialized = bincode::serialize(&ack_message).unwrap();

            let read_info = endpoint::ReadInfo {
                connection_read_info: connection::ReadInfo {
                    packet_bytes: serialized,
                    source_address: server_address,
                    interface_name: "lo".to_string(),
                },
                endpoint_id: server_id,
            };


            conman.handle_established_message(
                read_info,
                &mut None
            ).await;

            // The connections should now be in connected state
            for connection_entity in &conman.endpoints.iter().next().unwrap().1.connections {
                assert_eq!(connection_entity.connection.state, ConnectionState::Connected)
            }
        });
    }
}