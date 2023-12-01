use std::io::IoSliceMut;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::thread;
use std::thread::JoinHandle;
use log::error;
use crate::UdpSocketInfo;

struct ThreadedReceiver {
    thread: JoinHandle<()>,
    packet_channel: Receiver<Vec<u8>>,
    result_channel: Receiver<std::io::Result<usize>>,
}

impl ThreadedReceiver {
    pub fn new(udpsocket_info: Arc<UdpSocketInfo>) -> Self {
        let interface_name = udpsocket_info.interface_name.to_string();
        let (packet_sender, packet_receiver) = std::sync::mpsc::sync_channel::<Vec<u8>>(1000);
        let (result_sender, result_receiver) = std::sync::mpsc::sync_channel::<std::io::Result<usize>>(1000);

        let destination = udpsocket_info.destination_address;
        let src_ip = Some(udpsocket_info.local_address.ip());

        const MAX_GRO: usize = 1;

        let mut recv_buffer = [[0u8; 65535]; MAX_GRO];
        let mut meta_buffer = [quinn_udp::RecvMeta::default(); MAX_GRO];

        let thread = thread::Builder::new()
            .name(format!("Receiver thread: {}", interface_name))
            .spawn(move ||{


                loop {
                    let mut io_slices: [IoSliceMut; MAX_GRO] = unsafe {
                        let mut slices: [std::mem::MaybeUninit<IoSliceMut>; MAX_GRO] = std::mem::MaybeUninit::uninit_array();
                        for (slice, buffer) in slices.iter_mut().zip(&mut recv_buffer) {
                            std::ptr::write(slice.as_mut_ptr(), IoSliceMut::new(buffer));
                        }
                        std::mem::transmute::<_, [IoSliceMut; MAX_GRO]>(slices)
                    };


                    let socket_ref = (&udpsocket_info.socket).into();
                    println!("Waiting new reception");
                    let result = udpsocket_info.socket_state.recv(
                        socket_ref,
                        &mut io_slices,
                        &mut meta_buffer,
                    );

                    match result {
                        Ok(receptions) => {

                            for reception in 0..receptions {
                                let meta = meta_buffer[reception];
                                let buffer = recv_buffer[reception];

                                if meta.stride == meta.len {
                                    // Single packet mode
                                    println!("Receiving single packet");
                                    let packet = buffer[..meta.stride].to_vec();

                                    match packet_sender.try_send(packet) {
                                        Ok(_) => {},
                                        Err(e) => {
                                            match e {
                                                TrySendError::Full(_) => {
                                                    error!("packet receiver queue for interface: {} is full!", udpsocket_info.interface_name)
                                                }
                                                TrySendError::Disconnected(_) => {}
                                            }
                                        }
                                    }

                                } else {
                                    // Multiple packets
                                    println!("Receiving multiple packets");
                                    let packet_count = meta.len.div_ceil(meta.stride);
                                    let mut from = 0;
                                    let mut to = 0;

                                    for packet_number in 0..packet_count {
                                        from = packet_number * meta.stride;
                                        to = (packet_number+1) * meta.stride;

                                        if to > meta.len {
                                            to = meta.len
                                        }

                                        let packet = buffer[from..to].to_vec();

                                        match packet_sender.try_send(packet) {
                                            Ok(_) => {},
                                            Err(e) => {
                                                match e {
                                                    TrySendError::Full(_) => {
                                                        error!("packet receiver queue for interface: {} is full!", udpsocket_info.interface_name)
                                                    }
                                                    TrySendError::Disconnected(_) => {}
                                                }
                                            }
                                        }

                                    }

                                }


                            }

                            match result_sender.try_send(Ok(receptions)) {
                                Ok(_) => {},
                                Err(e) => {
                                    match e {
                                        TrySendError::Full(_) => {
                                            error!("packet receiver result queue for interface: {} is full!", udpsocket_info.interface_name)
                                        }
                                        TrySendError::Disconnected(_) => {}
                                    }
                                }
                            }
                        }

                        Err(e) => {
                            match result_sender.try_send(Err(e)) {
                                Ok(_) => {},
                                Err(e) => {
                                    match e {
                                        TrySendError::Full(_) => {
                                            error!("packet receiver result queue for interface: {} is full!", udpsocket_info.interface_name)
                                        }
                                        TrySendError::Disconnected(_) => {}
                                    }
                                }
                            }
                        }
                    }
                }

            }).unwrap();

        ThreadedReceiver {
            thread,
            packet_channel: packet_receiver,
            result_channel: result_receiver
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Error;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::Arc;
    use std::sync::mpsc::TryRecvError;
    use std::thread;
    use std::time::Duration;

    use crate::threaded_receiver::ThreadedReceiver;
    use crate::UdpSocketInfo;

    #[test]
    fn threaded_receiver_test() {
        let sender_socket = std::net::UdpSocket::bind("127.0.0.1:6000").unwrap();
        let receive_socket = std::net::UdpSocket::bind("127.0.0.2:5000").unwrap();

        sender_socket.connect("127.0.0.2:5000").unwrap();
        receive_socket.connect("127.0.0.1:6000").unwrap();

        let receive_socket_info = Arc::new(UdpSocketInfo {
            socket_state: quinn_udp::UdpSocketState::new(quinn_udp::UdpSockRef::from(&sender_socket)).unwrap(),
            socket: receive_socket,
            interface_name: "lo".to_string(),
            local_address: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)), 5000),
            destination_address: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 6000),
        });

        let thread_receiver = ThreadedReceiver::new(receive_socket_info.clone());

        let data_to_send = vec![0; 1000];

        let test_iterations = 94;

        for _ in 0..test_iterations {
            sender_socket.send(&data_to_send).unwrap();
        }

        thread::sleep(Duration::from_millis(500));

        let mut data_received = Vec::new();

        loop {
            let new_packet = thread_receiver.packet_channel.recv().unwrap();

            println!("Received packet with size: {}", new_packet.len());

            data_received.push(new_packet);

            match thread_receiver.result_channel.try_recv() {
                Ok(_received_packets) => {}
                Err(e) => {
                   match e {
                       TryRecvError::Empty => {}
                       TryRecvError::Disconnected => {
                            panic!("Result channel disconnected!")
                       }
                   }
                }
            }
            println!("Total received packets: {}", data_received.len());
            if data_received.len() == test_iterations {
                break
            }
        }

        assert_eq!(data_received, vec![data_to_send; test_iterations]);
    }
}