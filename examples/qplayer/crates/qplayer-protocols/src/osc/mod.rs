//! OSC (Open Sound Control) driver and router.
//!
//! Replaces C# `OSCDriver` and `OSCAddressRouter`.
//! Uses `rosc` for encoding/decoding and `std::net::UdpSocket` for transport.

use rosc::{OscMessage, OscPacket, OscType};
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// Events emitted by the OSC manager that the application should handle.
#[derive(Debug, Clone)]
pub enum OscEvent {
    Go { qid: Option<String> },
    Stop { qid: Option<String> },
    Pause { qid: Option<String> },
    Unpause { qid: Option<String> },
    Preload { qid: Option<String>, time: Option<f32> },
    Select { qid: String },
    Up,
    Down,
    Save,
    RemoteDiscovery { name: String, addr: Option<std::net::SocketAddr> },
    RemoteGo { target: String, qid: String },
    RemotePause { target: String, qid: String },
    RemoteUnpause { target: String, qid: String },
    RemoteStop { target: String, qid: String },
    RemotePreload { target: String, qid: String, time: f32 },
    RemotePing,
    RemoteUpdateShowAck { name: String, block: i32 },
    RemoteUpdateShowNack { name: String, block: i32 },
    RawMessage(OscMessage),
}

/// Low-level UDP OSC driver.
pub struct OscDriver {
    socket: Arc<UdpSocket>,
    tx_addr: std::net::SocketAddr,
    rx_thread: Option<JoinHandle<()>>,
    running: Arc<std::sync::atomic::AtomicBool>,
}

impl OscDriver {
    /// Bind to a local port and prepare for RX/TX.
    pub fn bind(nic: Ipv4Addr, rx_port: u16, tx_port: u16, subnet: Ipv4Addr) -> anyhow::Result<Self> {
        let broadcast = make_broadcast(nic, subnet);
        let bind_addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, rx_port);
        let socket = UdpSocket::bind(bind_addr)?;
        socket.set_nonblocking(false)?;

        let tx_addr: std::net::SocketAddr = SocketAddrV4::new(broadcast, tx_port).into();

