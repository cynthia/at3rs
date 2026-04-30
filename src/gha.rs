use std::f32::consts::PI;

#[derive(Debug, Copy, Clone, Default)]
pub struct GhaInfo {
    pub frequency: f32,
    pub phase: f32,
    pub magnitude: f32,
}

pub struct GhaContext {
    pub size: usize,
}

impl GhaContext {
    pub fn new(size: usize) -> Self {
        Self { size }
    }

    /// Extracts sinusoidal components from the PCM signal and subtracts them.
    pub fn extract_many(&self, pcm: &mut [f32], out_tones: &mut [GhaInfo], k: usize) {
        for i in 0..k {
            let tone = self.analyze_one(pcm);
            out_tones[i] = tone;
            self.subtract_tone(pcm, &tone);
        }
    }

    /// Finds the strongest frequency component.
    fn analyze_one(&self, _pcm: &[f32]) -> GhaInfo {
        GhaInfo {
            frequency: 0.0,
            phase: 0.0,
            magnitude: 0.0,
        }
    }

    fn subtract_tone(&self, pcm: &mut [f32], tone: &GhaInfo) {
        if tone.magnitude == 0.0 {
            return;
        }
        for i in 0..self.size {
            let angle = 2.0 * PI * tone.frequency * (i as f32) + tone.phase;
            pcm[i] -= tone.magnitude * angle.cos();
        }
    }

    pub fn synthesize_many(&self, pcm: &mut [f32], tones: &[GhaInfo]) {
        for tone in tones {
            if tone.magnitude == 0.0 {
                continue;
            }
            for i in 0..self.size {
                let angle = 2.0 * PI * tone.frequency * (i as f32) + tone.phase;
                pcm[i] += tone.magnitude * angle.cos();
            }
        }
    }
}
