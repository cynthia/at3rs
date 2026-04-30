use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use crate::atrac3::Atrac3Context;

pub const DEFAULT_SEARCH_WINDOW: usize = 4096;

#[derive(Debug, Clone)]
pub struct EvalMetrics {
    pub samples_compared: usize,
    pub best_offset: usize,
    pub snr_db: f64,
    pub psnr_db: f64,
    pub rmse: f64,
    pub max_abs_error: i16,
}

pub fn read_wav_samples(path: impl AsRef<Path>) -> io::Result<Vec<i16>> {
    let mut f = File::open(path)?;
    f.seek(SeekFrom::Start(44))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;

    let mut pcm = Vec::with_capacity(buf.len() / 2);
    for chunk in buf.chunks_exact(2) {
        pcm.push(i16::from_le_bytes([chunk[0], chunk[1]]));
    }
    Ok(pcm)
}

pub fn atrac3_roundtrip_pcm(input: &[i16], channels: u16, block_align: usize) -> Vec<i16> {
    let frame_len = 1024 * channels as usize;
    let mut output = vec![0i16; input.len()];
    let mut ctx = Atrac3Context::new(channels, block_align);

    for frame_start in (0..input.len()).step_by(frame_len) {
        let frame_end = (frame_start + frame_len).min(input.len());
        let mut in_frame = vec![0i16; frame_len];
        in_frame[..frame_end - frame_start].copy_from_slice(&input[frame_start..frame_end]);

        let mut bitstream = vec![0u8; block_align];
        let mut out_frame = vec![0i16; frame_len];
        ctx.encode_frame(&in_frame, &mut bitstream);
        ctx.decode_frame(&bitstream, &mut out_frame);

        output[frame_start..frame_end].copy_from_slice(&out_frame[..frame_end - frame_start]);
    }

    output
}

pub fn evaluate_pair(original: &[i16], reconstructed: &[i16], search_window: usize) -> EvalMetrics {
    let len = original.len().min(reconstructed.len());
    if len == 0 {
        return EvalMetrics {
            samples_compared: 0,
            best_offset: 0,
            snr_db: 0.0,
            psnr_db: 0.0,
            rmse: 0.0,
            max_abs_error: 0,
        };
    }

    let mut best = EvalMetrics {
        samples_compared: 0,
        best_offset: 0,
        snr_db: f64::NEG_INFINITY,
        psnr_db: 0.0,
        rmse: 0.0,
        max_abs_error: 0,
    };

    for offset in 0..search_window.min(len) {
        let compare_len = len - offset;
        let mut sig_power = 0.0f64;
        let mut noise_power = 0.0f64;
        let mut max_abs_error = 0i16;

        for i in 0..compare_len {
            let orig = original[i] as f64;
            let recon = reconstructed[i + offset] as f64;
            let diff = orig - recon;
            let abs_err = diff.abs().round().clamp(0.0, i16::MAX as f64) as i16;

            sig_power += orig * orig;
            noise_power += diff * diff;
            max_abs_error = max_abs_error.max(abs_err);
        }

        let snr_db = if noise_power == 0.0 {
            f64::INFINITY
        } else {
            10.0 * (sig_power / noise_power).log10()
        };

        if snr_db > best.snr_db {
            let rmse = (noise_power / compare_len as f64).sqrt();
            let psnr_db = if rmse == 0.0 {
                f64::INFINITY
            } else {
                20.0 * ((i16::MAX as f64) / rmse).log10()
            };

            best = EvalMetrics {
                samples_compared: compare_len,
                best_offset: offset,
                snr_db,
                psnr_db,
                rmse,
                max_abs_error,
            };
        }
    }

    best
}

pub fn evaluate_atrac3_roundtrip_file(
    path: impl AsRef<Path>,
    channels: u16,
    block_align: usize,
) -> io::Result<EvalMetrics> {
    let input = read_wav_samples(path)?;
    let reconstructed = atrac3_roundtrip_pcm(&input, channels, block_align);
    Ok(evaluate_pair(&input, &reconstructed, DEFAULT_SEARCH_WINDOW))
}
