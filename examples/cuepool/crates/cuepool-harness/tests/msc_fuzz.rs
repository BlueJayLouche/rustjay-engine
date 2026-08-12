use cuepool_harness::rng::Xorshift64;
use cuepool_protocols::msc::MamscPacket;

/// MamscPacket::try_read is fed straight from a UDP socket. Anything on the
/// venue LAN can send it anything. It returns Option, so malformed input is
/// already expected to be handled — this proves it, including truncation.
#[test]
fn arbitrary_packets_never_panic() {
    let mut rng = Xorshift64::new(0x4D5343);
    for _ in 0..50_000 {
        let len = rng.next_range(0, 600) as usize;
        let buf = rng.next_bytes(len);
        let _ = MamscPacket::try_read(&buf);
    }
}

/// Truncation is the classic length-prefix bug: a header that claims more bytes
/// than arrived. Take plausible packets and cut them at every offset.
#[test]
fn truncated_packets_never_panic() {
    let mut rng = Xorshift64::new(0xBADBEE);
    for _ in 0..2_000 {
        // Plausible MSC-ish frame: sysex start, manufacturer, device, command.
        let mut buf = vec![0xF0, 0x7F, rng.next_byte(), 0x02, rng.next_byte()];
        let data_len = rng.next_range(0, 64) as usize;
        buf.extend(rng.next_bytes(data_len));
        buf.push(0xF7);
        for cut in 0..buf.len() {
            let _ = MamscPacket::try_read(&buf[..cut]);
        }
    }
}
