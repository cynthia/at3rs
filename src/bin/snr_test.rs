use std::env;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

const SEARCH_WINDOW_FRAMES: isize = 8192;
const CORR_STRIDE_FRAMES: usize = 16;

#[derive(Clone, Debug)]
struct WavPcm {
    samples: Vec<i16>,
    channels: usize,
}

// Simple PCM16 WAV reader for the project-generated test files.
fn read_wav(path: &str) -> WavPcm {
    let mut f = File::open(path).unwrap_or_else(|_| panic!("Could not open {}", path));
    let mut riff = [0u8; 12];
    f.read_exact(&mut riff).expect("Could not read WAV header");
    assert_eq!(&riff[0..4], b"RIFF", "{path}: missing RIFF header");
    assert_eq!(&riff[8..12], b"WAVE", "{path}: missing WAVE header");

    let mut channels = None;
    let mut bits_per_sample = None;
    let mut data = None;

    loop {
        let mut chunk_header = [0u8; 8];
        match f.read_exact(&mut chunk_header) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(err) => panic!("{path}: could not read WAV chunk header: {err}"),
        }

        let chunk_id = &chunk_header[0..4];
        let chunk_len = u32::from_le_bytes([
            chunk_header[4],
            chunk_header[5],
            chunk_header[6],
            chunk_header[7],
        ]) as usize;
        let payload_pos = f.stream_position().expect("Could not get chunk position");

        match chunk_id {
            b"fmt " => {
                let mut fmt = vec![0u8; chunk_len];
                f.read_exact(&mut fmt).expect("Could not read fmt chunk");
                assert!(fmt.len() >= 16, "{path}: fmt chunk too short");
                let audio_format = u16::from_le_bytes([fmt[0], fmt[1]]);
                channels = Some(u16::from_le_bytes([fmt[2], fmt[3]]) as usize);
                bits_per_sample = Some(u16::from_le_bytes([fmt[14], fmt[15]]));
                assert_eq!(audio_format, 1, "{path}: only PCM WAV is supported");
            }
            b"data" => {
                let mut buf = vec![0u8; chunk_len];
                f.read_exact(&mut buf).expect("Could not read data chunk");
                data = Some(buf);
            }
            _ => {
                f.seek(SeekFrom::Start(payload_pos + chunk_len as u64))
                    .expect("Could not skip WAV chunk");
            }
        }

        if chunk_len % 2 != 0 {
            f.seek(SeekFrom::Current(1))
                .expect("Could not skip WAV pad byte");
        }
    }

    let channels = channels.unwrap_or_else(|| panic!("{path}: missing fmt chunk"));
    let bits_per_sample = bits_per_sample.unwrap_or_else(|| panic!("{path}: missing fmt chunk"));
    assert_eq!(bits_per_sample, 16, "{path}: only PCM16 WAV is supported");
    let buf = data.unwrap_or_else(|| panic!("{path}: missing data chunk"));

    let mut samples = Vec::with_capacity(buf.len() / 2);
    for i in (0..buf.len()).step_by(2) {
        if i + 1 < buf.len() {
            samples.push(i16::from_le_bytes([buf[i], buf[i + 1]]));
        }
    }

    WavPcm { samples, channels }
}

#[derive(Clone, Copy, Debug)]
struct PairMetrics {
    offset_frames: isize,
    samples: usize,
    correlation: f64,
    snr_db: f64,
    gain_adjusted_snr_db: f64,
    gain: f64,
}

fn overlap_for_offset(
    orig_frames: usize,
    recon_frames: usize,
    offset_frames: isize,
) -> Option<(usize, usize, usize)> {
    let orig_start = if offset_frames < 0 {
        (-offset_frames) as usize
    } else {
        0
    };
    let recon_start = if offset_frames > 0 {
        offset_frames as usize
    } else {
        0
    };
    if orig_start >= orig_frames || recon_start >= recon_frames {
        return None;
    }

    let len = (orig_frames - orig_start).min(recon_frames - recon_start);
    (len > 0).then_some((orig_start, recon_start, len))
}

fn correlation_at_offset(original: &WavPcm, reconstructed: &WavPcm, offset_frames: isize) -> f64 {
    if original.channels != reconstructed.channels {
        return f64::NEG_INFINITY;
    }
    let channels = original.channels;
    let Some((orig_start, recon_start, len)) = overlap_for_offset(
        original.samples.len() / channels,
        reconstructed.samples.len() / channels,
        offset_frames,
    ) else {
        return f64::NEG_INFINITY;
    };

    let mut dot = 0.0f64;
    let mut orig_power = 0.0f64;
    let mut recon_power = 0.0f64;
    for frame in (0..len).step_by(CORR_STRIDE_FRAMES) {
        let orig_idx = (orig_start + frame) * channels;
        let recon_idx = (recon_start + frame) * channels;
        for ch in 0..channels {
            let orig = original.samples[orig_idx + ch] as f64;
            let recon = reconstructed.samples[recon_idx + ch] as f64;
            dot += orig * recon;
            orig_power += orig * orig;
            recon_power += recon * recon;
        }
    }

    if orig_power == 0.0 || recon_power == 0.0 {
        f64::NEG_INFINITY
    } else {
        dot / (orig_power.sqrt() * recon_power.sqrt())
    }
}

