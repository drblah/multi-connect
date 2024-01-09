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
    pub tunnel_config: TunnelSettings
}

#[derive(Deserialize, Debug, Clone)]
pub struct Interface {
    pub interface_name: String,
    pub bind_address: SocketAddr,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ClientSettings {
    pub peer_id: u16,
    pub interfaces: Vec<Interface>,
    pub server_address: SocketAddr,
    pub tunnel_device_address: IpAddr
}

