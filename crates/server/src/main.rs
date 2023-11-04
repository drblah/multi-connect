use crate::connection_manager::ConnectionManager;
use anyhow::{Result};
use common::messages::EndpointId;
use smol::{future::FutureExt, net, Async};
use socket2::SockAddr;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use async_compat::Compat;
use log::{error, info};
use tokio_tun::{TunBuilder};


mod connection_manager;

enum Events {
    NewConnection((usize, SocketAddr)),
    NewEstablishedMessage(Result<(EndpointId, Vec<u8>, SocketAddr)>),
    ConnectionTimeout((EndpointId, SocketAddr)),
    PacketSorter(EndpointId),
    TunnelPacket(std::io::Result<usize>)
}


fn main() {
    env_logger::init();

    smol::block_on(Compat::new (async {
        let server_socket =
            socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, None).unwrap();
        server_socket.set_reuse_address(true).unwrap();
        let socketaddr: SocketAddr = "172.16.200.4:40000".parse().unwrap();

        server_socket.bind(&SockAddr::from(socketaddr)).unwrap();

        let server_socket = std::net::UdpSocket::from(server_socket);
        server_socket.set_nonblocking(true).unwrap();

        let server_socket = net::UdpSocket::from(Async::try_from(server_socket).unwrap());

        let mut udp_buffer = [0u8; 65535];
        let mut tun_buffer = [0u8; 65535];

        let tun_address: IpAddr = "10.12.0.1".parse().unwrap();

        let tun_address_ipv4 = match tun_address {
            IpAddr::V4(ipv4) => ipv4,
            IpAddr::V6(ipv6) => {
                panic!("Tun address is not an IPv4 address: {}", ipv6)
            }
        };
        //let mut tun_device = common::tun_device::AsyncTun::new("tun0", tun_address, "255.255.255.0".parse().unwrap()).await.unwrap();

        let mut tun = TunBuilder::new()
            .name("")
            .tap(false)
            .packet_info(false)
            .mtu(1424)
            .up()
            .address(tun_address_ipv4)
            .broadcast(Ipv4Addr::BROADCAST)
            .netmask(Ipv4Addr::new(255, 255, 255, 0))
            .try_build()
            .unwrap();


        let mut conman = ConnectionManager::new(socketaddr, 1, tun_address);

        loop {
            if conman.has_endpoints() {
                // In case we have active connections. Await new connection attempts and await messages
                // from existing connections.

                let wrapped_server = async {
                    Events::NewConnection(server_socket.recv_from(&mut udp_buffer).await.unwrap())
                };
                let wrapped_endpoints =
                    async { Events::NewEstablishedMessage(conman.await_incoming().await) };
                let wrapped_connection_timeout =
                    async { Events::ConnectionTimeout(conman.await_timeout().await) };
                let wrapped_packet_sorter = async {
                    Events::PacketSorter(conman.await_packet_sorters().await)
                };
                let wrapped_tunnel_device = async {
                    Events::TunnelPacket(tun.recv(&mut tun_buffer).await)
                };

                match wrapped_server
                    .race(wrapped_endpoints)
                    .race(wrapped_connection_timeout)
                    .race(wrapped_packet_sorter)
                    .race(wrapped_tunnel_device)
                    .await
                {
                    Events::NewConnection((len, addr)) => {
                        conman.handle_hello(udp_buffer[..len].to_vec(), addr).await;
                    }
                    Events::NewEstablishedMessage(result) => match result {
                        Ok((endpointid, message, source_address)) => {
                            //info!("Endpoint: {}, produced message: {:?}", endpointid, message);
                            conman.handle_established_message(message, endpointid, source_address, &mut tun).await;

                        }
                        Err(e) => {
                            error!("Encountered error: {}", e.to_string())
                        }
                    },
                    Events::ConnectionTimeout((endpoint, socket)) => {
                        conman.remove_connection(endpoint, socket)
                    }
                    Events::PacketSorter(endpoint_id) => {
                        conman.handle_packet_sorter_deadline(endpoint_id).await;
                    }
                    Events::TunnelPacket(maybe_packet) => {
                        match maybe_packet {
                            Ok(packet_length) => {
                                conman.handle_packet_from_tun(&tun_buffer[..packet_length]).await;
                            }
                            Err(e) => error!("Error while reading from tun device: {}", e.to_string())
                        }
                    }
                }
            } else {
                // In case we have no connections. Only await new ones
                info!("Server has no active Endpoints. Waiting...");

                let wrapped_server = async {
                    Events::NewConnection(server_socket.recv_from(&mut udp_buffer).await.unwrap())
                };

                match wrapped_server.await {
                    Events::NewConnection((len, addr)) => {
                        conman.handle_hello(udp_buffer[..len].to_vec(), addr).await;
                    }
                    _ => continue,
                }
            }
        }
    }))
}
