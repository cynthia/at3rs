use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::ATRAC3_SAMPLES_PER_FRAME;

#[derive(Clone, Copy, Debug)]
pub struct LoopPoints {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug)]
pub struct WavPcm {
    pub channels: u16,
    pub sample_rate: u32,
    pub samples: Vec<i16>,
}

#[derive(Debug)]
pub struct Atrac3Riff {
    pub channels: u16,
    pub sample_rate: u32,
    pub block_align: u16,
    pub total_samples_per_channel: Option<u32>,
    pub data: Vec<u8>,
}

pub fn read_atrac3_riff(path: impl AsRef<Path>) -> io::Result<Atrac3Riff> {
    let mut fin = File::open(path)?;

    let mut riff_header = [0u8; 12];
    fin.read_exact(&mut riff_header)?;
    if &riff_header[0..4] != b"RIFF" || &riff_header[8..12] != b"WAVE" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a RIFF/WAVE file",
        ));
    }

    let mut audio_format = 0u16;
    let mut channels = 0u16;
    let mut sample_rate = 0u32;
    let mut block_align = 0u16;
    let mut total_samples_per_channel = None;
    let mut data = Vec::new();

    loop {
        let mut chunk_header = [0u8; 8];
        if fin.read_exact(&mut chunk_header).is_err() {
            break;
        }

        let chunk_id = &chunk_header[0..4];
        let chunk_size = u32::from_le_bytes(chunk_header[4..8].try_into().unwrap()) as usize;
        let padded_size = chunk_size + (chunk_size & 1);

        match chunk_id {
            b"fmt " => {
                let mut fmt = vec![0u8; chunk_size];
                fin.read_exact(&mut fmt)?;
                if fmt.len() < 14 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "truncated fmt chunk",
                    ));
                }
                audio_format = u16::from_le_bytes(fmt[0..2].try_into().unwrap());
                channels = u16::from_le_bytes(fmt[2..4].try_into().unwrap());
                sample_rate = u32::from_le_bytes(fmt[4..8].try_into().unwrap());
                block_align = u16::from_le_bytes(fmt[12..14].try_into().unwrap());
                if padded_size > chunk_size {
                    fin.seek(SeekFrom::Current(1))?;
                }
            }
            b"fact" => {
                let mut fact = vec![0u8; chunk_size];
                fin.read_exact(&mut fact)?;
                if fact.len() >= 4 {
                    total_samples_per_channel =
                        Some(u32::from_le_bytes(fact[0..4].try_into().unwrap()));
                }
                if padded_size > chunk_size {
                    fin.seek(SeekFrom::Current(1))?;
                }
            }
            b"data" => {
                data = vec![0u8; chunk_size];
                fin.read_exact(&mut data)?;
                if padded_size > chunk_size {
                    fin.seek(SeekFrom::Current(1))?;
                }
            }
            _ => {
                fin.seek(SeekFrom::Current(padded_size as i64))?;
            }
        }
    }

    if audio_format != 0x0270 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsupported WAVE format 0x{audio_format:04x}; expected ATRAC3"),
        ));
    }
    if channels == 0 || sample_rate == 0 || block_align == 0 || data.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "missing fmt or data chunk in ATRAC3 input",
        ));
    }

    Ok(Atrac3Riff {
        channels,
        sample_rate,
        block_align,
        total_samples_per_channel,
        data,
    })
}

pub fn read_pcm_wav(path: impl AsRef<Path>) -> io::Result<WavPcm> {
    let mut fin = File::open(path)?;

    let mut riff_header = [0u8; 12];
    fin.read_exact(&mut riff_header)?;
    if &riff_header[0..4] != b"RIFF" || &riff_header[8..12] != b"WAVE" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a RIFF/WAVE file",
        ));
    }

    let mut audio_format = 0u16;
    let mut channels = 0u16;
    let mut sample_rate = 0u32;
    let mut bits_per_sample = 0u16;
    let mut sample_data = Vec::new();

    loop {
        let mut chunk_header = [0u8; 8];
        if fin.read_exact(&mut chunk_header).is_err() {
            break;
        }

        let chunk_id = &chunk_header[0..4];
        let chunk_size = u32::from_le_bytes(chunk_header[4..8].try_into().unwrap()) as usize;
        let padded_size = chunk_size + (chunk_size & 1);

        match chunk_id {
            b"fmt " => {
                let mut fmt = vec![0u8; chunk_size];
                fin.read_exact(&mut fmt)?;
                if fmt.len() < 16 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "truncated fmt chunk",
                    ));
                }
                audio_format = u16::from_le_bytes(fmt[0..2].try_into().unwrap());
                channels = u16::from_le_bytes(fmt[2..4].try_into().unwrap());
                sample_rate = u32::from_le_bytes(fmt[4..8].try_into().unwrap());
                bits_per_sample = u16::from_le_bytes(fmt[14..16].try_into().unwrap());
                if padded_size > chunk_size {
                    fin.seek(SeekFrom::Current(1))?;
                }
            }
            b"data" => {
                let mut data = vec![0u8; chunk_size];
                fin.read_exact(&mut data)?;
                sample_data = data;
                if padded_size > chunk_size {
                    fin.seek(SeekFrom::Current(1))?;
                }
            }
            _ => {
                fin.seek(SeekFrom::Current(padded_size as i64))?;
            }
        }
    }

    if audio_format != 1 || bits_per_sample != 16 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "only 16-bit PCM WAV input is supported",
        ));
    }
    if channels == 0 || sample_rate == 0 || sample_data.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "missing fmt or data chunk in WAV input",
        ));
    }

    let mut samples = Vec::with_capacity(sample_data.len() / 2);
    for chunk in sample_data.chunks_exact(2) {
        samples.push(i16::from_le_bytes([chunk[0], chunk[1]]));
    }

    Ok(WavPcm {
        channels,
        sample_rate,
        samples,
    })
}

