//! Streaming a point list to a laser DAC, over `laser-dac`.
//!
//! Gated behind the `dac` feature: talking to hardware pulls in libusb and
//! CMake, and everything up to the point list works without it.
//!
//! `laser-dac` owns the transport and the between-frame work — reconnection,
//! startup blanking, colour-delay compensation, and what to send when we
//! underrun. What arrives here has already been through [`crate::Optimiser`]
//! and [`crate::Safety`], so this module's only jobs are converting units and
//! keeping the session alive.
//!
//! **Nothing here has been run against a laser projector.** The IDN and
//! Ether Dream paths are network protocols and can be watched on the wire;
//! Helios and LaserCube need the hardware in hand.

use laser_dac::{Dac, Frame, FrameSession, FrameSessionConfig, LaserPoint as DacPoint};

/// What a discovered device is, re-exported so a host can name one without
/// depending on `laser-dac` itself.
pub use laser_dac::{DacInfo, DacType};

use crate::frame::LaserFrame;

/// Full scale of a DAC colour channel.
const FULL: f32 = u16::MAX as f32;

/// DACs currently visible, across every enabled transport.
///
/// USB devices appear as soon as they are plugged in; network ones — IDN,
/// Ether Dream, LaserCube — answer a discovery broadcast, so an empty list
/// shortly after start-up may just mean nothing has replied yet.
pub fn list() -> Vec<DacInfo> {
    laser_dac::list_devices().unwrap_or_else(|e| {
        log::warn!("[laser] could not list DACs: {e}");
        Vec::new()
    })
}

/// An open connection to one DAC.
///
/// Dropping it stops the stream, which leaves the projector dark — that is the
/// intended behaviour on quit or on any error path that unwinds.
pub struct DacOutput {
    session: FrameSession,
    info: DacInfo,
    points_per_second: u32,
}

impl DacOutput {
    /// Open the DAC with this id and start a frame session at `points_per_second`.
    ///
    /// The session starts **disarmed**: `laser-dac` will hold it dark until
    /// [`DacOutput::arm`], which mirrors [`crate::Safety`] rather than
    /// replacing it. Both have to agree before light leaves the projector.
    pub fn open(id: &str, points_per_second: u32) -> anyhow::Result<Self> {
        let dac: Dac = laser_dac::open_device(id)?;
        let (session, info) = dac.start_frame_session(FrameSessionConfig::new(points_per_second))?;
        Ok(Self { session, info, points_per_second })
    }

    pub fn info(&self) -> &DacInfo {
        &self.info
    }

    pub fn points_per_second(&self) -> u32 {
        self.points_per_second
    }

    /// Allow the DAC to emit. Fails if the device has gone away.
    pub fn arm(&self) -> anyhow::Result<()> {
        self.session.control().arm()?;
        Ok(())
    }

    /// Stop it emitting, now.
    pub fn disarm(&self) -> anyhow::Result<()> {
        self.session.control().disarm()?;
        Ok(())
    }

    pub fn is_armed(&self) -> bool {
        self.session.control().is_armed()
    }

    /// Whether the transport still has the device.
    pub fn connected(&self) -> bool {
        self.session.metrics().connected()
    }

    /// Send a frame, which the DAC then replays until the next one arrives.
    ///
    /// Replay is why a dropped frame is survivable — the beam keeps drawing the
    /// last good path rather than going dark — and also why the frame handed
    /// over must already be safe to leave running.
    pub fn send(&self, frame: &LaserFrame) {
        self.session.send_frame(Frame::new(to_dac_points(frame)));
    }

    /// Send a single dark point, parking the beam without stopping the session.
    pub fn blank(&self) {
        self.session
            .send_frame(Frame::new(vec![DacPoint::new(0.0, 0.0, 0, 0, 0, 0)]));
    }
}

/// Convert to the DAC's units: coordinates stay in -1..1, colours become 16-bit.
///
/// Intensity is set to the brightest channel rather than carried separately.
/// A laser material has no concept of it — the shader's colour is the whole
/// story — and projectors that use the intensity line expect it to track the
/// colour rather than sit at full scale while the beam is meant to be dark.
fn to_dac_points(frame: &LaserFrame) -> Vec<DacPoint> {
    frame
        .points
        .iter()
        .map(|p| {
            let (r, g, b) = (channel(p.r), channel(p.g), channel(p.b));
            DacPoint::new(p.x, p.y, r, g, b, r.max(g).max(b))
        })
        .collect()
}

fn channel(v: f32) -> u16 {
    (v.clamp(0.0, 1.0) * FULL).round() as u16
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::LaserPoint;

    #[test]
    fn colours_become_full_scale_sixteen_bit() {
        let frame = LaserFrame {
            points: vec![LaserPoint { x: 0.0, y: 0.0, r: 1.0, g: 0.5, b: 0.0, shape: 0 }],
        };

        let points = to_dac_points(&frame);

        assert_eq!(points[0].r, u16::MAX);
        assert_eq!(points[0].g, 32768);
        assert_eq!(points[0].b, 0);
    }

    // A blanked point must carry no intensity either, or the projector lights
    // up along every jump the optimiser inserted.
    #[test]
    fn a_blanked_point_has_no_intensity() {
        let frame = LaserFrame { points: vec![LaserPoint::blanked(0.5, -0.5, 0)] };

        let points = to_dac_points(&frame);

        assert_eq!((points[0].r, points[0].g, points[0].b), (0, 0, 0));
        assert_eq!(points[0].intensity, 0);
    }

    #[test]
    fn intensity_follows_the_brightest_channel() {
        let frame = LaserFrame {
            points: vec![LaserPoint { x: 0.0, y: 0.0, r: 0.0, g: 0.25, b: 0.0, shape: 0 }],
        };

        let points = to_dac_points(&frame);

        assert_eq!(points[0].intensity, points[0].g);
    }

    #[test]
    fn coordinates_pass_through_unchanged() {
        let frame = LaserFrame {
            points: vec![LaserPoint { x: -1.0, y: 1.0, r: 1.0, g: 1.0, b: 1.0, shape: 0 }],
        };

        let points = to_dac_points(&frame);

        assert_eq!((points[0].x, points[0].y), (-1.0, 1.0));
    }

    // Discovery with no hardware present must return an empty list rather than
    // failing — this is the state every developer machine is in.
    #[test]
    fn listing_dacs_with_none_attached_is_not_an_error() {
        let _ = list();
    }
}
