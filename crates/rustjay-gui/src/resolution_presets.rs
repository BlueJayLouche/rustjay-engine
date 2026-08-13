pub(crate) const RESOLUTION_PRESETS: [(&str, u32, u32); 10] = [
    ("Custom", 0, 0),
    ("NTSC (720x480)", 720, 480),
    ("PAL (720x576)", 720, 576),
    ("480p (640x480)", 640, 480),
    ("720p (1280x720)", 1280, 720),
    ("1080p (1920x1080)", 1920, 1080),
    ("1440p (2560x1440)", 2560, 1440),
    ("4K UHD (3840x2160)", 3840, 2160),
    ("Square 1:1 (1080x1080)", 1080, 1080),
    ("Vertical 9:16 (1080x1920)", 1080, 1920),
];

pub(crate) fn preset_dimensions(index: usize) -> Option<(u32, u32)> {
    RESOLUTION_PRESETS
        .get(index)
        .and_then(|(_, width, height)| (*width != 0 && *height != 0).then_some((*width, *height)))
}

pub(crate) fn detect_resolution_preset(width: u32, height: u32) -> usize {
    RESOLUTION_PRESETS
        .iter()
        .position(|(_, preset_width, preset_height)| {
            *preset_width == width && *preset_height == height
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_map_to_dimensions_and_back() {
        assert_eq!(preset_dimensions(0), None);

        for index in 1..RESOLUTION_PRESETS.len() {
            let (width, height) = preset_dimensions(index).expect("named preset has dimensions");
            assert_eq!(detect_resolution_preset(width, height), index);
        }
    }

    #[test]
    fn unmatched_dimensions_detect_as_custom() {
        assert_eq!(detect_resolution_preset(1234, 567), 0);
    }
}
