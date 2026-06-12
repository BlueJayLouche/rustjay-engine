//! DMX lighting output for rustjay — sACN (E1.31) and Art-Net over UDP.
//!
//! This crate is the **network spine** of the lighting subsystem (see
//! `LIGHTING_SACN.md`). It is pure CPU + networking: it knows nothing about
//! wgpu, surfaces, or sampling. Callers build a [`DmxFrame`] (universe → 512
//! channel bytes) and hand it to a [`DmxSender`], which paces transmission on a
//! background thread through a [`DmxTransport`] ([`SacnTransport`] or
//! [`ArtNetTransport`]).
//!
//! ```no_run
//! use rustjay_lighting::{DmxFrame, DmxSender, SacnTransport, Dest};
//!
//! let transport = SacnTransport::new(Dest::Multicast, 100, "vjarda").unwrap();
//! let sender = DmxSender::spawn(Box::new(transport), 44.0);
//!
//! let mut frame = DmxFrame::new();
//! let u = frame.universe_mut(1);
//! u[0] = 255; // fixture 1 red
//! sender.submit(frame);
//! ```

pub mod artnet;
pub mod color;
pub mod e131;
pub mod overlap;
pub mod scan;

mod dmx;
mod patch;
mod socket;
mod transport;
mod tx;

pub use color::{
    builtin_profiles, color_pipeline, ChannelRole, FixtureProfile, ProfileId, SegmentColor,
    WhiteMode,
};
pub use dmx::{DmxFrame, Universe, DMX_UNIVERSE_SIZE};
pub use overlap::{find_overlaps, segment_spans, Overlap, PatchSpan};
pub use patch::pack_fixtures;
pub use scan::{demux_tile, Axis, Corner, ScanOrder};
pub use socket::{rx_socket, tx_socket};
pub use transport::{ArtNetTransport, Dest, DmxTransport, SacnTransport};
pub use tx::DmxSender;
