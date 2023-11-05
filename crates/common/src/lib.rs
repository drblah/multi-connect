use nix::libc;
use std::io::Error;
use std::net::{IpAddr, Ipv4Addr, SocketAddrV4};
use smol::net::UdpSocket;
use socket2::{Domain, Socket, Type};
use std::net::UdpSocket as std_udp;
use smol::Async;

pub mod messages;
pub mod sequencer;
pub mod connection;
pub mod endpoint;
pub mod connection_manager;


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
fn make_socket(interface: &str, local_address: Option<Ipv4Addr>, local_port: Option<u16>, bind_to_device: bool) -> UdpSocket {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, None).unwrap();

    if bind_to_device {
        if let Err(err) = socket.bind_device(Some(interface.as_bytes())) {
            if matches!(err.raw_os_error(), Some(libc::ENODEV)) {
                panic!("error binding to device (`{}`): {}", interface, err);
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

    socket.bind(&address.into()).unwrap();

    let std_udp: std_udp = socket.into();
    std_udp.set_nonblocking(true).unwrap();

    let udp_socket: UdpSocket = UdpSocket::from(Async::try_from(std_udp).unwrap());

    udp_socket
}