        Ok(Self {
            socket: Arc::new(socket),
            tx_addr,
            rx_thread: None,
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }

    /// Start the RX thread. `on_msg` is called for every received OSC message.
    pub fn start<F>(&mut self, mut on_msg: F)
    where
        F: FnMut(OscMessage, std::net::SocketAddr) + Send + 'static,
    {
        self.running.store(true, std::sync::atomic::Ordering::Relaxed);
        let socket = Arc::clone(&self.socket);
        let running = Arc::clone(&self.running);
        self.rx_thread = Some(std::thread::spawn(move || {
            let mut buf = [0u8; 65536];
            while running.load(std::sync::atomic::Ordering::Relaxed) {
                match socket.recv_from(&mut buf) {
                    Ok((len, src)) => {
                        match rosc::decoder::decode_udp(&buf[..len]) {
                            Ok((_, packet)) => {
                                if let OscPacket::Message(msg) = packet {
                                    on_msg(msg, src);
                                }
                            }
                            Err(e) => {
                                log::warn!("OSC decode error from {src}: {e}");
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    Err(e) => {
                        log::warn!("OSC recv error: {e}");
                    }
                }
            }
        }));
    }

    /// Send an OSC message to the default TX address.
    pub fn send(&self, msg: OscMessage) -> anyhow::Result<()> {
        let packet = OscPacket::Message(msg);
        let bytes = rosc::encoder::encode(&packet)?;
        self.socket.send_to(&bytes, self.tx_addr)?;
        Ok(())
    }

    /// Send an OSC message to a specific address.
    pub fn send_to(&self, msg: OscMessage, addr: std::net::SocketAddr) -> anyhow::Result<()> {
        let packet = OscPacket::Message(msg);
        let bytes = rosc::encoder::encode(&packet)?;
        self.socket.send_to(&bytes, addr)?;
        Ok(())
    }

    pub fn tx_addr(&self) -> std::net::SocketAddr {
        self.tx_addr
    }
}

impl Drop for OscDriver {
    fn drop(&mut self) {
        self.running.store(false, std::sync::atomic::Ordering::Relaxed);
        if let Some(t) = self.rx_thread.take() {
            let _ = t.join();
        }
    }
}

fn make_broadcast(adapter: Ipv4Addr, subnet: Ipv4Addr) -> Ipv4Addr {
    let a = u32::from_be_bytes(adapter.octets());
    let s = u32::from_be_bytes(subnet.octets());
    Ipv4Addr::from(a | !s)
}

// ---------------------------------------------------------------------------
// Address router
// ---------------------------------------------------------------------------

/// Trie-based OSC address router supporting `?` single-segment wildcards.
pub struct OscRouter {
    root: RouterNode,
}

struct RouterNode {
    handlers: Vec<Box<dyn Fn(&OscMessage) + Send>>,
    children: HashMap<String, RouterNode>,
    wildcard: Option<Box<RouterNode>>,
}

impl OscRouter {
    pub fn new() -> Self {
        Self {
            root: RouterNode {
                handlers: Vec::new(),
                children: HashMap::new(),
                wildcard: None,
            },
        }
    }

    /// Subscribe a handler to an address pattern.
    /// Patterns are of the form `/foo/bar` or `/foo/?/bar`.
    pub fn subscribe<F>(&mut self, pattern: &str, handler: F)
    where
        F: Fn(&OscMessage) + Send + 'static,
    {
        let parts: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
        let mut node = &mut self.root;
        for part in parts {
            if part == "?" {
                if node.wildcard.is_none() {
                    node.wildcard = Some(Box::new(RouterNode {
                        handlers: Vec::new(),
                        children: HashMap::new(),
                        wildcard: None,
                    }));
                }
                node = node.wildcard.as_mut().unwrap();
            } else {
                if !node.children.contains_key(part) {
                    node.children.insert(
                        part.to_string(),
                        RouterNode {
                            handlers: Vec::new(),
                            children: HashMap::new(),
                            wildcard: None,
                        },
                    );
                }
                node = node.children.get_mut(part).unwrap();
            }
        }
        node.handlers.push(Box::new(handler));
    }

    /// Route a message to all matching handlers.
    pub fn route(&self, msg: &OscMessage) {
        let addr = msg.addr.clone();
        let parts: Vec<&str> = addr.split('/').filter(|s| !s.is_empty()).collect();
        Self::route_node(&self.root, &parts, 0, msg);
    }

    fn route_node(node: &RouterNode, parts: &[&str], idx: usize, msg: &OscMessage) {
        // Fire handlers at this node
        for h in &node.handlers {
            h(msg);
        }
        if idx >= parts.len() {
            return;
        }
        // Exact match
        if let Some(child) = node.children.get(parts[idx]) {
            Self::route_node(child, parts, idx + 1, msg);
        }
        // Wildcard match
        if let Some(ref wildcard) = node.wildcard {
            Self::route_node(wildcard, parts, idx + 1, msg);
        }
    }
}

// ---------------------------------------------------------------------------
// Manager
// ---------------------------------------------------------------------------

/// High-level OSC manager that wires QPlayer-specific address patterns to events.
#[allow(dead_code)]
pub struct OscManager {
    driver: OscDriver,
    router: Arc<Mutex<OscRouter>>,
    event_tx: std::sync::mpsc::Sender<OscEvent>,
}

impl OscManager {
    pub fn new(
        nic: Ipv4Addr,
        rx_port: u16,
        tx_port: u16,
        subnet: Ipv4Addr,
        event_tx: std::sync::mpsc::Sender<OscEvent>,
    ) -> anyhow::Result<Self> {
        let mut driver = OscDriver::bind(nic, rx_port, tx_port, subnet)?;
        let router = Arc::new(Mutex::new(OscRouter::new()));

        // Build router with QPlayer address patterns
        {
            let mut r = router.lock().unwrap();
            let tx = event_tx.clone();
            r.subscribe("/qplayer/go", move |msg| {
                let qid = msg.args.first().and_then(arg_to_string);
                let _ = tx.send(OscEvent::Go { qid });
            });
            let tx = event_tx.clone();
            r.subscribe("/qplayer/stop", move |msg| {
                let qid = msg.args.first().and_then(arg_to_string);
                let _ = tx.send(OscEvent::Stop { qid });
            });
            let tx = event_tx.clone();
            r.subscribe("/qplayer/pause", move |msg| {
                let qid = msg.args.first().and_then(arg_to_string);
                let _ = tx.send(OscEvent::Pause { qid });
            });
            let tx = event_tx.clone();
            r.subscribe("/qplayer/unpause", move |msg| {
                let qid = msg.args.first().and_then(arg_to_string);
                let _ = tx.send(OscEvent::Unpause { qid });
            });
            let tx = event_tx.clone();
            r.subscribe("/qplayer/preload", move |msg| {
                let qid = msg.args.first().and_then(arg_to_string);
                let time = msg.args.get(1).and_then(arg_to_f32);
                let _ = tx.send(OscEvent::Preload { qid, time });
            });
            let tx = event_tx.clone();
            r.subscribe("/qplayer/select", move |msg| {
                if let Some(qid) = msg.args.first().and_then(arg_to_string) {
                    let _ = tx.send(OscEvent::Select { qid });
                }
            });
            let tx = event_tx.clone();
            r.subscribe("/qplayer/up", move |_msg| {
                let _ = tx.send(OscEvent::Up);
            });
            let tx = event_tx.clone();
            r.subscribe("/qplayer/down", move |_msg| {
                let _ = tx.send(OscEvent::Down);
            });
            let tx = event_tx.clone();
            r.subscribe("/qplayer/save", move |_msg| {
                let _ = tx.send(OscEvent::Save);
            });

            // Remote control
            let tx = event_tx.clone();
            r.subscribe("/qplayer/remote/discovery", move |msg| {
                if let Some(name) = msg.args.first().and_then(arg_to_string) {
                    let _ = tx.send(OscEvent::RemoteDiscovery { name, addr: None });
                }
            });
            let tx = event_tx.clone();
            r.subscribe("/qplayer/remote/go", move |msg| {
                if let (Some(t), Some(q)) = (msg.args.first().and_then(arg_to_string), msg.args.get(1).and_then(arg_to_string)) {
                    let _ = tx.send(OscEvent::RemoteGo { target: t, qid: q });
                }
            });
            let tx = event_tx.clone();
            r.subscribe("/qplayer/remote/pause", move |msg| {
                if let (Some(t), Some(q)) = (msg.args.first().and_then(arg_to_string), msg.args.get(1).and_then(arg_to_string)) {
                    let _ = tx.send(OscEvent::RemotePause { target: t, qid: q });
                }
            });
            let tx = event_tx.clone();
            r.subscribe("/qplayer/remote/unpause", move |msg| {
                if let (Some(t), Some(q)) = (msg.args.first().and_then(arg_to_string), msg.args.get(1).and_then(arg_to_string)) {
                    let _ = tx.send(OscEvent::RemoteUnpause { target: t, qid: q });
                }
            });
            let tx = event_tx.clone();
            r.subscribe("/qplayer/remote/stop", move |msg| {
                if let (Some(t), Some(q)) = (msg.args.first().and_then(arg_to_string), msg.args.get(1).and_then(arg_to_string)) {
                    let _ = tx.send(OscEvent::RemoteStop { target: t, qid: q });
                }
            });
            let tx = event_tx.clone();
            r.subscribe("/qplayer/remote/preload", move |msg| {
                if let (Some(t), Some(q), Some(time)) = (
                    msg.args.first().and_then(arg_to_string),
                    msg.args.get(1).and_then(arg_to_string),
                    msg.args.get(2).and_then(arg_to_f32),
                ) {
                    let _ = tx.send(OscEvent::RemotePreload { target: t, qid: q, time });
                }
            });
            let tx = event_tx.clone();
            r.subscribe("/qplayer/remote/ping", move |_msg| {
                let _ = tx.send(OscEvent::RemotePing);
            });
            let tx = event_tx.clone();
            r.subscribe("/qplayer/remote/update-show-ack", move |msg| {
                if let (Some(name), Some(block)) = (
                    msg.args.first().and_then(arg_to_string),
                    msg.args.get(1).and_then(arg_to_i32),
                ) {
                    let _ = tx.send(OscEvent::RemoteUpdateShowAck { name, block });
                }
            });
            let tx = event_tx.clone();
            r.subscribe("/qplayer/remote/update-show-nack", move |msg| {
                if let (Some(name), Some(block)) = (
                    msg.args.first().and_then(arg_to_string),
                    msg.args.get(1).and_then(arg_to_i32),
                ) {
                    let _ = tx.send(OscEvent::RemoteUpdateShowNack { name, block });
                }
            });
        }

        let router_clone = Arc::clone(&router);
        driver.start(move |msg, _src| {
            if let Ok(r) = router_clone.lock() {
                r.route(&msg);
            }
        });

        Ok(Self {
            driver,
            router,
            event_tx,
        })
    }

    pub fn send(&self, msg: OscMessage) -> anyhow::Result<()> {
        self.driver.send(msg)
    }

    pub fn send_to(&self, msg: OscMessage, addr: std::net::SocketAddr) -> anyhow::Result<()> {
        self.driver.send_to(msg, addr)
    }

    pub fn tx_addr(&self) -> std::net::SocketAddr {
        self.driver.tx_addr()
    }
}

fn arg_to_string(arg: &OscType) -> Option<String> {
    match arg {
        OscType::String(s) => Some(s.clone()),
        OscType::Int(i) => Some(i.to_string()),
        OscType::Float(f) => Some(f.to_string()),
        OscType::Long(l) => Some(l.to_string()),
        OscType::Double(d) => Some(d.to_string()),
        _ => None,
    }
}

fn arg_to_f32(arg: &OscType) -> Option<f32> {
    match arg {
        OscType::Float(f) => Some(*f),
        OscType::Int(i) => Some(*i as f32),
        OscType::Double(d) => Some(*d as f32),
        OscType::Long(l) => Some(*l as f32),
        _ => None,
    }
}

fn arg_to_i32(arg: &OscType) -> Option<i32> {
    match arg {
        OscType::Int(i) => Some(*i),
        OscType::Long(l) => Some(*l as i32),
        OscType::Float(f) => Some(*f as i32),
        OscType::Double(d) => Some(*d as i32),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_router_exact_match() {
        let mut router = OscRouter::new();
        let received = Arc::new(Mutex::new(false));
        let r = Arc::clone(&received);
        router.subscribe("/qplayer/go", move |_msg| {
            *r.lock().unwrap() = true;
        });

        router.route(&OscMessage {
            addr: "/qplayer/go".into(),
            args: vec![],
        });

        assert!(*received.lock().unwrap());
    }

    #[test]
    fn test_router_wildcard() {
        let mut router = OscRouter::new();
        let received = Arc::new(Mutex::new(String::new()));
        let r = Arc::clone(&received);
        router.subscribe("/qplayer/?/go", move |msg| {
            *r.lock().unwrap() = msg.addr.clone();
        });

        router.route(&OscMessage {
            addr: "/qplayer/123/go".into(),
            args: vec![],
        });

        assert_eq!(*received.lock().unwrap(), "/qplayer/123/go");
    }

    #[test]
    fn test_broadcast_address() {
        let broadcast = make_broadcast(
            Ipv4Addr::new(192, 168, 1, 10),
            Ipv4Addr::new(255, 255, 255, 0),
        );
        assert_eq!(broadcast, Ipv4Addr::new(192, 168, 1, 255));
    }
}
