use std::env;

use at3rs::eval;

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("Usage: eval_metrics <wav> [<wav> ...] [--channels N] [--block-align BYTES]");
        std::process::exit(2);
    }

    let mut paths = Vec::new();
    let mut channels = 2u16;
    let mut block_align = 384usize;
    let mut idx = 0usize;

    while idx < args.len() {
        match args[idx].as_str() {
            "--channels" => {
                idx += 1;
                channels = args
                    .get(idx)
                    .and_then(|s| s.parse().ok())
                    .expect("invalid --channels value");
            }
            "--block-align" => {
                idx += 1;
                block_align = args
                    .get(idx)
                    .and_then(|s| s.parse().ok())
                    .expect("invalid --block-align value");
            }
            value => paths.push(value.to_string()),
        }
        idx += 1;
    }

    if paths.is_empty() {
        eprintln!("eval_metrics requires at least one WAV path");
        std::process::exit(2);
    }

    println!("file\toffset\tsamples\tsnr_db\tpsnr_db\trmse\tmax_abs_error");
    for path in paths {
        let metrics = eval::evaluate_atrac3_roundtrip_file(&path, channels, block_align)
            .unwrap_or_else(|err| panic!("failed to evaluate {path}: {err}"));
        println!(
            "{}\t{}\t{}\t{:.2}\t{:.2}\t{:.2}\t{}",
            path,
            metrics.best_offset,
            metrics.samples_compared,
            metrics.snr_db,
            metrics.psnr_db,
            metrics.rmse,
            metrics.max_abs_error
        );
    }
}
