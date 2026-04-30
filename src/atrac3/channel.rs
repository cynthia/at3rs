pub struct Atrac3ChannelUnit {
    pub delay_buf1: [f32; 46],
    pub delay_buf2: [f32; 46],
    pub delay_buf3: [f32; 46],
    pub an_delay_buf1: [f32; 46],
    pub an_delay_buf2: [f32; 46],
    pub an_delay_buf3: [f32; 46],
    pub mdct_buf: [[f32; 256]; 4],
    pub quantized_spectra: [[f32; 256]; 4],
    pub imdct_overlap: [[f32; 256]; 4],
}

impl Atrac3ChannelUnit {
    pub fn new() -> Self {
        Self {
            delay_buf1: [0.0; 46],
            delay_buf2: [0.0; 46],
            delay_buf3: [0.0; 46],
            an_delay_buf1: [0.0; 46],
            an_delay_buf2: [0.0; 46],
            an_delay_buf3: [0.0; 46],
            mdct_buf: [[0.0; 256]; 4],
            quantized_spectra: [[0.0; 256]; 4],
            imdct_overlap: [[0.0; 256]; 4],
        }
    }
}

impl Default for Atrac3ChannelUnit {
    fn default() -> Self {
        Self::new()
    }
}
