use std::fs::File;
use std::io::{self, Write};
use std::path::Path;

use crate::atrac3::Atrac3Context;
use crate::riff::{read_pcm_wav, write_at3_riff, LoopPoints, WavPcm};
use crate::ATRAC3_SAMPLES_PER_FRAME;

pub use crate::atrac3::config::{EncoderConfig, EncoderQuality};

/// Options for encoding PCM WAV input into RIFF/WAVE ATRAC3.
#[derive(Clone, Debug)]
pub struct EncodeOptions {
    /// Target ATRAC3 bitrate in kbps. Values are rounded to the nearest legal
    /// ATRAC3 frame budget for the channel count.
    pub bitrate_kbps: u32,
    /// Optional RIFF `smpl` loop points in PCM sample units.
    pub loop_points: Option<LoopPoints>,
    /// Optional frame limit for short debug encodes and tests.
    pub max_frames: Option<usize>,
    /// Explicit encoder tuning. Defaults are deterministic.
    pub config: EncoderConfig,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        Self {
            bitrate_kbps: 132,
            loop_points: None,
            max_frames: None,
            config: EncoderConfig::default(),
        }
    }
}

/// Summary returned after an encode operation.
#[derive(Clone, Debug)]
pub struct EncodeSummary {
    pub frames: usize,
    pub channels: u16,
    pub sample_rate: u32,
    pub block_align: usize,
    pub valid_samples_per_channel: u32,
}

/// ATRAC3 encoder facade used by the CLI and tests.
///
/// The encoder accepts 16-bit PCM WAV input at 44.1 kHz and writes Sony-tool
/// compatible RIFF/WAVE ATRAC3 output.
pub struct Encoder {
    options: EncodeOptions,
}

impl Encoder {
    pub fn new(options: EncodeOptions) -> Self {
        Self { options }
    }

    pub fn encode_wav_file(
        &self,
        input: impl AsRef<Path>,
        output: impl AsRef<Path>,
    ) -> io::Result<EncodeSummary> {
        let wav = read_pcm_wav(input)?;
        let (summary, frames) = self.encode_wav(&wav)?;

        let mut fout = File::create(output)?;
        write_at3_riff(
            &mut fout,
            summary.channels,
            summary.sample_rate,
            summary.block_align as u16,
            summary.valid_samples_per_channel,
            summary.frames as u32,
            self.options.loop_points,
        )?;
        for frame in &frames {
            fout.write_all(frame)?;
        }

        Ok(summary)
    }

    pub fn encode_wav(&self, wav: &WavPcm) -> io::Result<(EncodeSummary, Vec<Vec<u8>>)> {
        if wav.sample_rate != 44_100 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "only 44.1 kHz PCM WAV input is supported",
            ));
        }
        if !(wav.channels == 1 || wav.channels == 2) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "only mono and stereo WAV input are supported",
            ));
        }

        let block_align =
            choose_atrac3_block_align(wav.channels, wav.sample_rate, self.options.bitrate_kbps);
        let samples_per_frame = ATRAC3_SAMPLES_PER_FRAME * wav.channels as usize;
        let valid_samples_per_channel = (wav.samples.len() / wav.channels as usize) as u32;
        let mut num_frames = wav.samples.len().div_ceil(samples_per_frame);
        if let Some(limit) = self.options.max_frames.filter(|limit| *limit > 0) {
            num_frames = num_frames.min(limit);
        }

        let mut ctx =
            Atrac3Context::with_config(wav.channels, block_align, self.options.config.clone());
        let mut frames = Vec::with_capacity(num_frames);

        for frame_idx in 0..num_frames {
            let start = frame_idx * samples_per_frame;
            let end = (start + samples_per_frame).min(wav.samples.len());
            let mut pcm_frame = vec![0i16; samples_per_frame];
            pcm_frame[..end - start].copy_from_slice(&wav.samples[start..end]);

            let mut encoded = vec![0u8; block_align];
            let written = ctx.encode_frame(&pcm_frame, &mut encoded);
            if written > block_align {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "encoded frame exceeded block alignment budget",
                ));
            }
            frames.push(encoded);
        }

        Ok((
            EncodeSummary {
                frames: num_frames,
                channels: wav.channels,
                sample_rate: wav.sample_rate,
                block_align,
                valid_samples_per_channel,
            },
            frames,
        ))
    }
}

pub fn choose_atrac3_block_align(channels: u16, sample_rate: u32, bitrate_kbps: u32) -> usize {
    let candidates =
        [96usize, 152, 192].map(|bytes_per_channel| bytes_per_channel * channels as usize);
    let target = ((bitrate_kbps as u64 * 1000 * ATRAC3_SAMPLES_PER_FRAME as u64)
        / (sample_rate as u64 * 8)) as i64;

    candidates
        .into_iter()
        .min_by_key(|candidate| (target - *candidate as i64).abs())
        .unwrap_or(192 * channels as usize)
}
