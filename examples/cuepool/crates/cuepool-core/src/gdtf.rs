//! Minimal GDTF (General Device Type Format) import.
//!
//! A `.gdtf` file is a ZIP archive whose `description.xml` describes a
//! fixture type with one or more DMX modes. This module extracts just what a
//! CuePool [`FixtureProfile`](crate::lighting::FixtureProfile) needs: per
//! mode, the DMX channel layout with each channel's GDTF attribute mapped to
//! a [`ChannelRole`]. Unmapped channels become `Static(default)` using the
//! channel's GDTF default value, so mode/shutter/speed channels hold a sane
//! byte instead of 0 (which can blank a fixture).
//!
//! Deliberately not handled (import the fixture into a real console if you
//! need them): geometry-referenced channel repetition, DMX breaks other than
//! the first, mode-master switching, sub-attributes beyond the coarse/fine
//! byte pair. The ZIP reader is hand-rolled on `flate2` (stored + deflate
//! entries only — what every GDTF builder emits).

use crate::lighting::ChannelRole;

#[derive(Debug, Clone)]
pub struct GdtfFixture {
    pub name: String,
    pub manufacturer: String,
    pub modes: Vec<GdtfMode>,
}

#[derive(Debug, Clone)]
pub struct GdtfMode {
    pub name: String,
    /// Channel layout in DMX order (index 0 = channel 1).
    pub channels: Vec<ChannelRole>,
}

/// Parse a `.gdtf` file into its fixture description.
pub fn parse_gdtf(path: &std::path::Path) -> Result<GdtfFixture, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read failed: {e}"))?;
    let xml = zip_extract(&bytes, "description.xml")
        .ok_or_else(|| "no description.xml in archive (not a GDTF file?)".to_string())?;
    let xml = String::from_utf8_lossy(&xml).into_owned();
    parse_description(&xml)
}

fn parse_description(xml: &str) -> Result<GdtfFixture, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("bad XML: {e}"))?;
    let fixture_type = doc
        .descendants()
        .find(|n| n.has_tag_name("FixtureType"))
        .ok_or("no FixtureType element")?;
    let name = fixture_type.attribute("Name").unwrap_or("GDTF fixture").to_string();
    let manufacturer = fixture_type.attribute("Manufacturer").unwrap_or("").to_string();

    let mut modes = Vec::new();
    for mode in fixture_type.descendants().filter(|n| n.has_tag_name("DMXMode")) {
        let mode_name = mode.attribute("Name").unwrap_or("Mode").to_string();
        // (offset 0-based → role); size determined by the highest offset seen.
        let mut slots: Vec<Option<ChannelRole>> = Vec::new();
        let mut set = |idx: usize, role: ChannelRole| {
            if slots.len() <= idx {
                slots.resize(idx + 1, None);
            }
            slots[idx] = Some(role);
        };

        for ch in mode.descendants().filter(|n| n.has_tag_name("DMXChannel")) {
            // Only the first DMX break; virtual channels have Offset="None".
            let brk = ch.attribute("DMXBreak").unwrap_or("1");
            if brk != "1" && brk != "Overwrite" {
                continue;
            }
            let offsets: Vec<usize> = ch
                .attribute("Offset")
                .unwrap_or("None")
                .split(',')
                .filter_map(|s| s.trim().parse::<usize>().ok())
                .collect();
            let Some(&coarse) = offsets.first() else { continue };
            if coarse == 0 {
                continue;
            }

            let logical = ch.children().find(|n| n.has_tag_name("LogicalChannel"));
            let attribute = logical
                .and_then(|l| l.attribute("Attribute"))
                .unwrap_or("NoFeature");
            let default = channel_default(&ch, offsets.len());

            let (role, fine) = map_attribute(attribute, default);
            set(coarse - 1, role);
            if let Some(&f) = offsets.get(1)
                && f > 0 {
                    set(f - 1, fine);
                }
        }

        // Gaps in the declared layout hold 0.
        let channels: Vec<ChannelRole> =
            slots.into_iter().map(|s| s.unwrap_or(ChannelRole::Static(0))).collect();
        if !channels.is_empty() {
            modes.push(GdtfMode { name: mode_name, channels });
        }
    }
    if modes.is_empty() {
        return Err("no usable DMX modes found".into());
    }
    Ok(GdtfFixture { name, manufacturer, modes })
}

