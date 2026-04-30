use std::env;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};

#[derive(Debug)]
struct Wav {
    sample_rate: u32,
    channels: usize,
    pcm: Vec<f64>,
}

fn read_u16(buf: &[u8], pos: usize) -> u16 {
    u16::from_le_bytes([buf[pos], buf[pos + 1]])
}

fn read_u32(buf: &[u8], pos: usize) -> u32 {
    u32::from_le_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]])
}

fn read_wav(path: &str) -> io::Result<Wav> {
    let mut f = File::open(path)?;
    let mut header = [0u8; 12];
    f.read_exact(&mut header)?;
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a RIFF/WAVE file",
        ));
    }

    let mut sample_rate = 0;
    let mut channels = 0usize;
    let mut data = Vec::new();

    loop {
        let mut chunk_header = [0u8; 8];
        match f.read_exact(&mut chunk_header) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e),
        }
        let id = &chunk_header[0..4];
        let size = read_u32(&chunk_header, 4) as usize;
        let mut chunk = vec![0u8; size];
        f.read_exact(&mut chunk)?;
        if size & 1 != 0 {
            f.seek(SeekFrom::Current(1))?;
        }

        match id {
            b"fmt " => {
                let format = read_u16(&chunk, 0);
                channels = read_u16(&chunk, 2) as usize;
                sample_rate = read_u32(&chunk, 4);
                let bits_per_sample = read_u16(&chunk, 14);
                if format != 1 || bits_per_sample != 16 || channels == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "only 16-bit PCM WAV is supported",
                    ));
                }
            }
            b"data" => data = chunk,
            _ => {}
        }
    }

    if sample_rate == 0 || data.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "missing WAV fmt/data",
        ));
    }

    let mut pcm = Vec::with_capacity(data.len() / (2 * channels));
    for frame in data.chunks_exact(2 * channels) {
        let mut sum = 0.0;
        for ch in 0..channels {
            let pos = ch * 2;
            let sample = i16::from_le_bytes([frame[pos], frame[pos + 1]]) as f64;
            sum += sample;
        }
        pcm.push(sum / channels as f64);
    }

    Ok(Wav {
        sample_rate,
        channels,
        pcm,
    })
}

fn snr_db(signal: f64, noise: f64) -> f64 {
    if noise <= 0.0 {
        f64::INFINITY
    } else if signal <= 0.0 {
        f64::NEG_INFINITY
    } else {
        10.0 * (signal / noise).log10()
    }
}

fn best_decoded_offset(reference: &[f64], decoded: &[f64], max_offset: usize) -> usize {
    let len = reference.len().min(decoded.len());
    let eval_len = len.min(262_144);
    let mut best_offset = 0;
    let mut best_snr = f64::NEG_INFINITY;

    for offset in 0..max_offset.min(len) {
        let calc_len = eval_len.saturating_sub(offset);
        if calc_len < 4096 {
            break;
        }

        let mut signal = 0.0;
        let mut noise = 0.0;
        for i in 0..calc_len {
            let r = reference[i];
            let d = decoded[i + offset];
            signal += r * r;
            let e = r - d;
            noise += e * e;
        }

        let score = snr_db(signal, noise);
        if score > best_snr {
            best_snr = score;
            best_offset = offset;
        }
    }

    best_offset
}

fn parse_arg(args: &[String], idx: usize, default: f64) -> f64 {
    args.get(idx)
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(default)
}

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 4 {
        eprintln!(
            "Usage: sweep_metrics <reference.wav> <decoded.wav> <output.csv> [seconds=30] [start_hz=20] [end_hz=20000]"
        );
        std::process::exit(2);
    }

    let reference = read_wav(&args[1])?;
    let decoded = read_wav(&args[2])?;
    if reference.sample_rate != decoded.sample_rate {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "reference/decoded sample rates differ",
        ));
    }

    let seconds = parse_arg(&args, 4, 30.0);
    let start_hz = parse_arg(&args, 5, 20.0);
    let end_hz = parse_arg(&args, 6, 20_000.0);
    let rate = reference.sample_rate as f64;
    let offset = best_decoded_offset(&reference.pcm, &decoded.pcm, 4096);
    let usable = reference
        .pcm
        .len()
        .min(decoded.pcm.len().saturating_sub(offset));
    let window = 4096usize;
    let hop = 2048usize;
    let sweep_rate = (end_hz / start_hz).ln() / seconds;
    let mut out = File::create(&args[3])?;

    writeln!(
        out,
        "# reference={}, decoded={}, sample_rate={}, reference_channels={}, decoded_channels={}, alignment_offset_samples={}",
        args[1], args[2], reference.sample_rate, reference.channels, decoded.channels, offset
    )?;
    writeln!(out, "time_sec,frequency_hz,snr_db,rmse,peak_error")?;

    let mut worst_snr = f64::INFINITY;
    let mut worst_freq = 0.0;
    let mut best_snr = f64::NEG_INFINITY;
    let mut n = 0usize;
    let mut pos = 0usize;

    while pos + window <= usable {
        let mut signal = 0.0;
        let mut noise = 0.0;
        let mut peak_error = 0.0f64;
        for i in 0..window {
            let r = reference.pcm[pos + i];
            let d = decoded.pcm[offset + pos + i];
            signal += r * r;
            let e = r - d;
            noise += e * e;
            peak_error = peak_error.max(e.abs());
        }

        let center_time = (pos + window / 2) as f64 / rate;
        let freq = start_hz * (sweep_rate * center_time).exp();
        let snr = snr_db(signal, noise);
        let rmse = (noise / window as f64).sqrt();
        writeln!(
            out,
            "{:.6},{:.3},{:.3},{:.6},{:.6}",
            center_time, freq, snr, rmse, peak_error
        )?;

        if snr < worst_snr {
            worst_snr = snr;
            worst_freq = freq;
        }
        if snr > best_snr {
            best_snr = snr;
        }
        n += 1;
        pos += hop;
    }

    println!(
        "wrote {}: offset={} windows={} worst_snr={:.2}dB at {:.1}Hz best_snr={:.2}dB",
        args[3], offset, n, worst_snr, worst_freq, best_snr
    );

    Ok(())
}
