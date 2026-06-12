//! [`DmxTransport`] — pluggable wire protocol behind a [`DmxFrame`].
//!
//! Two implementations ship: [`SacnTransport`] (E1.31, multicast by default) and
//! [`ArtNetTransport`] (ArtDMX, broadcast by default). Both maintain a
//! per-universe sequence counter and packetise one UDP datagram per universe.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};

use crate::dmx::DmxFrame;
use crate::{artnet, e131, socket};

/// Where a transport sends its datagrams.
#[derive(Debug, Clone)]
pub enum Dest {
    /// Per-universe multicast group (sACN: `239.255.hi.lo`).
    Multicast,
    /// Limited broadcast (`255.255.255.255`) — the Art-Net default.
    Broadcast,
    /// A single unicast IPv4 address.
    Unicast(Ipv4Addr),
}

/// A wire protocol that can transmit a [`DmxFrame`].
pub trait DmxTransport: Send {
    /// Packetise and send every universe in `frame`.
    fn send(&mut self, frame: &DmxFrame);
}

// ─── sACN ──────────────────────────────────────────────────────────────────

/// sACN (E1.31) transmitter.
pub struct SacnTransport {
    socket: UdpSocket,
    dest: Dest,
    port: u16,
    priority: u8,
    source_name: String,
    seq: HashMap<u16, u8>,
}

impl SacnTransport {
    /// Create a transmitter. `priority` is the E1.31 priority (0–200, default
    /// 100 in most consoles); `source_name` populates the 64-byte Source Name.
    pub fn new(dest: Dest, priority: u8, source_name: impl Into<String>) -> std::io::Result<Self> {
        Ok(Self {
            socket: socket::tx_socket()?,
            dest,
            port: e131::SACN_PORT,
            priority,
            source_name: source_name.into(),
            seq: HashMap::new(),
        })
    }

    /// Override the destination UDP port (loopback tests only; production sACN
    /// is fixed at [`e131::SACN_PORT`]).
    pub fn with_dest_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    fn dest_addr(&self, universe: u16) -> SocketAddr {
        let ip = match self.dest {
            Dest::Multicast => e131::multicast_addr(universe),
            Dest::Broadcast => Ipv4Addr::BROADCAST,
            Dest::Unicast(ip) => ip,
        };
        SocketAddr::new(ip.into(), self.port)
    }
}

impl DmxTransport for SacnTransport {
    fn send(&mut self, frame: &DmxFrame) {
        for (universe, data) in frame.iter() {
            let seq = self.seq.entry(universe).or_insert(0);
            let pkt = e131::build_sacn(universe, self.priority, *seq, &self.source_name, data);
            *seq = seq.wrapping_add(1);
            let addr = self.dest_addr(universe);
            if let Err(e) = self.socket.send_to(&pkt, addr) {
                log::warn!("sACN send to {addr} (universe {universe}) failed: {e}");
            }
        }
    }
}

// ─── Art-Net ───────────────────────────────────────────────────────────────

/// Art-Net (ArtDMX) transmitter.
pub struct ArtNetTransport {
    socket: UdpSocket,
    dest: Dest,
    port: u16,
    seq: HashMap<u16, u8>,
}

impl ArtNetTransport {
    /// Create a transmitter. Defaults elsewhere wire `dest` to [`Dest::Broadcast`].
    pub fn new(dest: Dest) -> std::io::Result<Self> {
        Ok(Self {
            socket: socket::tx_socket()?,
            dest,
            port: artnet::ARTNET_PORT,
            seq: HashMap::new(),
        })
    }

    /// Override the destination UDP port (loopback tests only).
    pub fn with_dest_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    fn dest_addr(&self) -> SocketAddr {
        // Art-Net has no per-universe multicast; multicast falls back to broadcast.
        let ip = match self.dest {
            Dest::Unicast(ip) => ip,
            _ => Ipv4Addr::BROADCAST,
        };
        SocketAddr::new(ip.into(), self.port)
    }
}

impl DmxTransport for ArtNetTransport {
    fn send(&mut self, frame: &DmxFrame) {
        let addr = self.dest_addr();
        for (universe, data) in frame.iter() {
            // Art-Net sequence runs 1..=255 then wraps to 1 (0 disables it).
            let seq = self.seq.entry(universe).or_insert(0);
            *seq = if *seq == 255 { 1 } else { *seq + 1 };
            let pkt = artnet::build_artdmx(universe, *seq, data);
            if let Err(e) = self.socket.send_to(&pkt, addr) {
                log::warn!("Art-Net send to {addr} (universe {universe}) failed: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::e131::parse_sacn;
    use std::net::Ipv4Addr;
    use std::time::{Duration, Instant};

    /// Bind a localhost receiver, send three sACN frames to it via unicast, and
    /// assert the patched bytes + incrementing sequence arrive intact.
    #[test]
    fn sacn_loopback_unicast() {
        let rx = socket::rx_socket(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0)).unwrap();
        let port = rx.local_addr().unwrap().port();

        let mut tx = SacnTransport::new(Dest::Unicast(Ipv4Addr::LOCALHOST), 100, "rustjay")
            .unwrap()
            .with_dest_port(port);

        for expected_seq in 0u8..3 {
            let mut frame = DmxFrame::new();
            let u = frame.universe_mut(1);
            u[0] = 11;
            u[1] = 22;
            u[2] = 33;
            tx.send(&frame);

            let mut buf = [0u8; 700];
            let n = recv_with_timeout(&rx, &mut buf, Duration::from_secs(2))
                .expect("packet should arrive on loopback");
            let (uni, pri, data) = parse_sacn(&buf[..n]).expect("valid sACN");
            assert_eq!(uni, 1);
            assert_eq!(pri, 100);
            assert_eq!(&data[..3], &[11, 22, 33]);
            assert_eq!(buf[0x6F], expected_seq, "sequence should increment");
        }
    }

    fn recv_with_timeout(
        sock: &UdpSocket,
        buf: &mut [u8],
        timeout: Duration,
    ) -> Option<usize> {
        let start = Instant::now();
        while start.elapsed() < timeout {
            match sock.recv_from(buf) {
                Ok((n, _)) => return Some(n),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(_) => return None,
            }
        }
        None
    }
}
