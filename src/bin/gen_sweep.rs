use std::env;
use std::f64::consts::TAU;
use std::fs::File;
use std::io::{self, Write};

const SAMPLE_RATE: u32 = 44_100;
const CHANNELS: u16 = 2;
const BITS_PER_SAMPLE: u16 = 16;

fn write_le_u16<W: Write>(w: &mut W, v: u16) -> io::Result<()> {
    w.write_all(&v.to_le_bytes())
}

fn write_le_u32<W: Write>(w: &mut W, v: u32) -> io::Result<()> {
    w.write_all(&v.to_le_bytes())
}

fn write_wav(path: &str, samples: &[i16]) -> io::Result<()> {
    let mut f = File::create(path)?;
    let byte_rate = SAMPLE_RATE * CHANNELS as u32 * (BITS_PER_SAMPLE as u32 / 8);
    let block_align = CHANNELS * (BITS_PER_SAMPLE / 8);
    let data_size = (samples.len() * 2) as u32;

    f.write_all(b"RIFF")?;
    write_le_u32(&mut f, 36 + data_size)?;
    f.write_all(b"WAVE")?;
    f.write_all(b"fmt ")?;
    write_le_u32(&mut f, 16)?;
    write_le_u16(&mut f, 1)?;
    write_le_u16(&mut f, CHANNELS)?;
    write_le_u32(&mut f, SAMPLE_RATE)?;
    write_le_u32(&mut f, byte_rate)?;
    write_le_u16(&mut f, block_align)?;
    write_le_u16(&mut f, BITS_PER_SAMPLE)?;
    f.write_all(b"data")?;
    write_le_u32(&mut f, data_size)?;

    for sample in samples {
        f.write_all(&sample.to_le_bytes())?;
    }
    Ok(())
}

fn parse_arg(args: &[String], idx: usize, default: f64) -> f64 {
    args.get(idx)
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(default)
}

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!(
            "Usage: gen_sweep <output.wav> [seconds=30] [start_hz=20] [end_hz=20000] [amplitude=0.45]"
        );
        std::process::exit(2);
    }

    let path = &args[1];
    let seconds = parse_arg(&args, 2, 30.0).max(0.1);
    let start_hz = parse_arg(&args, 3, 20.0).max(1.0);
    let end_hz = parse_arg(&args, 4, 20_000.0).min(SAMPLE_RATE as f64 * 0.49);
    let amplitude = parse_arg(&args, 5, 0.45).clamp(0.0, 0.95);
    let frames = (seconds * SAMPLE_RATE as f64).round() as usize;
    let sweep_rate = (end_hz / start_hz).ln() / seconds;

    let mut samples = Vec::with_capacity(frames * CHANNELS as usize);
    for n in 0..frames {
        let t = n as f64 / SAMPLE_RATE as f64;
        let phase = TAU * start_hz * ((sweep_rate * t).exp() - 1.0) / sweep_rate;
        let sample = (phase.sin() * amplitude * i16::MAX as f64).round() as i16;
        samples.push(sample);
        samples.push(sample);
    }

    write_wav(path, &samples)?;
    println!(
        "wrote {}: {:.2}s log sweep {:.1}Hz..{:.1}Hz amplitude {:.2}",
        path, seconds, start_hz, end_hz, amplitude
    );
    Ok(())
}
