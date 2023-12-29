use std::io::IoSliceMut;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use smol::channel::{Receiver, TrySendError};
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;
use log::error;
use polling::{Event, Events, Poller};
use crate::UdpSocketInfo;

#[derive(Debug)]
pub struct ThreadedReceiver {
    thread: Option<JoinHandle<()>>,
    pub(crate) packet_channel: smol::channel::Receiver<std::io::Result<Vec<u8>>>,
    udpsocket_info: Arc<UdpSocketInfo>,
    poller: Arc<Poller>,
    thread_stopper: Arc<Mutex<bool>>
}

impl ThreadedReceiver {
    pub fn new(udpsocket_info: Arc<UdpSocketInfo>) -> Self {

        let udp_socket_info_own_copy = udpsocket_info.clone();

        let interface_name = udpsocket_info.interface_name.to_string();
        let (packet_sender, packet_receiver) = smol::channel::bounded::<std::io::Result<Vec<u8>>>(1000);

        let destination = udpsocket_info.destination_address;
        let src_ip = Some(udpsocket_info.local_address.ip());

        const MAX_GRO: usize = 1;

        let mut recv_buffer = [[0u8; 65535]; MAX_GRO];
        let mut meta_buffer = [quinn_udp::RecvMeta::default(); MAX_GRO];

        let poll_key = 1;
        let poller = Arc::new(Poller::new().unwrap());

        unsafe {
            poller.add(&udpsocket_info.socket, Event::readable(poll_key)).unwrap()
        }

        let stopper = Arc::new(Mutex::new(false));
        let thread_stopper = stopper.clone();


        let thread_poller = poller.clone();
        let thread = thread::Builder::new()
            .name(format!("Receiver thread: {}", interface_name))
            .spawn(move ||{

                let mut events = Events::new();
                loop {
                    events.clear();
                    thread_poller.wait(&mut events, None).unwrap();
                    {
                        let lock = thread_stopper.lock().unwrap();

                        if *lock == true {
                            return ()
                        }
                    }

                    for ev in events.iter() {
                        if ev.key == poll_key {

                            let mut io_slices: [IoSliceMut; MAX_GRO] = unsafe {
                                let mut slices: [std::mem::MaybeUninit<IoSliceMut>; MAX_GRO] = std::mem::MaybeUninit::uninit_array();
                                for (slice, buffer) in slices.iter_mut().zip(&mut recv_buffer) {
                                    std::ptr::write(slice.as_mut_ptr(), IoSliceMut::new(buffer));
                                }
                                std::mem::transmute::<_, [IoSliceMut; MAX_GRO]>(slices)
                            };


                            let socket_ref = (&udpsocket_info.socket).into();
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
                                            let packet = buffer[..meta.stride].to_vec();

                                            match packet_sender.try_send(Ok(packet)) {
                                                Ok(_) => {},
                                                Err(e) => {
                                                    match e {
                                                        TrySendError::Full(_) => {
                                                            error!("packet receiver queue for interface: {} is full!", udpsocket_info.interface_name)
                                                        }
                                                        TrySendError::Closed(_) => {}
                                                    }
                                                }
                                            }

                                        } else {
                                            // Multiple packets
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

                                                match packet_sender.try_send(Ok(packet)) {
                                                    Ok(_) => {},
                                                    Err(e) => {
                                                        match e {
                                                            TrySendError::Full(_) => {
                                                                error!("packet receiver queue for interface: {} is full!", udpsocket_info.interface_name)
                                                            }
                                                            TrySendError::Closed(_) => {}
                                                        }
                                                    }
                                                }

                                            }

                                        }


                                    }
                                }

                                Err(e) => {
                                    match packet_sender.try_send(Err(e)) {
                                        Ok(_) => {},
                                        Err(e) => {
                                            match e {
                                                TrySendError::Full(_) => {
                                                    error!("packet receiver result queue for interface: {} is full!", udpsocket_info.interface_name)
                                                }
                                                TrySendError::Closed(_) => {}
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Set interest in next read event on socket
                    thread_poller.modify(&udpsocket_info.socket, Event::readable(poll_key)).unwrap();
                }
            }).unwrap();

        ThreadedReceiver {
            thread: Some(thread),
            packet_channel: packet_receiver,
            udpsocket_info: udp_socket_info_own_copy,
            poller,
            thread_stopper: stopper
        }
    }

    pub async fn pop_next_packet(&self) -> std::io::Result<Vec<u8>> {
        match self.packet_channel.recv().await {
            Ok(recv_result) => { recv_result },
            Err(e) => {
                panic!("packet_channel for device {} closed!", self.udpsocket_info.interface_name)
            }
        }
    }
}

impl Drop for ThreadedReceiver {
    fn drop(&mut self) {
        {
            let mut lock = self.thread_stopper.lock().unwrap();
            *lock = true;
        }

        self.poller.notify().unwrap();

        if let Some(handle) = self.thread.take() { ;
            handle.join().unwrap();
        }

        self.poller.delete(&self.udpsocket_info.socket).unwrap();
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

        let test_iterations = 80;

        for _ in 0..test_iterations {
            sender_socket.send(&data_to_send).unwrap();
        }

        thread::sleep(Duration::from_millis(500));

        let mut data_received = Vec::new();

        smol::block_on( async {
            loop {
                let new_packet = thread_receiver.packet_channel.recv().await.unwrap().unwrap();

                println!("Received packet with size: {}", new_packet.len());

                data_received.push(new_packet);

                println!("Total received packets: {}", data_received.len());
                if data_received.len() == test_iterations {
                    break
                }
            }
        });

        assert_eq!(data_received, vec![data_to_send; test_iterations]);
    }
}