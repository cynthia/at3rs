#!/usr/bin/env python3
"""Correlate per-frame Sony decode error with at3rs/FOSS gain decisions."""

from __future__ import annotations

import argparse
import csv
import math
import struct
import sys
import wave
from pathlib import Path

import compare_gain_points
import run_eval


REPO_ROOT = run_eval.REPO_ROOT
ATRAC3_FRAME_SAMPLES = 1024


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path, help="PCM WAV fixture to analyze")
    parser.add_argument("--bitrate", type=int, default=132)
    parser.add_argument("--output-root", type=Path, default=REPO_ROOT / "output")
    parser.add_argument("--foss-encoder", type=Path)
    parser.add_argument("--sony-tool", type=Path, default=run_eval.DEFAULT_SONY_TOOL)
    parser.add_argument("--at3rs-quality", choices=("standard", "high"), default="high")
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument("--timeout", type=int, default=180)
    parser.add_argument("--top", type=int, default=30, help="number of worst windows to summarize")
    return parser.parse_args(argv)


def read_pcm16(path: Path) -> tuple[int, int, list[list[float]]]:
    with wave.open(str(path), "rb") as wav:
        channels = wav.getnchannels()
        sample_rate = wav.getframerate()
        sample_width = wav.getsampwidth()
        if sample_width != 2:
            raise run_eval.EvalError(f"expected 16-bit PCM WAV: {path}")
        raw = wav.readframes(wav.getnframes())

    samples = struct.unpack("<" + "h" * (len(raw) // 2), raw)
    per_channel = [[] for _ in range(channels)]
    for index, sample in enumerate(samples):
        per_channel[index % channels].append(float(sample))
    return sample_rate, channels, per_channel


def build_release_tools(no_build: bool, timeout: int) -> None:
    run_eval.build_release_tools(no_build, timeout)
    dump_tool = REPO_ROOT / "target" / "release" / "dump_at3_frame"
    if no_build:
        if not dump_tool.exists():
            raise run_eval.EvalError(f"missing release tool and --no-build was set: {dump_tool}")
        return
    if not dump_tool.exists():
        run_eval.checked_command(
            [
                "cargo",
                "build",
                "--release",
                "--manifest-path",
                str(REPO_ROOT / "Cargo.toml"),
                "--bin",
                "dump_at3_frame",
            ],
            timeout=timeout,
        )


def encode_inputs(
    wav: Path,
    out_dir: Path,
    bitrate: int,
    quality: str,
    foss_encoder: Path,
    timeout: int,
) -> tuple[Path, Path]:
    encoded_dir = out_dir / "encoded"
    encoded_dir.mkdir(parents=True, exist_ok=True)
    at3rs_at3 = encoded_dir / "at3rs.at3"
    foss_at3 = encoded_dir / "foss.at3"

    run_eval.log(f"encode at3rs: {wav.name}")
    compare_gain_points.encode_at3rs(wav, at3rs_at3, bitrate, quality, timeout)
    run_eval.log(f"encode foss: {wav.name}")
    compare_gain_points.encode_foss(wav, foss_at3, bitrate, foss_encoder, timeout)
    return at3rs_at3, foss_at3


def decode_sony(at3: Path, decoded_wav: Path, sony_tool: Path, timeout: int) -> None:
    wine, wine_env = run_eval.resolve_wine()
    decoded_wav.parent.mkdir(parents=True, exist_ok=True)
    run_eval.log(f"sony decode: {at3.name}")
    proc = run_eval.checked_command(
        [str(wine), str(sony_tool), "-d", str(at3), str(decoded_wav)],
        env=wine_env,
        timeout=timeout,
    )
    (decoded_wav.parent / "decode_sony.txt").write_text(proc.stdout, encoding="utf-8")


def snr_offset(reference_wav: Path, decoded_wav: Path, metrics_dir: Path, timeout: int, channels: int) -> int:
    metrics_dir.mkdir(parents=True, exist_ok=True)
    proc = run_eval.checked_command(
        [str(REPO_ROOT / "target" / "release" / "snr_test"), str(reference_wav), str(decoded_wav)],
        timeout=timeout,
    )
    (metrics_dir / "snr.txt").write_text(proc.stdout, encoding="utf-8")
    parsed = run_eval.parse_snr(proc.stdout)
    offset_samples = int(parsed.get("offset_samples") or "0")
    if offset_samples % channels != 0:
        raise run_eval.EvalError(f"unexpected non-frame-aligned offset: {offset_samples} samples")
    return offset_samples // channels


def point_count(rows: list[dict[str, str]], encoder: str, frame: int, channel: int, band: int) -> int:
    for row in rows:
        if (
            row["encoder"] == encoder
            and int(row["frame"]) == frame
            and int(row["channel"]) == channel
            and int(row["band"]) == band
        ):
            return int(row["point_count"])
    return 0


def allocation_by_key(rows: list[dict[str, str]]) -> dict[tuple[str, int, int], dict[str, str]]:
    out = {}
    for row in rows:
        out[(row["encoder"], int(row["frame"]), int(row["channel"]))] = row
    return out


def dump_gain_rows(
    at3rs_at3: Path, foss_at3: Path, block_align: int, timeout: int
) -> tuple[list[dict[str, str]], list[dict[str, str]], list[dict[str, str]], list[dict[str, str]]]:
    run_eval.log("dump at3rs gain/allocation")
    gain_rows, allocation_rows = compare_gain_points.dump_rows(at3rs_at3, "at3rs", block_align, timeout)
    run_eval.log("dump foss gain/allocation")
    foss_gain_rows, foss_allocation_rows = compare_gain_points.dump_rows(foss_at3, "foss", block_align, timeout)
    all_gain_rows = gain_rows + foss_gain_rows
    all_allocation_rows = allocation_rows + foss_allocation_rows
    return (
        all_gain_rows,
        compare_gain_points.build_diff_rows(all_gain_rows),
        all_allocation_rows,
        compare_gain_points.build_allocation_diff_rows(all_allocation_rows),
    )


def frame_error_rows(
    reference: list[list[float]],
    decoded: list[list[float]],
    offset_frames: int,
    gain_rows: list[dict[str, str]],
    allocation_rows: list[dict[str, str]],
    sample_rate: int,
) -> list[dict[str, str]]:
    allocation = allocation_by_key(allocation_rows)
    frame_count = min(len(ch) for ch in reference) // ATRAC3_FRAME_SAMPLES
    rows: list[dict[str, str]] = []

    for frame in range(frame_count):
        start = frame * ATRAC3_FRAME_SAMPLES
        decoded_start = start + offset_frames
        if decoded_start < 0:
            continue
        for channel, reference_channel in enumerate(reference):
            decoded_channel = decoded[channel]
            end = start + ATRAC3_FRAME_SAMPLES
            decoded_end = decoded_start + ATRAC3_FRAME_SAMPLES
            if end > len(reference_channel) or decoded_end > len(decoded_channel):
                continue
            src = reference_channel[start:end]
            rec = decoded_channel[decoded_start:decoded_end]
            signal = sum(sample * sample for sample in src)
            error = sum((sample - rec[index]) ** 2 for index, sample in enumerate(src))
            rms = math.sqrt(error / len(src))
            signal_rms = math.sqrt(signal / len(src))
            snr = 99.0 if error <= 0.0 else 10.0 * math.log10((signal + 1.0e-9) / (error + 1.0e-9))
            at3rs_alloc = allocation.get(("at3rs", frame, channel), {})
            foss_alloc = allocation.get(("foss", frame, channel), {})
            row = {
                "frame": str(frame),
                "channel": str(channel),
                "time_sec": f"{start / sample_rate:.6f}",
                "rms_error": f"{rms:.6f}",
                "signal_rms": f"{signal_rms:.6f}",
                "frame_snr_db": f"{snr:.3f}",
                "active_signal": "1" if signal_rms >= 512.0 else "0",
                "at3rs_num_bfu": at3rs_alloc.get("num_bfu", ""),
                "foss_num_bfu": foss_alloc.get("num_bfu", ""),
                "at3rs_coded_bfu": at3rs_alloc.get("coded_bfu", ""),
                "foss_coded_bfu": foss_alloc.get("coded_bfu", ""),
                "at3rs_max_selector": at3rs_alloc.get("max_selector", ""),
                "foss_max_selector": foss_alloc.get("max_selector", ""),
            }
            for band in range(4):
                at3rs_count = point_count(gain_rows, "at3rs", frame, channel, band)
                foss_count = point_count(gain_rows, "foss", frame, channel, band)
                row[f"band{band}_at3rs_gain_points"] = str(at3rs_count)
                row[f"band{band}_foss_gain_points"] = str(foss_count)
                row[f"band{band}_missing_vs_foss"] = str(max(0, foss_count - at3rs_count))
            row["total_missing_gain_points_vs_foss"] = str(
                sum(int(row[f"band{band}_missing_vs_foss"]) for band in range(4))
            )
            rows.append(row)
    return rows


def write_csv(path: Path, rows: list[dict[str, str]]) -> None:
    if not rows:
        path.write_text("", encoding="utf-8")
        return
    with path.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=list(rows[0].keys()))
        writer.writeheader()
        writer.writerows(rows)


def write_summary(
    path: Path,
    rows: list[dict[str, str]],
    gain_diff_rows: list[dict[str, str]],
    allocation_diff_rows: list[dict[str, str]],
    top: int,
) -> None:
    active_rows = [row for row in rows if row["active_signal"] == "1"]
    worst = sorted(active_rows, key=lambda row: float(row["rms_error"]), reverse=True)[:top]
    lowest_snr = sorted(active_rows, key=lambda row: float(row["frame_snr_db"]))[:top]
    rows_with_missing = [row for row in rows if int(row["total_missing_gain_points_vs_foss"]) > 0]
    worst_with_missing = [row for row in worst if int(row["total_missing_gain_points_vs_foss"]) > 0]

    lines = [
        "# Gain/Error Window Analysis",
        "",
        f"Analyzed frame/channel windows: {len(rows)}",
        f"Active-signal windows: {len(active_rows)}",
        f"Gain-diff frame/channel/band rows: {len(gain_diff_rows)}",
        f"Allocation-diff frame/channel rows: {len(allocation_diff_rows)}",
        f"Windows missing any FOSS gain point: {len(rows_with_missing)}",
        f"Worst {len(worst)} active windows missing any FOSS gain point: {len(worst_with_missing)}",
        "",
        "Worst active decode-error windows by RMS error:",
    ]
    for row in worst:
        lines.append(
            "- frame {frame} ch {channel} t={time_sec}s snr={frame_snr_db}dB "
            "rms_error={rms_error} missing_gain={total_missing_gain_points_vs_foss} "
            "num_bfu={at3rs_num_bfu}/{foss_num_bfu} coded_bfu={at3rs_coded_bfu}/{foss_coded_bfu}".format(
                **row
            )
        )
    lines.append("")
    lines.append("Lowest active frame-SNR windows:")
    for row in lowest_snr:
        lines.append(
            "- frame {frame} ch {channel} t={time_sec}s snr={frame_snr_db}dB "
            "rms_error={rms_error} signal_rms={signal_rms} missing_gain={total_missing_gain_points_vs_foss}".format(
                **row
            )
        )
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    try:
        wav = args.input.resolve()
        if wav.suffix.lower() != ".wav":
            raise run_eval.EvalError(f"input is not a .wav file: {wav}")
        if not args.sony_tool.exists():
            raise run_eval.EvalError(f"Sony PSP decoder not found: {args.sony_tool}")

        build_release_tools(args.no_build, args.timeout)
        foss_encoder = run_eval.resolve_foss_encoder(args.foss_encoder)

        out_dir = args.output_root / run_eval.git_ref_slug() / "gain_error" / wav.stem
        out_dir.mkdir(parents=True, exist_ok=True)
        at3rs_at3, foss_at3 = encode_inputs(
            wav, out_dir, args.bitrate, args.at3rs_quality, foss_encoder, args.timeout
        )
        decoded_wav = out_dir / "decoded" / "at3rs_sony.wav"
        decode_sony(at3rs_at3, decoded_wav, args.sony_tool, args.timeout)

        sample_rate, channels, reference_pcm = read_pcm16(wav)
        decoded_sample_rate, decoded_channels, decoded_pcm = read_pcm16(decoded_wav)
        if sample_rate != decoded_sample_rate or channels != decoded_channels:
            raise run_eval.EvalError(
                f"WAV shape mismatch: ref {sample_rate}Hz/{channels}ch, "
                f"decoded {decoded_sample_rate}Hz/{decoded_channels}ch"
            )
        offset_frames = snr_offset(wav, decoded_wav, out_dir / "metrics", args.timeout, channels)

        block_align = 384
        gain_rows, gain_diff_rows, allocation_rows, allocation_diff_rows = dump_gain_rows(
            at3rs_at3, foss_at3, block_align, args.timeout
        )
        compare_gain_points.write_csv(
            out_dir / "gain_points.csv",
            gain_rows,
            ["encoder", "frame", "channel", "band", "points", "point_count", "num_bfu", "coding_mode"],
        )
        compare_gain_points.write_csv(
            out_dir / "gain_point_diffs.csv",
            gain_diff_rows,
            [
                "frame",
                "channel",
                "band",
                "at3rs_points",
                "foss_points",
                "at3rs_count",
                "foss_count",
                "at3rs_num_bfu",
                "foss_num_bfu",
            ],
        )
        compare_gain_points.write_csv(
            out_dir / "allocation.csv",
            allocation_rows,
            [
                "encoder",
                "frame",
                "channel",
                "num_bfu",
                "coded_bfu",
                "max_selector",
                "coding_mode",
                "selectors",
                "sf_idx",
            ],
        )
        compare_gain_points.write_csv(
            out_dir / "allocation_diffs.csv",
            allocation_diff_rows,
            [
                "frame",
                "channel",
                "at3rs_num_bfu",
                "foss_num_bfu",
                "at3rs_coded_bfu",
                "foss_coded_bfu",
                "at3rs_max_selector",
                "foss_max_selector",
                "at3rs_coding_mode",
                "foss_coding_mode",
                "at3rs_selectors",
                "foss_selectors",
                "at3rs_sf_idx",
                "foss_sf_idx",
            ],
        )

        rows = frame_error_rows(reference_pcm, decoded_pcm, offset_frames, gain_rows, allocation_rows, sample_rate)
        write_csv(out_dir / "error_windows.csv", rows)
        write_summary(out_dir / "summary.md", rows, gain_diff_rows, allocation_diff_rows, args.top)
        run_eval.log(f"wrote {out_dir}")
        return 0
    except Exception as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
