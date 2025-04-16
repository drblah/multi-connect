use common::connection_manager::ConnectionManager;
use socket2::SockAddr;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;
use clap::Parser;
use log::{debug, error, info};
use tokio::select;
use tokio_tun::{TunBuilder};
use common::interface_logger::InterfaceLogger;
use common::packet_sorter_log::PacketSorterLogger;
use common::settings;

#[derive(Parser, Debug)]
#[clap(author, version, about)]
struct Args {
    /// Path to the configuration file
    #[clap(long, action = clap::ArgAction::Set)]
    config: String,
}


#[tokio::main(flavor = "current_thread")]
async fn main() {
    env_logger::init();

    let args = Args::parse();

    let settings: settings::ServerSettings =
        serde_json::from_str(std::fs::read_to_string(args.config).unwrap().as_str()).unwrap();

    info!("Using config: {:?}", settings);

    // Command socket for selective duplication
    let duplication_socket = tokio::net::UdpSocket::bind("0.0.0.0:1234").await.unwrap();
    let mut duplication_message_buffer = [0u8; 1500];

    let mut interface_logger = None;
    let mut packet_sorter_logger = None;

    if let Some(interface_logger_settings) = settings.interface_logger {
        interface_logger = Some(InterfaceLogger::new(interface_logger_settings.log_path.clone()).await)
    }

    if let Some(packet_sorter_logger_settings) = settings.packet_sorter_logger {
        packet_sorter_logger = Some(PacketSorterLogger::new(packet_sorter_logger_settings.log_path.clone()).await)
    }

    let server_socket =
        socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, None).unwrap();
    server_socket.set_reuse_address(true).unwrap();
    let socketaddr: SocketAddr = settings.server_bind_address; //"172.16.200.4:40000".parse().unwrap();

    server_socket.bind(&SockAddr::from(socketaddr)).unwrap();

    let server_socket = std::net::UdpSocket::from(server_socket);
    server_socket.set_nonblocking(true).unwrap();

    let server_socket = tokio::net::UdpSocket::from_std(server_socket).unwrap();

    let mut udp_buffer = [0u8; 65535];
    let mut tun_buffer = [0u8; 65535];

    let tun_address: IpAddr = settings.tunnel_config.tunnel_device_address;

    let tun_address_ipv4 = match tun_address {
        IpAddr::V4(ipv4) => ipv4,
        IpAddr::V6(ipv6) => {
            panic!("Tun address is not an IPv4 address: {}", ipv6)
        }
    };

    let server_interface_name = "DYN-interface".to_string();

    let tun_device = TunBuilder::new()
        .name("")
        .tap(false)
        .packet_info(false)
        .mtu(settings.tunnel_config.mtu)
        .up()
        .address(tun_address_ipv4.clone())
        .broadcast(Ipv4Addr::BROADCAST)
        .netmask(settings.tunnel_config.netmask.clone())
        .try_build()
        .unwrap();

    let default_route = Some(vec![
        common::router::Route {
            address: Ipv4Addr::new(0, 0, 0, 0),
            subnet_mask: Ipv4Addr::new(0, 0, 0, 0),
        },
        common::router::Route {
            address: tun_address_ipv4,
            subnet_mask: settings.tunnel_config.netmask,
        }
    ]);

    let mut flush_interface_log_timer = tokio::time::interval(Duration::from_secs(1));

    let mut conman = ConnectionManager::new(socketaddr, settings.peer_id, tun_address, default_route, settings.connection_timeout, settings.packet_sorter_deadline);

    loop {
        if conman.has_endpoints() {

            // In case we have active connections. Await new connection attempts and await messages
            // from existing connections.
            select! {
                maybe_new_connection = server_socket.recv_from(&mut udp_buffer) => {
                    if let Ok((len, addr)) = maybe_new_connection {
                        conman.handle_hello(udp_buffer[..len].to_vec(), addr, server_interface_name.clone()).await;
                    }
                }
                
                maybe_established_msg = conman.await_incoming() => {
                    match maybe_established_msg {
                        Ok(read_info) => {
                        //info!("Endpoint: {}, produced message: {:?}", endpointid, message);
                        conman.handle_established_message(
                            read_info,
                            &mut interface_logger).await;
                    }
                    Err(e) => {
                        error!("Encountered error: {}", e.to_string())
                    }
                    }
                }
                
                (endpoint, interface_name, socket) = conman.await_timeout() => {
                    conman.remove_connection(endpoint, interface_name, socket)
                }
                
                endpoint_id = conman.await_packet_sorters() => {
                    conman.handle_packet_sorter_deadline(endpoint_id).await;
                }
                
                maybe_packet = tun_device.recv(&mut tun_buffer) => {
                    match maybe_packet {
                        Ok(packet_length) => {
                            conman.handle_packet_from_tun(&tun_buffer[..packet_length]).await;
                        }
                        Err(e) => error!("Error while reading from tun device: {}", e.to_string())
                    }
                }
                
                (_endpoint_id, maybe_packet) = conman.await_endpoint_sorted_packets() => {
                    if let Some(packet) = maybe_packet {
                        if let Some(packet_sorter_logger) = &mut packet_sorter_logger {
                            packet_sorter_logger.add_log_line(
                                packet.seq
                            ).await
                        }

                        tun_device.send(packet.bytes.as_slice()).await.unwrap();
                    }
                }
                
                _ = flush_interface_log_timer.tick() => {
                    if let Some(interface_logger) = &mut interface_logger {
                            interface_logger.flush().await;
                        }
    
                        if let Some(packet_sorter_logger) = &mut packet_sorter_logger {
                            packet_sorter_logger.flush().await;
                        }
                }
                
                maybe_duplication_msg = duplication_socket.recv_from(&mut duplication_message_buffer) => {
                    if let Ok((size, _addr)) = maybe_duplication_msg {
                        let duplication_message = &duplication_message_buffer[..size];
                        debug!("Duplication raw: {:?}", duplication_message);
                        conman.handle_selective_duplication_command(duplication_message);
                    }
                }
            }
            
            
            

            // Clean up connections which was determined to be disconnected on the last iteration
            conman.remove_disconnected();
        } else {
            // In case we have no connections. Only await new ones
            info!("Server has no active Endpoints. Waiting...");
            
            select! {
                maybe_new_connection = server_socket.recv_from(&mut udp_buffer) => {
                    if let Ok((len, addr)) = maybe_new_connection {
                        conman.handle_hello(udp_buffer[..len].to_vec(), addr, server_interface_name.clone()).await;
                    }
                }
            }
        }
    }
}
