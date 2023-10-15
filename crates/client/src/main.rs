use std::net::UdpSocket;
use common::messages;

fn main() {
    let hello = messages::Hello {
        id: 154,
    };

    let hello_message = messages::Messages::Hello(hello);
    let serialized = bincode::serialize(&hello_message).unwrap();

    let client_socket = UdpSocket::bind("127.0.0.1:0").unwrap();

    client_socket.connect("127.0.0.2:40000").unwrap();

    client_socket.send(&serialized).unwrap();
    let mut buffer = [0u8; 1500];

    let msg_len = client_socket.recv(&mut buffer).unwrap();

    println!("Got: {:?}", buffer[..msg_len].to_vec());

    client_socket.send(&[1, 2, 3]).unwrap();
}
