#!/usr/bin/env python3
"""Compare emitted ATRAC3 gain points between at3rs and atracdenc."""

from __future__ import annotations

import argparse
import ast
import csv
import re
import sys
from pathlib import Path

import run_eval


REPO_ROOT = run_eval.REPO_ROOT


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path, help="PCM WAV fixture to compare")
    parser.add_argument("--bitrate", type=int, default=132)
    parser.add_argument("--output-root", type=Path, default=REPO_ROOT / "output")
    parser.add_argument("--foss-encoder", type=Path)
    parser.add_argument("--at3rs-quality", choices=("standard", "high"), default="high")
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument("--timeout", type=int, default=180)
    return parser.parse_args(argv)


def data_chunk_size(path: Path) -> int:
    data = path.read_bytes()
    if len(data) < 12 or data[:4] != b"RIFF" or data[8:12] != b"WAVE":
        raise run_eval.EvalError(f"expected RIFF/WAVE ATRAC3 file: {path}")
    pos = 12
    while pos + 8 <= len(data):
        chunk_id = data[pos : pos + 4]
        size = int.from_bytes(data[pos + 4 : pos + 8], "little")
        if chunk_id == b"data":
            return size
        pos += 8 + size + (size & 1)
    raise run_eval.EvalError(f"missing data chunk: {path}")


def encode_at3rs(wav: Path, out_at3: Path, bitrate: int, quality: str, timeout: int) -> None:
    args = [
        str(REPO_ROOT / "target" / "release" / "at3rs"),
        "-e",
        str(wav),
        str(out_at3),
        str(bitrate),
    ]
    if quality != "standard":
        args.extend(["--quality", quality])
    proc = run_eval.checked_command(args, timeout=timeout)
    (out_at3.parent / "at3rs_encode.txt").write_text(proc.stdout, encoding="utf-8")


def encode_foss(wav: Path, out_at3: Path, bitrate: int, foss_encoder: Path, timeout: int) -> None:
    proc = run_eval.checked_command(
        [
            str(foss_encoder),
            "-e",
            "atrac3",
            "-i",
            str(wav),
            "-o",
            str(out_at3),
            "--bitrate",
            run_eval.foss_bitrate_arg(bitrate),
        ],
        timeout=timeout,
    )
    (out_at3.parent / "foss_encode.txt").write_text(proc.stdout, encoding="utf-8")


def parse_dump(output: str, encoder: str, frame: int) -> list[dict[str, str]]:
    rows: list[dict[str, str]] = []
    for line in output.splitlines():
        if not line.startswith("ch ") or " num_bfu=" not in line:
            continue
        match = re.search(r"ch (\d+).*gains=(\[.*?\]) num_bfu=(\d+) coding_mode=(\d+)", line)
        if not match:
            continue
        ch = int(match.group(1))
        gains = ast.literal_eval(match.group(2))
        num_bfu = match.group(3)
        coding_mode = match.group(4)
        for band, points in enumerate(gains):
            rows.append(
                {
                    "encoder": encoder,
                    "frame": str(frame),
                    "channel": str(ch),
                    "band": str(band),
                    "points": " ".join(f"{level}:{loc}" for level, loc in points),
                    "point_count": str(len(points)),
                    "num_bfu": num_bfu,
                    "coding_mode": coding_mode,
                }
            )
    return rows


def dump_gain_rows(at3: Path, encoder: str, block_align: int, timeout: int) -> list[dict[str, str]]:
    frame_count = data_chunk_size(at3) // block_align
    rows: list[dict[str, str]] = []
    for frame in range(frame_count):
        proc = run_eval.checked_command(
            [
                str(REPO_ROOT / "target" / "release" / "dump_at3_frame"),
                str(at3),
                str(block_align),
                str(frame),
            ],
            timeout=timeout,
        )
        rows.extend(parse_dump(proc.stdout, encoder, frame))
    return rows


def write_csv(path: Path, rows: list[dict[str, str]], fieldnames: list[str]) -> None:
    with path.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=fieldnames)
        writer.writeheader()
        writer.writerows(rows)


