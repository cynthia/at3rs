use crate::gha::GhaInfo;

pub fn analyze_frame(subbands: &[f32; 2048], tones: &[GhaInfo], bit_alloc: &mut [u8; 32]) {
    let mut energies = [0.0f32; 16];
    for b in 0..16 {
        let start = b * 128;
        let end = start + 128;
        energies[b] = subbands[start..end].iter().map(|&x| x * x).sum();
    }

    for b in 0..16 {
        let tone_energy: f32 = tones
            .iter()
            .filter(|t| is_tone_in_band(t, b))
            .map(|t| t.magnitude * t.magnitude)
            .sum();

        let ratio = tone_energy / (energies[b] + 1e-6);

        let tonality = if ratio > 3.05e-5 {
            ((0.5 - ratio) * 10.0).atan() * 0.36406 + 0.5
        } else {
            1.0
        };

        bit_alloc[b] = if tonality < 0.3 {
            8
        } else if tonality < 0.7 {
            4
        } else {
            0
        };
    }
}

fn is_tone_in_band(tone: &GhaInfo, band: usize) -> bool {
    let low = (band as f32) / 32.0;
    let high = ((band + 1) as f32) / 32.0;
    tone.frequency >= low && tone.frequency < high
}
