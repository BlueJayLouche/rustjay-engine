/// A decoded video frame ready for GPU upload.
#[derive(Debug, Clone)]
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>, // RGBA8
    pub pts: f64,      // seconds
}

impl VideoFrame {
    pub fn new(width: u32, height: u32, data: Vec<u8>, pts: f64) -> Self {
        debug_assert_eq!(data.len(), (width * height * 4) as usize);
        Self {
            width,
            height,
            data,
            pts,
        }
    }
}