def build_diff_rows(rows: list[dict[str, str]]) -> list[dict[str, str]]:
    grouped: dict[tuple[str, str, str], dict[str, dict[str, str]]] = {}
    for row in rows:
        key = (row["frame"], row["channel"], row["band"])
        grouped.setdefault(key, {})[row["encoder"]] = row

    diff_rows = []
    for (frame, channel, band), by_encoder in sorted(
        grouped.items(), key=lambda item: tuple(int(v) for v in item[0])
    ):
        at3rs = by_encoder.get("at3rs", {})
        foss = by_encoder.get("foss", {})
        at3rs_points = at3rs.get("points", "")
        foss_points = foss.get("points", "")
        if at3rs_points == foss_points:
            continue
        diff_rows.append(
            {
                "frame": frame,
                "channel": channel,
                "band": band,
                "at3rs_points": at3rs_points,
                "foss_points": foss_points,
                "at3rs_count": at3rs.get("point_count", "0"),
                "foss_count": foss.get("point_count", "0"),
                "at3rs_num_bfu": at3rs.get("num_bfu", ""),
                "foss_num_bfu": foss.get("num_bfu", ""),
            }
        )
    return diff_rows


def write_summary(path: Path, rows: list[dict[str, str]], diff_rows: list[dict[str, str]]) -> None:
    totals: dict[tuple[str, str], int] = {}
    frame_bands: dict[str, set[tuple[str, str, str]]] = {}
    for row in rows:
        key = (row["encoder"], row["band"])
        totals[key] = totals.get(key, 0) + int(row["point_count"])
        frame_bands.setdefault(row["encoder"], set()).add((row["frame"], row["channel"], row["band"]))

    lines = [
        "# Gain Point Comparison",
        "",
        f"Compared frame/channel/band rows: {sum(len(v) for v in frame_bands.values())}",
        f"Mismatched frame/channel/band rows: {len(diff_rows)}",
        "",
        "Total emitted gain points by encoder/band:",
    ]
    for encoder in ("at3rs", "foss"):
        for band in range(4):
            lines.append(f"- {encoder} band {band}: {totals.get((encoder, str(band)), 0)}")
    lines.append("")
    lines.append("Top mismatches:")
    for row in diff_rows[:20]:
        lines.append(
            f"- frame {row['frame']} ch {row['channel']} band {row['band']}: "
            f"at3rs=[{row['at3rs_points']}] foss=[{row['foss_points']}]"
        )
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    try:
        wav = args.input.resolve()
        if wav.suffix.lower() != ".wav":
            raise run_eval.EvalError(f"input is not a .wav file: {wav}")
        run_eval.build_release_tools(args.no_build, args.timeout)
        foss_encoder = run_eval.resolve_foss_encoder(args.foss_encoder)

        out_dir = args.output_root / run_eval.git_ref_slug() / "gain_compare" / wav.stem
        encoded_dir = out_dir / "encoded"
        encoded_dir.mkdir(parents=True, exist_ok=True)
        at3rs_at3 = encoded_dir / "at3rs.at3"
        foss_at3 = encoded_dir / "foss.at3"

        run_eval.log(f"encode at3rs: {wav.name}")
        encode_at3rs(wav, at3rs_at3, args.bitrate, args.at3rs_quality, args.timeout)
        run_eval.log(f"encode foss: {wav.name}")
        encode_foss(wav, foss_at3, args.bitrate, foss_encoder, args.timeout)

        block_align = 384
        run_eval.log("dump at3rs gain points")
        rows = dump_gain_rows(at3rs_at3, "at3rs", block_align, args.timeout)
        run_eval.log("dump foss gain points")
        rows.extend(dump_gain_rows(foss_at3, "foss", block_align, args.timeout))

        fieldnames = [
            "encoder",
            "frame",
            "channel",
            "band",
            "points",
            "point_count",
            "num_bfu",
            "coding_mode",
        ]
        write_csv(out_dir / "gain_points.csv", rows, fieldnames)

        diff_rows = build_diff_rows(rows)
        write_csv(
            out_dir / "gain_point_diffs.csv",
            diff_rows,
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
        write_summary(out_dir / "summary.md", rows, diff_rows)
        run_eval.log(f"wrote {out_dir}")
        return 0
    except Exception as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
