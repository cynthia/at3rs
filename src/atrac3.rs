use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::f32::consts::PI;
use std::sync::Arc;

#[path = "atrac3/channel.rs"]
pub mod channel;
#[path = "atrac3/common.rs"]
pub mod common;
#[path = "atrac3/config.rs"]
pub mod config;
#[path = "atrac3/debug.rs"]
pub mod debug;
pub use channel::Atrac3ChannelUnit;
use common::*;
use config::EncoderConfig;
pub use debug::*;

type GainPoint = (u8, u8);

#[derive(Clone, Debug)]
struct TonalComponent {
    abs_pos: usize,
    coded_values: usize,
    quant_selector: usize,
    sf_idx: usize,
    energy: f32,
    mantissas: Vec<i16>,
}

#[derive(Clone)]
struct BfuCoding {
    ch: usize,
    block: usize,
    importance: f32,
    max_val: f32,
    table_idx: usize,
    sf_idx: usize,
    sf_write_offset: i32,
    mantissas: Vec<i16>,
    vlc_symbols: Vec<u8>,
    reconstructed: Vec<f32>,
    vlc_bits: usize,
    clc_bits: usize,
    bit_count: usize,
    energy_err: f32,
}

impl BfuCoding {
    fn new(
        ch: usize,
        block: usize,
        importance: f32,
        max_val: f32,
        table_idx: usize,
        block_len: usize,
    ) -> Self {
        Self {
            ch,
            block,
            importance,
            max_val,
            table_idx,
            sf_idx: 0,
            sf_write_offset: 0,
            mantissas: vec![0; block_len],
            vlc_symbols: vec![0; block_len],
            reconstructed: vec![0.0; block_len],
            vlc_bits: 0,
            clc_bits: 0,
            bit_count: 0,
            energy_err: 1.0,
        }
    }
}

pub struct Atrac3Context {
    pub channels: u16,
    pub block_align: usize,
    pub units: Vec<Atrac3ChannelUnit>,
    pub simulated_spectra: [[[f32; 256]; 4]; 2],
    fft_128: Arc<dyn Fft<f32>>,
    config: EncoderConfig,
    loudness_state: f32,
    gain_last_level: [[f32; 4]; 2],
    gain_last_target: [[f32; 4]; 2],
    prev_bfu_coded: [[bool; 32]; 2],
    prev_bfu_ready: [bool; 2],
}

impl Atrac3Context {
    fn stage_debug_enabled(&self) -> bool {
        self.config.stage_debug
    }

    fn vlc_enabled(&self) -> bool {
        self.config.vlc_enabled()
    }

    fn analysis_spectrum_scale(&self) -> f32 {
        self.config.analysis_scale
    }

    fn ath_gate_scale(&self) -> f32 {
        self.config.ath_gate_scale
    }

    pub fn new(channels: u16, block_align: usize) -> Self {
        Self::with_config(channels, block_align, EncoderConfig::default())
    }

    pub fn with_config(channels: u16, block_align: usize, config: EncoderConfig) -> Self {
        let mut units = Vec::new();
        for _ in 0..channels {
            units.push(Atrac3ChannelUnit::new());
        }

        let mut planner = FftPlanner::new();
        let fft_128 = planner.plan_fft_forward(128);

        Self {
            channels,
            block_align,
            units,
            simulated_spectra: [[[0.0; 256]; 4]; 2],
            fft_128,
            config,
            loudness_state: ATRAC3_LOUDNESS_FACTOR,
            gain_last_level: [[0.0; 4]; 2],
            gain_last_target: [[0.0; 4]; 2],
            prev_bfu_coded: [[false; 32]; 2],
            prev_bfu_ready: [false; 2],
        }
    }

    fn mdct_calc_forward(&self, pcm_in: &[f32; 512], spectra_out: &mut [f32; 256], odd_band: bool) {
        let n = 512;
        let n2 = n >> 1;
        let n4 = n >> 2;
        let n34 = 3 * n4;
        let n54 = 5 * n4;

        let scale = 1.0f32;
        let s_scale = (scale / (n as f32)).sqrt();
        let alpha = 2.0 * PI / (8.0 * n as f32);
        let omega = 2.0 * PI / (n as f32);

        let mut cos_tab = [0.0f32; 128];
        let mut sin_tab = [0.0f32; 128];
        for i in 0..128 {
            cos_tab[i] = s_scale * (omega * (i as f32) + alpha).cos();
            sin_tab[i] = s_scale * (omega * (i as f32) + alpha).sin();
        }

        let mut fft_buf = [Complex::new(0.0, 0.0); 128];

        for i in (0..n4).step_by(2) {
            let r0 = pcm_in[n34 - 1 - i] + pcm_in[n34 + i];
            let i0 = pcm_in[n4 + i] - pcm_in[n4 - 1 - i];
            let c = cos_tab[i / 2];
            let s = sin_tab[i / 2];
            fft_buf[i / 2] = Complex::new(r0 * c + i0 * s, i0 * c - r0 * s);
        }

        for i in (n4..n2).step_by(2) {
            let r0 = pcm_in[n34 - 1 - i] - pcm_in[i - n4];
            let i0 = pcm_in[n4 + i] + pcm_in[n54 - 1 - i];
            let c = cos_tab[i / 2];
            let s = sin_tab[i / 2];
            fft_buf[i / 2] = Complex::new(r0 * c + i0 * s, i0 * c - r0 * s);
        }

        self.fft_128.process(&mut fft_buf);

        for i in (0..n2).step_by(2) {
            let r0 = fft_buf[i / 2].re;
            let i0 = fft_buf[i / 2].im;
            let c = cos_tab[i / 2];
            let s = sin_tab[i / 2];

            let val1 = -r0 * c - i0 * s;
            let val2 = -r0 * s + i0 * c;

            spectra_out[i] = val1;
            spectra_out[n2 - 1 - i] = val2;
        }

        if odd_band {
            spectra_out.reverse();
        }
    }

    /// Inverse MDCT mapping 256 frequency spectra into 256 time-domain samples (with 512-point overlap-add)
    fn imdct_calc(
        &self,
        spectra: &[f32; 256],
        pcm_out: &mut [f32; 256],
        overlap_buf: &mut [f32; 256],
        odd_band: bool,
    ) {
        let n = 512;
        let n2 = n >> 1;
        let n4 = n >> 2;
        let n34 = 3 * n4;
        let n54 = 5 * n4;

        let scale = 256.0f32;
        let s_scale = (scale / (n as f32)).sqrt();
        let alpha = 2.0 * PI / (8.0 * n as f32);
        let omega = 2.0 * PI / (n as f32);

        let mut cos_tab = [0.0f32; 128];
        let mut sin_tab = [0.0f32; 128];
        for i in 0..128 {
            cos_tab[i] = s_scale * (omega * (i as f32) + alpha).cos();
            sin_tab[i] = s_scale * (omega * (i as f32) + alpha).sin();
        }

        let mut fft_buf = [Complex::new(0.0, 0.0); 128];

        let mut reordered = *spectra;
        if odd_band {
            reordered.reverse();
        }

        for i in (0..n2).step_by(2) {
            let r0 = reordered[i];
            let i0 = reordered[n2 - 1 - i];

            let c = cos_tab[i / 2];
            let s = sin_tab[i / 2];
            fft_buf[i / 2] = Complex::new(-2.0 * (i0 * s + r0 * c), -2.0 * (i0 * c - r0 * s));
        }

        self.fft_128.process(&mut fft_buf);

        let mut buf = [0.0; 512];
        for i in (0..n4).step_by(2) {
            let r0 = fft_buf[i / 2].re;
            let i0 = fft_buf[i / 2].im;
            let c = cos_tab[i / 2];
            let s = sin_tab[i / 2];

            let r1 = r0 * c + i0 * s;
            let i1 = r0 * s - i0 * c;

            buf[n34 - 1 - i] = r1;
            buf[n34 + i] = r1;
            buf[n4 + i] = i1;
            buf[n4 - 1 - i] = -i1;
        }

        for i in (n4..n2).step_by(2) {
            let r0 = fft_buf[i / 2].re;
            let i0 = fft_buf[i / 2].im;
            let c = cos_tab[i / 2];
            let s = sin_tab[i / 2];

            let r1 = r0 * c + i0 * s;
            let i1 = r0 * s - i0 * c;

            buf[n34 - 1 - i] = r1;
            buf[i - n4] = -r1;
            buf[n4 + i] = i1;
            buf[n54 - 1 - i] = i1;
        }

        for i in 0..256 {
            pcm_out[i] = overlap_buf[i] + buf[i] * (2.0 * DECODE_WINDOW[i]);
            overlap_buf[i] = buf[256 + i] * (2.0 * DECODE_WINDOW[255 - i]);
        }
    }

    pub fn analyze_frame(&mut self, pcm_in: &[f32], subbands: &mut [[f32; 256]; 4], ch: usize) {
        let unit = &mut self.units[ch];
        let mut temp_buf1 = [0.0; 1024 + 46];
        let mut temp_buf2 = [0.0; 512 + 46];
        let mut temp_buf3 = [0.0; 512 + 46];

        let mut out1 = [0.0; 512];
        let mut out2 = [0.0; 512];

        pqf_forward(
            pcm_in,
            &mut out1,
            &mut out2,
            &mut unit.an_delay_buf3,
            &mut temp_buf1,
        );

        let mut b0 = [0.0; 256];
        let mut b1 = [0.0; 256];
        pqf_forward(
            &out1,
            &mut b0,
            &mut b1,
            &mut unit.an_delay_buf1,
            &mut temp_buf2,
        );
        subbands[0] = b0;
        subbands[1] = b1;

        let mut b3 = [0.0; 256];
        let mut b2 = [0.0; 256];
        pqf_forward(
            &out2,
            &mut b3,
            &mut b2,
            &mut unit.an_delay_buf2,
            &mut temp_buf3,
        );
        subbands[3] = b3;
        subbands[2] = b2;
    }

    pub fn synthesize_frame(&mut self, subbands: &[[f32; 256]; 4], pcm_out: &mut [f32], ch: usize) {
        let unit = &mut self.units[ch];
        let mut temp_buf1 = [0.0; 512 + 46];
        let mut temp_buf2 = [0.0; 512 + 46];
        let mut temp_buf3 = [0.0; 1024 + 46];

        let mut out1 = [0.0; 512];
        let mut out2 = [0.0; 512];

        iqmf(
            &subbands[0],
            &subbands[1],
            256,
            &mut out1,
            &mut unit.delay_buf1,
            &mut temp_buf1,
        );

        iqmf(
            &subbands[3],
            &subbands[2],
            256,
            &mut out2,
            &mut unit.delay_buf2,
            &mut temp_buf2,
        );

        iqmf(
            &out1,
            &out2,
            512,
            pcm_out,
            &mut unit.delay_buf3,
            &mut temp_buf3,
        );
    }

