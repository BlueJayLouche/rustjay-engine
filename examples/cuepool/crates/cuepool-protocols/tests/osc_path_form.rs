//! End-to-end check of the two `/qplayer` cue-addressing forms over a real
//! socket: the argument form (`/qplayer/go` + `"1.1"`) and the path form
//! (`/qplayer/go/1.1`) must raise the same targeted event. The path form is
//! what address-templating controllers (Nodel, TouchOSC layouts) send, and it
//! used to fall through to a bare GO on the standby cue.

use cuepool_protocols::osc::{OscEvent, OscManager};
use rosc::{OscMessage, OscPacket, OscType};
use std::net::{Ipv4Addr, UdpSocket};
use std::time::Duration;

fn encode(addr: &str, args: Vec<OscType>) -> Vec<u8> {
    rosc::encoder::encode(&OscPacket::Message(OscMessage {
        addr: addr.into(),
        args,
    }))
    .expect("OSC message must encode")
}

#[test]
fn path_form_and_arg_form_raise_the_same_targeted_events() {
    // Ask the OS for a free port, then hand it to the manager (same dance as
    // osc_unmatched_logging.rs — a fixed port would collide on build machines).
    let probe = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("loopback bind");
    let rx_port = probe
        .local_addr()
        .expect("bound socket has an address")
        .port();
    drop(probe);

    let (tx, events) = std::sync::mpsc::channel();
    let _manager = OscManager::new(Ipv4Addr::LOCALHOST, rx_port, 9001, tx)
        .expect("OSC manager binds the port the probe just released");

    let sender = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("loopback bind");
    let dest = (Ipv4Addr::LOCALHOST, rx_port);
    let sent = [
        encode("/qplayer/go/1.1", vec![]),
        encode("/qplayer/go", vec![OscType::String("2.12".into())]),
        encode("/qplayer/select/4.3", vec![]),
        encode("/qplayer/stop/1.3", vec![]),
    ];
    for datagram in &sent {
        sender.send_to(datagram, dest).expect("send");
    }

    // One RX thread handles datagrams in arrival order.
    let recv = || {
        events
            .recv_timeout(Duration::from_secs(5))
            .expect("every message sent over loopback must raise an event")
    };

    match recv() {
        OscEvent::Go { qid: Some(qid) } if qid == "1.1" => {}
        other => panic!("/qplayer/go/1.1 must GO cue 1.1, got {other:?}"),
    }
    match recv() {
        OscEvent::Go { qid: Some(qid) } if qid == "2.12" => {}
        other => panic!("argument form must keep working, got {other:?}"),
    }
    match recv() {
        OscEvent::Select { qid } if qid == "4.3" => {}
        other => panic!("/qplayer/select/4.3 must select cue 4.3, got {other:?}"),
    }
    match recv() {
        OscEvent::Stop { qid: Some(qid) } if qid == "1.3" => {}
        other => panic!("/qplayer/stop/1.3 must stop cue 1.3, got {other:?}"),
    }
}