fn metrics_at_offset(
    original: &WavPcm,
    reconstructed: &WavPcm,
    offset_frames: isize,
    correlation: f64,
) -> PairMetrics {
    let channels = original.channels;
    let Some((orig_start, recon_start, len)) = overlap_for_offset(
        original.samples.len() / channels,
        reconstructed.samples.len() / channels,
        offset_frames,
    ) else {
        return PairMetrics {
            offset_frames,
            samples: 0,
            correlation,
            snr_db: f64::NEG_INFINITY,
            gain_adjusted_snr_db: f64::NEG_INFINITY,
            gain: 0.0,
        };
    };

    let mut sig_power = 0.0f64;
    let mut recon_power = 0.0f64;
    let mut dot = 0.0f64;
    let mut noise_power = 0.0f64;

    for frame in 0..len {
        let orig_idx = (orig_start + frame) * channels;
        let recon_idx = (recon_start + frame) * channels;
        for ch in 0..channels {
            let orig = original.samples[orig_idx + ch] as f64;
            let recon = reconstructed.samples[recon_idx + ch] as f64;
            sig_power += orig * orig;
            recon_power += recon * recon;
            dot += orig * recon;
            let diff = orig - recon;
            noise_power += diff * diff;
        }
    }

    let gain = if recon_power > 0.0 {
        dot / recon_power
    } else {
        0.0
    };
    let mut gain_adjusted_noise_power = 0.0f64;
    for frame in 0..len {
        let orig_idx = (orig_start + frame) * channels;
        let recon_idx = (recon_start + frame) * channels;
        for ch in 0..channels {
            let orig = original.samples[orig_idx + ch] as f64;
            let recon = reconstructed.samples[recon_idx + ch] as f64 * gain;
            let diff = orig - recon;
            gain_adjusted_noise_power += diff * diff;
        }
    }

    PairMetrics {
        offset_frames,
        samples: len * channels,
        correlation,
        snr_db: snr_from_power(sig_power, noise_power),
        gain_adjusted_snr_db: snr_from_power(sig_power, gain_adjusted_noise_power),
        gain,
    }
}

fn snr_from_power(signal: f64, noise: f64) -> f64 {
    if noise == 0.0 {
        f64::INFINITY
    } else if signal == 0.0 {
        f64::NEG_INFINITY
    } else {
        10.0 * (signal / noise).log10()
    }
}

fn calculate_metrics(original: &WavPcm, reconstructed: &WavPcm) -> PairMetrics {
    assert_eq!(
        original.channels, reconstructed.channels,
        "channel count mismatch"
    );
    if original.samples.is_empty() || reconstructed.samples.is_empty() {
        return PairMetrics {
            offset_frames: 0,
            samples: 0,
            correlation: 0.0,
            snr_db: 0.0,
            gain_adjusted_snr_db: 0.0,
            gain: 0.0,
        };
    }

    let mut best_offset = 0;
    let mut best_corr = f64::NEG_INFINITY;
    for offset in -SEARCH_WINDOW_FRAMES..=SEARCH_WINDOW_FRAMES {
        let corr = correlation_at_offset(original, reconstructed, offset);
        if corr > best_corr {
            best_corr = corr;
            best_offset = offset;
        }
    }

    metrics_at_offset(original, reconstructed, best_offset, best_corr)
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        println!("Usage: snr_test <original.wav> <decoded.wav>");
        return;
    }

    println!("Loading original: {}", args[1]);
    let original = read_wav(&args[1]);

    println!("Loading decoded: {}", args[2]);
    let reconstructed = read_wav(&args[2]);

    let metrics = calculate_metrics(&original, &reconstructed);
    let offset_samples = metrics.offset_frames * original.channels as isize;

    println!("------------------------------------------------");
    println!("Original Samples: {}", original.samples.len());
    println!("Decoded Samples:  {}", reconstructed.samples.len());
    println!(
        "Best alignment offset: {} samples ({} frames)",
        offset_samples, metrics.offset_frames
    );
    println!("Compared Samples: {}", metrics.samples);
    println!("Correlation:      {:.6}", metrics.correlation);
    println!("Round-trip SNR:   {:.2} dB", metrics.snr_db);
    println!("Gain-adjusted SNR:{:>8.2} dB", metrics.gain_adjusted_snr_db);
    println!("Best gain:        {:.6}", metrics.gain);
    println!("------------------------------------------------");
}
