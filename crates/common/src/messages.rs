use std::net::IpAddr;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub type EndpointId = u16;

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct Packet {
    pub seq: u64,
    pub id: EndpointId,
    #[serde(with = "serde_bytes")]
    pub bytes: Vec<u8>,
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct Hello {
    pub id: EndpointId,
    pub session_id: Uuid,
    pub tun_address: IpAddr,
    pub hello_seq: u64
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct HelloAck {
    pub id: EndpointId,
    pub session_id: Uuid,
    pub tun_address: IpAddr,
    pub hello_ack_seq: u64
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct KeepAlive {
    pub id: EndpointId,
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub enum Messages {
    Packet(Packet),
    Hello(Hello),
    HelloAck(HelloAck),
    KeepAlive(KeepAlive),
}
