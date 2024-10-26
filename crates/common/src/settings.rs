use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
pub struct TunnelSettings {
    pub tunnel_device_address: IpAddr,
    pub netmask: Ipv4Addr,
    pub mtu: i32,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ServerSettings {
    pub peer_id: u16,
    pub server_bind_address: SocketAddr,
    pub tunnel_config: TunnelSettings,
    pub connection_timeout: u64,
    pub packet_sorter_deadline: u64,
    pub interface_logger: Option<InterfaceLoggerSettings>,
    pub packet_sorter_logger: Option<PacketSorterLoggerSettings>
}

#[derive(Deserialize, Debug, Clone)]
pub struct Interface {
    pub interface_name: String,
    pub bind_address: SocketAddr,
    pub server_address: SocketAddr,
}

#[derive(Deserialize, Debug, Clone)]
pub struct RouteConfig {
    pub address: Ipv4Addr,
    pub subnet_mask: Ipv4Addr
}

#[derive(Deserialize, Debug, Clone)]
pub struct InterfaceLoggerSettings {
    pub log_path: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct PacketSorterLoggerSettings {
    pub log_path: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ClientSettings {
    pub peer_id: u16,
    pub interfaces: Vec<Interface>,
    pub server_id: u16,
    pub tunnel_config: TunnelSettings,
    pub static_routes: Vec<RouteConfig>,
    pub connection_timeout: u64,
    pub packet_sorter_deadline: u64,
    pub interface_logger: Option<InterfaceLoggerSettings>,
    pub packet_sorter_logger: Option<PacketSorterLoggerSettings>
}

