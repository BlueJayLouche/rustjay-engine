//! [`DmxSender`] — a paced background transmit thread.
//!
//! The render/update thread `submit`s the newest [`DmxFrame`] into a shared
//! latest-wins cell; a background thread re-sends that frame at a fixed rate
//! (DMX keep-alive, so fixtures don't time out on static content). Submitting
//! never blocks on the socket and never queues stale frames.

use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam::channel::{bounded, Sender};

use crate::dmx::DmxFrame;
use crate::transport::DmxTransport;

/// Owns the transmit thread and the shared latest-frame cell.
pub struct DmxSender {
    latest: Arc<Mutex<DmxFrame>>,
    shutdown: Sender<()>,
    handle: Option<JoinHandle<()>>,
}

impl DmxSender {
    /// Spawn a sender that drives `transport` at `fps` frames per second
    /// (clamped to a sane floor). Default lighting rate is 44 Hz.
    pub fn spawn(mut transport: Box<dyn DmxTransport>, fps: f32) -> Self {
        let latest = Arc::new(Mutex::new(DmxFrame::new()));
        let (sd_tx, sd_rx) = bounded::<()>(1);
        let latest_thread = Arc::clone(&latest);
        let interval = Duration::from_secs_f32(1.0 / fps.clamp(1.0, 60.0));

        let handle = std::thread::spawn(move || {
            let ticker = crossbeam::channel::tick(interval);
            loop {
                crossbeam::select! {
                    recv(ticker) -> _ => {
                        let frame = match latest_thread.lock() {
                            Ok(g) => g.clone(),
                            Err(p) => p.into_inner().clone(),
                        };
                        if !frame.is_empty() {
                            transport.send(&frame);
                        }
                    }
                    recv(sd_rx) -> _ => break,
                }
            }
        });

        Self {
            latest,
            shutdown: sd_tx,
            handle: Some(handle),
        }
    }

    /// Replace the frame the transmit thread re-sends each tick. Latest wins;
    /// this is non-blocking and cheap.
    pub fn submit(&self, frame: DmxFrame) {
        match self.latest.lock() {
            Ok(mut g) => *g = frame,
            Err(p) => *p.into_inner() = frame,
        }
    }

    /// Read the latest frame without removing it. Useful for UI mirrors.
    pub fn peek_latest(&self) -> DmxFrame {
        match self.latest.lock() {
            Ok(g) => g.clone(),
            Err(p) => p.into_inner().clone(),
        }
    }

    /// Signal the transmit thread to stop and join it.
    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        let _ = self.shutdown.send(());
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for DmxSender {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dmx::Universe;

    /// Records every universe-0 byte-0 value it is asked to send.
    struct MockTransport {
        sent: Arc<Mutex<Vec<u8>>>,
    }
    impl DmxTransport for MockTransport {
        fn send(&mut self, frame: &DmxFrame) {
            if let Some(u) = frame.get(1) {
                let u: &Universe = u;
                self.sent.lock().unwrap().push(u[0]);
            }
        }
    }

    #[test]
    fn paces_and_resends_latest() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let transport = Box::new(MockTransport {
            sent: Arc::clone(&sent),
        });
        // 60 Hz → ~16 ms ticks.
        let sender = DmxSender::spawn(transport, 60.0);

        let mut frame = DmxFrame::new();
        frame.universe_mut(1)[0] = 42;
        sender.submit(frame);

        // Let several ticks fire; the same value should be re-sent (keep-alive).
        std::thread::sleep(Duration::from_millis(80));
        sender.shutdown();

        let values = sent.lock().unwrap();
        assert!(
            values.len() >= 2,
            "expected multiple keep-alive sends, got {}",
            values.len()
        );
        assert!(values.iter().all(|&v| v == 42), "all sends carry latest frame");
    }

    #[test]
    fn peek_latest_returns_submitted_frame() {
        let transport = Box::new(MockTransport {
            sent: Arc::new(Mutex::new(Vec::new())),
        });
        let sender = DmxSender::spawn(transport, 60.0);

        let mut frame = DmxFrame::new();
        frame.universe_mut(1)[0] = 42;
        sender.submit(frame.clone());
        assert_eq!(sender.peek_latest().get(1).map(|u| u[0]), Some(42));

        let mut frame2 = DmxFrame::new();
        frame2.universe_mut(1)[0] = 99;
        sender.submit(frame2);
        assert_eq!(sender.peek_latest().get(1).map(|u| u[0]), Some(99));

        sender.shutdown();
    }

    #[test]
    fn empty_frame_is_not_sent() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let transport = Box::new(MockTransport {
            sent: Arc::clone(&sent),
        });
        let sender = DmxSender::spawn(transport, 60.0);
        std::thread::sleep(Duration::from_millis(50));
        sender.shutdown();
        assert!(sent.lock().unwrap().is_empty());
    }
}
