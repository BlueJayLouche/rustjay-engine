//! MA MIDI Show Control (MSC) over UDP.
//!
//! Replaces C# `MAMSCDriver` and `MAMSCPacket`.
//! MSC packets are wrapped in a proprietary UDP envelope with a header
//! `b"GMA\0MSC\0"` followed by a little-endian length and a MIDI SysEx
//! MSC message.

use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// Events emitted by the MSC manager.
#[derive(Debug, Clone)]
pub enum MscEvent {
    Go { qid: String, executor: Option<u8>, page: Option<u8> },
    TimedGo { qid: String, executor: Option<u8>, page: Option<u8>, time: MscTime },
    Stop { qid: Option<String>, executor: Option<u8>, page: Option<u8> },
    Resume { qid: Option<String>, executor: Option<u8>, page: Option<u8> },
    Set { fader: u8, page: u8, value: f32 },
    Fire { macro_num: u8 },
    GoOff { qid: String, executor: Option<u8>, page: Option<u8> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MscTime {
    pub hours: u8,
    pub minutes: u8,
    pub seconds: u8,
    pub frames: u8,
    pub fraction: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MscCommand {
    Unknown = 0,
    Go = 1,
    Stop = 2,
    Resume = 3,
    TimedGo = 4,
    Set = 6,
    Fire = 7,
    GoOff = 11,
}

impl From<u8> for MscCommand {
    fn from(v: u8) -> Self {
        match v {
            1 => MscCommand::Go,
            2 => MscCommand::Stop,
            3 => MscCommand::Resume,
            4 => MscCommand::TimedGo,
            6 => MscCommand::Set,
            7 => MscCommand::Fire,
            11 => MscCommand::GoOff,
            _ => MscCommand::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MscCommandFormat {
    Unknown = 0,
    GeneralLighting = 1,
    MovingLights = 2,
    All = 0x7f,
}

impl From<u8> for MscCommandFormat {
    fn from(v: u8) -> Self {
        match v {
            1 => MscCommandFormat::GeneralLighting,
            2 => MscCommandFormat::MovingLights,
            0x7f => MscCommandFormat::All,
            _ => MscCommandFormat::Unknown,
        }
    }
}

/// Parsed MA-MSC packet.
#[derive(Debug, Clone)]
pub struct MamscPacket {
    pub device_id: u8,
    pub command_format: MscCommandFormat,
    pub command: MscCommand,
    pub data: MscData,
}

#[derive(Debug, Clone)]
pub enum MscData {
    Go { qid: String, executor: Option<u8>, page: Option<u8> },
    Stop { qid: Option<String>, executor: Option<u8>, page: Option<u8> },
    Resume { qid: Option<String>, executor: Option<u8>, page: Option<u8> },
    TimedGo { qid: String, executor: Option<u8>, page: Option<u8>, time: MscTime },
    Set { fader: u8, page: u8, value: f32 },
    Fire { macro_num: u8 },
    GoOff { qid: String, executor: Option<u8>, page: Option<u8> },
    None,
}

const HEADER: &[u8] = b"GMA\0MSC\0";

impl MamscPacket {
    /// Attempt to parse an MA-MSC packet from raw bytes.
    pub fn try_read(buf: &[u8]) -> Option<Self> {
        if buf.len() < 12 {
            return None;
        }
        let mut pos = 0;

        // Check header
        if &buf[..HEADER.len()] != HEADER {
            return None;
        }
        pos += HEADER.len();

        let len = u32::from_le_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]) as usize;
        pos += 4;
        if pos + len > buf.len() {
            return None;
        }

        let buf = &buf[pos..pos + len];
        if buf.len() < 7 {
            return None;
        }

        // MIDI SysEx header
        if buf[0] != 0xf0 || buf[1] != 0x7f {
            return None;
        }
        let device_id = buf[2];
        if buf[3] != 0x02 {
            return None;
        }
        let command_format = MscCommandFormat::from(buf[4]);
        let command = MscCommand::from(buf[5]);

        if *buf.last()? != 0xf7 {
            return None;
        }
        let data_buf = &buf[6..buf.len() - 1];

        let data = parse_command_data(command, data_buf)?;

        Some(Self {
            device_id,
            command_format,
            command,
            data,
        })
    }
}

fn parse_command_data(cmd: MscCommand, buf: &[u8]) -> Option<MscData> {
    match cmd {
        MscCommand::Go => {
            let (qid, rest) = read_qid(buf)?;
            let (executor, page) = read_executor_page(rest);
            Some(MscData::Go { qid, executor, page })
        }
        MscCommand::Stop => {
            if buf.is_empty() {
                return Some(MscData::Stop { qid: None, executor: None, page: None });
            }
            let (qid, rest) = read_qid(buf)?;
            let (executor, page) = read_executor_page(rest);
            Some(MscData::Stop { qid: Some(qid), executor, page })
        }
        MscCommand::Resume => {
            if buf.is_empty() {
                return Some(MscData::Resume { qid: None, executor: None, page: None });
            }
            let (qid, rest) = read_qid(buf)?;
            let (executor, page) = read_executor_page(rest);
            Some(MscData::Resume { qid: Some(qid), executor, page })
        }
        MscCommand::TimedGo => {
            if buf.len() < 5 {
                return None;
            }
            let time = MscTime {
                hours: buf[0],
                minutes: buf[1],
                seconds: buf[2],
                frames: buf[3],
                fraction: buf[4],
            };
            let (qid, rest) = read_qid(&buf[5..])?;
            let (executor, page) = read_executor_page(rest);
            Some(MscData::TimedGo { qid, executor, page, time })
        }
        MscCommand::Set => {
            if buf.len() < 4 {
                return None;
            }
            let fader = buf[0];
            let page = buf[1];
            let low = buf[2] as u16;
            let high = buf[3] as u16;
            let raw = low | (high << 7);
            let value = raw as f32 / (128.0 * 128.0);
            Some(MscData::Set { fader, page, value })
        }
        MscCommand::Fire => {
            if buf.is_empty() {
                return None;
            }
            Some(MscData::Fire { macro_num: buf[0] })
        }
        MscCommand::GoOff => {
            let (qid, rest) = read_qid(buf)?;
            let (executor, page) = read_executor_page(rest);
            Some(MscData::GoOff { qid, executor, page })
        }
        _ => Some(MscData::None),
    }
}

fn read_qid(buf: &[u8]) -> Option<(String, &[u8])> {
    let nul = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let qid = std::str::from_utf8(&buf[..nul]).ok()?.to_string();
    let rest = if nul < buf.len() { &buf[nul + 1..] } else { &[] };
    Some((qid, rest))
}

fn read_executor_page(buf: &[u8]) -> (Option<u8>, Option<u8>) {
    if buf.is_empty() {
        return (None, None);
    }
    let sep = buf.iter().position(|&b| b == 0 || b == b'.').unwrap_or(buf.len());
    let executor = std::str::from_utf8(&buf[..sep])
        .ok()
        .and_then(|s| s.parse().ok());
    let page = if sep < buf.len() {
        std::str::from_utf8(&buf[sep + 1..])
            .ok()
            .and_then(|s| s.parse().ok())
    } else {
        None
    };
    (executor, page)
}

// ---------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------

/// Low-level UDP MSC driver.
#[allow(dead_code)]
pub struct MamscDriver {
    socket: Arc<UdpSocket>,
    tx_addr: std::net::SocketAddr,
    rx_thread: Option<JoinHandle<()>>,
    running: Arc<std::sync::atomic::AtomicBool>,
}

impl MamscDriver {
    pub fn bind(nic: Ipv4Addr, rx_port: u16, tx_port: u16, subnet: Ipv4Addr) -> anyhow::Result<Self> {
        let broadcast = make_broadcast(nic, subnet);
        let bind_addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, rx_port);
        let socket = UdpSocket::bind(bind_addr)?;
        let tx_addr: std::net::SocketAddr = SocketAddrV4::new(broadcast, tx_port).into();

        Ok(Self {
            socket: Arc::new(socket),
            tx_addr,
            rx_thread: None,
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }

    pub fn start<F>(&mut self, mut on_msg: F)
    where
        F: FnMut(MamscPacket, std::net::SocketAddr) + Send + 'static,
    {
        self.running.store(true, std::sync::atomic::Ordering::Relaxed);
        let socket = Arc::clone(&self.socket);
        let running = Arc::clone(&self.running);
        self.rx_thread = Some(std::thread::spawn(move || {
            let mut buf = [0u8; 65536];
            while running.load(std::sync::atomic::Ordering::Relaxed) {
                match socket.recv_from(&mut buf) {
                    Ok((len, src)) => {
                        if let Some(pkt) = MamscPacket::try_read(&buf[..len]) {
                            on_msg(pkt, src);
                        } else {
                            log::warn!("Malformed MA-MSC packet from {src}");
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    Err(e) => {
                        log::warn!("MSC recv error: {e}");
                    }
                }
            }
        }));
    }
}

impl Drop for MamscDriver {
    fn drop(&mut self) {
        self.running.store(false, std::sync::atomic::Ordering::Relaxed);
        if let Some(t) = self.rx_thread.take() {
            let _ = t.join();
        }
    }
}

/// High-level MSC manager with command filtering.
#[allow(dead_code)]
pub struct MscManager {
    driver: MamscDriver,
    subscribers: Arc<Mutex<Vec<(MscCommandFlags, Box<dyn Fn(&MamscPacket) + Send>)>>>,
    event_tx: std::sync::mpsc::Sender<MscEvent>,
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct MscCommandFlags: u16 {
        const NONE = 0;
        const GO = 1;
        const STOP = 1 << 2;
        const RESUME = 1 << 3;
        const TIMED_GO = 1 << 4;
        const SET = 1 << 5;
        const FIRE = 1 << 6;
        const GO_OFF = 1 << 7;
    }
}

impl MscManager {
    pub fn new(
        nic: Ipv4Addr,
        rx_port: u16,
        tx_port: u16,
        subnet: Ipv4Addr,
        event_tx: std::sync::mpsc::Sender<MscEvent>,
    ) -> anyhow::Result<Self> {
        let mut driver = MamscDriver::bind(nic, rx_port, tx_port, subnet)?;
        let subscribers: Arc<Mutex<Vec<(MscCommandFlags, Box<dyn Fn(&MamscPacket) + Send>)>>> =
            Arc::new(Mutex::new(Vec::new()));
        let subs = Arc::clone(&subscribers);
        driver.start(move |pkt, _src| {
            let flags = command_to_flags(pkt.command);
            if let Ok(lock) = subs.lock() {
                for (cmd_flags, handler) in lock.iter() {
                    if flags.intersects(*cmd_flags) {
                        handler(&pkt);
                    }
                }
            }
        });

        Ok(Self {
            driver,
            subscribers,
            event_tx,
        })
    }

    pub fn subscribe<F>(&self, commands: MscCommandFlags, handler: F)
    where
        F: Fn(&MamscPacket) + Send + 'static,
    {
        self.subscribers.lock().unwrap().push((commands, Box::new(handler)));
    }
}

fn command_to_flags(cmd: MscCommand) -> MscCommandFlags {
    match cmd {
        MscCommand::Go => MscCommandFlags::GO,
        MscCommand::Stop => MscCommandFlags::STOP,
        MscCommand::Resume => MscCommandFlags::RESUME,
        MscCommand::TimedGo => MscCommandFlags::TIMED_GO,
        MscCommand::Set => MscCommandFlags::SET,
        MscCommand::Fire => MscCommandFlags::FIRE,
        MscCommand::GoOff => MscCommandFlags::GO_OFF,
        _ => MscCommandFlags::NONE,
    }
}

// Re-export broadcast helper from parent
fn make_broadcast(adapter: Ipv4Addr, subnet: Ipv4Addr) -> Ipv4Addr {
    let a = u32::from_be_bytes(adapter.octets());
    let s = u32::from_be_bytes(subnet.octets());
    Ipv4Addr::from(a | !s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mamsc_packet_go() {
        // Build a synthetic Go packet
        let mut buf = Vec::new();
        buf.extend_from_slice(HEADER);
        let len_pos = buf.len();
        buf.extend_from_slice(&0u32.to_le_bytes()); // placeholder for length

        let data_start = buf.len();
        buf.push(0xf0);
        buf.push(0x7f);
        buf.push(0x01); // device id
        buf.push(0x02); // MSC sysex
        buf.push(0x01); // command format = GeneralLighting
        buf.push(0x01); // command = Go
        buf.extend_from_slice(b"1.5"); // qid
        buf.push(0x00); // null separator
        buf.push(b'2');
        buf.push(b'.');
        buf.push(b'3');
        buf.push(0xf7);

        let len = (buf.len() - data_start) as u32;
        buf[len_pos..len_pos + 4].copy_from_slice(&len.to_le_bytes());

        let pkt = MamscPacket::try_read(&buf).unwrap();
        assert_eq!(pkt.device_id, 1);
        assert_eq!(pkt.command, MscCommand::Go);
        assert!(matches!(pkt.data, MscData::Go { qid, executor, page } if qid == "1.5" && executor == Some(2) && page == Some(3)));
    }

    #[test]
    fn test_mamsc_packet_stop_blank() {
        let mut buf = Vec::new();
        buf.extend_from_slice(HEADER);
        let len_pos = buf.len();
        buf.extend_from_slice(&0u32.to_le_bytes());

        let data_start = buf.len();
        buf.push(0xf0);
        buf.push(0x7f);
        buf.push(0x01);
        buf.push(0x02);
        buf.push(0x01);
        buf.push(0x02); // Stop
        buf.push(0xf7);

        let len = (buf.len() - data_start) as u32;
        buf[len_pos..len_pos + 4].copy_from_slice(&len.to_le_bytes());

        let pkt = MamscPacket::try_read(&buf).unwrap();
        assert_eq!(pkt.command, MscCommand::Stop);
        assert!(matches!(pkt.data, MscData::Stop { qid: None, .. }));
    }
}
