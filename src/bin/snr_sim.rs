use std::env;

use at3rs::eval;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: snr_sim <input.wav> [channels] [block_align]");
        std::process::exit(2);
    }

    let channels = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(2);
    let block_align = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(384);

    let metrics = eval::evaluate_atrac3_roundtrip_file(&args[1], channels, block_align)
        .expect("evaluation failed");

    println!("offset_samples: {}", metrics.best_offset);
    println!("samples_compared: {}", metrics.samples_compared);
    println!("snr_db: {:.2}", metrics.snr_db);
    println!("psnr_db: {:.2}", metrics.psnr_db);
    println!("rmse: {:.2}", metrics.rmse);
    println!("max_abs_error: {}", metrics.max_abs_error);
}