    pub fn encode_frame(&mut self, pcm_in: &[i16], bitstream_out: &mut [u8]) -> usize {
        if self.stage_debug_enabled() {
            eprintln!("encode_frame:start");
        }
        let mut spectra = [[[0.0f32; 256]; 4]; 2];
        let mut subbands = [[[0.0f32; 256]; 4]; 2];

        let mut planar_pcm = vec![vec![0.0f32; 1024]; self.channels as usize];
        for ch in 0..self.channels as usize {
            for i in 0..1024 {
                planar_pcm[ch][i] = pcm_in[i * self.channels as usize + ch] as f32 / 32768.0;
            }
        }

        let analysis_scale = self.analysis_spectrum_scale();
        let mut frame_gains: Vec<[Vec<GainPoint>; 4]> = (0..self.channels as usize)
            .map(|_| std::array::from_fn(|_| Vec::new()))
            .collect();
        for ch in 0..self.channels as usize {
            if self.stage_debug_enabled() {
                eprintln!("encode_frame:analyze ch={}", ch);
            }
            self.analyze_frame(&planar_pcm[ch], &mut subbands[ch], ch);
            let gains = self.detect_gain_points(ch, &subbands[ch]);

            for band in 0..4 {
                let odd_band = (band & 1) != 0;
                let mut mdct_in = [0.0; 512];
                let mut prev_windowed = self.units[ch].mdct_buf[band];
                let mut cur_band = subbands[ch][band];
                self.apply_gain_modulation(&mut prev_windowed, &mut cur_band, &gains[band]);
                mdct_in[..256].copy_from_slice(&prev_windowed);
                for i in 0..256 {
                    let cur = cur_band[i];
                    self.units[ch].mdct_buf[band][i] = ENCODE_WINDOW[i] * cur;
                    mdct_in[256 + i] = ENCODE_WINDOW[255 - i] * cur;
                }

                let mut spectrum = [0.0; 256];
                self.mdct_calc_forward(&mdct_in, &mut spectrum, odd_band);
                for coeff in &mut spectrum {
                    *coeff *= analysis_scale;
                }
                spectra[ch][band] = spectrum;
            }
            frame_gains[ch] = gains;
        }

        bitstream_out.fill(0);

        let flat_spectra: Vec<[f32; 1024]> = (0..self.channels as usize)
            .map(|ch| self.flatten_channel_spectrum(&spectra[ch]))
            .collect();
        let per_channel_loudness: Vec<f32> = flat_spectra
            .iter()
            .map(|spec| self.frame_channel_loudness(spec))
            .collect();
        let loudness_norm = self.tracked_loudness_norm(&per_channel_loudness);

        let channel_budget_bytes = self.block_align / self.channels as usize;
        let mut frame_bytes = Vec::with_capacity(self.block_align);

        for ch in 0..self.channels as usize {
            if self.stage_debug_enabled() {
                eprintln!("encode_frame:plan ch={}", ch);
            }
            let tonal_components = self.detect_tonal_components(&flat_spectra[ch]);
            let mut residual_spectrum = flat_spectra[ch];
            self.remove_tonal_components_from_residual(&mut residual_spectrum, &tonal_components);
            let extra_header_bits = self.gain_extra_bits(&frame_gains[ch], 4)
                + self.tonal_components_extra_bits(&tonal_components, 4);
            let prev_bfu_coded = if self.prev_bfu_ready[ch] {
                Some(&self.prev_bfu_coded[ch])
            } else {
                None
            };
            let plans = self.build_bfu_plan(
                ch,
                &residual_spectrum,
                loudness_norm,
                extra_header_bits,
                prev_bfu_coded,
            );
            if self.stage_debug_enabled() {
                eprintln!("encode_frame:write ch={}", ch);
            }
            let unit_bytes = self.write_channel_sound_unit(
                ch,
                &plans,
                &frame_gains[ch],
                &tonal_components,
                channel_budget_bytes,
            );
            self.store_quantized_channel_spectrum(ch, &plans);
            for plan in &plans {
                self.prev_bfu_coded[ch][plan.block] = plan.table_idx != 0;
            }
            self.prev_bfu_ready[ch] = true;
            frame_bytes.extend_from_slice(&unit_bytes);
        }

        let len = frame_bytes.len().min(bitstream_out.len());
        bitstream_out[..len].copy_from_slice(&frame_bytes[..len]);
        if self.stage_debug_enabled() {
            eprintln!("encode_frame:done");
        }
        len
    }

    pub fn decode_frame(&mut self, _bitstream: &[u8], pcm_out: &mut [i16]) {
        let mut subbands = [[[0.0f32; 256]; 4]; 2];

        for ch in 0..self.channels as usize {
            for band in 0..4 {
                let odd_band = (band & 1) != 0;
                let mut pcm_band = [0.0; 256];

                let mut overlap_data = self.units[ch].imdct_overlap[band];
                let spectra_data = self.units[ch].quantized_spectra[band];

                self.imdct_calc(&spectra_data, &mut pcm_band, &mut overlap_data, odd_band);

                self.units[ch].imdct_overlap[band] = overlap_data;
                subbands[ch][band] = pcm_band;
            }
        }

        let mut planar_pcm = vec![vec![0.0f32; 1024]; self.channels as usize];

        for ch in 0..self.channels as usize {
            self.synthesize_frame(&subbands[ch], &mut planar_pcm[ch], ch);
        }

        for ch in 0..self.channels as usize {
            for i in 0..1024 {
                let sample = (planar_pcm[ch][i] * 32768.0).clamp(-32768.0, 32767.0);
                pcm_out[i * self.channels as usize + ch] = sample as i16;
            }
        }
    }
}

impl Atrac3Context {
    fn selector_max_quant(&self, table_idx: usize) -> f32 {
        match table_idx {
            1 => 1.5,
            2 => 2.5,
            3 => 3.5,
            4 => 4.5,
            5 => 7.5,
            6 => 15.5,
            7 => 31.5,
            _ => 0.0,
        }
    }

    fn selector_quant_limit(&self, table_idx: usize) -> i16 {
        match table_idx {
            1 => 1,
            2 => 2,
            3 => 3,
            4 => 4,
            5 => 7,
            6 => 15,
            7 => 31,
            _ => 0,
        }
    }

    fn sf_index_for_value(&self, target_sf: f32) -> usize {
        let target_sf = target_sf.clamp(0.0, 1.0);
        let mut sf_idx = 63usize;
        for i in 0..64 {
            if SF_TABLE[i] >= target_sf {
                sf_idx = i;
                break;
            }
        }
        sf_idx
    }

    fn avg_mag_bias(&self, avg_mag_metric: f32) -> f32 {
        if avg_mag_metric <= 2.9 {
            -0.75
        } else if avg_mag_metric <= 3.0 {
            -0.5
        } else if avg_mag_metric <= 3.1 {
            -0.25
        } else if avg_mag_metric <= 3.2 {
            0.0
        } else if avg_mag_metric <= 3.3 {
            0.25
        } else {
            0.5
        }
    }

    fn importance_boost(&self, importance: f32) -> f32 {
        if importance >= 10.0 {
            2.0
        } else if importance >= 6.0 {
            1.0
        } else if importance >= 3.5 {
            0.5
        } else {
            0.0
        }
    }

    fn block_band(&self, block: usize) -> usize {
        for band in 1..ATRAC3_BLOCKS_PER_BAND.len() {
            if block < ATRAC3_BLOCKS_PER_BAND[band] {
                return band - 1;
            }
        }
        ATRAC3_BLOCKS_PER_BAND.len() - 2
    }

    fn selector_spread_divisor(&self, block: usize) -> f32 {
        if block < 3 {
            2.8
        } else if block < 10 {
            2.6
        } else if block < 15 {
            3.3
        } else if block <= 20 {
            3.6
        } else if block <= 28 {
            4.2
        } else {
            6.0
        }
    }

    fn analyze_scale_factor_spread(&self, sf_indices: &[usize]) -> f32 {
        if sf_indices.is_empty() {
            return 0.0;
        }
        let mean = sf_indices.iter().map(|&v| v as f32).sum::<f32>() / sf_indices.len() as f32;
        let variance = sf_indices
            .iter()
            .map(|&v| {
                let d = v as f32 - mean;
                d * d
            })
            .sum::<f32>()
            / sf_indices.len() as f32;
        variance.sqrt().min(14.0) / 14.0
    }

    fn initial_selector_for_block(
        &self,
        sf_idx: usize,
        max_val: f32,
        block: usize,
        spread: f32,
        shift: f32,
        band_boost: f32,
    ) -> usize {
        if max_val < 1.0e-7 {
            return 0;
        }
        let fix = ATRAC3_FIXED_ALLOC_TABLE[block];
        let x = self.selector_spread_divisor(block);
        let tmp = spread * (sf_idx as f32 / x) + (1.0 - spread) * fix - shift + band_boost;
        let tmp_i = tmp as i32;
        if tmp_i > 7 {
            7
        } else if tmp_i < 0 {
            0
        } else if tmp_i == 0 {
            1
        } else {
            tmp_i as usize
        }
    }

    fn quantize_block_energy_adjusted(
        &self,
        scaled: &[f32],
        max_quant: f32,
        quant_limit: i16,
    ) -> (Vec<i16>, f32) {
        let mut mantissas = vec![0i16; scaled.len()];
        let mut orig_energy = 0.0f32;
        let mut recon_energy = 0.0f32;
        let inv2 = 1.0 / (max_quant * max_quant);
        let limit = quant_limit as i32;
        let mut candidates: Vec<(f32, usize)> = Vec::new();

        for (i, &value) in scaled.iter().enumerate() {
            let t = value * max_quant;
            orig_energy += value * value;
            let q = self
                .round_ties_even(t)
                .clamp(-(quant_limit as i32), quant_limit as i32) as i16;
            mantissas[i] = q;
            recon_energy += (q as f32) * (q as f32) * inv2;

            let delta = t - (t.trunc() + 0.5);
            if delta.abs() < 0.25 {
                candidates.push((delta.abs(), i));
            }
        }

        candidates.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        if recon_energy < orig_energy {
            for &(_, i) in &candidates {
                let t = scaled[i] * max_quant;
                let q = mantissas[i] as i32;
                if (q.abs() as f32) < t.abs() && q.abs() < limit {
                    let mut adjusted = q;
                    if adjusted > 0 {
                        adjusted += 1;
                    } else if adjusted < 0 {
                        adjusted -= 1;
                    } else {
                        adjusted = if t > 0.0 { 1 } else { -1 };
                    }
                    let next_energy =
                        recon_energy - (q * q) as f32 * inv2 + (adjusted * adjusted) as f32 * inv2;
                    if (next_energy - orig_energy).abs() < (recon_energy - orig_energy).abs() {
                        mantissas[i] = adjusted as i16;
                        recon_energy = next_energy;
                    }
                }
            }
        } else if recon_energy > orig_energy {
            for &(_, i) in &candidates {
                let t = scaled[i] * max_quant;
                let q = mantissas[i] as i32;
                if (q.abs() as f32) > t.abs() {
                    let adjusted = if q > 0 {
                        q - 1
                    } else if q < 0 {
                        q + 1
                    } else {
                        q
                    };
                    let next_energy =
                        recon_energy - (q * q) as f32 * inv2 + (adjusted * adjusted) as f32 * inv2;
                    if (next_energy - orig_energy).abs() < (recon_energy - orig_energy).abs() {
                        mantissas[i] = adjusted as i16;
                        recon_energy = next_energy;
                    }
                }
            }
        }

        let energy_err = if recon_energy > 1.0e-12 {
            orig_energy / recon_energy
        } else {
            1.0
        };
        (mantissas, energy_err)
    }

    fn round_ties_even(&self, value: f32) -> i32 {
        let floor = value.floor();
        let frac = value - floor;
        if frac < 0.5 {
            floor as i32
        } else if frac > 0.5 {
            floor as i32 + 1
        } else {
            let lower = floor as i32;
            if lower & 1 == 0 {
                lower
            } else {
                lower + 1
            }
        }
    }

