use std::future::Future;
use std::net::SocketAddr;
use smol::{Async, net, future, future::FutureExt};
use futures::select;
use socket2::SockAddr;
use anyhow::Result;
use common::messages::EndpointId;
use crate::connection_manager::ConnectionManager;

mod connection_manager;

enum Events {
    NewConnection((usize, SocketAddr)),
    NewEstablishedMessage(Result<(EndpointId, Vec<u8>)>),
    ConnectionTimeout((EndpointId, SocketAddr))
}

fn main() {
    smol::block_on(async {
        let server_socket =
            socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, None).unwrap();
        server_socket.set_reuse_address(true).unwrap();
        let socketaddr: SocketAddr = "127.0.0.2:40000".parse().unwrap();

        server_socket.bind(&SockAddr::from(socketaddr)).unwrap();

        let server_socket = std::net::UdpSocket::from(server_socket);
        server_socket.set_nonblocking(true).unwrap();

        let server_socket = net::UdpSocket::from(Async::try_from(server_socket).unwrap());

        let mut buffer = [0u8; 65535];

        let mut conman = ConnectionManager::new(socketaddr, 1);

        loop {

            if conman.has_endpoints() {
                // In case we have active connections. Await new connection attempts and await messages
                // from existing connections.

                let wrapped_server = async { Events::NewConnection(server_socket.recv_from(&mut buffer).await.unwrap()) };
                let wrapped_endpoints = async { Events::NewEstablishedMessage( conman.await_incoming().await ) };
                let wrapped_timeout = async { Events::ConnectionTimeout( conman.await_timeout().await ) };

                match wrapped_server.race(wrapped_endpoints).race(wrapped_timeout).await {
                    Events::NewConnection((len, addr)) => {
                        conman.handle_hello(buffer[..len].to_vec(), addr).await;
                    }
                    Events::NewEstablishedMessage(result) => {
                        match result {
                            Ok((endpointid, message)) => {
                                println!("Endpoint: {}, produced message: {:?}", endpointid, message)
                            }
                            Err(e) => {
                                eprintln!("Encountered error: {}", e.to_string())
                            }

                        }
                    }
                    Events::ConnectionTimeout((endpoint, socket)) => {
                        conman.remove_connection(endpoint, socket)
                    }

                }

            } else {
                // In case we have no connections. Only await new ones
                println!("Server has no active Endpoints. Waiting...");

                let wrapped_server = async { Events::NewConnection(server_socket.recv_from(&mut buffer).await.unwrap()) };

                match wrapped_server.await {
                    Events::NewConnection((len, addr)) => {
                        conman.handle_hello(buffer[..len].to_vec(), addr).await;
                    }
                    _ => continue
                }
            }
        }

    })


}