use std::env;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};

use at3rs::atrac3;

fn read_wav_pcm(path: &str) -> io::Result<(u16, Vec<i16>)> {
    let mut fin = File::open(path)?;

    let mut riff_header = [0u8; 12];
    fin.read_exact(&mut riff_header)?;
    if &riff_header[0..4] != b"RIFF" || &riff_header[8..12] != b"WAVE" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a RIFF/WAVE file",
        ));
    }

    let mut channels = 0u16;
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
                channels = u16::from_le_bytes(fmt[2..4].try_into().unwrap());
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

    let mut samples = Vec::with_capacity(sample_data.len() / 2);
    for chunk in sample_data.chunks_exact(2) {
        samples.push(i16::from_le_bytes([chunk[0], chunk[1]]));
    }

    Ok((channels, samples))
}

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("Usage: dump_bfu <input.wav> [block_align] [frame_index]");
        std::process::exit(2);
    }

    let path = &args[0];
    let block_align = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(384usize);
    let frame_index = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0usize);
    let (channels, samples) = read_wav_pcm(path)?;
    let frame_len = 1024 * channels as usize;
    let mut frame = vec![0i16; frame_len];
    let frame_start = frame_index.saturating_mul(frame_len);
    let copy_len = frame_len.min(samples.len().saturating_sub(frame_start));
    if copy_len > 0 {
        frame[..copy_len].copy_from_slice(&samples[frame_start..frame_start + copy_len]);
    }

    let mut ctx = atrac3::Atrac3Context::new(channels, block_align);
    if env::var("AT3RS_DUMP_BFU_WARM").ok().as_deref() == Some("1") {
        let mut encoded = vec![0u8; block_align];
        for warm_frame in 0..frame_index {
            let warm_start = warm_frame.saturating_mul(frame_len);
            let warm_end = (warm_start + frame_len).min(samples.len());
            let mut warm_pcm = vec![0i16; frame_len];
            if warm_end > warm_start {
                warm_pcm[..warm_end - warm_start].copy_from_slice(&samples[warm_start..warm_end]);
            }
            ctx.encode_frame(&warm_pcm, &mut encoded);
        }
    }
    let analysis = ctx.debug_first_frame_analysis(&frame);

    println!(
        "input {}\tframe_index {}\tblock_align {}",
        path, frame_index, block_align
    );
    println!();

    for channel in &analysis.channels {
        println!(
            "analysis channel {}\tpcm_rms {:.6}\tpcm_max {:.6}",
            channel.channel, channel.pcm_rms, channel.pcm_max
        );
        println!("band\tsubband_rms\tsubband_max\tmdct_rms\tmdct_max");
        for band in &channel.bands {
            println!(
                "{}\t{:.6}\t{:.6}\t{:.6}\t{:.6}",
                band.band, band.subband_rms, band.subband_max, band.mdct_rms, band.mdct_max
            );
        }
        println!();
    }

    for channel in analysis.plans {
        println!(
            "channel {}\tactive_blocks {}\ttotal_bits {}",
            channel.channel, channel.active_blocks, channel.total_bits
        );
        println!("block\tselector\tsf_idx\tstart\tend\tbits\tmax\tenergy\trecon_energy\tdistortion\timportance");
        for bfu in channel.blocks {
            println!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{:.6}\t{:.6}\t{:.6}\t{:.6}\t{:.3}",
                bfu.block,
                bfu.table_idx,
                bfu.sf_idx,
                bfu.start,
                bfu.end,
                bfu.bit_count,
                bfu.max_val,
                bfu.energy,
                bfu.recon_energy,
                bfu.distortion,
                bfu.importance
            );
        }
        println!();
    }

    Ok(())
}
