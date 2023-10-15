use serde::{Deserialize, Serialize};

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
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct HelloAck {
    pub id: EndpointId,
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
