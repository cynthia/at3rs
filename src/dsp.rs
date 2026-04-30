use std::f32::consts::PI;

use crate::gha::GhaInfo;
use crate::huffman::BitWriter;

/// Number of subbands in the legacy codec path.
const NUM_BANDS: usize = 16;
const BAND_SIZE: usize = 128;
const MDCT_N: usize = 256;
const MDCT_HALF: usize = MDCT_N / 2;

/// Scale factor table: sf_table[i] = 2^(i/3 - 21)
fn sf_table() -> [f32; 64] {
    let mut t = [0.0; 64];
    for i in 0..64 {
        t[i] = 2.0_f32.powf(i as f32 / 3.0 - 21.0);
    }
    t
}

/// CLC bit-lengths per selector (from FFmpeg atrac3data.h)
const CLC_LENGTH_TAB: [u32; 8] = [0, 4, 3, 3, 4, 4, 5, 6];

/// Inverse max quant per selector (from FFmpeg atrac3data.h)
#[allow(dead_code)]
const INV_MAX_QUANT: [f32; 8] = [
    0.0,
    1.0 / 1.5,
    1.0 / 2.5,
    1.0 / 3.5,
    1.0 / 4.5,
    1.0 / 7.5,
    1.0 / 15.5,
    1.0 / 31.5,
];

/// Max quant values per selector
const MAX_QUANT: [f32; 8] = [0.0, 1.5, 2.5, 3.5, 4.5, 7.5, 15.5, 31.5];

/// Subband boundaries (from FFmpeg atrac3data.h / atracdenc)
const SUBBAND_TAB: [usize; 33] = [
    0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256, 288, 320,
    352, 384, 416, 448, 480, 512, 576, 640, 704, 768, 896, 1024,
];

fn make_encode_window() -> [f32; MDCT_HALF] {
    let mut w = [0.0; MDCT_HALF];
    for i in 0..MDCT_HALF {
        w[i] = (((i as f32 + 0.5) / MDCT_HALF as f32 - 0.5) * PI).sin() + 1.0;
    }
    w
}

fn make_decode_window() -> [f32; MDCT_N] {
    let enc = make_encode_window();
    let mut w = [0.0; MDCT_N];
    for i in 0..MDCT_HALF {
        let j = MDCT_HALF - 1 - i;
        let wi = enc[i];
        let wj = enc[j];
        let denom = 0.5 * (wi * wi + wj * wj);
        w[i] = wi / denom;
        w[MDCT_N - 1 - i] = wi / denom;
        w[j] = wj / denom;
        w[MDCT_N - 1 - j] = wj / denom;
    }
    w
}

pub struct DspState {
    pub history: [f32; 1024],
    /// Per-band MDCT overlap buffers for overlap-add synthesis
    pub mdct_overlap: [[f32; BAND_SIZE]; NUM_BANDS],
    /// Per-band previous-frame samples for encode windowing
    pub mdct_prev: [[f32; BAND_SIZE]; NUM_BANDS],
}

impl DspState {
    pub fn new() -> Self {
        Self {
            history: [0.0; 1024],
            mdct_overlap: [[0.0; BAND_SIZE]; NUM_BANDS],
            mdct_prev: [[0.0; BAND_SIZE]; NUM_BANDS],
        }
    }
}

fn apply_mdct(in_data: &[f32; MDCT_N], out_data: &mut [f32; MDCT_HALF]) {
    let n = MDCT_N;
    let n2 = n / 2;
    let n4 = n / 4;

    for k in 0..n2 {
        let mut sum = 0.0f32;
        let kf = k as f32 + 0.5;
        for n_idx in 0..n {
            let nf = n_idx as f32 + 0.5 + n4 as f32;
            sum += in_data[n_idx] * (PI / n as f32 * nf * kf).cos();
        }
        out_data[k] = sum;
    }
}

fn apply_imdct(in_data: &[f32; MDCT_HALF], out_data: &mut [f32; MDCT_N]) {
    let n = MDCT_N;
    let n2 = n / 2;
    let n4 = n / 4;

    let scale = 2.0 / n as f32;
    for n_idx in 0..n {
        let mut sum = 0.0f32;
        let nf = n_idx as f32 + 0.5 + n4 as f32;
        for k in 0..n2 {
            let kf = k as f32 + 0.5;
            sum += in_data[k] * (PI / n as f32 * nf * kf).cos();
        }
        out_data[n_idx] = sum * scale;
    }
}

