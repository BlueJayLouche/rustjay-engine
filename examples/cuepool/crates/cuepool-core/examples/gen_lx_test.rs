//! One-shot generator for testFiles/lightingTest.qproj (see QPLAYER lighting L1).

use cuepool_core::lighting::{
    FixtureLook, LightingProtocol, PatchedFixture, PixelMapSegment, SegmentSource,
};
use cuepool_core::{Cue, CueBase, FadeType, LoopMode, ShowFile};
use rust_decimal::Decimal;
use std::collections::BTreeMap;

fn main() {
    let mut show = ShowFile::default();
    show.show_settings.title = "Lighting Test".into();

    show.lighting.enabled = true;
    show.lighting.protocol = LightingProtocol::Sacn;
    show.lighting.dest_ip = "127.0.0.1".into();
    show.lighting.fps = 30.0;
    show.lighting.fixtures = vec![
        PatchedFixture {
            id: 1,
            name: "Head".into(),
            profile_id: "moving_head_16bit".into(),
            universe: 1,
            address: 1,
        },
        PatchedFixture {
            id: 2,
            name: "Dim".into(),
            profile_id: "dimmer".into(),
            universe: 1,
            address: 10,
        },
    ];

    // Q1: snap — red at full, pan 0.25.
    let mut snap1 = BTreeMap::new();
    snap1.insert(
        1u32,
        FixtureLook { dimmer: 1.0, color: [1.0, 0.0, 0.0], pan: 0.25, ..Default::default() },
    );
    snap1.insert(2u32, FixtureLook { dimmer: 0.75, ..Default::default() });

    // Q2: 2s fade — blue, dim out, pan 0.75.
    let mut snap2 = BTreeMap::new();
    snap2.insert(
        1u32,
        FixtureLook { dimmer: 0.0, color: [0.0, 0.0, 1.0], pan: 0.75, ..Default::default() },
    );
    snap2.insert(2u32, FixtureLook { dimmer: 0.25, ..Default::default() });

    // Pixel-map segment: full pixmap texture → 4×1 RGB grid on universe 2.
    show.lighting.segments = vec![PixelMapSegment {
        cols: 4,
        universe: 2,
        source: SegmentSource::PixelMap,
        ..PixelMapSegment::new(1)
    }];

    show.cues = vec![
        Cue::Lighting {
            base: CueBase { qid: Decimal::from(1), name: "Red look (snap)".into(), ..Default::default() },
            snapshot: snap1,
            fade_time: 0.0,
            fade_type: FadeType::Linear,
        },
        Cue::Lighting {
            base: CueBase { qid: Decimal::from(2), name: "Blue look (2s fade)".into(), ..Default::default() },
            snapshot: snap2,
            fade_time: 2.0,
            fade_type: FadeType::Linear,
        },
        // Q3: still image into the pixel-map texture (RGBW vertical stripes).
        Cue::PixelMap {
            base: CueBase { qid: Decimal::from(3), name: "Stripes still".into(), ..Default::default() },
            path: "/Users/ac/developer/rust/rustjay-engine/testFiles/lx_stripes.png".into(),
        },
        // Q4: looping stripes video into the pixel-map texture.
        Cue::PixelMap {
            base: CueBase {
                qid: Decimal::from(4),
                name: "Stripes video".into(),
                loop_mode: LoopMode::LoopedInfinite,
                ..Default::default()
            },
            path: "/Users/ac/developer/rust/rustjay-engine/testFiles/lx_stripes.mp4".into(),
        },
    ];

    let path = std::env::args().nth(1).expect("usage: gen_lx_test <out.qproj>");
    std::fs::write(&path, serde_json::to_string_pretty(&show).unwrap()).unwrap();
    println!("wrote {path}");
}
