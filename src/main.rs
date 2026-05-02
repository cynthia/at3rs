use std::env;
use std::io;

use at3rs::atrac3::Atrac3Context;
use at3rs::encoder::{EncodeOptions, Encoder, EncoderConfig, EncoderQuality};
use at3rs::riff::{read_atrac3_riff, write_pcm_wav, LoopPoints};
use at3rs::ATRAC3_SAMPLES_PER_FRAME;

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 || args[1] == "-h" || args[1] == "--help" {
        print_usage();
        return Ok(());
    }
    if args.len() < 4 {
        print_usage();
        return Err(invalid_input("expected -e or -d arguments"));
    }

    match args[1].as_str() {
        "-e" => {
            let options = parse_encode_options(&args[4..])?;
            let summary = Encoder::new(options).encode_wav_file(&args[2], &args[3])?;
            println!(
                "Encoded {} frames, {} channels, {} Hz, {} bytes/frame",
                summary.frames, summary.channels, summary.sample_rate, summary.block_align
            );
            Ok(())
        }
        "-d" => decode_pipeline(&args[2], &args[3]),
        _ => {
            print_usage();
            Err(invalid_input("unsupported command"))
        }
    }
}

fn print_usage() {
    println!("ATRAC3 Rust Encoder");
    println!("Usage: at3rs -e <input.wav> <output.at3> [bitrate_kbps] [options]");
    println!("       at3rs -d <input.at3> <output.wav>  # experimental, known poor quality");
    println!();
    println!("Options:");
    println!("  -loop <start> <end>         write RIFF smpl loop points");
    println!("  --max-frames <n>            encode at most n ATRAC3 frames");
    println!("  --force-clc                 disable VLC and write fixed-length mantissas");
    println!("  --disable-vlc               alias for --force-clc");
    println!("  --quality <standard|high>   apply a named encoder quality preset");
    println!("  --enable-gain-v2            enable experimental gain-control syntax");
    println!("  --enable-tonal-components   enable experimental tonal-component syntax");
    println!("  --stage-debug               print encoder stage diagnostics");
    println!("  --analysis-scale <value>    override MDCT analysis scale");
    println!("  --ath-gate-scale <value>    override ATH gate scale");
}

fn decode_pipeline(input: &str, output: &str) -> io::Result<()> {
    let atrac = read_atrac3_riff(input)?;
    if atrac.block_align == 0 {
        return Err(invalid_input("ATRAC3 block alignment is zero"));
    }
    if atrac.channels == 0 || atrac.channels > 2 {
        return Err(invalid_input(
            "only mono and stereo ATRAC3 streams are supported",
        ));
    }

    let block_align = atrac.block_align as usize;
    let frame_samples = ATRAC3_SAMPLES_PER_FRAME * atrac.channels as usize;
    let mut ctx = Atrac3Context::new(atrac.channels, block_align);
    let mut pcm = Vec::with_capacity((atrac.data.len() / block_align) * frame_samples);

    for frame in atrac.data.chunks_exact(block_align) {
        let mut out_frame = vec![0i16; frame_samples];
        ctx.decode_frame(frame, &mut out_frame);
        pcm.extend_from_slice(&out_frame);
    }

    if let Some(total_samples_per_channel) = atrac.total_samples_per_channel {
        let wanted = total_samples_per_channel as usize * atrac.channels as usize;
        pcm.truncate(wanted.min(pcm.len()));
    }

    write_pcm_wav(output, atrac.channels, atrac.sample_rate, &pcm)?;
    println!(
        "Decoded {} samples, {} channels, {} Hz (experimental decoder)",
        pcm.len() / atrac.channels as usize,
        atrac.channels,
        atrac.sample_rate
    );
    Ok(())
}

fn parse_encode_options(args: &[String]) -> io::Result<EncodeOptions> {
    let mut options = EncodeOptions::default();
    let mut config = EncoderConfig::default();
    let mut saw_bitrate = false;
    let mut idx = 0usize;

    while idx < args.len() {
        match args[idx].as_str() {
            "-loop" => {
                if idx + 2 >= args.len() {
                    return Err(invalid_input("expected -loop <start> <end>"));
                }
                let start = parse_u32(&args[idx + 1], "invalid loop start")?;
                let end = parse_u32(&args[idx + 2], "invalid loop end")?;
                options.loop_points = Some(LoopPoints { start, end });
                idx += 3;
            }
            "--max-frames" => {
                if idx + 1 >= args.len() {
                    return Err(invalid_input("expected --max-frames <n>"));
                }
                options.max_frames = Some(parse_usize(&args[idx + 1], "invalid max frame count")?);
                idx += 2;
            }
            "--force-clc" | "--disable-vlc" => {
                config = config.with_force_clc(true);
                idx += 1;
            }
            "--quality" => {
                if idx + 1 >= args.len() {
                    return Err(invalid_input("expected --quality <standard|high>"));
                }
                config = config.with_quality(parse_quality(&args[idx + 1])?);
                idx += 2;
            }
            "--enable-gain-v2" => {
                config = config.with_experimental_gain_v2(true);
                idx += 1;
            }
            "--enable-tonal-components" => {
                config = config.with_experimental_tonal_components(true);
                idx += 1;
            }
            "--stage-debug" => {
                config = config.with_stage_debug(true);
                idx += 1;
            }
            "--analysis-scale" => {
                if idx + 1 >= args.len() {
                    return Err(invalid_input("expected --analysis-scale <value>"));
                }
                config = config.with_analysis_scale(parse_positive_f32(
                    &args[idx + 1],
                    "invalid analysis scale",
                )?);
                idx += 2;
            }
            "--ath-gate-scale" => {
                if idx + 1 >= args.len() {
                    return Err(invalid_input("expected --ath-gate-scale <value>"));
                }
                config = config.with_ath_gate_scale(parse_positive_f32(
                    &args[idx + 1],
                    "invalid ATH gate scale",
                )?);
                idx += 2;
            }
            value if value.starts_with('-') => {
                return Err(invalid_input(format!("unknown option: {value}")));
            }
            value => {
                if saw_bitrate {
                    return Err(invalid_input(format!(
                        "unexpected positional argument: {value}"
                    )));
                }
                options.bitrate_kbps = value
                    .parse::<u32>()
                    .map_err(|_| invalid_input("invalid bitrate"))?;
                saw_bitrate = true;
                idx += 1;
            }
        }
    }

    options.config = config;
    Ok(options)
}

fn parse_u32(value: &str, message: &'static str) -> io::Result<u32> {
    value.parse::<u32>().map_err(|_| invalid_input(message))
}

fn parse_usize(value: &str, message: &'static str) -> io::Result<usize> {
    value.parse::<usize>().map_err(|_| invalid_input(message))
}

fn parse_positive_f32(value: &str, message: &'static str) -> io::Result<f32> {
    value
        .parse::<f32>()
        .ok()
        .filter(|parsed| *parsed > 0.0)
        .ok_or_else(|| invalid_input(message))
}

fn parse_quality(value: &str) -> io::Result<EncoderQuality> {
    match value {
        "standard" => Ok(EncoderQuality::Standard),
        "high" => Ok(EncoderQuality::High),
        _ => Err(invalid_input("invalid quality preset")),
    }
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}