pub fn write_pcm_wav(
    path: impl AsRef<Path>,
    channels: u16,
    sample_rate: u32,
    samples: &[i16],
) -> io::Result<()> {
    let mut fout = File::create(path)?;
    let bits_per_sample = 16u16;
    let block_align = channels * (bits_per_sample / 8);
    let byte_rate = sample_rate * block_align as u32;
    let data_size = (samples.len() * 2) as u32;

    fout.write_all(b"RIFF")?;
    fout.write_all(&(36 + data_size).to_le_bytes())?;
    fout.write_all(b"WAVE")?;
    fout.write_all(b"fmt ")?;
    fout.write_all(&16u32.to_le_bytes())?;
    fout.write_all(&1u16.to_le_bytes())?;
    fout.write_all(&channels.to_le_bytes())?;
    fout.write_all(&sample_rate.to_le_bytes())?;
    fout.write_all(&byte_rate.to_le_bytes())?;
    fout.write_all(&block_align.to_le_bytes())?;
    fout.write_all(&bits_per_sample.to_le_bytes())?;
    fout.write_all(b"data")?;
    fout.write_all(&data_size.to_le_bytes())?;
    for sample in samples {
        fout.write_all(&sample.to_le_bytes())?;
    }
    Ok(())
}

pub fn write_at3_riff<W: Write>(
    out: &mut W,
    channels: u16,
    sample_rate: u32,
    block_align: u16,
    total_samples: u32,
    num_frames: u32,
    loop_points: Option<LoopPoints>,
) -> io::Result<()> {
    let fmt_size = 32u32;
    let fact_size = 8u32;
    let data_size = num_frames * block_align as u32;
    let smpl_size = if loop_points.is_some() { 60u32 } else { 0u32 };
    let riff_size = 4
        + (8 + fmt_size)
        + (8 + fact_size)
        + if smpl_size > 0 { 8 + smpl_size } else { 0 }
        + (8 + data_size);

    out.write_all(b"RIFF")?;
    out.write_all(&riff_size.to_le_bytes())?;
    out.write_all(b"WAVE")?;

    out.write_all(b"fmt ")?;
    out.write_all(&fmt_size.to_le_bytes())?;
    out.write_all(&0x0270u16.to_le_bytes())?;
    out.write_all(&channels.to_le_bytes())?;
    out.write_all(&sample_rate.to_le_bytes())?;
    out.write_all(
        &((block_align as u32 * sample_rate) / ATRAC3_SAMPLES_PER_FRAME as u32).to_le_bytes(),
    )?;
    out.write_all(&block_align.to_le_bytes())?;
    out.write_all(&0u16.to_le_bytes())?;
    out.write_all(&14u16.to_le_bytes())?;
    out.write_all(&1u16.to_le_bytes())?;
    out.write_all(&0x1000u32.to_le_bytes())?;
    out.write_all(&0u16.to_le_bytes())?;
    out.write_all(&0u16.to_le_bytes())?;
    out.write_all(&1u16.to_le_bytes())?;
    out.write_all(&0u16.to_le_bytes())?;

    out.write_all(b"fact")?;
    out.write_all(&fact_size.to_le_bytes())?;
    out.write_all(&total_samples.to_le_bytes())?;
    out.write_all(&(ATRAC3_SAMPLES_PER_FRAME as u32).to_le_bytes())?;

    if let Some(loop_info) = loop_points {
        out.write_all(b"smpl")?;
        out.write_all(&smpl_size.to_le_bytes())?;
        out.write_all(&0u32.to_le_bytes())?;
        out.write_all(&0u32.to_le_bytes())?;
        out.write_all(&(1_000_000_000u32 / sample_rate).to_le_bytes())?;
        out.write_all(&60u32.to_le_bytes())?;
        out.write_all(&0u32.to_le_bytes())?;
        out.write_all(&0u32.to_le_bytes())?;
        out.write_all(&0u32.to_le_bytes())?;
        out.write_all(&1u32.to_le_bytes())?;
        out.write_all(&0u32.to_le_bytes())?;
        out.write_all(&0u32.to_le_bytes())?;
        out.write_all(&0u32.to_le_bytes())?;
        out.write_all(&loop_info.start.to_le_bytes())?;
        out.write_all(&loop_info.end.to_le_bytes())?;
        out.write_all(&0u32.to_le_bytes())?;
        out.write_all(&0u32.to_le_bytes())?;
    }

    out.write_all(b"data")?;
    out.write_all(&data_size.to_le_bytes())?;
    Ok(())
}
