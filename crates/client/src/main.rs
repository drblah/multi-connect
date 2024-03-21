use common::{connection_manager, ConnectionInfo, settings};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};
use anyhow::Result;
use async_compat::Compat;
use clap::Parser;
use futures::StreamExt;
use smol::future::{FutureExt};
use common::messages::{EndpointId, Packet};
use log::{debug, error, info};
use tokio_tun::{TunBuilder};
use common::interface_logger::InterfaceLogger;
use common::packet_sorter_log::PacketSorterLogger;
use common::router::Route;

#[derive(Parser, Debug)]
#[clap(author, version, about)]
struct Args {
    /// Path to the configuration file
    #[clap(long, action = clap::ArgAction::Set)]
    config: String,
}

enum Events {
    NewEstablishedMessage(Result<(EndpointId, Vec<u8>, SocketAddr, (String, SocketAddr))>),
    ConnectionTimeout((EndpointId, String, SocketAddr)),
    PacketSorter(EndpointId),
    TunnelPacket(std::io::Result<usize>),
    #[allow(dead_code)]
    SendKeepalive(Option<Instant>),
    NewSortedPacket((EndpointId, Option<Packet>))
}

fn main() {
    env_logger::init();

    let args = Args::parse();

    let settings: settings::ClientSettings =
        serde_json::from_str(std::fs::read_to_string(args.config).unwrap().as_str()).unwrap();

    info!("Using config: {:?}", settings);

    smol::block_on(Compat::new(async {

        let mut interface_logger = None;
        let mut packet_sorter_logger = None;

        if let Some(interface_logger_settings) = settings.interface_logger {
            interface_logger = Some(InterfaceLogger::new(interface_logger_settings.log_path.clone()).await)
        }

        if let Some(packet_sorter_logger_settings) = settings.packet_sorter_logger {
            packet_sorter_logger = Some(PacketSorterLogger::new(packet_sorter_logger_settings.log_path.clone()).await)
        }

        let client_tun_ip = settings.tunnel_config.tunnel_device_address; //"10.12.0.5".parse().unwrap();

        let client_tun_ipv4 = match client_tun_ip {
            IpAddr::V4(ipv4) => ipv4,
            IpAddr::V6(ipv6) => {
                panic!("Tun address is not an IPv4 address: {}", ipv6)
            }
        };

        let tun = TunBuilder::new()
            .name("")
            .tap(false)
            .packet_info(false)
            .mtu(settings.tunnel_config.mtu)
            .up()
            .address(client_tun_ipv4.clone())
            .broadcast(Ipv4Addr::BROADCAST)
            .netmask(settings.tunnel_config.netmask.clone())
            .try_build()
            .unwrap();

        // TODO: Can we handle this in a more generic way?
        let client_socket_address = settings.interfaces.first().unwrap().bind_address; //"172.16.200.2:0".parse().unwrap();

        let server_socket_address = settings.server_address; //"172.16.200.4:40000".parse().unwrap();
        let server_endpoint_id = settings.server_id;

        let client_id = settings.peer_id;

        let mut tun_buffer = [0u8; 65535];

        let mut static_routes = Vec::new();
        for route in &settings.static_routes {
            static_routes.push(
                Route {
                    address: route.address,
                    subnet_mask: route.subnet_mask,
                }
            )
        }

        static_routes.push(
            common::router::Route {
                address: client_tun_ipv4,
                subnet_mask: settings.tunnel_config.netmask, }
        );

        let static_routes = Some(static_routes);

        let mut connection_manager = connection_manager::ConnectionManager::new(client_socket_address, client_id, client_tun_ip, static_routes, settings.connection_timeout, settings.packet_sorter_deadline);

        let mut keepalive_timer = smol::Timer::interval(Duration::from_secs(1));

        let mut connection_info = Vec::new();

        for interface_config in &settings.interfaces {
            connection_info.push(ConnectionInfo {
                interface_name: interface_config.interface_name.clone(),
                local_address: interface_config.bind_address,
                destination_address: server_socket_address,
                destination_endpoint_id: server_endpoint_id
            });

            connection_manager.create_new_connection(
                interface_config.interface_name.clone(),
                interface_config.bind_address,
                server_socket_address,
                server_endpoint_id
            ).await.unwrap()
        }

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
                let wrapped_keepalive_timer = async {
                    Events::SendKeepalive( keepalive_timer.next().await )
                };
                let wrapped_new_sorted_packet = async {
                    Events::NewSortedPacket(connection_manager.await_endpoint_sorted_packets().await)
                };

                match wrapped_keepalive_timer
                    .or(
                        wrapped_connection_timeout
                        .race(wrapped_packet_sorter)
                        .race(wrapped_tunnel_device)
                        .race(wrapped_endpoints)
                        .race(wrapped_new_sorted_packet)
                    )
                    .await
                {
                    Events::NewEstablishedMessage(result) => match result {
                        Ok((endpointid, message, source_address, receiver_interface)) => {
                            //info!("Endpoint: {}, produced message: {:?}", endpointid, message);
                            connection_manager.handle_established_message(message, endpointid, source_address, receiver_interface, &mut interface_logger).await;

                        }
                        Err(e) => {
                            error!("Encountered error: {}", e.to_string())
                        }
                    },
                    Events::ConnectionTimeout((endpoint, interface_name, socket)) => {
                        connection_manager.remove_connection(endpoint, interface_name, socket)
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
                    Events::SendKeepalive(_) => {
                        info!("Sending keepalive");
                        connection_manager.greet_all_endpoints().await;

                        // Reopen dead connections
                        connection_manager.ensure_endpoint_connections(server_endpoint_id, &connection_info).await;
                        if let Some(interface_logger) = &mut interface_logger {
                            interface_logger.flush().await;
                        }

                        if let Some(packet_sorter_logger) = &mut packet_sorter_logger {
                            packet_sorter_logger.flush().await;
                        }

                    }
                    Events::NewSortedPacket((_endpoint_id, maybe_packet)) => {
                        if let Some(packet) = maybe_packet {
                            if let Some(packet_sorter_logger) = &mut packet_sorter_logger {
                                packet_sorter_logger.add_log_line(
                                    packet.seq
                                ).await
                            }

                            tun.send(packet.bytes.as_slice()).await.unwrap();
                        }
                    }
                }
                connection_manager.remove_disconnected();

            } else {
                info!("No active connections. Awaiting new connection attempts.");

                for interface_config in &settings.interfaces {
                    match connection_manager.create_new_connection(
                        interface_config.interface_name.clone(),
                        interface_config.bind_address,
                        server_socket_address,
                        server_endpoint_id
                    ).await {
                        Ok(()) => { debug!("Restarted connection on: {}", interface_config.interface_name) }
                        Err(_) => { error!("Failed to restart connection on: {}", interface_config.interface_name) }
                    }
                }
            }
        }
    }));
}