/// GDTF attribute name → (coarse role, fine-byte role). Unknown attributes
/// hold their GDTF default so the fixture behaves (shutter open, mode set).
fn map_attribute(attribute: &str, default: u8) -> (ChannelRole, ChannelRole) {
    use ChannelRole::*;
    let a = attribute;
    let simple = |r: ChannelRole| (r, Static(0));
    match a {
        "Dimmer" => simple(Dimmer),
        "Pan" => (Pan, PanFine),
        "Tilt" => (Tilt, TiltFine),
        "Zoom" => simple(Zoom),
        "ColorAdd_R" | "ColorRGB_Red" => simple(Red),
        "ColorAdd_G" | "ColorRGB_Green" => simple(Green),
        "ColorAdd_B" | "ColorRGB_Blue" => simple(Blue),
        "ColorAdd_W" | "ColorRGB_White" => simple(White),
        "ColorAdd_WW" | "ColorAdd_CW" => simple(White),
        "ColorAdd_A" | "ColorRGB_Amber" => simple(Amber),
        "ColorAdd_UV" | "ColorRGB_UV" => simple(Uv),
        _ if a.starts_with("Shutter") && a.contains("Strobe") => simple(Strobe),
        "StrobeModeShutter" | "StrobeFrequency" => simple(Strobe),
        _ if a.starts_with("Gobo") => simple(Gobo),
        _ => (Static(default), Static(default)),
    }
}

/// The channel's default DMX byte: `Default`/`InitialFunction` values are
/// written as `value/bytecount` (e.g. `32768/2`); scale to one byte.
fn channel_default(ch: &roxmltree::Node, _offsets: usize) -> u8 {
    let raw = ch
        .descendants()
        .filter(|n| n.has_tag_name("ChannelFunction"))
        .find_map(|f| f.attribute("Default"))
        .unwrap_or("0/1");
    let mut parts = raw.split('/');
    let value: u64 = parts.next().and_then(|v| v.trim().parse().ok()).unwrap_or(0);
    let bytes: u32 = parts.next().and_then(|b| b.trim().parse().ok()).unwrap_or(1);
    (value >> (8 * bytes.saturating_sub(1))).min(255) as u8
}

// ─── Mini ZIP reader (stored + deflate) ─────────────────────────────────────

