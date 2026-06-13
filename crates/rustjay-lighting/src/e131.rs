//! sACN / E1.31 (ANSI E1.31-2016) packet build + parse.
//!
//! Ported from stageLX's `stagelx-io::sacn`, trimmed to the pure packet layer
//! (no bevy/crossbeam). The builder produces a 638-byte E1.31 Data Packet; the
//! parser is used by loopback tests and could back an RX path later.

use std::net::Ipv4Addr;

pub const SACN_PORT: u16 = 5568;

/// ACN Packet Identifier per ANSI E1.17 (`"ASC-E1.17\0\0\0"`).
const ACN_ID: &[u8; 12] = b"ASC-E1.17\x00\x00\x00";

/// Fixed UUID4-format CID for rustjay (RFC 4122 §4.4).
///
/// Distinct from stageLX's CID so consoles see a stable, separate source
/// identity. Byte 6 high nibble = 4 (version 4); byte 8 high two bits = 10
/// (variant 1). Value: `7a9f3c2e-4b1d-4e8a-9c6f-1d2b3a4c5e6f`.
const CID: [u8; 16] = [
    0x7a, 0x9f, 0x3c, 0x2e, 0x4b, 0x1d, 0x4e, 0x8a, 0x9c, 0x6f, 0x1d, 0x2b, 0x3a, 0x4c, 0x5e, 0x6f,
];

pub const SACN_PACKET_LEN: usize = 638;

/// Encode a PDU Flags & Length field: top 4 bits = 0x7, bottom 12 bits =
/// `total - pdu_start`.
fn fl(pdu_start: usize, total: usize) -> [u8; 2] {
    let len = total - pdu_start;
    [0x70 | ((len >> 8) as u8 & 0x0F), (len & 0xFF) as u8]
}

/// Build a 638-byte E1.31 Data Packet for one universe.
///
/// `source_name` is written into the 64-byte Source Name field (truncated to
/// 63 bytes + NUL). `data` is the 512-slot DMX buffer (start code 0x00 is added
/// by this function).
pub fn build_sacn(
    universe: u16,
    priority: u8,
    sequence: u8,
    source_name: &str,
    data: &[u8; 512],
) -> Vec<u8> {
    const TOTAL: usize = SACN_PACKET_LEN;
    let mut p = vec![0u8; TOTAL];

    // Preamble / post-amble / ACN identifier (bytes 0–15).
    p[0] = 0x00;
    p[1] = 0x10; // Preamble Size
    p[2] = 0x00;
    p[3] = 0x00; // Post-amble Size
    p[4..16].copy_from_slice(ACN_ID);

    // Root PDU (starts at 0x10 = 16).
    let [h, l] = fl(0x10, TOTAL);
    p[0x10] = h;
    p[0x11] = l;
    p[0x12..0x16].copy_from_slice(&[0x00, 0x00, 0x00, 0x04]); // VECTOR_ROOT_E131_DATA
    p[0x16..0x26].copy_from_slice(&CID);

    // Framing PDU (starts at 0x26 = 38).
    let [h, l] = fl(0x26, TOTAL);
    p[0x26] = h;
    p[0x27] = l;
    p[0x28..0x2C].copy_from_slice(&[0x00, 0x00, 0x00, 0x02]); // VECTOR_E131_DATA_PACKET

    // Source Name (64 bytes, NUL-padded). Truncate to 63 bytes to guarantee a
    // terminating NUL within the field.
    let name = source_name.as_bytes();
    let n = name.len().min(63);
    p[0x2C..0x2C + n].copy_from_slice(&name[..n]);

    p[0x6C] = priority;
    // Sync address = 0 (0x6D–0x6E), Sequence number, Options = 0.
    p[0x6F] = sequence;
    // Universe (big-endian).
    p[0x71] = (universe >> 8) as u8;
    p[0x72] = (universe & 0xFF) as u8;

    // DMP PDU (starts at 0x73 = 115).
    let [h, l] = fl(0x73, TOTAL);
    p[0x73] = h;
    p[0x74] = l;
    p[0x75] = 0x02; // VECTOR_DMP_SET_PROPERTY
    p[0x76] = 0xA1; // Address Type & Data Type
    // First Property Address = 0, Address Increment = 1.
    p[0x79] = 0x00;
    p[0x7A] = 0x01;
    // Property Count = 513 (start code + 512 slots).
    p[0x7B] = 0x02;
    p[0x7C] = 0x01;
    // Start code 0x00 (null / standard DMX).
    p[0x7D] = 0x00;
    // DMX data.
    p[0x7E..0x27E].copy_from_slice(data);

    p
}

