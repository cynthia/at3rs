# at3rs

Rust ATRAC3 encoder workbench.

ATRAC, ATRAC3, ATRAC3plus, ATRAC Advanced Lossless and their logos are trademarks of Sony Corporation.

## Repository Layout

- `src/`: encoder, DSP, container I/O, and analysis tools.
- `tests/`: regression tests, including one always-on fixture quality baseline.
- `fixtures/`: flat WAV fixture corpus used by tests and evaluation.
- `eval/`: Python evaluation runner for local Sony-decoded round trips.
- `third_party/`: local reference tools and metric binaries.
- `output/`: generated reports, WAVs, and metrics; not tracked.

## Current State

- The at3rs encoder can produce RIFF ATRAC3 files that Sony's PSP ATRAC tool can decode.
- at3rs includes an experimental `-d` decoder hook for diagnostics and evaluation, but `decode_frame` is known-broken for quality and must not be treated as a supported decoder.
- Local round-trip evaluation compares at3rs and Sony encode/decode paths, including the broken at3rs decoder cells so decoder regressions are visible.
- SNR, ViSQOL, and PEAQ can be collected when the required third-party binaries are present.
- Output artifacts are grouped by git ref under `output/<git-ref>/`.

## Current Limitations

- Encoding quality is still under active investigation and should not be considered production quality.
- Decoder quality is currently much worse than encoder quality; expect `* -> at3rs` cells to score poorly.
- The local Sony decode loop requires `third_party/ATRAC-Codec-TOOL/psp_at3tool.exe` plus CrossOver on macOS or Wine on Linux.
- Linux metric support expects binaries with the same names under `third_party/linux_x86_64/`; only macOS arm64 binaries are currently checked in.
- ViSQOL and PEAQ are diagnostic metrics, not replacements for listening tests.

## Common Commands

Build and run the standard tests:

```sh
cargo test --release
```

Run the full ignored payload quality gate:

```sh
cargo test --release --test payload_quality -- --ignored --nocapture
```

Encode one WAV:

```sh
cargo run --release -- -e fixtures/test.wav output/test.at3 132
```

Encode with the conservative high-quality preset:

```sh
cargo run --release -- -e fixtures/test.wav output/test.at3 132 --quality high
```

`--quality high` currently enables the tightened gain-control candidate path and
the current conservative allocation tuning. It does not enable tonal-component
coding or analysis-scale changes; those remain explicit experimental flags
because they are strongly content-dependent.

`--enable-tonal-components` writes experimental ATRAC3 tonal-component syntax.
The syntax path is aligned with the FOSS encoder's VLC tonal coding mode, but it
is not a quality win today. In particular, sine-sweep tests show that the
remaining tonal/synthetic weakness is more likely in allocation or quantization
than in tonal-component syntax itself.

Run the encoder/decoder comparison matrix for every WAV in `fixtures/`:

```sh
python3 eval/run_eval.py fixtures
```

The default cells are `at3rs -> at3rs`, `at3rs -> sony`, `sony -> at3rs`, and `sony -> sony`. Treat `* -> at3rs` as diagnostic only until the decoder is fixed.

Refresh 30-second payload fixtures from sibling FLAC payloads:

```sh
python3 eval/import_payload_fixtures.py ../payloads
```

See [docs/evaluation.md](docs/evaluation.md) for the evaluation workflow, dependencies, and output format.

## Library Surface

The crate is marked `publish = false` while the bitstream quality work is still moving, but the supported in-repo API is `at3rs::Encoder` with `EncodeOptions` and `EncoderConfig`. WAV parsing and ATRAC3 RIFF writing live in `at3rs::riff`; `main.rs` is only a CLI wrapper.

Encoder tuning is explicit. The CLI exposes the supported knobs as flags such as `--quality`, `--force-clc`, `--enable-gain-v2`, `--enable-tonal-components`, `--analysis-scale`, and `--ath-gate-scale`; the encoder no longer reads a broad set of hidden tuning environment variables.