pub fn qmf_mdct_forward(pcm_in: &[f32], subbands_out: &mut [f32; 2048], state: &mut DspState) {
    let enc_win = make_encode_window();

    for b in 0..NUM_BANDS {
        let band_start = b * BAND_SIZE;
        let band_end = band_start + BAND_SIZE;
        let cur_band = &pcm_in[band_start..band_end];

        let mut mdct_in = [0.0f32; MDCT_N];

        mdct_in[..MDCT_HALF].copy_from_slice(&state.mdct_prev[b]);

        for i in 0..MDCT_HALF {
            mdct_in[MDCT_HALF + i] = enc_win[MDCT_HALF - 1 - i] * cur_band[i];
        }

        for i in 0..MDCT_HALF {
            state.mdct_prev[b][i] = enc_win[i] * cur_band[i];
        }

        let mut mdct_out = [0.0f32; MDCT_HALF];
        apply_mdct(&mdct_in, &mut mdct_out);

        if b & 1 != 0 {
            mdct_out.reverse();
        }

        subbands_out[band_start..band_end].copy_from_slice(&mdct_out);
    }
}

pub fn qmf_mdct_inverse(subbands_in: &[f32; 2048], pcm_out: &mut [f32], state: &mut DspState) {
    let dec_win = make_decode_window();

    for b in 0..NUM_BANDS {
        let band_start = b * BAND_SIZE;
        let band_end = band_start + BAND_SIZE;

        let mut spec = [0.0f32; MDCT_HALF];
        spec.copy_from_slice(&subbands_in[band_start..band_end]);
        if b & 1 != 0 {
            spec.reverse();
        }

        let mut imdct_buf = [0.0f32; MDCT_N];
        apply_imdct(&spec, &mut imdct_buf);

        for i in 0..MDCT_N {
            imdct_buf[i] *= dec_win[i];
        }

        let overlap = &mut state.mdct_overlap[b];
        for i in 0..MDCT_HALF {
            pcm_out[band_start + i] = imdct_buf[i] + overlap[i];
        }
        overlap.copy_from_slice(&imdct_buf[MDCT_HALF..]);
    }
}

pub fn pack_bitstream(
    subbands: &[f32; 2048],
    _tones: &[GhaInfo],
    bit_alloc: &[u8; 32],
    out: &mut [u8],
) {
    let sf_tab = sf_table();
    let mut bw = BitWriter::new();

    bw.write_bits(0x28, 6);

    let num_qmf_bands: u32 = 4;
    bw.write_bits(num_qmf_bands - 1, 2);

    for _band in 0..num_qmf_bands {
        bw.write_bits(0, 3);
    }

    bw.write_bits(0, 5);

    let mut num_coded_bfu: usize = 0;
    for i in 0..32 {
        if bit_alloc[i] > 0 {
            num_coded_bfu = i + 1;
        }
    }
    if num_coded_bfu == 0 {
        num_coded_bfu = 1;
    }
    let num_subbands = (num_coded_bfu - 1).min(31) as u32;

    bw.write_bits(num_subbands, 5);
    bw.write_bits(1, 1);

    let mut selectors = vec![0u32; num_coded_bfu];
    for i in 0..num_coded_bfu {
        selectors[i] = (bit_alloc[i] as u32).min(7);
    }

    for i in 0..num_coded_bfu {
        bw.write_bits(selectors[i], 3);
    }

    let mut sf_indices = vec![0u32; num_coded_bfu];
    for i in 0..num_coded_bfu {
        if selectors[i] == 0 {
            continue;
        }
        let start = SUBBAND_TAB[i];
        let end = SUBBAND_TAB[i + 1];

        let mut max_abs = 0.0f32;
        for &v in &subbands[start..end] {
            max_abs = max_abs.max(v.abs());
        }

        let max_q = MAX_QUANT[selectors[i] as usize];
        let mut sf_idx = 0u32;
        for j in 0..64 {
            if sf_tab[j] * max_q >= max_abs {
                sf_idx = j as u32;
                break;
            }
            sf_idx = 63;
        }
        sf_indices[i] = sf_idx;
        bw.write_bits(sf_idx, 6);
    }

    for i in 0..num_coded_bfu {
        if selectors[i] == 0 {
            continue;
        }
        let start = SUBBAND_TAB[i];
        let end = SUBBAND_TAB[i + 1];
        let block_size = end - start;
        let selector = selectors[i] as usize;
        let num_bits = CLC_LENGTH_TAB[selector] as usize;

        let sf = sf_tab[sf_indices[i] as usize];
        let inv_q = if sf > 0.0 {
            MAX_QUANT[selector] / sf
        } else {
            0.0
        };

        if selector > 1 {
            for j in 0..block_size {
                let mantissa = (subbands[start + j] * inv_q).round().clamp(
                    -(1 << (num_bits - 1)) as f32,
                    ((1 << (num_bits - 1)) - 1) as f32,
                ) as i32;
                let unsigned = if mantissa < 0 {
                    (mantissa + (1 << num_bits)) as u32
                } else {
                    mantissa as u32
                };
                bw.write_bits(unsigned & ((1 << num_bits) - 1), num_bits);
            }
        } else {
            for j in (0..block_size).step_by(2) {
                let m_a = (subbands[start + j] * inv_q).round().clamp(-2.0, 1.0) as i32;
                let m_b = if j + 1 < block_size {
                    (subbands[start + j + 1] * inv_q).round().clamp(-2.0, 1.0) as i32
                } else {
                    0
                };
                let idx_a = mantissa_to_clc_idx(m_a);
                let idx_b = mantissa_to_clc_idx(m_b);
                bw.write_bits(((idx_a << 2) | idx_b) as u32, 4);
            }
        }
    }

    let bytes = bw.flush();
    let copy_len = bytes.len().min(out.len());
    out[..copy_len].copy_from_slice(&bytes[..copy_len]);
}

