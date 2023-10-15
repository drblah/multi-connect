use anyhow::Result;
use common::messages::{EndpointId, HelloAck, Messages};
use futures::future::select_all;
use futures::StreamExt;
use smol::future::FutureExt;
use smol::lock::Mutex;
use smol::net::UdpSocket;
use smol::Async;
use socket2::SockAddr;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, SystemTime};

#[derive(Debug, PartialEq)]
enum ConnectionState {
    Startup,
    Connected,
    Disconnected,
}

#[derive(Debug)]
struct Connection {
    socket: UdpSocket,
    last_hello: SystemTime,
    deadline_ticker: Mutex<smol::Timer>,
    state: ConnectionState,
    buffer: Mutex<[u8; 65535]>,
}

impl Connection {
    fn new(socket: UdpSocket) -> Connection {
        Connection {
            socket,
            last_hello: SystemTime::now(),
            deadline_ticker: Mutex::new(smol::Timer::after(Duration::from_secs(1))),
            state: ConnectionState::Startup,
            buffer: Mutex::new([0; 65535]),
        }
    }

    pub fn refresh_hello(&mut self) {
        self.last_hello = SystemTime::now();
    }

    pub async fn read(&self) -> Result<Vec<u8>> {
        let mut buffer_lock = self.buffer.lock().await;
        let message_length = self.socket.recv(buffer_lock.as_mut_slice()).await?;

        Ok(buffer_lock[..message_length].to_vec())
    }

    pub async fn write(&self, packet: Vec<u8>) {
        let _len = self.socket.send(&packet).await.unwrap();
    }

    pub async fn await_deadline(&self) -> SocketAddr {
        let mut deadline_lock = self.deadline_ticker.lock().await;

        deadline_lock.next().await;
        self.socket.peer_addr().unwrap()
    }
}

struct Endpoint {
    id: EndpointId,
    connections: HashMap<SocketAddr, Connection>,
    tx_counter: u64,
    rx_counter: u64,
}

impl Endpoint {
    fn new(id: EndpointId) -> Self {
        Endpoint {
            id,
            connections: HashMap::new(),
            tx_counter: 0,
            rx_counter: 0,
        }
    }

    async fn add_connection(
        &mut self,
        source_address: SocketAddr,
        local_address: SocketAddr,
    ) -> Result<(), std::io::Error> {
        // We already know the connection, so we update the last seen time
        if let Some(connection) = self.connections.get_mut(&source_address) {
            connection.last_hello = SystemTime::now()
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

    async fn await_connections(&self) -> Result<(EndpointId, Vec<u8>)> {
        let mut futures = Vec::new();

        for (_, connection) in self.connections.iter() {
            futures.push(connection.read().boxed())
        }

        let (item_resolved, ready_future_index, _remaining_futures) = select_all(futures).await;

        Ok((self.id, item_resolved?))
    }

    async fn await_connection_deadlines(&self) -> (EndpointId, SocketAddr) {
        let mut futures = Vec::new();

        for (_, connection) in self.connections.iter() {
            futures.push(connection.await_deadline().boxed())
        }

        let (item_resolved, ready_future_index, _remaining_futures) = select_all(futures).await;

        (self.id, item_resolved)
    }

    pub fn has_connections(&self) -> bool {
        self.connections.len() != 0
    }
}

pub struct ConnectionManager {
    endpoints: HashMap<EndpointId, Endpoint>,
    local_address: SocketAddr,
    pub own_id: EndpointId,
}

impl ConnectionManager {
    pub fn new(local_address: SocketAddr, own_id: EndpointId) -> ConnectionManager {
        ConnectionManager {
            endpoints: HashMap::new(),
            local_address,
            own_id,
        }
    }

    pub async fn handle_hello(&mut self, message: Vec<u8>, source_address: SocketAddr) {
        if let Ok(decoded) = bincode::deserialize::<Messages>(&message) {
            println!("Hello decoded!");
            println!("{:?}", decoded);

            if let Messages::Hello(decoded) = decoded {
                // We know the endpoint already
                if let Some(endpoint) = self.endpoints.get_mut(&decoded.id) {
                    match endpoint
                        .add_connection(source_address, self.local_address)
                        .await
                    {
                        Ok(()) => {
                            println!(
                                "Added {} as a new connection to EP: {}",
                                source_address, decoded.id
                            );

                            endpoint.acknowledge(self.own_id).await
                        }
                        Err(e) => {
                            eprintln!(
                                "Failed to add {} as new connection to EP: {} due to error: {}",
                                source_address,
                                decoded.id,
                                e.to_string()
                            )
                        }
                    }
                } else {
                    // Endpoint is unknown. We must create it first
                    let mut new_endpoint = Endpoint::new(decoded.id.clone());
                    match new_endpoint
                        .add_connection(source_address, self.local_address)
                        .await
                    {
                        Ok(()) => {
                            println!(
                                "Added {} as a new connection to EP: {}",
                                source_address, decoded.id
                            )
                        }
                        Err(e) => {
                            eprintln!(
                                "Failed to add {} as new connection to EP: {} due to error: {}",
                                source_address,
                                decoded.id,
                                e.to_string()
                            )
                        }
                    }

                    new_endpoint.acknowledge(self.own_id).await;
                    self.endpoints.insert(decoded.id, new_endpoint);
                }
            }
        } else {
            println!("Failed to decode. But lets say hi anyways :^)");
        }
    }

    pub fn has_endpoints(&self) -> bool {
        self.endpoints.len() != 0
    }

    pub async fn await_incoming(&self) -> Result<(EndpointId, Vec<u8>)> {
        let mut futures = Vec::new();

        for (_, endpoint) in self.endpoints.iter() {
            if endpoint.has_connections() {
                futures.push(endpoint.await_connections().boxed())
            }
        }

        let (item_resolved, ready_future_index, _remaining_futures) = select_all(futures).await;

        Ok(item_resolved?)
    }

    pub async fn await_timeout(&self) -> (EndpointId, SocketAddr) {
        let mut futures = Vec::new();

        for (_, endpoint) in self.endpoints.iter() {
            if endpoint.has_connections() {
                futures.push(endpoint.await_connection_deadlines().boxed())
            }
        }

        let (item_resolved, ready_future_index, _remaining_futures) = select_all(futures).await;

        item_resolved
    }

    pub fn remove_connection(&mut self, id: EndpointId, address: SocketAddr) {
        if let Some(endpoint) = self.endpoints.get_mut(&id) {
            println!(
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
        }
    }
}
