//! Art-Net (ArtDMX) packet build + parse.
//!
//! Ported from stageLX's `stagelx-io::artnet`, trimmed to the pure packet layer.
//! Universe is a flat `u16`; Art-Net's 15-bit PortAddress occupies the low 15
//! bits (low byte + `(>>8) & 0x7F` high byte = Net/Subnet/Universe nibbles).
//! Unlike stageLX, the builder takes a per-universe `sequence` byte.

pub const ARTNET_PORT: u16 = 6454;

/// Build an ArtDMX packet (18-byte header + 512 data bytes).
///
/// `port_address` is the 15-bit Art-Net PortAddress (Net/Subnet/Universe). A
/// non-zero `sequence` lets receivers detect out-of-order packets; pass 0 to
/// disable sequencing for a universe.
pub fn build_artdmx(port_address: u16, sequence: u8, data: &[u8; 512]) -> Vec<u8> {
    let mut pkt = Vec::with_capacity(18 + 512);
    pkt.extend_from_slice(b"Art-Net\0");
    pkt.push(0x00);
    pkt.push(0x50); // OpCode ArtDMX = 0x5000 (little-endian)
    pkt.push(0x00);
    pkt.push(14); // ProtVer 14 (big-endian)
    pkt.push(sequence);
    pkt.push(0); // Physical
    pkt.push((port_address & 0xFF) as u8);
    pkt.push(((port_address >> 8) & 0x7F) as u8);
    pkt.push(0x02);
    pkt.push(0x00); // Length 512 (big-endian)
    pkt.extend_from_slice(data);
    pkt
}

/// Parse an ArtDMX packet. Returns `(port_address, sequence, dmx_data)` or
/// `None` if the buffer is not a valid ArtDMX packet.
pub fn parse_artdmx(buf: &[u8]) -> Option<(u16, u8, &[u8])> {
    if buf.len() < 18 || &buf[..8] != b"Art-Net\0" {
        return None;
    }
    let opcode = u16::from_le_bytes([buf[8], buf[9]]);
    if opcode != 0x5000 {
        return None;
    }
    let sequence = buf[12];
    let port_address = (buf[14] as u16) | ((buf[15] as u16 & 0x7F) << 8);
    let length = u16::from_be_bytes([buf[16], buf[17]]) as usize;
    if length == 0 || buf.len() < 18 + length {
        return None;
    }
    Some((port_address, sequence, &buf[18..18 + length]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_artdmx_layout_is_correct() {
        let mut data = [0u8; 512];
        data[0] = 9;
        data[511] = 5;
        let pkt = build_artdmx(0x0102, 3, &data);

        assert_eq!(pkt.len(), 18 + 512);
        assert_eq!(&pkt[..8], b"Art-Net\0");
        // OpCode ArtDMX 0x5000 LE.
        assert_eq!(&pkt[8..10], &[0x00, 0x50]);
        // ProtVer 14 BE.
        assert_eq!(&pkt[10..12], &[0x00, 14]);
        // Sequence, physical.
        assert_eq!(pkt[12], 3);
        assert_eq!(pkt[13], 0);
        // PortAddress: low byte then high (15-bit).
        assert_eq!(pkt[14], 0x02);
        assert_eq!(pkt[15], 0x01);
        // Length 512 BE.
        assert_eq!(&pkt[16..18], &[0x02, 0x00]);
        // Data.
        assert_eq!(pkt[18], 9);
        assert_eq!(pkt[18 + 511], 5);
    }

    #[test]
    fn high_universe_bit15_is_masked() {
        // 0x8000 sets bit 15, which is not part of the PortAddress.
        let pkt = build_artdmx(0x8123, 0, &[0u8; 512]);
        assert_eq!(pkt[14], 0x23);
        assert_eq!(pkt[15], 0x01); // 0x81 & 0x7F = 0x01
    }

    #[test]
    fn parse_roundtrip() {
        let mut data = [0u8; 512];
        for (i, b) in data.iter_mut().enumerate() {
            *b = (i % 256) as u8;
        }
        let pkt = build_artdmx(300, 11, &data);
        let (addr, seq, parsed) = parse_artdmx(&pkt).expect("valid packet");
        assert_eq!(addr, 300);
        assert_eq!(seq, 11);
        assert_eq!(parsed, &data[..]);
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(parse_artdmx(&[0u8; 4]).is_none());
        assert!(parse_artdmx(&[0xffu8; 530]).is_none());
    }
}
