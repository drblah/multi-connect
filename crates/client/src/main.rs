use std::io::{Error};
use common::messages;
use std::net::{IpAddr, UdpSocket};
use std::time::Instant;
use async_compat::Compat;
use smol::future::{FutureExt};
use smol::stream::StreamExt;
use tun::TunPacket;
use common::messages::Messages;
use futures::SinkExt;
use log::error;

enum ClientEvents {
    NewMessage(usize),
    HelloTimer(Option<Instant>),
    TunPacket(Option<core::result::Result<TunPacket, Error>>)
}

fn main() {

    smol::block_on(Compat::new(async {
        let client_ip = "10.12.0.5".parse().unwrap();

        /*
        let mut client_tun = common::tun_device::AsyncTun::new(
            "client_tun",
            client_ip,
            "255.255.255.0".parse().unwrap(),
        ).await.unwrap();

         */

        //let mut tun_device = common::tun_device::AsyncTun::new("tun0", tun_address, "255.255.255.0".parse().unwrap()).await.unwrap();
        let mut config = tun::Configuration::default();

        config
            .address(client_ip)
            .netmask("255.255.255.0".parse::<IpAddr>().unwrap())
            .up();

        config.platform(|config| {
            config.packet_information(false);
        });

        let tun_device = tun::create_as_async(&config).unwrap();
        let mut stream = tun_device.into_framed();

        let hello = messages::Hello { id: 154, session_id: uuid::Uuid::new_v4(), tun_address: client_ip };

        let hello_message = messages::Messages::Hello(hello);
        let serialized_hello = bincode::serialize(&hello_message).unwrap();

        let client_socket = UdpSocket::bind("172.16.200.2:0").unwrap();


        client_socket.connect("172.16.200.4:40000").unwrap();

        let client_socket = smol::Async::new(client_socket).unwrap();

        client_socket.send(&serialized_hello).await.unwrap();
        let mut buffer = [0u8; 65535];

        let mut hello_timer = smol::Timer::interval(std::time::Duration::from_secs(1));
        let mut tx_counter = 0;


        loop {

            let wrapped_hello_timer = async {
                ClientEvents::HelloTimer(hello_timer.next().await)
            };
            let wrapped_client_socket = async {
                ClientEvents::NewMessage(client_socket.recv(&mut buffer).await.unwrap())
            };
            let wrapped_tun_device = async {
                ClientEvents::TunPacket(stream.next().await)
            };

            match wrapped_client_socket
                .race(wrapped_hello_timer)
                .race(wrapped_tun_device)
                .await {
                ClientEvents::HelloTimer(_) => {
                    println!("Sending hello again");
                    client_socket.send(&serialized_hello).await.unwrap();
                }
                ClientEvents::NewMessage(length) => {
                    let msg: Messages = bincode::deserialize(&buffer[..length]).unwrap();
                    match msg {
                        Messages::Packet(pkt) => {
                            //println!("Got packet: {:?}", pkt);
                            stream.send(
                                TunPacket::new(pkt.bytes.to_vec())
                            ).await.unwrap();
                        }
                        Messages::Hello(_) => {}
                        Messages::HelloAck(ack) => {
                            println!("Got hello ack: {:?}", ack);
                        }
                        Messages::KeepAlive(_) => {}
                    }
                }
                ClientEvents::TunPacket(maybe_packet) => {
                    if let Some(packet) = maybe_packet {
                        match packet {
                            Ok(pkt) => {
                                let pkt = messages::Packet { seq: tx_counter, id: 154, bytes: pkt.get_bytes().to_vec() };
                                tx_counter += 1;
                                let pkt_message = messages::Messages::Packet(pkt);
                                let serialized = bincode::serialize(&pkt_message).unwrap();
                                client_socket.send(&serialized).await.unwrap();
                            }
                            Err(e) => {error!("Error while reading from tun device: {}", e.to_string())}

                        }
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
    }));
    //client_socket.send(&[1, 2, 3]).unwrap();
}
