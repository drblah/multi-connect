#![feature(io_error_more)]
#![feature(maybe_uninit_uninit_array)]
#![feature(int_roundings)]

use nix::libc;
use std::io::Error;
use anyhow::Result;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use smol::net::UdpSocket;
use socket2::{Domain, Socket, Type};
use std::net::UdpSocket as std_udp;
use log::error;
use smol::Async;
use crate::messages::EndpointId;

pub mod messages;
pub mod sequencer;
pub mod connection;
pub mod endpoint;
pub mod connection_manager;
mod threaded_sender;
mod threaded_receiver;

#[derive(Debug)]
pub struct UdpSocketInfo {
    socket: std::net::UdpSocket,
    socket_state: quinn_udp::UdpSocketState,
    interface_name: String,
    local_address: SocketAddr,
    destination_address: SocketAddr,
}


pub fn interface_to_ipaddr(interface: &str) -> Result<Ipv4Addr, Error> {
    let interfaces = pnet_datalink::interfaces();
    let interface = interfaces
        .into_iter()
        .find(|iface| iface.name == interface)
        .ok_or_else(|| std::io::ErrorKind::NotFound)?;

    let ipaddr = interface
        .ips
        .into_iter()
        .find(|ip| ip.is_ipv4())
        .ok_or_else(|| std::io::ErrorKind::AddrNotAvailable)?;


    if let IpAddr::V4(ipaddr) = ipaddr.ip() {
        return Ok(ipaddr)
    }

    Err(Error::from(std::io::ErrorKind::AddrNotAvailable))
}


/// This function makes a tokio UdpSocket which is bound to the specified IP and port
/// Optionally, it can also bind to a specific network device. Binding ensures this
/// socket will *always* use the specific interface, and the routed belonging to that interface.
/// This is useful when multiple interfaces can route to the same destination, but you want
/// to control which interface is used. *NB*: The bind to device socket option is only supported
/// on Linux.
fn make_socket(interface: &str, local_address: Option<Ipv4Addr>, local_port: Option<u16>, bind_to_device: bool) -> Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, None).unwrap();

    if bind_to_device {
        if let Err(err) = socket.bind_device(Some(interface.as_bytes())) {
            if matches!(err.raw_os_error(), Some(libc::ENODEV)) {
                error!("error binding to device (`{}`): {}", interface, err);
                return Err(anyhow::Error::new(err))
            } else {
                panic!("unexpected error binding device: {}", err);
            }
        }
    }


    let local_address = match local_address {
        Some(local_address) => {
            local_address
        }
        None => {
            interface_to_ipaddr(interface).unwrap()
        }
    };

    let address = if local_port.is_some() {
        SocketAddrV4::new(local_address, local_port.unwrap())
    } else {
        SocketAddrV4::new(local_address, 0)
    };

    socket.bind(&address.into())?;

    let std_udp: std_udp = socket.into();
    std_udp.set_nonblocking(true)?;

    let udp_socket: UdpSocket = UdpSocket::from(Async::try_from(std_udp)?);

    Ok(udp_socket)
}

fn make_std_socket(interface: &str, local_address: Option<Ipv4Addr>, local_port: Option<u16>, bind_to_device: bool) -> Result<std::net::UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, None).unwrap();

    if bind_to_device {
        if let Err(err) = socket.bind_device(Some(interface.as_bytes())) {
            if matches!(err.raw_os_error(), Some(libc::ENODEV)) {
                error!("error binding to device (`{}`): {}", interface, err);
                return Err(anyhow::Error::new(err))
            } else {
                panic!("unexpected error binding device: {}", err);
            }
        }
    }


    let local_address = match local_address {
        Some(local_address) => {
            local_address
        }
        None => {
            interface_to_ipaddr(interface).unwrap()
        }
    };

    let address = if local_port.is_some() {
        SocketAddrV4::new(local_address, local_port.unwrap())
    } else {
        SocketAddrV4::new(local_address, 0)
    };

    socket.bind(&address.into())?;

    let std_udp: std_udp = socket.into();
    std_udp.set_nonblocking(true)?;

    Ok(std_udp)
}

pub struct ConnectionInfo {
    pub interface_name: String,
    pub local_address: SocketAddr,
    pub destination_address: SocketAddr,
    pub destination_endpoint_id: EndpointId,
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::Read;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::Arc;
    use crate::threaded_receiver::ThreadedReceiver;
    use crate::threaded_sender::ThreadedSender;
    use crate::UdpSocketInfo;

    fn create_random_vec(size: usize) -> Vec<u8> {
        let mut vec = vec![0u8; size];
        let mut file = File::open("/dev/urandom").expect("Failed to open /dev/urandom");
        file.read_exact(&mut vec).expect("Failed to read bytes");
        vec
    }

    #[test]
    fn integration_gso_gro() {
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

        let sender_socket_info = Arc::new(UdpSocketInfo {
            socket_state: quinn_udp::UdpSocketState::new(quinn_udp::UdpSockRef::from(&sender_socket)).unwrap(),
            socket: sender_socket,
            interface_name: "lo".to_string(),
            local_address: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 6000),
            destination_address: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)), 5000),
        });

        let thread_receiver = ThreadedReceiver::new(receive_socket_info.clone());
        let thread_sender = ThreadedSender::new(sender_socket_info.clone());

        let data_to_send = create_random_vec(1000);;

        let test_iterations = 49;

        smol::block_on( async {
            for _ in 0..test_iterations {
                thread_sender.packet_channel.send(data_to_send.clone()).await.unwrap();
            }
        });


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