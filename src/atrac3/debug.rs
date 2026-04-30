#[derive(Debug, Clone)]
pub struct DebugBfu {
    pub block: usize,
    pub start: usize,
    pub end: usize,
    pub table_idx: usize,
    pub sf_idx: usize,
    pub max_val: f32,
    pub importance: f32,
    pub bit_count: usize,
    pub energy: f32,
    pub recon_energy: f32,
    pub distortion: f32,
}

#[derive(Debug, Clone)]
pub struct DebugChannelPlan {
    pub channel: usize,
    pub active_blocks: usize,
    pub total_bits: usize,
    pub blocks: Vec<DebugBfu>,
}

#[derive(Debug, Clone)]
pub struct DebugBandMetrics {
    pub band: usize,
    pub subband_rms: f32,
    pub subband_max: f32,
    pub mdct_rms: f32,
    pub mdct_max: f32,
}

#[derive(Debug, Clone)]
pub struct DebugChannelAnalysis {
    pub channel: usize,
    pub pcm_rms: f32,
    pub pcm_max: f32,
    pub bands: Vec<DebugBandMetrics>,
}

#[derive(Debug, Clone)]
pub struct DebugFrameAnalysis {
    pub channels: Vec<DebugChannelAnalysis>,
    pub plans: Vec<DebugChannelPlan>,
}