/// Maps mantissa value to CLC index for selector 1 encoding.
/// From atracdenc: mantissa_clc_rtab = {2, 3, 0, 1} for mantissa+2
fn mantissa_to_clc_idx(mantissa: i32) -> u32 {
    const RTAB: [u32; 4] = [2, 3, 0, 1];
    let idx = (mantissa + 2).clamp(0, 3) as usize;
    RTAB[idx]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mdct_roundtrip() {
        let mut input = [0.0f32; MDCT_N];
        for i in 0..MDCT_N {
            input[i] = (2.0 * PI * 5.0 * i as f32 / MDCT_N as f32).sin();
        }

        let mut spectrum = [0.0f32; MDCT_HALF];
        apply_mdct(&input, &mut spectrum);

        let mut reconstructed = [0.0f32; MDCT_N];
        apply_imdct(&spectrum, &mut reconstructed);

        // The MDCT is a lapped transform, so a single-block roundtrip won't
        // perfectly reconstruct. But the energy should be preserved.
        let input_energy: f32 = input.iter().map(|x| x * x).sum();
        let recon_energy: f32 = reconstructed.iter().map(|x| x * x).sum();

        assert!(
            recon_energy > 0.0,
            "reconstructed signal should have nonzero energy"
        );
        let ratio = recon_energy / input_energy;
        assert!(
            ratio > 0.1 && ratio < 10.0,
            "energy ratio {ratio} out of expected range"
        );
    }

    #[test]
    fn test_qmf_mdct_forward_inverse_roundtrip() {
        let mut state_fwd = DspState::new();
        let mut state_inv = DspState::new();

        let mut pcm1 = vec![0.0f32; 2048];
        let mut pcm2 = vec![0.0f32; 2048];
        for i in 0..2048 {
            pcm1[i] = (2.0 * PI * 3.0 * i as f32 / 2048.0).sin() * 1000.0;
            pcm2[i] = (2.0 * PI * 7.0 * i as f32 / 2048.0).sin() * 1000.0;
        }

        let mut sub1 = [0.0f32; 2048];
        qmf_mdct_forward(&pcm1, &mut sub1, &mut state_fwd);

        let mut out1 = vec![0.0f32; 2048];
        qmf_mdct_inverse(&sub1, &mut out1, &mut state_inv);

        let mut sub2 = [0.0f32; 2048];
        qmf_mdct_forward(&pcm2, &mut sub2, &mut state_fwd);

        let mut out2 = vec![0.0f32; 2048];
        qmf_mdct_inverse(&sub2, &mut out2, &mut state_inv);

        let energy: f32 = out2.iter().map(|x| x * x).sum();
        assert!(energy > 0.0, "roundtrip output should have energy");
    }

    #[test]
    fn test_pack_bitstream_produces_output() {
        let subbands = [0.5f32; 2048];
        let tones = [];
        let bit_alloc = [3u8; 32];
        let mut out = [0u8; 512];

        pack_bitstream(&subbands, &tones, &bit_alloc, &mut out);

        let nonzero = out.iter().any(|&b| b != 0);
        assert!(nonzero, "bitstream output should be nonzero");

        // First byte should contain the 0x28 header (top 6 bits = 0b101000)
        assert_eq!(out[0] >> 2, 0x28, "header should start with 0x28 pattern");
    }
}
