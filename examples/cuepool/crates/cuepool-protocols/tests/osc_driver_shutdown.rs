//! Dropping an OSC driver must return. The RX thread only observes the stop
//! flag between reads, so an unbounded `recv_from` on a quiet port would park
//! it forever and the join in `Drop` with it.

use cuepool_protocols::osc::OscDriver;
use std::net::{Ipv4Addr, UdpSocket};
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn dropping_an_idle_driver_returns() {
    // A free port picked by the OS, then released: a fixed one would collide
    // with whatever else is running on the build machine. Nothing is ever sent
    // to it, which is the case that used to hang.
    let probe = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("loopback bind");
    let rx_port = probe
        .local_addr()
        .expect("bound socket has an address")
        .port();
    drop(probe);

    let mut driver =
        OscDriver::bind(Ipv4Addr::LOCALHOST, rx_port, 9001).expect("bind the released port");
    driver.start(|_msg, _src| {});

    // Drop on a worker: a hang here must fail the test rather than wedge the
    // whole run, which is what a bare `drop(driver)` would do.
    let (done_tx, done_rx) = mpsc::channel();
    std::thread::spawn(move || {
        drop(driver);
        let _ = done_tx.send(());
    });

    assert!(
        done_rx.recv_timeout(Duration::from_secs(5)).is_ok(),
        "the RX thread must wake up on its own to see the stop flag"
    );
}