/// Parse an E1.31 Data Packet. Returns `(universe, priority, dmx_data)` or
/// `None` if the buffer is not a valid null-start-code data packet.
pub fn parse_sacn(buf: &[u8]) -> Option<(u16, u8, &[u8])> {
    if buf.len() < 0x7E {
        return None;
    }
    if &buf[4..16] != ACN_ID {
        return None;
    }
    // Root vector must be VECTOR_ROOT_E131_DATA.
    if buf[0x12..0x16] != [0x00, 0x00, 0x00, 0x04] {
        return None;
    }
    // Framing vector must be VECTOR_E131_DATA_PACKET.
    if buf[0x28..0x2C] != [0x00, 0x00, 0x00, 0x02] {
        return None;
    }
    // DMP vector.
    if buf[0x75] != 0x02 {
        return None;
    }
    // Only accept the null start code.
    if buf[0x7D] != 0x00 {
        return None;
    }
    let priority = buf[0x6C];
    let universe = ((buf[0x71] as u16) << 8) | (buf[0x72] as u16);
    if universe == 0 || universe > 63999 {
        return None;
    }
    let end = buf.len().min(0x7E + 512);
    Some((universe, priority, &buf[0x7E..end]))
}

/// Multicast address for a sACN universe: `239.255.(universe>>8).(universe&0xFF)`.
pub fn multicast_addr(universe: u16) -> Ipv4Addr {
    Ipv4Addr::new(239, 255, (universe >> 8) as u8, (universe & 0xFF) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_sacn_layout_is_correct() {
        let mut data = [0u8; 512];
        data[0] = 1;
        data[1] = 2;
        data[2] = 3;
        let pkt = build_sacn(1, 100, 0, "rustjay", &data);

        assert_eq!(pkt.len(), SACN_PACKET_LEN);
        // Preamble + ACN identifier.
        assert_eq!(&pkt[0..4], &[0x00, 0x10, 0x00, 0x00]);
        assert_eq!(&pkt[4..16], ACN_ID);
        // Root + framing + DMP vectors.
        assert_eq!(&pkt[0x12..0x16], &[0x00, 0x00, 0x00, 0x04]);
        assert_eq!(&pkt[0x28..0x2C], &[0x00, 0x00, 0x00, 0x02]);
        assert_eq!(pkt[0x75], 0x02);
        // CID.
        assert_eq!(&pkt[0x16..0x26], &CID);
        // Source name.
        assert_eq!(&pkt[0x2C..0x2C + 7], b"rustjay");
        assert_eq!(pkt[0x2C + 7], 0x00, "source name must be NUL-terminated");
        // Priority, sequence, universe.
        assert_eq!(pkt[0x6C], 100);
        assert_eq!(pkt[0x6F], 0);
        assert_eq!(pkt[0x71], 0x00);
        assert_eq!(pkt[0x72], 0x01);
        // Start code + data.
        assert_eq!(pkt[0x7D], 0x00);
        assert_eq!(&pkt[0x7E..0x7E + 3], &[1, 2, 3]);
    }

    #[test]
    fn universe_big_endian_high_byte() {
        let pkt = build_sacn(0x0102, 100, 0, "x", &[0u8; 512]);
        assert_eq!(pkt[0x71], 0x01);
        assert_eq!(pkt[0x72], 0x02);
    }

    #[test]
    fn long_source_name_is_truncated_and_terminated() {
        let long = "a".repeat(100);
        let pkt = build_sacn(1, 100, 0, &long, &[0u8; 512]);
        // 63 bytes of 'a', then NUL at offset 63 within the 64-byte field.
        assert!(pkt[0x2C..0x2C + 63].iter().all(|&b| b == b'a'));
        assert_eq!(pkt[0x2C + 63], 0x00);
    }

    #[test]
    fn parse_roundtrip() {
        let mut data = [0u8; 512];
        for (i, b) in data.iter_mut().enumerate() {
            *b = (i % 256) as u8;
        }
        let pkt = build_sacn(42, 120, 7, "rustjay", &data);
        let (u, pri, parsed) = parse_sacn(&pkt).expect("valid packet");
        assert_eq!(u, 42);
        assert_eq!(pri, 120);
        assert_eq!(parsed, &data[..]);
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(parse_sacn(&[0u8; 10]).is_none());
        assert!(parse_sacn(&[0xffu8; 638]).is_none());
    }

    #[test]
    fn multicast_addresses() {
        assert_eq!(multicast_addr(1), Ipv4Addr::new(239, 255, 0, 1));
        assert_eq!(multicast_addr(0x0102), Ipv4Addr::new(239, 255, 1, 2));
    }
}
