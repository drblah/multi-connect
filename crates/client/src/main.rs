use common::{connection_manager};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use anyhow::Result;
use async_compat::Compat;
use smol::future::{FutureExt};
use common::messages::{EndpointId};
use log::{error, info};
use tokio_tun::{TunBuilder};

enum Events {
    NewEstablishedMessage(Result<(EndpointId, Vec<u8>, SocketAddr)>),
    ConnectionTimeout((EndpointId, SocketAddr)),
    PacketSorter(EndpointId),
    TunnelPacket(std::io::Result<usize>)
}

fn main() {
    env_logger::init();

    smol::block_on(Compat::new(async {

        let client_tun_ip = "10.12.0.5".parse().unwrap();

        let client_tun_ipv4 = match client_tun_ip {
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
            .address(client_tun_ipv4)
            .broadcast(Ipv4Addr::BROADCAST)
            .netmask(Ipv4Addr::new(255, 255, 255, 0))
            .try_build()
            .unwrap();

        let client_socket_address = "172.16.200.2:0".parse().unwrap();

        let server_socket_address = "172.16.200.4:40000".parse().unwrap();
        let server_endpoint_id = 1;

        let veth1_name = "veth1";
        let veth2_name = "veth2";

        let veth1_ip = "172.16.200.2:0".parse().unwrap();
        let veth2_ip = "172.16.200.3:0".parse().unwrap();

        let client_id = 154;

        let mut tun_buffer = [0u8; 65535];

        let mut connection_manager = connection_manager::ConnectionManager::new(client_socket_address, client_id, client_tun_ip);

        connection_manager.create_new_connection(
            veth1_name,
            veth1_ip,
            server_socket_address,
            server_endpoint_id
        ).await;

        connection_manager.create_new_connection(
            veth2_name,
            veth2_ip,
            server_socket_address,
            server_endpoint_id
        ).await;

        loop {
            if connection_manager.has_endpoints() {
                let wrapped_endpoints =
                    async { Events::NewEstablishedMessage(connection_manager.await_incoming().await) };
                let wrapped_connection_timeout =
                    async { Events::ConnectionTimeout(connection_manager.await_timeout().await) };
                let wrapped_packet_sorter = async {
                    Events::PacketSorter(connection_manager.await_packet_sorters().await)
                };
                let wrapped_tunnel_device = async {
                    Events::TunnelPacket(tun.recv(&mut tun_buffer).await)
                };

                match wrapped_connection_timeout
                    .race(wrapped_packet_sorter)
                    .race(wrapped_tunnel_device)
                    .race(wrapped_endpoints)
                    .await
                {
                    Events::NewEstablishedMessage(result) => match result {
                        Ok((endpointid, message, source_address)) => {
                            info!("Endpoint: {}, produced message: {:?}", endpointid, message);
                            connection_manager.handle_established_message(message, endpointid, source_address, &mut tun).await;

                        }
                        Err(e) => {
                            error!("Encountered error: {}", e.to_string())
                        }
                    },
                    Events::ConnectionTimeout((endpoint, socket)) => {
                        connection_manager.remove_connection(endpoint, socket)
                    }
                    Events::PacketSorter(endpoint_id) => {
                        connection_manager.handle_packet_sorter_deadline(endpoint_id).await;
                    }
                    Events::TunnelPacket(maybe_packet) => {
                        match maybe_packet {
                            Ok(packet_length) => {
                                connection_manager.handle_packet_from_tun(&tun_buffer[..packet_length]).await;
                            }
                            Err(e) => error!("Error while reading from tun device: {}", e.to_string())
                        }
                    }
                }
                connection_manager.remove_disconnected();

            } else {
                info!("No active connections. Awaiting new connection attempts.");

                connection_manager.create_new_connection(
                    veth1_name,
                    veth1_ip,
                    server_socket_address,
                    server_endpoint_id
                ).await;

                connection_manager.create_new_connection(
                    veth2_name,
                    veth2_ip,
                    server_socket_address,
                    server_endpoint_id
                ).await;
            }
        }
    }));
}