    fn quantize_block_plain(
        &self,
        scaled: &[f32],
        max_quant: f32,
        quant_limit: i16,
    ) -> (Vec<i16>, f32) {
        let mut mantissas = vec![0i16; scaled.len()];
        let mut orig_energy = 0.0f32;
        let mut recon_energy = 0.0f32;
        let inv2 = 1.0 / (max_quant * max_quant);
        let quant_limit = quant_limit as i32;

        for (i, &value) in scaled.iter().enumerate() {
            let t = value * max_quant;
            orig_energy += value * value;
            let q = self.round_ties_even(t).clamp(-quant_limit, quant_limit) as i16;
            mantissas[i] = q;
            recon_energy += (q as f32) * (q as f32) * inv2;
        }

        let energy_err = if recon_energy > 1.0e-12 {
            orig_energy / recon_energy
        } else {
            1.0
        };
        (mantissas, energy_err)
    }

    fn scale_factor_search_enabled(&self) -> bool {
        self.config.scale_factor_search_enabled()
    }

    fn experimental_sf_write_offset(&self) -> i32 {
        self.config.experimental_sf_write_offset
    }

    fn tonal_quant_boost_enabled(&self) -> bool {
        self.config.tonal_quant_boost_enabled()
    }

    fn tonal_quant_boost(&self) -> f32 {
        self.config.tonal_quant_boost_factor
    }

    fn quantize_for_plan(
        &self,
        scaled: &[f32],
        block: usize,
        max_quant: f32,
        quant_limit: i16,
    ) -> (Vec<i16>, f32) {
        if block > 18 || (block >= 6 && self.spectral_flatness(scaled) >= 0.18) {
            self.quantize_block_energy_adjusted(scaled, max_quant, quant_limit)
        } else {
            self.quantize_block_plain(scaled, max_quant, quant_limit)
        }
    }

    fn search_scale_factor_for_plan(
        &self,
        spectrum: &[f32],
        plan: &BfuCoding,
        max_quant: f32,
        quant_limit: i16,
    ) -> Option<(usize, Vec<i16>, f32)> {
        if !self.scale_factor_search_enabled() || plan.table_idx == 0 {
            return None;
        }

        let source_energy = spectrum.iter().map(|v| v * v).sum::<f32>().max(1.0e-18);
        let base_sf = plan.sf_idx;
        let start = base_sf.saturating_sub(4);
        let end = (base_sf + 5).min(63);
        let mut best: Option<(f32, usize, Vec<i16>, f32)> = None;

        for sf_idx in start..=end {
            let sf = SF_TABLE[sf_idx];
            let mut saturated = 0usize;
            let scaled: Vec<f32> = spectrum
                .iter()
                .map(|&v| {
                    let raw = v / sf;
                    if raw.abs() >= 0.99999 {
                        saturated += 1;
                    }
                    raw.clamp(-0.99999, 0.99999)
                })
                .collect();
            let (quantized, energy_err) =
                self.quantize_for_plan(&scaled, plan.block, max_quant, quant_limit);
            if self.tail_sf_needs_headroom(plan, &quantized, quant_limit) {
                continue;
            }

            let mut recon_energy = 0.0f32;
            let mut mse = 0.0f32;
            for (&orig, &q) in spectrum.iter().zip(quantized.iter()) {
                let recon = q as f32 * sf / max_quant;
                recon_energy += recon * recon;
                let diff = orig - recon;
                mse += diff * diff;
            }
            let (detail_weight, energy_weight) = self.texture_error_weights(spectrum);
            let energy_log_error = self.asymmetric_energy_log_error(source_energy, recon_energy);
            let normalized_mse = mse / source_energy;
            let saturation_penalty = saturated as f32 / spectrum.len().max(1) as f32;
            let sf_distance = (sf_idx as i32 - base_sf as i32).unsigned_abs() as f32;
            let score = detail_weight * normalized_mse
                + energy_weight * energy_log_error
                + 2.0 * saturation_penalty
                + 0.002 * sf_distance;

            if best
                .as_ref()
                .map(|(best_score, _, _, _)| score < *best_score)
                .unwrap_or(true)
            {
                best = Some((score, sf_idx, quantized, energy_err));
            }
        }

        best.map(|(_, sf_idx, quantized, energy_err)| (sf_idx, quantized, energy_err))
    }

    fn flatten_channel_spectrum(&self, channel_spectra: &[[f32; 256]; 4]) -> [f32; 1024] {
        let mut flat = [0.0f32; 1024];
        for band in 0..4 {
            flat[band * 256..(band + 1) * 256].copy_from_slice(&channel_spectra[band]);
        }
        flat
    }

    fn frame_channel_loudness(&self, spectrum: &[f32; 1024]) -> f32 {
        spectrum
            .iter()
            .zip(ATRAC3_LOUDNESS_CURVE.iter())
            .map(|(v, w)| v * v * w)
            .sum()
    }

    fn tracked_loudness_norm(&mut self, per_channel: &[f32]) -> f32 {
        self.loudness_state = if self.channels == 2 && per_channel.len() >= 2 {
            0.98 * self.loudness_state + 0.01 * (per_channel[0] + per_channel[1])
        } else {
            0.98 * self.loudness_state + 0.02 * per_channel[0]
        };
        (self.loudness_state / ATRAC3_LOUDNESS_FACTOR).clamp(0.001, 1.0)
    }

    fn debug_channel_plan_from_bfus(
        &self,
        ch: usize,
        spectrum: &[f32; 1024],
        plans: &[BfuCoding],
    ) -> DebugChannelPlan {
        let active_blocks = plans
            .iter()
            .rposition(|p| p.table_idx != 0)
            .map(|v| v + 1)
            .unwrap_or(1);
        let mut blocks = Vec::with_capacity(active_blocks);

        for plan in plans.iter().take(active_blocks) {
            let start = ATRAC3_SUBBAND_TAB[plan.block];
            let end = ATRAC3_SUBBAND_TAB[plan.block + 1];
            let input = &spectrum[start..end];
            let mut energy = 0.0f32;
            let mut recon_energy = 0.0f32;
            let mut distortion = 0.0f32;
            for (orig, recon) in input.iter().zip(plan.reconstructed.iter()) {
                energy += orig * orig;
                recon_energy += recon * recon;
                let diff = orig - recon;
                distortion += diff * diff;
            }

            blocks.push(DebugBfu {
                block: plan.block,
                start,
                end,
                table_idx: plan.table_idx,
                sf_idx: plan.sf_idx,
                max_val: plan.max_val,
                importance: plan.importance,
                bit_count: plan.bit_count,
                energy,
                recon_energy,
                distortion,
            });
        }

        DebugChannelPlan {
            channel: ch,
            active_blocks,
            total_bits: self.bfu_plan_bits(plans),
            blocks,
        }
    }

    fn rms_and_max(&self, values: &[f32]) -> (f32, f32) {
        let mut energy = 0.0f32;
        let mut max_val = 0.0f32;
        for &v in values {
            energy += v * v;
            max_val = max_val.max(v.abs());
        }
        let rms = if values.is_empty() {
            0.0
        } else {
            (energy / values.len() as f32).sqrt()
        };
        (rms, max_val)
    }

    fn experimental_gain_enabled(&self) -> bool {
        self.config.experimental_gain
    }

    fn experimental_gain_v2_enabled(&self) -> bool {
        self.config.experimental_gain_v2
    }

    fn gain_level(&self, level_idx: u8) -> f32 {
        2.0_f32.powi(ATRAC3_GAIN_NEUTRAL_LEVEL as i32 - level_idx as i32)
    }

    fn gain_interp(&self, cur_level: u8, next_level: u8) -> f32 {
        2.0_f32.powf((cur_level as f32 - next_level as f32) / ATRAC3_GAIN_LOC_SIZE as f32)
    }

    fn relation_to_gain_idx(&self, mut ratio: f32) -> u8 {
        fn floor_log2_u32(v: u32) -> u8 {
            if v == 0 {
                0
            } else {
                (u32::BITS - 1 - v.leading_zeros()) as u8
            }
        }

        if ratio <= 0.5 {
            ratio = 1.0 / ratio.max(0.00048828125);
            4u8.saturating_add(floor_log2_u32(ratio as u32)).min(15)
        } else {
            ratio = ratio.min(16.0);
            4u8.saturating_sub(floor_log2_u32(ratio as u32))
        }
    }

    fn median3_at(&self, values: &[f32; 32], idx: usize) -> f32 {
        let mut window = [
            values[idx.saturating_sub(1)],
            values[idx],
            values[(idx + 1).min(values.len() - 1)],
        ];
        window.sort_by(|a, b| a.total_cmp(b));
        window[1]
    }

    fn boundary_transient_score(&self, env: &[f32; 32], loc: usize, win: usize) -> f32 {
        let left_start = loc.saturating_sub(win);
        let left = env[left_start..loc].iter().copied().fold(0.0f32, f32::max);
        let right_end = (loc + win).min(env.len());
        let right = env[loc..right_end].iter().copied().fold(0.0f32, f32::max);
        let eps = 1.0e-9;
        ((right + eps) / (left + eps)).max((left + eps) / (right + eps))
    }