/// Extract one file from a ZIP archive by walking the central directory.
// ponytail: hand-rolled reader instead of the `zip` crate — GDTF archives are
// vanilla stored/deflate zips; swap in the crate if exotic archives appear.
fn zip_extract(data: &[u8], want: &str) -> Option<Vec<u8>> {
    // End Of Central Directory: scan backwards for PK\x05\x06.
    let eocd = data
        .windows(4)
        .rposition(|w| w == [0x50, 0x4b, 0x05, 0x06])?;
    let cd_offset = u32::from_le_bytes(data.get(eocd + 16..eocd + 20)?.try_into().ok()?) as usize;
    let entries = u16::from_le_bytes(data.get(eocd + 10..eocd + 12)?.try_into().ok()?) as usize;

    let mut p = cd_offset;
    for _ in 0..entries {
        if data.get(p..p + 4)? != [0x50, 0x4b, 0x01, 0x02] {
            return None;
        }
        let method = u16::from_le_bytes(data.get(p + 10..p + 12)?.try_into().ok()?);
        let csize = u32::from_le_bytes(data.get(p + 20..p + 24)?.try_into().ok()?) as usize;
        let name_len = u16::from_le_bytes(data.get(p + 28..p + 30)?.try_into().ok()?) as usize;
        let extra_len = u16::from_le_bytes(data.get(p + 30..p + 32)?.try_into().ok()?) as usize;
        let comment_len = u16::from_le_bytes(data.get(p + 32..p + 34)?.try_into().ok()?) as usize;
        let local_off = u32::from_le_bytes(data.get(p + 42..p + 46)?.try_into().ok()?) as usize;
        let name = std::str::from_utf8(data.get(p + 46..p + 46 + name_len)?).ok()?;

        if name == want {
            // Local header: sizes may be in the data descriptor, so trust the
            // central directory's csize; skip the local name/extra fields.
            if data.get(local_off..local_off + 4)? != [0x50, 0x4b, 0x03, 0x04] {
                return None;
            }
            let lname = u16::from_le_bytes(data.get(local_off + 26..local_off + 28)?.try_into().ok()?) as usize;
            let lextra = u16::from_le_bytes(data.get(local_off + 28..local_off + 30)?.try_into().ok()?) as usize;
            let start = local_off + 30 + lname + lextra;
            let raw = data.get(start..start + csize)?;
            return match method {
                0 => Some(raw.to_vec()),
                8 => {
                    use std::io::Read;
                    let mut out = Vec::new();
                    flate2::read::DeflateDecoder::new(raw).read_to_end(&mut out).ok()?;
                    Some(out)
                }
                _ => None, // exotic compression — not seen in GDTF builders
            };
        }
        p += 46 + name_len + extra_len + comment_len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use ChannelRole::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<GDTF DataVersion="1.2">
  <FixtureType Name="Test Spot" Manufacturer="ACME">
    <DMXModes>
      <DMXMode Name="Standard" Geometry="Base">
        <DMXChannels>
          <DMXChannel DMXBreak="1" Offset="1,2" Geometry="Yoke">
            <LogicalChannel Attribute="Pan"><ChannelFunction Attribute="Pan" Default="32768/2"/></LogicalChannel>
          </DMXChannel>
          <DMXChannel DMXBreak="1" Offset="3" Geometry="Head">
            <LogicalChannel Attribute="Dimmer"><ChannelFunction Attribute="Dimmer" Default="0/1"/></LogicalChannel>
          </DMXChannel>
          <DMXChannel DMXBreak="1" Offset="4">
            <LogicalChannel Attribute="ColorAdd_R"><ChannelFunction Attribute="ColorAdd_R" Default="0/1"/></LogicalChannel>
          </DMXChannel>
          <DMXChannel DMXBreak="1" Offset="5">
            <LogicalChannel Attribute="Control1"><ChannelFunction Attribute="Control1" Default="42/1"/></LogicalChannel>
          </DMXChannel>
          <DMXChannel DMXBreak="1" Offset="None">
            <LogicalChannel Attribute="Virtual"><ChannelFunction Attribute="Virtual"/></LogicalChannel>
          </DMXChannel>
          <DMXChannel DMXBreak="1" Offset="6">
            <LogicalChannel Attribute="Shutter1Strobe"><ChannelFunction Attribute="Shutter1Strobe" Default="0/1"/></LogicalChannel>
          </DMXChannel>
        </DMXChannels>
      </DMXMode>
      <DMXMode Name="Basic" Geometry="Base">
        <DMXChannels>
          <DMXChannel DMXBreak="1" Offset="1">
            <LogicalChannel Attribute="Dimmer"><ChannelFunction Attribute="Dimmer"/></LogicalChannel>
          </DMXChannel>
        </DMXChannels>
      </DMXMode>
    </DMXModes>
  </FixtureType>
</GDTF>"#;

    #[test]
    fn parses_modes_and_maps_attributes() {
        let f = parse_description(SAMPLE).unwrap();
        assert_eq!(f.name, "Test Spot");
        assert_eq!(f.manufacturer, "ACME");
        assert_eq!(f.modes.len(), 2);

        let std_mode = &f.modes[0];
        assert_eq!(std_mode.name, "Standard");
        assert_eq!(
            std_mode.channels,
            vec![
                Pan,
                PanFine,       // 16-bit pair from Offset="1,2"
                Dimmer,
                Red,
                Static(42),    // unknown Control1 holds its GDTF default
                Strobe,
            ],
        );
        assert_eq!(f.modes[1].channels, vec![Dimmer]);
    }

    #[test]
    fn sixteen_bit_default_takes_high_byte() {
        // Default="32768/2" → high byte 128 (would apply if Pan were unmapped).
        let doc = roxmltree::Document::parse(
            r#"<DMXChannel><LogicalChannel Attribute="X"><ChannelFunction Attribute="X" Default="32768/2"/></LogicalChannel></DMXChannel>"#,
        )
        .unwrap();
        assert_eq!(channel_default(&doc.root_element(), 2), 128);
    }

    #[test]
    fn zip_roundtrip_stored_and_deflate() {
        use flate2::write::DeflateEncoder;
        use std::io::Write;

        // Build a minimal zip by hand: one deflate entry named description.xml.
        let content = b"<GDTF/>";
        let mut compressed = Vec::new();
        DeflateEncoder::new(&mut compressed, flate2::Compression::default())
            .write_all(content)
            .unwrap();

        let name = b"description.xml";
        let mut z: Vec<u8> = Vec::new();
        // Local file header.
        z.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04, 20, 0, 0, 0, 8, 0, 0, 0, 0, 0]);
        z.extend_from_slice(&[0; 4]); // crc (unchecked by our reader)
        z.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        z.extend_from_slice(&(content.len() as u32).to_le_bytes());
        z.extend_from_slice(&(name.len() as u16).to_le_bytes());
        z.extend_from_slice(&0u16.to_le_bytes());
        z.extend_from_slice(name);
        z.extend_from_slice(&compressed);
        // Central directory.
        let cd_start = z.len();
        z.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02, 20, 0, 20, 0, 0, 0, 8, 0, 0, 0, 0, 0]);
        z.extend_from_slice(&[0; 4]);
        z.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        z.extend_from_slice(&(content.len() as u32).to_le_bytes());
        z.extend_from_slice(&(name.len() as u16).to_le_bytes());
        z.extend_from_slice(&[0; 12]); // extra/comment/disk/attrs
        z.extend_from_slice(&0u32.to_le_bytes()); // local header offset
        z.extend_from_slice(name);
        let cd_len = z.len() - cd_start;
        // EOCD.
        z.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0, 1, 0, 1, 0]);
        z.extend_from_slice(&(cd_len as u32).to_le_bytes());
        z.extend_from_slice(&(cd_start as u32).to_le_bytes());
        z.extend_from_slice(&0u16.to_le_bytes());

        assert_eq!(zip_extract(&z, "description.xml").as_deref(), Some(&content[..]));
        assert_eq!(zip_extract(&z, "missing.xml"), None);
    }
}
