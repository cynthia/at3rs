#!/usr/bin/env python3
"""Evaluate legal at3rs encode variants against decoded PCM and pick winners."""

from __future__ import annotations

import argparse
import csv
import shutil
import sys
from pathlib import Path

import run_eval


REPO_ROOT = run_eval.REPO_ROOT

VARIANTS: list[tuple[str, list[str]]] = [
    ("default", []),
    ("analysis_0.225", ["--analysis-scale", "0.225"]),
    ("analysis_0.245", ["--analysis-scale", "0.245"]),
    ("ath_0.00005", ["--ath-gate-scale", "0.00005"]),
    ("ath_0.00020", ["--ath-gate-scale", "0.00020"]),
    ("force_clc", ["--force-clc"]),
    ("tonal", ["--enable-tonal-components"]),
    ("gain_v2", ["--enable-gain-v2"]),
]

SCORE_MODES = ("guarded-peaq", "peaq", "visqol", "snr")


def candidate_score(
    row: dict[str, str],
    mode: str,
    best_snr: float | None = None,
    best_visqol: float | None = None,
    snr_tolerance_db: float = 0.50,
    visqol_tolerance: float = 0.03,
) -> tuple[float, float, float]:
    """Higher is better."""
    peaq = parse_float(row.get("peaq_odg", ""))
    visqol = parse_float(row.get("visqol_moslqo", ""))
    snr = parse_float(row.get("gain_adjusted_snr_db", ""))
    peaq_score = peaq if peaq is not None else -99.0
    visqol_score = visqol if visqol is not None else -99.0
    snr_score = snr if snr is not None else -99.0

    if mode == "peaq":
        return (peaq_score, visqol_score, snr_score)
    if mode == "visqol":
        return (visqol_score, peaq_score, snr_score)
    if mode == "snr":
        return (snr_score, peaq_score, visqol_score)
    if mode == "guarded-peaq":
        if peaq is None and visqol is None:
            return (snr_score, -99.0, -99.0)
        if best_snr is not None and snr is not None and snr < best_snr - snr_tolerance_db:
            return (-100.0, peaq_score, visqol_score)
        if (
            best_visqol is not None
            and visqol is not None
            and visqol < best_visqol - visqol_tolerance
        ):
            return (-100.0, peaq_score, snr_score)
        return (peaq_score, visqol_score, snr_score)
    raise ValueError(f"unknown score mode: {mode}")


def parse_float(value: str | None) -> float | None:
    if not value:
        return None
    try:
        return float(value)
    except ValueError:
        return None


def best_metric(rows: list[dict[str, str]], metric: str) -> float | None:
    values = [parse_float(row[metric]) for row in rows]
    return max((value for value in values if value is not None), default=None)


def evaluate_variant(
    wav: Path,
    variant_name: str,
    variant_args: list[str],
    fixture_dir: Path,
    bitrate: int,
    wine: Path,
    wine_env: dict[str, str],
    sony_tool: Path,
    tools_dir: Path,
    visqol_model: Path | None,
    skip_perceptual: bool,
    timeout: int,
) -> dict[str, str]:
    encoded = fixture_dir / "encoded" / f"{variant_name}.at3"
    decoded = fixture_dir / "decoded" / f"{variant_name}_sony.wav"
    metrics_dir = fixture_dir / "metrics" / variant_name
    encoded.parent.mkdir(parents=True, exist_ok=True)
    decoded.parent.mkdir(parents=True, exist_ok=True)
    metrics_dir.mkdir(parents=True, exist_ok=True)

    row = {
        "input": str(wav),
        "variant": variant_name,
        "status": "failed",
        "snr_db": "",
        "gain_adjusted_snr_db": "",
        "correlation": "",
        "gain": "",
        "offset_samples": "",
        "visqol_moslqo": "",
        "peaq_odg": "",
        "peaq_distortion_index": "",
        "perceptual_status": "",
        "encoded_path": str(encoded),
        "decoded_path": str(decoded),
        "metrics_dir": str(metrics_dir),
        "error": "",
    }

    try:
        run_eval.log(f"{wav.name} {variant_name}: encode")
        encode = run_eval.checked_command(
            [
                str(REPO_ROOT / "target" / "release" / "at3rs"),
                "-e",
                str(wav),
                str(encoded),
                str(bitrate),
                *variant_args,
            ],
            timeout=timeout,
        )
        (metrics_dir / "encode.txt").write_text(encode.stdout, encoding="utf-8")

        run_eval.log(f"{wav.name} {variant_name}: sony decode")
        decode = run_eval.checked_command(
            [str(wine), str(sony_tool), "-d", str(encoded), str(decoded)],
            env=wine_env,
            timeout=timeout,
        )
        (metrics_dir / "decode_sony.txt").write_text(decode.stdout, encoding="utf-8")

        snr = run_eval.checked_command(
            [str(REPO_ROOT / "target" / "release" / "snr_test"), str(wav), str(decoded)],
            timeout=timeout,
        )
        (metrics_dir / "snr.txt").write_text(snr.stdout, encoding="utf-8")
        row.update(run_eval.parse_snr(snr.stdout))

        if skip_perceptual:
            row["perceptual_status"] = "skipped_by_user"
        else:
            row.update(
                run_eval.run_perceptual_metrics(
                    wav, decoded, metrics_dir, tools_dir, visqol_model, timeout
                )
            )

        row["status"] = "ok"
        run_eval.log(
            f"{wav.name} {variant_name}: ok snr={row['snr_db']} "
            f"visqol={row['visqol_moslqo'] or 'skipped'} peaq={row['peaq_odg'] or 'skipped'}"
        )
    except Exception as exc:
        row["error"] = str(exc)
        (metrics_dir / "error.txt").write_text(row["error"], encoding="utf-8")
        run_eval.log(f"{wav.name} {variant_name}: failed")

    return row


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path, help="WAV fixture file or directory containing .wav fixtures")
    parser.add_argument("--bitrate", type=int, default=132, help="ATRAC3 bitrate in kbps")
    parser.add_argument("--output-root", type=Path, default=REPO_ROOT / "output")
    parser.add_argument("--sony-tool", type=Path, default=run_eval.DEFAULT_SONY_TOOL)
    parser.add_argument("--tools-dir", type=Path)
    parser.add_argument("--visqol-model", type=Path)
    parser.add_argument("--skip-perceptual", action="store_true")
    parser.add_argument(
        "--score-mode",
        choices=SCORE_MODES,
        default="guarded-peaq",
        help="Candidate selection policy. guarded-peaq rejects PEAQ wins that regress SNR/ViSQOL too far.",
    )
    parser.add_argument(
        "--snr-tolerance-db",
        type=float,
        default=0.50,
        help="SNR drop allowed by guarded-peaq relative to the best candidate.",
    )
    parser.add_argument(
        "--visqol-tolerance",
        type=float,
        default=0.03,
        help="ViSQOL drop allowed by guarded-peaq relative to the best candidate.",
    )
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument("--timeout", type=int, default=180)
    return parser.parse_args(argv)


