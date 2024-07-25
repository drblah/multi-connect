use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::router::Route;

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
    pub static_routes: Option<Vec<Route>>,
    pub hello_seq: u64
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct HelloAck {
    pub id: EndpointId,
    pub session_id: Uuid,
    pub static_routes: Option<Vec<Route>>,
    pub hello_ack_seq: u64
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub enum Messages {
    Packet(Packet),
    Hello(Hello),
    HelloAck(HelloAck)
}

#[derive(Deserialize, Debug, Clone)]
pub struct InterfaceState {
    pub interface_name: String,
    pub enabled: bool
}

#[derive(Deserialize, Debug, Clone)]
pub enum DuplicationCommands {
    InterfaceState(InterfaceState)
}
