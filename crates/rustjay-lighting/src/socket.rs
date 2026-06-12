//! UDP socket helpers for DMX transports.
//!
//! Ported from stageLX's `create_tuned_udp_socket`. Transports send from an
//! ephemeral local port; `set_broadcast` is enabled so Art-Net broadcast works
//! without per-call configuration.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};

/// Create a non-blocking UDP socket suitable for DMX transmission.
///
/// Bound to `0.0.0.0:0` (ephemeral source port). Broadcast and address reuse
/// are enabled; on macOS port reuse is enabled too. The send buffer is left at
/// the OS default (DMX frames are small and latest-wins).
pub fn tx_socket() -> std::io::Result<UdpSocket> {
    let bind = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0);
    let socket = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, None)?;
    socket.set_nonblocking(true)?;
    socket.set_broadcast(true)?;
    socket.set_reuse_address(true)?;
    #[cfg(target_os = "macos")]
    socket.set_reuse_port(true)?;
    socket.bind(&bind.into())?;
    Ok(socket.into())
}

/// Bind a non-blocking UDP receive socket to `addr` (used by loopback tests and
/// a future RX path). Enables address/port reuse and a 4 MiB receive buffer.
pub fn rx_socket(addr: SocketAddr) -> std::io::Result<UdpSocket> {
    let domain = if addr.is_ipv4() {
        socket2::Domain::IPV4
    } else {
        socket2::Domain::IPV6
    };
    let socket = socket2::Socket::new(domain, socket2::Type::DGRAM, None)?;
    socket.set_nonblocking(true)?;
    socket.set_reuse_address(true)?;
    #[cfg(target_os = "macos")]
    socket.set_reuse_port(true)?;
    socket.set_recv_buffer_size(4 * 1024 * 1024)?;
    socket.bind(&addr.into())?;
    Ok(socket.into())
}
