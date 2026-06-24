//! MIDI input driver.
//!
//! ponytail: opens the first available MIDI input port and forwards raw
//! voice messages as `MidiEvent`s. SysEx / clock are ignored for now.

use std::sync::{mpsc, Arc, Mutex};

/// Events emitted by the MIDI manager that the application should handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MidiEvent {
    NoteOn { channel: u8, note: u8, velocity: u8 },
    NoteOff { channel: u8, note: u8, velocity: u8 },
    CC { channel: u8, cc: u8, value: u8 },
}

/// Minimal MIDI input manager.
pub struct MidiManager {
    #[allow(dead_code)]
    input: Arc<Mutex<midir::MidiInputConnection<()>>>,
    event_rx: mpsc::Receiver<MidiEvent>,
}

impl MidiManager {
    /// Try to open the first available MIDI input port.
    pub fn new() -> anyhow::Result<Self> {
        let midi_in = midir::MidiInput::new("QPlayer MIDI")?;
        let ports = midi_in.ports();
        let port = ports.first().ok_or_else(|| anyhow::anyhow!("no MIDI input ports"))?;
        let port_name = midi_in.port_name(port).unwrap_or_default();
        log::info!("Opening MIDI input port: {}", port_name);

        let (event_tx, event_rx) = mpsc::channel();
        let input = midi_in.connect(
            port,
            "qplayer-midi-in",
            move |_stamp, message, _| {
                if let Some(ev) = parse_midi(message) {
                    let _ = event_tx.send(ev);
                }
            },
            (),
        )?;

        Ok(Self {
            input: Arc::new(Mutex::new(input)),
            event_rx,
        })
    }

    /// Drain pending MIDI events.
    pub fn try_recv(&self) -> Option<MidiEvent> {
        self.event_rx.try_recv().ok()
    }
}

fn parse_midi(message: &[u8]) -> Option<MidiEvent> {
    if message.is_empty() {
        return None;
    }
    let status = message[0];
    let channel = (status & 0x0F) + 1; // 1-based channel to match UI convention
    match status & 0xF0 {
        0x80 if message.len() >= 2 => Some(MidiEvent::NoteOff {
            channel,
            note: message[1],
            velocity: message.get(2).copied().unwrap_or(0),
        }),
        0x90 if message.len() >= 2 => {
            let velocity = message.get(2).copied().unwrap_or(0);
            if velocity == 0 {
                Some(MidiEvent::NoteOff {
                    channel,
                    note: message[1],
                    velocity: 0,
                })
            } else {
                Some(MidiEvent::NoteOn {
                    channel,
                    note: message[1],
                    velocity,
                })
            }
        }
        0xB0 if message.len() >= 2 => Some(MidiEvent::CC {
            channel,
            cc: message[1],
            value: message.get(2).copied().unwrap_or(0),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_note_on() {
        assert_eq!(
            parse_midi(&[0x91, 60, 100]),
            Some(MidiEvent::NoteOn {
                channel: 2,
                note: 60,
                velocity: 100,
            })
        );
    }

    #[test]
    fn test_parse_note_on_zero_velocity_is_note_off() {
        assert_eq!(
            parse_midi(&[0x90, 60, 0]),
            Some(MidiEvent::NoteOff {
                channel: 1,
                note: 60,
                velocity: 0,
            })
        );
    }

    #[test]
    fn test_parse_cc() {
        assert_eq!(
            parse_midi(&[0xB0, 7, 127]),
            Some(MidiEvent::CC {
                channel: 1,
                cc: 7,
                value: 127,
            })
        );
    }
}
