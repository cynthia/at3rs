#!/usr/bin/env python3
"""Import 30-second WAV fixtures from a directory of FLAC payloads."""

from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("payload_dir", type=Path, nargs="?", default=REPO_ROOT.parent / "payloads")
    parser.add_argument("--output-dir", type=Path, default=REPO_ROOT / "fixtures")
    parser.add_argument("--seconds", type=int, default=30)
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    if not args.payload_dir.is_dir():
        print(f"error: payload directory not found: {args.payload_dir}", file=sys.stderr)
        return 2

    flacs = sorted(args.payload_dir.glob("*.flac"))
    if not flacs:
        print(f"error: no .flac payloads found in {args.payload_dir}", file=sys.stderr)
        return 2

    args.output_dir.mkdir(parents=True, exist_ok=True)
    for src in flacs:
        out = args.output_dir / f"{src.stem}_{args.seconds}s.wav"
        cmd = [
            "ffmpeg",
            "-y",
            "-loglevel",
            "error",
            "-t",
            str(args.seconds),
            "-i",
            str(src),
            "-ar",
            "44100",
            "-ac",
            "2",
            "-sample_fmt",
            "s16",
            str(out),
        ]
        proc = subprocess.run(cmd, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, check=False)
        if proc.returncode != 0:
            print(f"error: failed to import {src}\n{proc.stdout}", file=sys.stderr)
            return proc.returncode
        print(out)

    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