def discover_inputs(path: Path) -> list[Path]:
    if path.is_file():
        if path.suffix.lower() != ".wav":
            raise run_eval.EvalError(f"input is not a .wav file: {path}")
        return [path.resolve()]
    return [p.resolve() for p in run_eval.discover_wav_files(path.resolve())]


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    try:
        wavs = discover_inputs(args.input)
        if not args.sony_tool.exists():
            raise run_eval.EvalError(f"Sony PSP decoder not found: {args.sony_tool}")
        run_eval.build_release_tools(args.no_build, args.timeout)
        wine, wine_env = run_eval.resolve_wine()
        tools_dir = run_eval.resolve_tools_dir(args.tools_dir) if not args.skip_perceptual else Path("")
        visqol_model = (
            run_eval.resolve_visqol_model(args.visqol_model, tools_dir)
            if not args.skip_perceptual
            else None
        )

        output_dir = args.output_root / run_eval.git_ref_slug() / "closed_loop" / args.input.stem
        output_dir.mkdir(parents=True, exist_ok=True)
        rows: list[dict[str, str]] = []
        winners: list[dict[str, str]] = []

        for wav in wavs:
            fixture_dir = output_dir / wav.stem
            candidate_rows = [
                evaluate_variant(
                    wav,
                    variant_name,
                    variant_args,
                    fixture_dir,
                    args.bitrate,
                    wine,
                    wine_env,
                    args.sony_tool,
                    tools_dir,
                    visqol_model,
                    args.skip_perceptual,
                    args.timeout,
                )
                for variant_name, variant_args in VARIANTS
            ]
            rows.extend(candidate_rows)
            ok_rows = [row for row in candidate_rows if row["status"] == "ok"]
            if ok_rows:
                best_snr = best_metric(ok_rows, "gain_adjusted_snr_db")
                best_visqol = best_metric(ok_rows, "visqol_moslqo")
                winner = max(
                    ok_rows,
                    key=lambda row: candidate_score(
                        row,
                        args.score_mode,
                        best_snr=best_snr,
                        best_visqol=best_visqol,
                        snr_tolerance_db=args.snr_tolerance_db,
                        visqol_tolerance=args.visqol_tolerance,
                    ),
                )
                winners.append(winner)
                selected_dir = fixture_dir / "selected"
                selected_dir.mkdir(parents=True, exist_ok=True)
                shutil.copy2(winner["encoded_path"], selected_dir / "selected.at3")
                shutil.copy2(winner["decoded_path"], selected_dir / "selected_sony.wav")
                score = candidate_score(
                    winner,
                    args.score_mode,
                    best_snr=best_snr,
                    best_visqol=best_visqol,
                    snr_tolerance_db=args.snr_tolerance_db,
                    visqol_tolerance=args.visqol_tolerance,
                )
                (selected_dir / "winner.txt").write_text(
                    f"variant={winner['variant']}\n"
                    f"score_mode={args.score_mode}\n"
                    f"score={score}\n"
                    f"snr_db={winner['snr_db']}\n"
                    f"gain_adjusted_snr_db={winner['gain_adjusted_snr_db']}\n"
                    f"visqol_moslqo={winner['visqol_moslqo']}\n"
                    f"peaq_odg={winner['peaq_odg']}\n",
                    encoding="utf-8",
                )
                run_eval.log(f"{wav.name}: winner {winner['variant']} score={score}")

        fieldnames = [
            "input",
            "variant",
            "status",
            "snr_db",
            "gain_adjusted_snr_db",
            "correlation",
            "gain",
            "offset_samples",
            "visqol_moslqo",
            "peaq_odg",
            "peaq_distortion_index",
            "perceptual_status",
            "encoded_path",
            "decoded_path",
            "metrics_dir",
            "error",
        ]
        for name, selected_rows in [("summary.csv", rows), ("winners.csv", winners)]:
            with (output_dir / name).open("w", newline="", encoding="utf-8") as handle:
                writer = csv.DictWriter(handle, fieldnames=fieldnames)
                writer.writeheader()
                writer.writerows(selected_rows)

        run_eval.log(f"wrote {output_dir}")
        return 0
    except Exception as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
