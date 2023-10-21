use common::messages;
use std::net::UdpSocket;
use std::time::Instant;
use smol::future::{FutureExt};
use smol::stream::StreamExt;
use common::messages::Messages;

enum ClientEvents {
    NewMessage(usize),
    HelloTimer(Option<Instant>)
}

fn main() {

    smol::block_on(async {
        let client_ip = "10.12.0.5".parse().unwrap();

        let mut client_tun = common::tun_device::AsyncTun::new(
            "client_tun",
            client_ip,
            "255.255.255.0".parse().unwrap(),
        ).unwrap();

        let hello = messages::Hello { id: 154, session_id: uuid::Uuid::new_v4(), tun_address: client_ip };

        let hello_message = messages::Messages::Hello(hello);
        let serialized_hello = bincode::serialize(&hello_message).unwrap();

        let client_socket = UdpSocket::bind("127.0.0.1:0").unwrap();


        client_socket.connect("127.0.0.2:40000").unwrap();

        let client_socket = smol::Async::new(client_socket).unwrap();

        client_socket.send(&serialized_hello).await.unwrap();
        let mut buffer = [0u8; 65535];

        let mut hello_timer = smol::Timer::interval(std::time::Duration::from_secs(1));

        loop {

            let wrapped_hello_timer = async {
                ClientEvents::HelloTimer(hello_timer.next().await)
            };
            let wrapped_client_socket = async {
                ClientEvents::NewMessage(client_socket.recv(&mut buffer).await.unwrap())
            };

            match wrapped_client_socket
                .race(wrapped_hello_timer).await {
                ClientEvents::HelloTimer(_) => {
                    println!("Sending hello again");
                    client_socket.send(&serialized_hello).await.unwrap();
                }
                ClientEvents::NewMessage(length) => {
                    let msg: Messages = bincode::deserialize(&buffer[..length]).unwrap();
                    match msg {
                        Messages::Packet(pkt) => {
                            println!("Got packet: {:?}", pkt);
                            client_tun.write(
                                &pkt.bytes
                            ).await.unwrap();
                        }
                        Messages::Hello(_) => {}
                        Messages::HelloAck(ack) => {
                            println!("Got hello ack: {:?}", ack);
                        }
                        Messages::KeepAlive(_) => {}
                    }
                }
            }


            /*
            let msg_len = client_socket.recv(&mut buffer).await.unwrap();

            println!("Got: {:?}", buffer[..msg_len].to_vec());

            let msg: Messages = bincode::deserialize(&buffer[..msg_len]).unwrap();

            match msg {
                Messages::Packet(pkt) => {
                    println!("Got packet: {:?}", pkt);
                    client_tun.write(
                        &pkt.bytes
                    ).await.unwrap();
                }
                Messages::Hello(_) => {}
                Messages::HelloAck(ack) => {
                    println!("Got hello ack: {:?}", ack);
                }
                Messages::KeepAlive(_) => {}
            }

             */


        }
    });
    //client_socket.send(&[1, 2, 3]).unwrap();
}
