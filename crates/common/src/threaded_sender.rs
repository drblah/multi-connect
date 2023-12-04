use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::thread;
use std::thread::JoinHandle;
use log::{error, warn};
use smol::channel::{TryRecvError, TrySendError};
use crate::UdpSocketInfo;

#[derive(Debug)]
pub struct ThreadedSender {
    thread: JoinHandle<()>,
    packet_channel: smol::channel::Sender<Vec<u8>>,
    result_channel: smol::channel::Receiver<std::io::Result<usize>>,
    udpsocket_info: Arc<UdpSocketInfo>
}

impl ThreadedSender {
    pub fn new(udpsocket_info: Arc<UdpSocketInfo>) -> Self {

        let udp_socket_info_own_copy = udpsocket_info.clone();

        let interface_name = udpsocket_info.interface_name.to_string();
        let (packet_sender, packet_receiver) = smol::channel::bounded::<Vec<u8>>(1000);
        let (result_sender, result_receiver) = smol::channel::bounded::<std::io::Result<usize>>(1000);

        let destination = udpsocket_info.destination_address;
        let src_ip = Some(udpsocket_info.local_address.ip());

        let thread = thread::Builder::new()
            .name(format!("Sender thread: {}", interface_name))
            .spawn(move || {

                smol::block_on( async {
                    loop {
                        let mut packets_to_send = Vec::new();
                        let mut transmits = Vec::new();

                        if let Ok(packet_bytes) = packet_receiver.recv().await {
                            let mut was_while_run = false;

                            let segment_size = packet_bytes.len();
                            let mut accumulated_bytes = packet_bytes.len();
                            packets_to_send.push(packet_bytes);

                            while let Ok(more_bytes) = packet_receiver.try_recv() {

                                let more_bytes_length = more_bytes.len();

                                if more_bytes_length > segment_size {
                                    was_while_run = true;
                                    // Commit current packets to send into a transmit and

                                    transmits.push(
                                        vec_to_transmit(packets_to_send.clone(), destination, src_ip, segment_size)
                                    );

                                    // put this new segment in its own transmit, then abort
                                    transmits.push(
                                        vec_to_transmit(vec![more_bytes], destination, src_ip, segment_size)
                                    );

                                    break
                                } else if more_bytes_length < segment_size || packets_to_send.len() == 31 {
                                    was_while_run = true;
                                    // Use this segment as the last in the current transmit, then abort
                                    accumulated_bytes += more_bytes_length;
                                    packets_to_send.push(more_bytes);

                                    transmits.push(
                                        vec_to_transmit(packets_to_send.clone(), destination, src_ip, segment_size)
                                    );



                                    break
                                } else {
                                    // Add this segment to the current transmit, then continue
                                    accumulated_bytes += more_bytes_length;
                                    packets_to_send.push(more_bytes);
                                }
                            }

                            if !was_while_run {
                                transmits.push(
                                    vec_to_transmit(packets_to_send, destination, src_ip, segment_size)
                                );
                            }

                            let socket_ref = (&udpsocket_info.socket).into();
                            let send_result = udpsocket_info.socket_state.send(
                                socket_ref,
                                &transmits
                            );

                            if let Ok(_) = result_sender.try_send(send_result) {

                            } else {
                                warn!("result sender channel cannot keep up for threaded_sender: {}", udpsocket_info.interface_name)
                            }

                        }
                    }
                });
            }).unwrap();

        ThreadedSender {
            thread,
            packet_channel: packet_sender,
            result_channel: result_receiver,
            udpsocket_info: udp_socket_info_own_copy
        }
    }

    pub async fn try_send(&self, packet: Vec<u8>) {
        match self.packet_channel.try_send(packet) {
            Ok(_) => {}
            Err(e) => {
                match e {
                    TrySendError::Full(_) => {
                        error!("Threaded_sender packet_channel for interface: {} is full. Dropping packets!", self.udpsocket_info.interface_name)
                    }
                    TrySendError::Closed(_) => {
                        panic!("Threaded_sender packet_channel is closed for interface: {}", self.udpsocket_info.interface_name)
                    }
                }
            }
        }
    }

    pub async fn try_get_result(&self) -> std::io::Result<usize> {
        match self.result_channel.try_recv() {
            Ok(new_result) => {new_result},
            Err(e) => {
                match e {
                    TryRecvError::Empty => { Ok(0) }
                    TryRecvError::Closed => {
                        panic!("Threaded_sender result_channel is closed for interface: {}", self.udpsocket_info.interface_name)
                    }
                }
            }
        }
    }
}

fn vec_to_transmit(packets_to_send: Vec<Vec<u8>>, destination: SocketAddr, src_ip: Option<IpAddr>, segment_size: usize) -> quinn_udp::Transmit {

    let segment_size = if segment_size == 0 {
        None
    } else {
        Some(segment_size)
    };

    // Flatten packets into a single byte array
    let mut flattened = Vec::new();
    for packet in packets_to_send {
        flattened.extend(packet);
    }

    let contents = bytes::Bytes::from(flattened);

    quinn_udp::Transmit {
        destination,
        ecn: None,
        contents,
        segment_size,
        src_ip
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    use crate::threaded_sender::ThreadedSender;
    use crate::UdpSocketInfo;

    #[test]
    fn threaded_sender_test() {
        let sender_socket = std::net::UdpSocket::bind("127.0.0.1:6000").unwrap();
        let receive_socket = std::net::UdpSocket::bind("127.0.0.2:5000").unwrap();

        sender_socket.connect("127.0.0.2:5000").unwrap();

        let sender_socket_info = Arc::new(UdpSocketInfo {
            socket_state: quinn_udp::UdpSocketState::new(quinn_udp::UdpSockRef::from(&sender_socket)).unwrap(),
            socket: sender_socket,
            interface_name: "lo".to_string(),
            local_address: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 6000),
            destination_address: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)), 5000),
        });

        let thread_sender = ThreadedSender::new(sender_socket_info.clone());

        let data_to_send = vec![0; 1000];

        smol::block_on( async {
        for _ in 0..100 {
            thread_sender.packet_channel.send(data_to_send.clone()).await.unwrap();
        }
        });

        let mut recv_buffer = [0; 65000];
        thread::sleep(Duration::from_millis(1000));
        let (recv_len, source) = receive_socket.recv_from(&mut recv_buffer).unwrap();

        assert_eq!(recv_buffer[..recv_len].to_vec(), data_to_send);

    }
}