# Evaluation Workflow

This repository keeps fixture WAV files in a single flat directory:

```text
fixtures/*.wav
```

Generated evaluation artifacts are written to:

```text
output/<git-ref>/eval/<input-directory-name>/
```

The git ref slug includes a dirty marker when the working tree has uncommitted changes, so listening samples and metric reports can be tied back to the code state that produced them.

## Dependencies

Required:

- Rust toolchain with Cargo.
- `third_party/ATRAC-Codec-TOOL/psp_at3tool.exe`.
- macOS: CrossOver installed at `/Applications/Crossover.app/`.
- Linux: `wine` available in `PATH`.

Optional but recommended:

- `ffmpeg` for 48 kHz metric normalization.
- `third_party/mac_aarch64/visqol` or `third_party/linux_x86_64/visqol`.
- `third_party/mac_aarch64/PQevalAudio` or `third_party/linux_x86_64/PQevalAudio`.
- ViSQOL model file `model/libsvm_nu_svr_model.txt` under the tools directory, or an explicit `--visqol-model` path.

Environment overrides:

- `AT3RS_WINE`: Wine executable path.
- `AT3RS_WINEPREFIX`: Wine prefix path.

## Running Evaluation

Evaluate all WAV files in a directory:

```sh
python3 eval/run_eval.py fixtures
```

Refresh 30-second WAV fixtures from FLAC payloads:

```sh
python3 eval/import_payload_fixtures.py ../payloads
```

By default, each fixture produces an encoder/decoder matrix:

- `at3rs -> at3rs`
- `at3rs -> sony`
- `sony -> at3rs`
- `sony -> sony`

The `* -> at3rs` cells use the experimental at3rs decoder. They are useful for
tracking decoder work, but the decoder is known to be poor quality today.

Use a specific bitrate:

```sh
python3 eval/run_eval.py fixtures --bitrate 132
```

Skip ViSQOL and PEAQ when only round-trip WAVs and SNR are needed:

```sh
python3 eval/run_eval.py fixtures --skip-perceptual
```

Evaluate multiple fixtures concurrently:

```sh
python3 eval/run_eval.py fixtures --jobs 4
```

`--jobs` parallelizes by fixture. Each fixture still runs its matrix cells sequentially, and the final CSV/report are sorted by fixture for stable comparisons. The same setting can be provided with `AT3RS_EVAL_JOBS`.

Run only the at3rs encode paths:

```sh
python3 eval/run_eval.py fixtures --no-sony-reference
```

Use an explicit tools directory:

```sh
python3 eval/run_eval.py fixtures --tools-dir third_party/mac_aarch64
```

Use an explicit ViSQOL model:

```sh
python3 eval/run_eval.py fixtures --visqol-model /path/to/libsvm_nu_svr_model.txt
```

The runner builds `target/release/at3rs` and `target/release/snr_test` unless `--no-build` is passed.

## Output

Each input fixture gets a subdirectory:

```text
output/<git-ref>/eval/<input-dir>/<fixture-stem>/
  encoded/at3rs.at3
  encoded/sony.at3
  decoded/at3rs_at3rs.wav
  decoded/at3rs_sony.wav
  decoded/sony_at3rs.wav
  decoded/sony_sony.wav
  metrics/at3rs_at3rs/
  metrics/at3rs_sony/
  metrics/sony_at3rs/
  metrics/sony_sony/
```

The run also writes:

- `_meta.json`: git revision, input directory, bitrate, and tool paths.
- `summary.csv`: one row per fixture/cell with SNR and perceptual metrics.
- `report.html`: self-contained HTML report with matrix tables and base64-inlined metric plots.

## Tests

Standard tests include a baseline SNR gate against `fixtures/iwish_30s.wav`:

```sh
cargo test --release
```

The full payload quality gate is ignored by default because it is slower:

```sh
cargo test --release --test payload_quality -- --ignored --nocapture
```

Additional fixture hygiene tests ensure WAV fixtures remain flat under `fixtures/`.

## Interpreting Results

SNR is useful for catching large regressions and alignment failures. ViSQOL and PEAQ are useful for trend tracking, but they do not fully describe ATRAC artifacts such as transient crackle, tonal smearing, or ringing. Keep listening comparisons alongside metric reports when evaluating encoder changes.
