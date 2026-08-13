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

/// The two tests above almost never clear the outer `GMA\0MSC\0` header + length
/// prefix, so they exercise the rejection path but not the command parser behind
/// it — where the real index hazards live (per-command `buf[n]` reads, the QID
/// and executor/page string scans). This primes a correctly-framed packet and
/// fuzzes only the inner sysex payload, so `parse_command_data` and its helpers
/// actually run against hostile data, then truncates every framed packet too.
#[test]
fn well_framed_packets_with_fuzzed_payloads_never_panic() {
    const HEADER: &[u8] = b"GMA\0MSC\0";
    let mut rng = Xorshift64::new(0x6D736364);
    for _ in 0..40_000 {
        // Inner MIDI sysex: F0 7F <device> 02 <format> <command> <data...> F7.
        let mut sysex = vec![0xF0, 0x7F, rng.next_byte(), 0x02, rng.next_byte(), rng.next_byte()];
        let data_len = rng.next_range(0, 24) as usize;
        sysex.extend(rng.next_bytes(data_len));
        sysex.push(0xF7);

        let mut pkt = Vec::with_capacity(HEADER.len() + 4 + sysex.len());
        pkt.extend_from_slice(HEADER);
        pkt.extend_from_slice(&(sysex.len() as u32).to_le_bytes());
        pkt.extend_from_slice(&sysex);

        let _ = MamscPacket::try_read(&pkt);
        // Cutting a validly-framed packet drives the length-prefix bounds checks
        // with a real header in front of them.
        for cut in 0..pkt.len() {
            let _ = MamscPacket::try_read(&pkt[..cut]);
        }
    }
}
