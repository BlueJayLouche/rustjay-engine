//! End-to-end check that an OSC address nobody subscribed to reaches the log
//! exactly once, over a real socket rather than the router in isolation.

use cuepool_protocols::osc::OscManager;
use rosc::{OscMessage, OscPacket};
use std::net::{Ipv4Addr, UdpSocket};
use std::sync::Mutex;
use std::time::Duration;

static CAPTURED: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct CaptureLogger;

impl log::Log for CaptureLogger {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        CAPTURED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(record.args().to_string());
    }

    fn flush(&self) {}
}

fn lines_mentioning(needle: &str) -> Vec<String> {
    CAPTURED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|line| line.contains(needle))
        .cloned()
        .collect()
}

fn encode(addr: &str) -> Vec<u8> {
    rosc::encoder::encode(&OscPacket::Message(OscMessage {
        addr: addr.into(),
        args: vec![],
    }))
    .expect("a bare OSC message must encode")
}

#[test]
fn an_unsubscribed_address_is_logged_once() {
    log::set_logger(&CaptureLogger).expect("no other logger in this test binary");
    log::set_max_level(log::LevelFilter::Trace);

    // Ask the OS for a free port, then hand it to the manager: a fixed port
    // would collide with whatever else is running on the build machine.
    let probe = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("loopback bind");
    let rx_port = probe
        .local_addr()
        .expect("bound socket has an address")
        .port();
    drop(probe);

    let (tx, events) = std::sync::mpsc::channel();
    // Only the RX port is bound; the TX port is a destination this test never
    // sends to, so any value does.
    let _manager = OscManager::new(Ipv4Addr::LOCALHOST, rx_port, 9001, tx)
        .expect("OSC manager binds the port the probe just released");

    let sender = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("loopback bind");
    let dest = (Ipv4Addr::LOCALHOST, rx_port);
    // A controller with a typo repeats itself; a subscribed address must stay
    // silent no matter how often it arrives.
    for _ in 0..5 {
        sender.send_to(&encode("/qplayer/gt"), dest).expect("send");
        sender.send_to(&encode("/qplayer/go"), dest).expect("send");
    }

    // The RX thread is asynchronous. Datagrams are handled in arrival order, so
    // the fifth Go event means all five typo'd messages have been through the
    // router too — no sleep-and-hope needed.
    for _ in 0..5 {
        events
            .recv_timeout(Duration::from_secs(5))
            .expect("every /qplayer/go sent over loopback must raise an event");
    }

    assert_eq!(
        lines_mentioning("/qplayer/gt").len(),
        1,
        "the unsubscribed address is reported once, not once per datagram: {:?}",
        lines_mentioning("/qplayer")
    );
    assert!(
        lines_mentioning("/qplayer/go").is_empty(),
        "a subscribed address must not be reported: {:?}",
        lines_mentioning("/qplayer/go")
    );
}
