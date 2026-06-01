use anyhow::{Context, Result};
use get_if_addrs::{get_if_addrs, IfAddr};
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::fmt;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkInterface {
    pub name: String,
    pub addr: Ipv4Addr,
}

impl fmt::Display for NetworkInterface {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} ({})", self.name, self.addr)
    }
}

pub struct MulticastSender {
    socket: UdpSocket,
    destination: SocketAddrV4,
}

impl MulticastSender {
    pub fn new(multicast_ip: Ipv4Addr, port: u16, interface: Option<Ipv4Addr>) -> Result<Self> {
        Self::new_with_loopback(multicast_ip, port, interface, false)
    }

    fn new_with_loopback(
        multicast_ip: Ipv4Addr,
        port: u16,
        interface: Option<Ipv4Addr>,
        loopback: bool,
    ) -> Result<Self> {
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
            .context("failed to create UDP socket")?;
        socket
            .bind(&SockAddr::from(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)))
            .context("failed to bind UDP socket")?;
        socket
            .set_multicast_ttl_v4(16)
            .context("failed to set multicast TTL")?;
        socket
            .set_multicast_loop_v4(loopback)
            .context("failed to configure multicast loopback")?;

        if let Some(interface_addr) = interface {
            socket
                .set_multicast_if_v4(&interface_addr)
                .context("failed to set multicast interface")?;
        }

        Ok(Self {
            socket: socket.into(),
            destination: SocketAddrV4::new(multicast_ip, port),
        })
    }

    pub fn send(&self, packet: &[u8]) -> Result<usize> {
        self.socket
            .send_to(packet, self.destination)
            .context("failed to send multicast packet")
    }
}

pub fn ipv4_interfaces() -> Vec<NetworkInterface> {
    let mut interfaces = get_if_addrs()
        .map(|interfaces| {
            interfaces
                .into_iter()
                .filter_map(|interface| match interface.addr {
                    IfAddr::V4(v4) => Some(NetworkInterface {
                        name: interface.name,
                        addr: v4.ip,
                    }),
                    IfAddr::V6(_) => None,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    interfaces.sort_by(|left, right| left.name.cmp(&right.name).then(left.addr.cmp(&right.addr)));
    interfaces
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    #[ignore = "requires local multicast loopback support on the host network stack"]
    fn sends_packet_to_joined_multicast_group() {
        let group = Ipv4Addr::new(239, 10, 10, 250);
        let receiver = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)).unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        receiver
            .join_multicast_v4(&group, &Ipv4Addr::UNSPECIFIED)
            .unwrap();
        let port = receiver.local_addr().unwrap().port();

        let sender = MulticastSender::new_with_loopback(group, port, None, true).unwrap();
        sender.send(b"fas-test").unwrap();

        let mut buf = [0u8; 64];
        let (len, _) = receiver.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..len], b"fas-test");
    }
}