    fn detect_gain_points_for_band_v2(
        &mut self,
        ch: usize,
        band: usize,
        band_samples: &[f32; 256],
    ) -> Vec<GainPoint> {
        if band == 0 || band >= 3 {
            return Vec::new();
        }

        let mut rms = [0.0f32; 32];
        for subframe in 0..32 {
            let start = subframe * ATRAC3_GAIN_LOC_SIZE;
            let mut energy = 0.0f32;
            for &sample in &band_samples[start..start + ATRAC3_GAIN_LOC_SIZE] {
                energy += sample * sample;
            }
            rms[subframe] = (energy / ATRAC3_GAIN_LOC_SIZE as f32).sqrt();
        }

        let saved_last_level = self.gain_last_level[ch][band];
        let target = rms[31];
        self.gain_last_level[ch][band] = target;
        self.gain_last_target[ch][band] = target;

        if saved_last_level < 1.0e-6 || target < 1.0e-6 {
            return Vec::new();
        }

        let mut filtered = [0.0f32; 32];
        for idx in 0..32 {
            filtered[idx] = self.median3_at(&rms, idx);
        }

        let mut sf_level = [ATRAC3_GAIN_NEUTRAL_LEVEL; 32];
        for idx in 0..32 {
            sf_level[idx] = self.relation_to_gain_idx(filtered[idx] / target);
        }

        let mut target_sf = 0usize;
        for sf in (0..31).rev() {
            if sf_level[sf] != ATRAC3_GAIN_NEUTRAL_LEVEL {
                target_sf = sf + 1;
                break;
            }
        }
        if target_sf == 0 {
            return Vec::new();
        }

        let mut transitions: Vec<(usize, u8, u8)> = Vec::new();
        let mut prev = ATRAC3_GAIN_NEUTRAL_LEVEL;
        for sf in (0..target_sf).rev() {
            let level = sf_level[sf];
            if level == prev {
                continue;
            }
            let loc = sf + 1;
            let delta = level.abs_diff(prev);
            let score = self.boundary_transient_score(&filtered, loc, 3);
            if loc == target_sf || delta >= 2 || score >= 1.9 {
                transitions.push((loc, level, delta));
                prev = level;
            }
        }
        transitions.reverse();

        if transitions.len() > 6 {
            transitions.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| b.0.cmp(&a.0)));
            transitions.truncate(6);
            transitions.sort_by_key(|v| v.0);
        }

        transitions
            .into_iter()
            .filter_map(|(loc, level, _)| (loc <= 31).then_some((level, loc as u8)))
            .collect()
    }

    fn detect_gain_points_for_band(&self, band_samples: &[f32; 256]) -> Vec<GainPoint> {
        if !self.experimental_gain_enabled() {
            return Vec::new();
        }

        let mut subframe_rms = [0.0f32; 32];
        for subframe in 0..32 {
            let start = subframe * ATRAC3_GAIN_LOC_SIZE;
            let mut energy = 0.0f32;
            for &sample in &band_samples[start..start + ATRAC3_GAIN_LOC_SIZE] {
                energy += sample * sample;
            }
            subframe_rms[subframe] = (energy / ATRAC3_GAIN_LOC_SIZE as f32).sqrt();
        }

        let mut sorted = subframe_rms;
        sorted.sort_by(|a, b| a.total_cmp(b));
        let noise_floor = sorted[7].max(1.0e-7);
        let baseline = sorted[15].max(noise_floor);
        let (peak_idx, peak) = subframe_rms
            .iter()
            .copied()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap_or((0, 0.0));
        let ratio = peak / baseline;

        if peak < 2.0e-4 || ratio < 5.0 {
            return Vec::new();
        }

        let level = if ratio >= 18.0 {
            1
        } else if ratio >= 10.0 {
            2
        } else {
            3
        };
        let location = peak_idx.saturating_sub(1).min(31) as u8;
        vec![(level, location)]
    }

    fn detect_gain_points(&mut self, ch: usize, subbands: &[[f32; 256]; 4]) -> [Vec<GainPoint>; 4] {
        if self.experimental_gain_v2_enabled() {
            return [
                self.detect_gain_points_for_band_v2(ch, 0, &subbands[0]),
                self.detect_gain_points_for_band_v2(ch, 1, &subbands[1]),
                self.detect_gain_points_for_band_v2(ch, 2, &subbands[2]),
                self.detect_gain_points_for_band_v2(ch, 3, &subbands[3]),
            ];
        }

        [
            self.detect_gain_points_for_band(&subbands[0]),
            self.detect_gain_points_for_band(&subbands[1]),
            self.detect_gain_points_for_band(&subbands[2]),
            self.detect_gain_points_for_band(&subbands[3]),
        ]
    }

    fn gain_extra_bits(&self, gains: &[Vec<GainPoint>; 4], qmf_bands: usize) -> usize {
        gains
            .iter()
            .take(qmf_bands)
            .map(|points| points.len() * 9)
            .sum()
    }

    fn tonal_components_enabled(&self) -> bool {
        self.config.experimental_tonal_components
    }

    fn detect_tonal_components(&self, spectrum: &[f32; 1024]) -> Vec<TonalComponent> {
        if !self.tonal_components_enabled() {
            return Vec::new();
        }

        let mut components = Vec::new();
        for bfu in 8..29 {
            let block_start = ATRAC3_SUBBAND_TAB[bfu];
            let block_end = ATRAC3_SUBBAND_TAB[bfu + 1];
            let block_len = block_end - block_start;
            if block_len == 0 {
                continue;
            }

            let flatness = self.spectral_flatness(&spectrum[block_start..block_end]);
            if flatness >= 0.01 {
                continue;
            }

            let mut best_start = None;
            let mut best_len = 1usize;
            let mut best_score = 0.0f32;
            let max_len = block_len.min(5);
            for start in block_start..block_end {
                let max_len_for_start = max_len.min(block_end - start);
                let mut score = 0.0f32;
                for len in 1..=max_len_for_start {
                    score += spectrum[start + len - 1].abs();
                    if score > best_score {
                        best_score = score;
                        best_start = Some(start);
                        best_len = len;
                    }
                }
            }

            let Some(abs_pos) = best_start else {
                continue;
            };
            let component_energy = spectrum[abs_pos..abs_pos + best_len]
                .iter()
                .map(|v| v * v)
                .sum::<f32>();
            if component_energy < 1.0e-10 {
                continue;
            }
            let max_val = spectrum[abs_pos..abs_pos + best_len]
                .iter()
                .fold(0.0f32, |acc, &v| acc.max(v.abs()));
            if max_val < 1.0e-7 {
                continue;
            }
            let sf_idx = self.sf_index_for_value(max_val);
            let sf = SF_TABLE[sf_idx];
            let quant_selector = 7;
            let max_quant = self.selector_max_quant(quant_selector);
            let quant_limit = self.selector_quant_limit(quant_selector);
            let mantissas = spectrum[abs_pos..abs_pos + best_len]
                .iter()
                .map(|&v| {
                    self.round_ties_even((v / sf).clamp(-0.99999, 0.99999) * max_quant)
                        .clamp(-(quant_limit as i32), quant_limit as i32) as i16
                })
                .collect::<Vec<_>>();

            components.push(TonalComponent {
                abs_pos,
                coded_values: best_len,
                quant_selector,
                sf_idx,
                energy: component_energy,
                mantissas,
            });
        }

        if let Some(protected) = self.concentrated_tonal_blocks(spectrum) {
            let total_energy = spectrum.iter().map(|v| v * v).sum::<f32>().max(1.0e-18);
            for spec_block in 0..4 {
                let block_start = spec_block * 64;
                let block_energy = spectrum[block_start..block_start + 64]
                    .iter()
                    .map(|v| v * v)
                    .sum::<f32>();
                let mut best_start = None;
                let mut best_len = 0usize;
                let mut best_score = 0.0f32;
                for coded_values in [2usize, 4] {
                    for rel in (4..=(64 - coded_values)).step_by(coded_values) {
                        let abs = block_start + rel;
                        let energy = spectrum[abs..abs + coded_values]
                            .iter()
                            .map(|v| v * v)
                            .sum::<f32>();
                        let score = energy / (coded_values as f32).sqrt();
                        if score > best_score {
                            best_score = score;
                            best_start = Some(abs);
                            best_len = coded_values;
                        }
                    }
                }

                let Some(abs_pos) = best_start else {
                    continue;
                };
                if components.iter().any(|component| {
                    abs_pos < component.abs_pos + component.coded_values
                        && component.abs_pos < abs_pos + best_len
                }) {
                    continue;
                }
                let component_energy = spectrum[abs_pos..abs_pos + best_len]
                    .iter()
                    .map(|v| v * v)
                    .sum::<f32>();
                let bfu = (0..32)
                    .find(|&idx| {
                        abs_pos >= ATRAC3_SUBBAND_TAB[idx] && abs_pos < ATRAC3_SUBBAND_TAB[idx + 1]
                    })
                    .unwrap_or(0);
                if !protected[bfu] || component_energy < 1.0e-10 {
                    continue;
                }
                if component_energy / block_energy.max(1.0e-18) < 0.55
                    || component_energy / total_energy < 0.015
                {
                    continue;
                }

                let max_val = spectrum[abs_pos..abs_pos + best_len]
                    .iter()
                    .fold(0.0f32, |acc, &v| acc.max(v.abs()));
                if max_val < 1.0e-7 {
                    continue;
                }
                let sf_idx = self.sf_index_for_value(max_val);
                let sf = SF_TABLE[sf_idx];
                let quant_selector = 7;
                let max_quant = self.selector_max_quant(quant_selector);
                let quant_limit = self.selector_quant_limit(quant_selector);
                let mantissas = spectrum[abs_pos..abs_pos + best_len]
                    .iter()
                    .map(|&v| {
                        self.round_ties_even((v / sf).clamp(-0.99999, 0.99999) * max_quant)
                            .clamp(-(quant_limit as i32), quant_limit as i32)
                            as i16
                    })
                    .collect::<Vec<_>>();

                components.push(TonalComponent {
                    abs_pos,
                    coded_values: best_len,
                    quant_selector,
                    sf_idx,
                    energy: component_energy,
                    mantissas,
                });
            }
        }

        components.sort_by(|a, b| b.energy.total_cmp(&a.energy));
        components.truncate(31);
        components.sort_by_key(|component| component.abs_pos);
        components
    }

    fn spectral_flatness(&self, spectrum: &[f32]) -> f32 {
        if spectrum.is_empty() {
            return 1.0;
        }

        let floor = 1.0e-12f64;
        let mut arith_mean = 0.0f64;
        let mut mean_log = 0.0f64;
        for &coeff in spectrum {
            let energy = (coeff * coeff).max(0.0) as f64;
            arith_mean += energy;
            mean_log += energy.max(floor).ln();
        }
        arith_mean /= spectrum.len() as f64;
        mean_log /= spectrum.len() as f64;
        if arith_mean <= floor {
            return 1.0;
        }

        ((mean_log.exp() / arith_mean).clamp(0.0, 1.0)) as f32
    }

    fn tonal_components_extra_bits(
        &self,
        components: &[TonalComponent],
        qmf_bands: usize,
    ) -> usize {
        if components.is_empty() {
            return 0;
        }

        let groups = self.tonal_component_groups(components);
        if groups.is_empty() {
            return 0;
        }

        let mut bits = 2;
        for group in groups {
            let mut active_spec_blocks = [false; 16];
            for component in &group {
                active_spec_blocks[component.abs_pos / 64] = true;
            }
            let active_qmf_bands = (0..qmf_bands)
                .filter(|&band| (0..4).any(|local| active_spec_blocks[band * 4 + local]))
                .count();

            let component_bits = group
                .iter()
                .map(|component| {
                    let mantissa_bits = match component.quant_selector {
                        1 => 4 * ((component.coded_values + 1) / 2),
                        2 | 3 => 3 * component.coded_values,
                        4 | 5 => 4 * component.coded_values,
                        6 => 5 * component.coded_values,
                        7 => 6 * component.coded_values,
                        _ => 0,
                    };
                    6 + 6 + mantissa_bits
                })
                .sum::<usize>();
            bits += qmf_bands + 3 + 3 + active_qmf_bands * 4 * 3 + component_bits;
        }

        bits
    }

    fn tonal_component_groups(&self, components: &[TonalComponent]) -> Vec<Vec<TonalComponent>> {
        let mut sorted = components.to_vec();
        sorted.sort_by(|a, b| {
            (a.quant_selector, a.coded_values, a.abs_pos).cmp(&(
                b.quant_selector,
                b.coded_values,
                b.abs_pos,
            ))
        });

        let mut groups: Vec<Vec<TonalComponent>> = Vec::new();
        for component in sorted {
            let spec_block = component.abs_pos / 64;
            let can_extend = groups.last().is_some_and(|group| {
                group[0].quant_selector == component.quant_selector
                    && group[0].coded_values == component.coded_values
                    && group
                        .iter()
                        .filter(|candidate| candidate.abs_pos / 64 == spec_block)
                        .count()
                        < 7
            });
            if can_extend {
                groups.last_mut().unwrap().push(component);
            } else {
                groups.push(vec![component]);
            }
            if groups.len() >= 31 {
                break;
            }
        }
        groups
    }

    fn remove_tonal_components_from_residual(
        &self,
        spectrum: &mut [f32; 1024],
        components: &[TonalComponent],
    ) {
        for component in components {
            let end = (component.abs_pos + component.mantissas.len()).min(spectrum.len());
            for coeff in &mut spectrum[component.abs_pos..end] {
                *coeff = 0.0;
            }
        }
    }

    fn apply_gain_modulation(
        &self,
        prev_windowed: &mut [f32; 256],
        cur_unwindowed: &mut [f32; 256],
        gains: &[GainPoint],
    ) {
        if gains.is_empty() {
            return;
        }

        let scale = self.gain_level(gains[0].0);
        let mut pos = 0usize;
        for (idx, &(level_idx, location)) in gains.iter().enumerate() {
            let last_pos = (location as usize) << ATRAC3_GAIN_LOC_SCALE;
            let next_level = gains
                .get(idx + 1)
                .map(|&(level, _)| level)
                .unwrap_or(ATRAC3_GAIN_NEUTRAL_LEVEL);
            let mut level = self.gain_level(level_idx);
            let gain_inc = self.gain_interp(level_idx, next_level);

            while pos < last_pos.min(256) {
                prev_windowed[pos] /= scale;
                cur_unwindowed[pos] /= level;
                pos += 1;
            }
            while pos < (last_pos + ATRAC3_GAIN_LOC_SIZE).min(256) {
                prev_windowed[pos] /= scale;
                cur_unwindowed[pos] /= level;
                level *= gain_inc;
                pos += 1;
            }
        }

        while pos < 256 {
            prev_windowed[pos] /= scale;
            pos += 1;
        }
    }

    fn store_quantized_channel_spectrum(&mut self, ch: usize, plans: &[BfuCoding]) {
        let mut flat = [0.0f32; 1024];
        for plan in plans {
            let start = ATRAC3_SUBBAND_TAB[plan.block];
            let end = ATRAC3_SUBBAND_TAB[plan.block + 1];
            flat[start..end].copy_from_slice(&plan.reconstructed);
        }
        for band in 0..4 {
            self.units[ch].quantized_spectra[band]
                .copy_from_slice(&flat[band * 256..(band + 1) * 256]);
        }
    }

    fn build_bfu_plan(
        &self,
        ch: usize,
        spectrum: &[f32; 1024],
        loudness_norm: f32,
        extra_header_bits: usize,
        prev_bfu_coded: Option<&[bool; 32]>,
    ) -> Vec<BfuCoding> {
        if self.stage_debug_enabled() {
            eprintln!("build_bfu_plan:start ch={}", ch);
        }
        #[derive(Clone, Copy)]
        struct BlockStats {
            max_val: f32,
            energy: f32,
            avg_abs: f32,
            sf_idx: usize,
            importance: f32,
        }

        let tonal_quant_boost = self.tonal_quant_boost();
        let tonal_frame_boost = if self.tonal_quant_boost_enabled()
            && self.concentrated_tonal_blocks(spectrum).is_some()
        {
            tonal_quant_boost
        } else {
            1.0
        };
        let boosted_spectrum;
        let planning_spectrum: &[f32; 1024] = if (tonal_frame_boost - 1.0).abs() > f32::EPSILON {
            boosted_spectrum = std::array::from_fn(|idx| spectrum[idx] * tonal_frame_boost);
            &boosted_spectrum
        } else {
            spectrum
        };

        let mut stats = Vec::with_capacity(32);
        let mut sf_indices = Vec::with_capacity(32);
        for block in 0..32 {
            let start = ATRAC3_SUBBAND_TAB[block];
            let end = ATRAC3_SUBBAND_TAB[block + 1];
            let slice = &planning_spectrum[start..end];

            let mut max_val = 0.0f32;
            let mut energy = 0.0f32;
            let mut avg_abs = 0.0f32;
            for &coeff in slice {
                let v = coeff.abs();
                max_val = max_val.max(v);
                energy += coeff * coeff;
                avg_abs += v;
            }
            avg_abs /= slice.len().max(1) as f32;
            let sf_idx = self.sf_index_for_value(max_val.max(1.0e-7));
            sf_indices.push(sf_idx);

            let importance = (energy.sqrt() + max_val * 8.0)
                * (1.0 + ATRAC3_FIXED_ALLOC_TABLE[block] * 0.2)
                * (1.0 + self.block_band(block) as f32 * 0.05);
            stats.push(BlockStats {
                max_val,
                energy,
                avg_abs,
                sf_idx,
                importance,
            });
        }

        let spread = self.analyze_scale_factor_spread(&sf_indices);
        let block_gain_boost: Vec<f32> = stats
            .iter()
            .enumerate()
            .map(|(block, stat)| {
                let avg_mag_metric = (stat.avg_abs.max(1.0e-12)).log10() + 6.0;
                let shape_bias =
                    (ATRAC3_SELECTOR_SHAPE_HINT[block] - ATRAC3_FIXED_ALLOC_TABLE[block]) * 0.35;
                self.importance_boost(stat.importance)
                    + self.avg_mag_bias(avg_mag_metric)
                    + shape_bias
            })
            .collect();

        let ath_gate_scale = self.ath_gate_scale();
        let build_with_shift = |shift: f32| {
            let mut plans = Vec::with_capacity(32);
            for block in 0..32 {
                let start = ATRAC3_SUBBAND_TAB[block];
                let end = ATRAC3_SUBBAND_TAB[block + 1];
                let slice = &planning_spectrum[start..end];
                let stat = stats[block];
                let band_boost = block_gain_boost[block];
                let mut table_idx = self.initial_selector_for_block(
                    stat.sf_idx,
                    stat.max_val,
                    block,
                    spread,
                    shift,
                    band_boost,
                );
                let mut skip_threshold = ATRAC3_ATH_BFU[block] * loudness_norm * ath_gate_scale;
                if let Some(prev) = prev_bfu_coded {
                    skip_threshold *= if prev[block] { 0.82 } else { 1.12 };
                }
                if stat.energy < skip_threshold {
                    table_idx = 0;
                }

                let mut plan = BfuCoding::new(
                    ch,
                    block,
                    stat.importance,
                    stat.max_val,
                    table_idx,
                    end - start,
                );
                if (tonal_frame_boost - 1.0).abs() > f32::EPSILON {
                    plan.sf_write_offset = -3;
                }
                self.refresh_bfu_plan(slice, &mut plan);
                plans.push(plan);
            }
            self.apply_channel_coding_mode(&mut plans);
            self.consider_energy_error(&mut plans, spectrum);
            plans
        };

        let channel_budget_bytes = self.block_align / self.channels as usize;
        let mut budget_bits = channel_budget_bytes
            .saturating_mul(8)
            .saturating_sub(extra_header_bits);
        if self.vlc_enabled() {
            budget_bits = budget_bits.saturating_sub(self.config.vlc_bit_safety);
        }
        let mut lo = -8.0f32;
        let mut hi = 20.0f32;
        let mut best_under: Option<Vec<BfuCoding>> = None;
        let mut best_over: Option<Vec<BfuCoding>> = None;

        for iter in 0..24 {
            let mid = (lo + hi) * 0.5;
            let plans = build_with_shift(mid);
            let bits = self.bfu_plan_bits(&plans);
            if self.stage_debug_enabled() {
                eprintln!(
                    "build_bfu_plan:shift ch={} iter={} mid={} bits={}",
                    ch, iter, mid, bits
                );
            }
            if bits > budget_bits {
                best_over = Some(plans);
                lo = mid;
            } else {
                best_under = Some(plans);
                hi = mid;
            }
        }

        let mut plans = best_under
            .or(best_over)
            .unwrap_or_else(|| build_with_shift(0.0));
        if self.config.enable_tail_prune {
            self.prune_weak_tail_bfus(planning_spectrum, &mut plans);
        }
        if self.config.experimental_concentrate_bits {
            self.prune_low_precision_tail_bfus(planning_spectrum, &mut plans);
        }
        if self.config.experimental_tonal_prune {
            self.prune_concentrated_tonal_bfus(planning_spectrum, &mut plans);
        }
        self.fit_bfu_plan_to_budget(planning_spectrum, &mut plans, budget_bits);
        if self.config.enable_tail_sf_smooth {
            self.smooth_tail_scalefactors(planning_spectrum, &mut plans);
        }
        if !self.config.disable_reference_shape {
            self.nudge_bfu_plan_to_reference_shape(planning_spectrum, &mut plans, budget_bits);
        }
        if self.config.experimental_rebalance_bfus {
            self.rebalance_bfu_precision(planning_spectrum, &mut plans, budget_bits);
        }
        if self.config.cap_high_bfus {
            self.clear_high_bfus(&mut plans, 30);
        }
        if self.config.enable_tail_sf_smooth {
            self.smooth_tail_scalefactors(planning_spectrum, &mut plans);
        }
        self.fit_bfu_plan_to_budget(planning_spectrum, &mut plans, budget_bits);

        self.apply_channel_coding_mode(&mut plans);
        if self.stage_debug_enabled() {
            eprintln!(
                "build_bfu_plan:done ch={} extra_header_bits={}",
                ch, extra_header_bits
            );
        }

        plans
    }

    fn clear_high_bfus(&self, plans: &mut [BfuCoding], first_block: usize) {
        for plan in plans.iter_mut().skip(first_block) {
            plan.table_idx = 0;
            plan.sf_idx = 0;
            plan.mantissas.fill(0);
            plan.vlc_symbols.fill(0);
            plan.reconstructed.fill(0.0);
            plan.bit_count = 0;
            plan.vlc_bits = 0;
            plan.clc_bits = 0;
        }
        self.apply_channel_coding_mode(plans);
    }

    pub fn debug_first_frame_plan(&mut self, pcm_in: &[i16]) -> Vec<DebugChannelPlan> {
        self.debug_first_frame_analysis(pcm_in).plans
    }

    pub fn debug_first_frame_analysis(&mut self, pcm_in: &[i16]) -> DebugFrameAnalysis {
        let mut spectra = [[[0.0f32; 256]; 4]; 2];
        let mut subbands = [[[0.0f32; 256]; 4]; 2];

        let mut planar_pcm = vec![vec![0.0f32; 1024]; self.channels as usize];
        for ch in 0..self.channels as usize {
            for i in 0..1024 {
                planar_pcm[ch][i] = pcm_in[i * self.channels as usize + ch] as f32 / 32768.0;
            }
        }

        let analysis_scale = self.analysis_spectrum_scale();
        for ch in 0..self.channels as usize {
            self.analyze_frame(&planar_pcm[ch], &mut subbands[ch], ch);
            for band in 0..4 {
                let odd_band = (band & 1) != 0;
                let mut mdct_in = [0.0; 512];
                mdct_in[..256].copy_from_slice(&self.units[ch].mdct_buf[band]);
                for i in 0..256 {
                    let cur = subbands[ch][band][i];
                    mdct_in[256 + i] = ENCODE_WINDOW[255 - i] * cur;
                }

                let mut spectrum = [0.0; 256];
                self.mdct_calc_forward(&mdct_in, &mut spectrum, odd_band);
                for coeff in &mut spectrum {
                    *coeff *= analysis_scale;
                }
                spectra[ch][band] = spectrum;
            }
        }

        let mut plans_out = Vec::with_capacity(self.channels as usize);
        let mut channel_analysis = Vec::with_capacity(self.channels as usize);

        let flat_spectra: Vec<[f32; 1024]> = (0..self.channels as usize)
            .map(|ch| self.flatten_channel_spectrum(&spectra[ch]))
            .collect();
        let per_channel_loudness: Vec<f32> = flat_spectra
            .iter()
            .map(|spec| self.frame_channel_loudness(spec))
            .collect();
        let loudness_norm = if self.channels == 2 && per_channel_loudness.len() >= 2 {
            (0.98 * self.loudness_state
                + 0.01 * (per_channel_loudness[0] + per_channel_loudness[1]))
                / ATRAC3_LOUDNESS_FACTOR
        } else {
            (0.98 * self.loudness_state + 0.02 * per_channel_loudness[0]) / ATRAC3_LOUDNESS_FACTOR
        };

        for ch in 0..self.channels as usize {
            let flat_spectrum = flat_spectra[ch];
            let plans = self.build_bfu_plan(ch, &flat_spectrum, loudness_norm.max(0.001), 0, None);

            let (pcm_rms, pcm_max) = self.rms_and_max(&planar_pcm[ch]);
            let mut bands = Vec::with_capacity(4);
            for band in 0..4 {
                let (subband_rms, subband_max) = self.rms_and_max(&subbands[ch][band]);
                let (mdct_rms, mdct_max) = self.rms_and_max(&spectra[ch][band]);
                bands.push(DebugBandMetrics {
                    band,
                    subband_rms,
                    subband_max,
                    mdct_rms,
                    mdct_max,
                });
            }
            channel_analysis.push(DebugChannelAnalysis {
                channel: ch,
                pcm_rms,
                pcm_max,
                bands,
            });
            plans_out.push(self.debug_channel_plan_from_bfus(ch, &flat_spectrum, &plans));
        }

        DebugFrameAnalysis {
            channels: channel_analysis,
            plans: plans_out,
        }
    }

    fn fit_bfu_plan_to_budget(
        &self,
        spectrum: &[f32; 1024],
        plans: &mut [BfuCoding],
        budget_bits: usize,
    ) {
        for _ in 0..512 {
            let total_bits = self.bfu_plan_bits(plans);
            if total_bits > budget_bits {
                let mut candidate = None;
                let mut best_score = f32::INFINITY;
                for (idx, plan) in plans.iter().enumerate() {
                    if plan.table_idx == 0 {
                        continue;
                    }

                    let mut downgraded = plan.clone();
                    downgraded.table_idx -= 1;
                    let start = ATRAC3_SUBBAND_TAB[downgraded.block];
                    let end = ATRAC3_SUBBAND_TAB[downgraded.block + 1];
                    self.refresh_bfu_plan(&spectrum[start..end], &mut downgraded);
                    if downgraded.bit_count >= plan.bit_count {
                        continue;
                    }

                    let saved = (plan.bit_count - downgraded.bit_count).max(1) as f32;
                    let old_error = self.bfu_block_weighted_error(&spectrum[start..end], plan);
                    let new_error =
                        self.bfu_block_weighted_error(&spectrum[start..end], &downgraded);
                    let score = (new_error - old_error).max(0.0) / saved;
                    if score < best_score {
                        best_score = score;
                        candidate = Some((idx, downgraded));
                    }
                }
                let Some((idx, downgraded)) = candidate else {
                    break;
                };
                plans[idx] = downgraded;
                self.apply_channel_coding_mode(plans);
            } else {
                let remaining = budget_bits - total_bits;
                let mut candidate = None;
                let mut best_score = 0.0f32;
                for (idx, plan) in plans.iter().enumerate() {
                    if plan.table_idx == 0 || plan.table_idx >= 7 || plan.max_val < 1.0e-7 {
                        continue;
                    }
                    let mut upgraded = plan.clone();
                    upgraded.table_idx += 1;
                    let start = ATRAC3_SUBBAND_TAB[upgraded.block];
                    let end = ATRAC3_SUBBAND_TAB[upgraded.block + 1];
                    self.refresh_bfu_plan(&spectrum[start..end], &mut upgraded);
                    if upgraded.bit_count <= plan.bit_count {
                        continue;
                    }
                    let delta = upgraded.bit_count - plan.bit_count;
                    if delta > remaining {
                        continue;
                    }
                    let old_error = self.bfu_block_weighted_error(&spectrum[start..end], plan);
                    let new_error = self.bfu_block_weighted_error(&spectrum[start..end], &upgraded);
                    let score = (old_error - new_error).max(0.0) / delta as f32;
                    if score > best_score {
                        best_score = score;
                        candidate = Some((idx, upgraded));
                    }
                }
                let Some((idx, upgraded)) = candidate else {
                    break;
                };
                plans[idx] = upgraded;
            }
        }
    }

    fn bfu_block_weighted_error(&self, spectrum: &[f32], plan: &BfuCoding) -> f32 {
        let mut distortion = 0.0f32;
        let mut source_energy = 0.0f32;
        let mut recon_energy = 0.0f32;
        for (idx, &orig) in spectrum.iter().enumerate() {
            let recon = plan.reconstructed.get(idx).copied().unwrap_or(0.0);
            let diff = orig - recon;
            distortion += diff * diff;
            source_energy += orig * orig;
            recon_energy += recon * recon;
        }

        let (detail_weight, energy_weight) = self.texture_error_weights(spectrum);
        let energy_error = self.asymmetric_energy_log_error(source_energy, recon_energy);
        let normalized_energy_error = energy_error * source_energy.max(1.0e-18);
        (detail_weight * distortion + energy_weight * normalized_energy_error)
            * (1.0 + plan.importance)
    }

    fn asymmetric_energy_log_error(&self, source_energy: f32, recon_energy: f32) -> f32 {
        if source_energy <= 1.0e-18 {
            return 0.0;
        }

        let log_ratio = (recon_energy.max(1.0e-18) / source_energy).log2();
        if log_ratio < 0.0 {
            log_ratio.abs() * 1.3
        } else {
            log_ratio * 0.95
        }
    }

    fn texture_error_weights(&self, spectrum: &[f32]) -> (f32, f32) {
        if spectrum.is_empty() {
            return (1.0, 0.35);
        }

        let mut energy = 0.0f32;
        let mut peak = 0.0f32;
        for &coeff in spectrum {
            let e = coeff * coeff;
            energy += e;
            peak = peak.max(e);
        }
        if energy <= 1.0e-18 {
            return (1.0, 0.35);
        }

        let flatness = self.spectral_flatness(spectrum);
        let crest = (peak / (energy / spectrum.len() as f32)).max(1.0);
        let peakiness = ((crest.log2() - 1.0) / 5.0).clamp(0.0, 1.0);
        let tonality = (0.65 * (1.0 - flatness) + 0.35 * peakiness).clamp(0.0, 1.0);

        let detail_weight = 0.85 + 0.35 * tonality;
        let energy_weight = 0.25 + 0.20 * flatness + 0.10 * (1.0 - tonality);
        (detail_weight, energy_weight)
    }

    fn bfu_plan_bits(&self, plans: &[BfuCoding]) -> usize {
        let num_blocks = plans
            .iter()
            .rposition(|p| p.table_idx != 0)
            .map(|v| v + 1)
            .unwrap_or(1);
        let qmf_bands = self.header_qmf_band_count(num_blocks);
        let mut total = 6 + 2 + qmf_bands * 3 + 5 + 5 + 1 + num_blocks * 3;
        for plan in plans.iter().take(num_blocks) {
            if plan.table_idx != 0 {
                total += 6 + plan.bit_count;
            }
        }
        total
    }

    fn header_qmf_band_count(&self, num_blocks: usize) -> usize {
        if self.config.static_qmf_bands {
            return 4;
        }

        for band_count in 1..=4 {
            if num_blocks <= ATRAC3_BLOCKS_PER_BAND[band_count] {
                return band_count;
            }
        }
        4
    }

    fn consider_energy_error(&self, plans: &mut [BfuCoding], spectrum: &[f32; 1024]) {
        loop {
            let mut adjusted = false;
            let lim = plans.len().min(10);
            for idx in 0..lim {
                let energy_err = plans[idx].energy_err;
                if ((energy_err > 0.0 && energy_err < 0.7) || energy_err > 1.2)
                    && plans[idx].table_idx > 0
                    && plans[idx].table_idx < 7
                {
                    plans[idx].table_idx += 1;
                    let start = ATRAC3_SUBBAND_TAB[plans[idx].block];
                    let end = ATRAC3_SUBBAND_TAB[plans[idx].block + 1];
                    self.refresh_bfu_plan(&spectrum[start..end], &mut plans[idx]);
                    adjusted = true;
                }
            }
            if !adjusted {
                break;
            }
        }
        self.apply_channel_coding_mode(plans);
    }

    fn channel_prefers_vlc(&self, plans: &[BfuCoding]) -> bool {
        if self.config.force_clc {
            return false;
        }
        if !self.vlc_enabled() {
            return false;
        }
        if let Some(max_selector) = self.config.vlc_max_selector {
            if plans
                .iter()
                .any(|plan| plan.table_idx != 0 && plan.table_idx > max_selector)
            {
                return false;
            }
        }

        let num_blocks = plans
            .iter()
            .rposition(|p| p.table_idx != 0)
            .map(|v| v + 1)
            .unwrap_or(1);
        let mut spectral_clc_bits = 0usize;
        let mut spectral_vlc_bits = 0usize;

        for plan in plans.iter().take(num_blocks) {
            if plan.table_idx == 0 {
                continue;
            }
            spectral_clc_bits += plan.clc_bits;
            spectral_vlc_bits += plan.vlc_bits;
        }

        spectral_vlc_bits < spectral_clc_bits
    }

    fn apply_channel_coding_mode(&self, plans: &mut [BfuCoding]) {
        let use_vlc = self.channel_prefers_vlc(plans);
        for plan in plans {
            plan.bit_count = if use_vlc {
                plan.vlc_bits
            } else {
                plan.clc_bits
            };
        }
    }

    fn prune_weak_tail_bfus(&self, spectrum: &[f32; 1024], plans: &mut [BfuCoding]) {
        let num_blocks = plans
            .iter()
            .rposition(|p| p.table_idx != 0)
            .map(|v| v + 1)
            .unwrap_or(0);
        if num_blocks <= 1 {
            return;
        }

        for idx in 1..num_blocks {
            let prev = plans[idx - 1].table_idx;
            if plans[idx].table_idx > prev {
                plans[idx].table_idx = prev;
                let start = ATRAC3_SUBBAND_TAB[idx];
                let end = ATRAC3_SUBBAND_TAB[idx + 1];
                self.refresh_bfu_plan(&spectrum[start..end], &mut plans[idx]);
            }
        }

        self.apply_channel_coding_mode(plans);
    }

    fn prune_low_precision_tail_bfus(&self, spectrum: &[f32; 1024], plans: &mut [BfuCoding]) {
        let total_energy = spectrum.iter().map(|v| v * v).sum::<f32>().max(1.0e-18);
        for idx in 10..plans.len() {
            if plans[idx].table_idx == 0 {
                continue;
            }
            let start = ATRAC3_SUBBAND_TAB[idx];
            let end = ATRAC3_SUBBAND_TAB[idx + 1];
            let block_energy = spectrum[start..end].iter().map(|v| v * v).sum::<f32>();
            let relative_energy = block_energy / total_energy;
            let low_precision = plans[idx].table_idx <= if idx >= 18 { 2 } else { 1 };
            if low_precision && relative_energy < 0.015 {
                plans[idx].table_idx = 0;
                plans[idx].sf_idx = 0;
                plans[idx].mantissas.fill(0);
                plans[idx].vlc_symbols.fill(0);
                plans[idx].reconstructed.fill(0.0);
                plans[idx].bit_count = 0;
                plans[idx].vlc_bits = 0;
                plans[idx].clc_bits = 0;
            }
        }

        self.apply_channel_coding_mode(plans);
    }

    fn concentrated_tonal_blocks(&self, spectrum: &[f32; 1024]) -> Option<[bool; 32]> {
        let mut block_energy = [0.0f32; 32];
        for block in 0..32 {
            let start = ATRAC3_SUBBAND_TAB[block];
            let end = ATRAC3_SUBBAND_TAB[block + 1];
            block_energy[block] = spectrum[start..end].iter().map(|v| v * v).sum();
        }

        let total_energy = block_energy.iter().sum::<f32>().max(1.0e-18);
        let mut ranked: Vec<(usize, f32)> = block_energy.iter().copied().enumerate().collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
        let dominant_energy = ranked
            .iter()
            .take(4)
            .map(|(_, energy)| *energy)
            .sum::<f32>();
        if dominant_energy / total_energy < 0.82 {
            return None;
        }

        let mut protected = [false; 32];
        for &(idx, _) in ranked.iter().take(4) {
            protected[idx] = true;
        }
        Some(protected)
    }

    fn prune_concentrated_tonal_bfus(&self, spectrum: &[f32; 1024], plans: &mut [BfuCoding]) {
        let mut block_energy = [0.0f32; 32];
        for block in 0..32 {
            let start = ATRAC3_SUBBAND_TAB[block];
            let end = ATRAC3_SUBBAND_TAB[block + 1];
            block_energy[block] = spectrum[start..end].iter().map(|v| v * v).sum();
        }

        let total_energy = block_energy.iter().sum::<f32>().max(1.0e-18);
        let Some(protected) = self.concentrated_tonal_blocks(spectrum) else {
            return;
        };
        let max_energy = block_energy
            .iter()
            .copied()
            .fold(0.0f32, f32::max)
            .max(1.0e-18);
        for idx in 0..plans.len() {
            if plans[idx].table_idx == 0 {
                continue;
            }
            if protected[idx] {
                continue;
            }

            let relative_total = block_energy[idx] / total_energy;
            let relative_peak = block_energy[idx] / max_energy;
            if relative_total >= 0.010 || relative_peak >= 0.020 {
                continue;
            }

            plans[idx].table_idx = 0;
            plans[idx].sf_idx = 0;
            plans[idx].mantissas.fill(0);
            plans[idx].vlc_symbols.fill(0);
            plans[idx].reconstructed.fill(0.0);
            plans[idx].bit_count = 0;
            plans[idx].vlc_bits = 0;
            plans[idx].clc_bits = 0;
        }

        self.apply_channel_coding_mode(plans);
    }

    fn smooth_tail_scalefactors(&self, spectrum: &[f32; 1024], plans: &mut [BfuCoding]) {
        for idx in 14..plans.len() {
            if plans[idx].table_idx == 0 || plans[idx - 1].table_idx == 0 {
                continue;
            }
            let max_sf = plans[idx - 1].sf_idx.saturating_sub(1);
            if plans[idx].sf_idx > max_sf {
                let start = ATRAC3_SUBBAND_TAB[plans[idx].block];
                let end = ATRAC3_SUBBAND_TAB[plans[idx].block + 1];
                plans[idx].sf_idx = max_sf;
                self.requantize_bfu_plan_with_sf(&spectrum[start..end], &mut plans[idx]);
            }
        }
        self.apply_channel_coding_mode(plans);
    }

    fn nudge_bfu_plan_to_reference_shape(
        &self,
        spectrum: &[f32; 1024],
        plans: &mut [BfuCoding],
        budget_bits: usize,
    ) {
        for idx in 0..plans.len() {
            let target = ATRAC3_SELECTOR_SHAPE_HINT[idx] as usize;
            while target > 0 && plans[idx].table_idx > target {
                plans[idx].table_idx -= 1;
                let start = ATRAC3_SUBBAND_TAB[plans[idx].block];
                let end = ATRAC3_SUBBAND_TAB[plans[idx].block + 1];
                self.refresh_bfu_plan(&spectrum[start..end], &mut plans[idx]);
            }
        }
        self.apply_channel_coding_mode(plans);

        for idx in 0..plans.len() {
            let target = ATRAC3_SELECTOR_SHAPE_HINT[idx] as usize;
            while plans[idx].table_idx > 0 && plans[idx].table_idx < target {
                let mut candidate = plans[idx].clone();
                candidate.table_idx += 1;
                let start = ATRAC3_SUBBAND_TAB[candidate.block];
                let end = ATRAC3_SUBBAND_TAB[candidate.block + 1];
                self.refresh_bfu_plan(&spectrum[start..end], &mut candidate);

                let old = std::mem::replace(&mut plans[idx], candidate);
                self.apply_channel_coding_mode(plans);
                if self.bfu_plan_bits(plans) > budget_bits {
                    plans[idx] = old;
                    self.apply_channel_coding_mode(plans);
                    break;
                }
            }
        }
    }

    fn bfu_plan_weighted_error(&self, spectrum: &[f32; 1024], plans: &[BfuCoding]) -> f32 {
        let mut error = 0.0f32;
        for plan in plans {
            let start = ATRAC3_SUBBAND_TAB[plan.block];
            let end = ATRAC3_SUBBAND_TAB[plan.block + 1];
            let mut distortion = 0.0f32;
            for (idx, &orig) in spectrum[start..end].iter().enumerate() {
                let recon = plan.reconstructed.get(idx).copied().unwrap_or(0.0);
                let diff = orig - recon;
                distortion += diff * diff;
            }
            error += distortion * (1.0 + plan.importance);
        }
        error
    }

    fn rebalance_bfu_precision(
        &self,
        spectrum: &[f32; 1024],
        plans: &mut [BfuCoding],
        budget_bits: usize,
    ) {
        for _ in 0..5 {
            let baseline_error = self.bfu_plan_weighted_error(spectrum, plans);
            let mut best: Option<(f32, Vec<BfuCoding>)> = None;
            let mut donors: Vec<(f32, usize)> = plans
                .iter()
                .enumerate()
                .filter(|(_, plan)| plan.table_idx != 0)
                .map(|(idx, plan)| {
                    let start = ATRAC3_SUBBAND_TAB[plan.block];
                    let end = ATRAC3_SUBBAND_TAB[plan.block + 1];
                    let energy = spectrum[start..end].iter().map(|v| v * v).sum::<f32>();
                    let score = energy * (1.0 + plan.importance) / plan.bit_count.max(1) as f32;
                    (score, idx)
                })
                .collect();
            donors.sort_by(|a, b| a.0.total_cmp(&b.0));

            for &(_, donor) in donors.iter().take(10) {
                if plans[donor].table_idx == 0 {
                    continue;
                }

                let mut candidate = plans.to_vec();
                candidate[donor].table_idx -= 1;
                let start = ATRAC3_SUBBAND_TAB[candidate[donor].block];
                let end = ATRAC3_SUBBAND_TAB[candidate[donor].block + 1];
                self.refresh_bfu_plan(&spectrum[start..end], &mut candidate[donor]);
                self.apply_channel_coding_mode(&mut candidate);
                self.fit_bfu_plan_to_budget(spectrum, &mut candidate, budget_bits);

                if self.bfu_plan_bits(&candidate) > budget_bits {
                    continue;
                }

                let candidate_error = self.bfu_plan_weighted_error(spectrum, &candidate);
                if candidate_error < baseline_error * 0.995
                    && best
                        .as_ref()
                        .map(|(best_error, _)| candidate_error < *best_error)
                        .unwrap_or(true)
                {
                    best = Some((candidate_error, candidate));
                }
            }

            let Some((_, next)) = best else {
                break;
            };
            plans.clone_from_slice(&next);
        }
    }

    fn best_sf_idx_for_selector(&self, _spectrum: &[f32], table_idx: usize, max_val: f32) -> usize {
        if table_idx == 0 || max_val < 1.0e-7 {
            return 0;
        }
        self.sf_index_for_value(max_val)
    }

    fn refresh_bfu_plan(&self, spectrum: &[f32], plan: &mut BfuCoding) {
        plan.bit_count = 0;
        plan.vlc_bits = 0;
        plan.clc_bits = 0;
        plan.energy_err = 1.0;
        if plan.table_idx == 0 || plan.max_val < 1.0e-7 {
            plan.table_idx = 0;
            plan.sf_idx = 0;
            plan.mantissas.fill(0);
            plan.vlc_symbols.fill(0);
            plan.reconstructed.fill(0.0);
            return;
        }

        let sf_idx = self.best_sf_idx_for_selector(spectrum, plan.table_idx, plan.max_val);
        plan.sf_idx = sf_idx;
        self.requantize_bfu_plan_with_sf(spectrum, plan);
    }

    fn requantize_bfu_plan_with_sf(&self, spectrum: &[f32], plan: &mut BfuCoding) {
        plan.bit_count = 0;
        plan.vlc_bits = 0;
        plan.clc_bits = 0;
        plan.energy_err = 1.0;
        plan.mantissas.fill(0);
        plan.vlc_symbols.fill(0);
        plan.reconstructed.fill(0.0);
        if plan.table_idx == 0 {
            return;
        }

        let max_quant = self.selector_max_quant(plan.table_idx);
        let quant_limit = self.selector_quant_limit(plan.table_idx);
        let (quantized, energy_err) = if let Some((sf_idx, quantized, energy_err)) =
            self.search_scale_factor_for_plan(spectrum, plan, max_quant, quant_limit)
        {
            plan.sf_idx = sf_idx;
            (quantized, energy_err)
        } else {
            loop {
                let sf = SF_TABLE[plan.sf_idx];
                let scaled: Vec<f32> = spectrum
                    .iter()
                    .map(|&v| (v / sf).clamp(-0.99999, 0.99999))
                    .collect();
                let (quantized, energy_err) =
                    self.quantize_for_plan(&scaled, plan.block, max_quant, quant_limit);

                if !self.tail_sf_needs_headroom(plan, &quantized, quant_limit) || plan.sf_idx >= 63
                {
                    break (quantized, energy_err);
                }
                plan.sf_idx += 1;
            }
        };
        plan.energy_err = energy_err;
        let sf_offset = self.experimental_sf_write_offset();
        let total_sf_offset = sf_offset + plan.sf_write_offset;
        if total_sf_offset != 0 {
            plan.sf_idx = (plan.sf_idx as i32 + total_sf_offset).clamp(0, 63) as usize;
        }
        let sf = SF_TABLE[plan.sf_idx];

        let clc_bits_per_coeff = match plan.table_idx {
            1 => 4,
            2 | 3 => 3,
            4 | 5 => 4,
            6 => 5,
            7 => 6,
            _ => 0,
        };

        if plan.table_idx == 1 {
            let vlc_table = &crate::huffman::SPECTRAL_VLC[0];
            for pair in 0..(spectrum.len() / 2) {
                let qa = quantized[pair * 2];
                let qb = quantized[pair * 2 + 1];
                let huff_symbol = self.selector1_vlc_symbol(qa, qb);
                plan.mantissas[pair * 2] = qa;
                plan.mantissas[pair * 2 + 1] = qb;
                plan.vlc_symbols[pair * 2] = huff_symbol;
                plan.vlc_bits += vlc_table.entries[huff_symbol as usize].len as usize;
                plan.reconstructed[pair * 2] = qa as f32 * sf / max_quant;
                plan.reconstructed[pair * 2 + 1] = qb as f32 * sf / max_quant;
            }
            plan.clc_bits = clc_bits_per_coeff * (spectrum.len() / 2);
        } else {
            let vlc_table = &crate::huffman::SPECTRAL_VLC[plan.table_idx - 1];
            for (i, _) in spectrum.iter().enumerate() {
                let q = quantized[i];
                let huff_symbol = self.scalar_vlc_symbol(q);
                plan.mantissas[i] = q;
                plan.vlc_symbols[i] = huff_symbol;
                plan.vlc_bits += vlc_table.entries[huff_symbol as usize].len as usize;
                plan.reconstructed[i] = q as f32 * sf / max_quant;
            }
            plan.clc_bits = clc_bits_per_coeff * spectrum.len();
        }
        plan.bit_count = plan.clc_bits;
    }

    fn tail_sf_needs_headroom(
        &self,
        plan: &BfuCoding,
        quantized: &[i16],
        quant_limit: i16,
    ) -> bool {
        if plan.block < 16 || plan.table_idx > 3 || quant_limit <= 0 {
            return false;
        }

        let saturated = quantized
            .iter()
            .filter(|&&q| q.abs() >= quant_limit)
            .count();
        let threshold = if plan.table_idx == 2 { 12 } else { 10 };
        saturated > threshold
    }

    fn write_channel_sound_unit(
        &self,
        ch: usize,
        plans: &[BfuCoding],
        gains: &[Vec<GainPoint>; 4],
        tonal_components: &[TonalComponent],
        budget_bytes: usize,
    ) -> Vec<u8> {
        let mut bw = crate::huffman::BitWriter::new();
        bw.write_bits(0x28, 6);
        let num_blocks = plans
            .iter()
            .rposition(|p| p.table_idx != 0)
            .map(|v| v + 1)
            .unwrap_or(1);
        let qmf_bands = self.header_qmf_band_count(num_blocks);
        bw.write_bits((qmf_bands - 1) as u32, 2);
        for points in gains.iter().take(qmf_bands) {
            bw.write_bits(points.len().min(7) as u32, 3);
            for &(level, location) in points.iter().take(7) {
                bw.write_bits(level.min(15) as u32, 4);
                bw.write_bits(location.min(31) as u32, 5);
            }
        }
        self.write_tonal_components(tonal_components, qmf_bands, &mut bw);
        let spectral_clc_bits: usize = plans
            .iter()
            .take(num_blocks)
            .filter(|plan| plan.table_idx != 0)
            .map(|plan| plan.clc_bits)
            .sum();
        let spectral_vlc_bits: usize = plans
            .iter()
            .take(num_blocks)
            .filter(|plan| plan.table_idx != 0)
            .map(|plan| plan.vlc_bits)
            .sum();
        let _spectral_vlc_bits = spectral_vlc_bits;
        let _spectral_clc_bits = spectral_clc_bits;
        let coding_mode = if self.channel_prefers_vlc(plans) {
            0
        } else {
            1
        };
        bw.write_bits((num_blocks - 1) as u32, 5);
        bw.write_bits(coding_mode, 1);
        for plan in plans.iter().take(num_blocks) {
            bw.write_bits(plan.table_idx as u32, 3);
        }
        for plan in plans.iter().take(num_blocks) {
            if plan.table_idx != 0 {
                bw.write_bits(plan.sf_idx as u32, 6);
            }
        }
        for plan in plans.iter().take(num_blocks) {
            if plan.table_idx == 0 {
                continue;
            }
            if coding_mode == 1 {
                self.write_bfu_clc(plan, &mut bw);
            } else {
                let vlc_table = &crate::huffman::SPECTRAL_VLC[plan.table_idx - 1];
                if plan.table_idx == 1 {
                    for pair in 0..(plan.mantissas.len() / 2) {
                        let symbol = plan.vlc_symbols[pair * 2] as usize;
                        let entry = &vlc_table.entries[symbol];
                        bw.write_bits(entry.code, entry.len as usize);
                    }
                } else {
                    for &symbol in &plan.vlc_symbols[..plan.mantissas.len()] {
                        let entry = &vlc_table.entries[symbol as usize];
                        bw.write_bits(entry.code, entry.len as usize);
                    }
                }
            }
            debug_assert!(plan.ch == ch);
        }
        let mut out = bw.flush().to_vec();
        if self.stage_debug_enabled() {
            eprintln!(
                "write_channel_sound_unit ch={} mode={} num_bfu={} qmf_bands={} bits={} budget_bits={} spectral_clc={} spectral_vlc={}",
                ch,
                coding_mode,
                num_blocks,
                qmf_bands,
                bw.bits_written(),
                budget_bytes * 8,
                spectral_clc_bits,
                spectral_vlc_bits
            );
        }
        out.resize(budget_bytes, 0);
        out
    }

    fn write_tonal_components(
        &self,
        components: &[TonalComponent],
        qmf_bands: usize,
        bw: &mut crate::huffman::BitWriter,
    ) {
        if components.is_empty() {
            bw.write_bits(0, 5);
            return;
        }

        let groups = self.tonal_component_groups(components);
        if groups.is_empty() {
            bw.write_bits(0, 5);
            return;
        }

        bw.write_bits(groups.len().min(31) as u32, 5);
        bw.write_bits(1, 2);

        for components in groups.iter().take(31) {
            let mut qmf_flags = vec![false; qmf_bands];
            let mut counts = [0usize; 16];
            for component in components {
                let spec_block = component.abs_pos / 64;
                let qmf_band = spec_block / 4;
                if qmf_band < qmf_bands {
                    qmf_flags[qmf_band] = true;
                    counts[spec_block] += 1;
                }
            }

            for &flag in &qmf_flags {
                bw.write_bits(u32::from(flag), 1);
            }
            bw.write_bits((components[0].coded_values - 1) as u32, 3);
            bw.write_bits(components[0].quant_selector as u32, 3);

            for qmf_band in 0..qmf_bands {
                if !qmf_flags[qmf_band] {
                    continue;
                }
                for local_block in 0..4 {
                    let spec_block = qmf_band * 4 + local_block;
                    bw.write_bits(counts[spec_block].min(7) as u32, 3);
                    for component in components
                        .iter()
                        .filter(|component| component.abs_pos / 64 == spec_block)
                        .take(7)
                    {
                        bw.write_bits(component.sf_idx as u32, 6);
                        bw.write_bits((component.abs_pos % 64) as u32, 6);
                        let mantissa_bits = match component.quant_selector {
                            1 => 4,
                            2 | 3 => 3,
                            4 | 5 => 4,
                            6 => 5,
                            7 => 6,
                            _ => 0,
                        };
                        if component.quant_selector == 1 {
                            for pair in component.mantissas.chunks(2) {
                                let a = pair.first().copied().unwrap_or(0);
                                let b = pair.get(1).copied().unwrap_or(0);
                                bw.write_bits(self.selector1_clc_pair_bits(a, b), 4);
                            }
                        } else {
                            for &mantissa in component.mantissas.iter().take(component.coded_values)
                            {
                                bw.write_bits(
                                    self.make_signed_bits(mantissa, mantissa_bits),
                                    mantissa_bits,
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    fn selector1_vlc_symbol(&self, a: i16, b: i16) -> u8 {
        match (a, b) {
            (0, 0) => 0,
            (0, 1) => 1,
            (0, -1) => 2,
            (1, 0) => 3,
            (-1, 0) => 4,
            (1, 1) => 5,
            (1, -1) => 6,
            (-1, 1) => 7,
            (-1, -1) => 8,
            _ => 0,
        }
    }

    fn scalar_vlc_symbol(&self, q: i16) -> u8 {
        let mut sym = if q < 0 {
            (((-q) as u8) << 1) | 1
        } else {
            (q as u8) << 1
        };
        if sym != 0 {
            sym -= 1;
        }
        sym
    }

    fn make_signed_bits(&self, val: i16, bits: usize) -> u32 {
        let shift = 32 - bits;
        (((val as i32) << shift) >> shift) as u32 & ((1u32 << bits) - 1)
    }

    fn clc_pair_code(&self, mantissa: i16) -> u32 {
        match mantissa {
            -2 => 2,
            -1 => 3,
            0 => 0,
            1 => 1,
            _ => 0,
        }
    }

    fn selector1_clc_pair_bits(&self, a: i16, b: i16) -> u32 {
        (self.clc_pair_code(a) << 2) | self.clc_pair_code(b)
    }

    fn write_bfu_clc(&self, plan: &BfuCoding, bw: &mut crate::huffman::BitWriter) {
        match plan.table_idx {
            1 => {
                for pair in 0..(plan.mantissas.len() / 2) {
                    let code = (self.clc_pair_code(plan.mantissas[pair * 2]) << 2)
                        | self.clc_pair_code(plan.mantissas[pair * 2 + 1]);
                    bw.write_bits(code, 4);
                }
            }
            2 | 3 => {
                for &mantissa in &plan.mantissas {
                    bw.write_bits(self.make_signed_bits(mantissa, 3), 3);
                }
            }
            4 | 5 => {
                for &mantissa in &plan.mantissas {
                    bw.write_bits(self.make_signed_bits(mantissa, 4), 4);
                }
            }
            6 => {
                for &mantissa in &plan.mantissas {
                    bw.write_bits(self.make_signed_bits(mantissa, 5), 5);
                }
            }
            7 => {
                for &mantissa in &plan.mantissas {
                    bw.write_bits(self.make_signed_bits(mantissa, 6), 6);
                }
            }
            _ => {}
        }
    }
}

fn pqf_forward(
    in_pcm: &[f32],
    lower: &mut [f32],
    upper: &mut [f32],
    delay_buf: &mut [f32; 46],
    temp: &mut [f32],
) {
    let n_in = in_pcm.len();

    temp[..46].copy_from_slice(delay_buf);

    temp[46..46 + n_in].copy_from_slice(in_pcm);

    let window = &*QMF_WINDOW;

    for j in (0..n_in).step_by(2) {
        let mut s_low = 0.0;
        let mut s_high = 0.0;

        for i in 0..24 {
            let idx1 = 47 + j - (2 * i);
            let idx2 = idx1 - 1;

            s_low += window[2 * i] * temp[idx1];
            s_high += window[(2 * i) + 1] * temp[idx2];
        }

        lower[j / 2] = s_low + s_high;
        upper[j / 2] = s_low - s_high;
    }

    delay_buf.copy_from_slice(&temp[n_in..n_in + 46]);
}

fn iqmf(
    in_lo: &[f32],
    in_hi: &[f32],
    n_in: usize,
    p_out: &mut [f32],
    delay_buf: &mut [f32; 46],
    temp: &mut [f32],
) {
    temp[..46].copy_from_slice(delay_buf);

    for i in (0..n_in).step_by(2) {
        temp[46 + i * 2 + 0] = in_lo[i] + in_hi[i];
        temp[46 + i * 2 + 1] = in_lo[i] - in_hi[i];
        temp[46 + i * 2 + 2] = in_lo[i + 1] + in_hi[i + 1];
        temp[46 + i * 2 + 3] = in_lo[i + 1] - in_hi[i + 1];
    }

    for j in 0..n_in {
        let mut s1 = 0.0;
        let mut s2 = 0.0;

        let window = &*QMF_WINDOW;
        for i in (0..48).step_by(2) {
            s1 += temp[j * 2 + i] * window[i];
            s2 += temp[j * 2 + i + 1] * window[i + 1];
        }

        p_out[j * 2 + 0] = s2;
        p_out[j * 2 + 1] = s1;
    }

    delay_buf.copy_from_slice(&temp[n_in * 2..n_in * 2 + 46]);
}
