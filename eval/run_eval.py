#!/usr/bin/env python3
"""Run local ATRAC3 round-trip evaluation for a directory of WAV fixtures."""

from __future__ import annotations

import argparse
import base64
import concurrent.futures
import csv
import datetime as dt
import html
import io
import json
import os
import platform
import re
import shutil
import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_SONY_TOOL = REPO_ROOT / "third_party" / "ATRAC-Codec-TOOL" / "psp_at3tool.exe"
DEFAULT_MAC_CX_ROOT = Path("/Applications/Crossover.app/Contents/SharedSupport/CrossOver")
DEFAULT_MAC_WINE = DEFAULT_MAC_CX_ROOT / "lib/wine/x86_64-unix/wine"
MATRIX_CELLS = [
    ("at3rs", "at3rs"),
    ("at3rs", "sony"),
    ("sony", "at3rs"),
    ("sony", "sony"),
]
AT3RS_ONLY_CELLS = [("at3rs", "at3rs"), ("at3rs", "sony")]
CELL_COLORS = {
    "at3rs->at3rs": "#0969da",
    "at3rs->sony": "#8250df",
    "sony->at3rs": "#bf8700",
    "sony->sony": "#cf222e",
}


class EvalError(RuntimeError):
    pass


def log(message: str) -> None:
    print(message, flush=True)


def run_command(
    args: list[str],
    *,
    env: dict[str, str] | None = None,
    timeout: int,
    cwd: Path = REPO_ROOT,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        args,
        cwd=str(cwd),
        env=env,
        timeout=timeout,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )


def checked_command(
    args: list[str],
    *,
    env: dict[str, str] | None = None,
    timeout: int,
    cwd: Path = REPO_ROOT,
) -> subprocess.CompletedProcess[str]:
    try:
        proc = run_command(args, env=env, timeout=timeout, cwd=cwd)
    except subprocess.TimeoutExpired as exc:
        raise EvalError(f"command timed out after {timeout}s: {' '.join(args)}") from exc
    if proc.returncode != 0:
        raise EvalError(f"command failed ({proc.returncode}): {' '.join(args)}\n{proc.stdout}")
    return proc


def git_ref_slug() -> str:
    branch = run_command(["git", "rev-parse", "--abbrev-ref", "HEAD"], timeout=20)
    short_head = run_command(["git", "rev-parse", "--short", "HEAD"], timeout=20)
    if short_head.returncode != 0:
        return "unknown"

    branch_name = branch.stdout.strip() if branch.returncode == 0 else "detached"
    if branch_name == "HEAD":
        branch_name = "detached"
    safe_branch = re.sub(r"[^A-Za-z0-9._-]+", "-", branch_name).strip("-") or "detached"
    ref = f"{safe_branch}@{short_head.stdout.strip()}"

    dirty = run_command(["git", "diff", "--quiet"], timeout=20)
    staged = run_command(["git", "diff", "--cached", "--quiet"], timeout=20)
    untracked = run_command(["git", "ls-files", "--others", "--exclude-standard"], timeout=20)
    is_dirty = dirty.returncode != 0 or staged.returncode != 0 or bool(untracked.stdout.strip())
    return f"{ref}-dirty" if is_dirty else ref


def git_head() -> str:
    proc = run_command(["git", "rev-parse", "HEAD"], timeout=20)
    return proc.stdout.strip() if proc.returncode == 0 else "unknown"


def discover_wav_files(input_dir: Path) -> list[Path]:
    if not input_dir.is_dir():
        raise EvalError(f"input path is not a directory: {input_dir}")
    wavs = sorted(path for path in input_dir.glob("*.wav") if path.is_file())
    if not wavs:
        raise EvalError(f"no .wav files found in {input_dir}")
    return wavs


def build_release_tools(no_build: bool, timeout: int) -> None:
    required = [
        REPO_ROOT / "target" / "release" / "at3rs",
        REPO_ROOT / "target" / "release" / "snr_test",
    ]
    if no_build and any(not path.exists() for path in required):
        missing = ", ".join(str(path) for path in required if not path.exists())
        raise EvalError(f"missing release tools and --no-build was set: {missing}")
    if no_build or all(path.exists() for path in required):
        return
    checked_command(
        [
            "cargo",
            "build",
            "--release",
            "--manifest-path",
            str(REPO_ROOT / "Cargo.toml"),
            "--bin",
            "at3rs",
            "--bin",
            "snr_test",
        ],
        timeout=timeout,
    )


def resolve_wine() -> tuple[Path, dict[str, str]]:
    env = os.environ.copy()
    override = os.environ.get("AT3RS_WINE")
    if override:
        wine = Path(override)
        if not wine.exists():
            raise EvalError(f"AT3RS_WINE points to a missing binary: {wine}")
        return wine, env

    system = platform.system()
    if system == "Darwin":
        if not DEFAULT_MAC_WINE.exists():
            raise EvalError(
                "CrossOver Wine is required on macOS and was not found at "
                f"{DEFAULT_MAC_WINE}. Set AT3RS_WINE to override."
            )
        env.setdefault("CX_ROOT", str(DEFAULT_MAC_CX_ROOT))
        env.setdefault("WINEPREFIX", str(REPO_ROOT.parent / ".wine-at3tool"))
        return DEFAULT_MAC_WINE, env

    if system == "Linux":
        wine_path = shutil.which("wine")
        if not wine_path:
            raise EvalError("wine is required on Linux but was not found in PATH")
        env.setdefault("WINEPREFIX", str(REPO_ROOT.parent / ".wine-at3tool"))
        return Path(wine_path), env

    raise EvalError(f"unsupported platform for Sony decoder: {system}")


def resolve_tools_dir(explicit: Path | None) -> Path:
    if explicit:
        tools_dir = explicit
    elif platform.system() == "Darwin":
        tools_dir = REPO_ROOT / "third_party" / "mac_aarch64"
    elif platform.system() == "Linux":
        tools_dir = REPO_ROOT / "third_party" / "linux_x86_64"
    else:
        raise EvalError(f"unsupported platform for perceptual tools: {platform.system()}")

    if not tools_dir.is_dir():
        raise EvalError(f"perceptual tool directory not found: {tools_dir}")
    return tools_dir


def resolve_visqol_model(explicit: Path | None, tools_dir: Path) -> Path | None:
    if explicit:
        if not explicit.exists():
            raise EvalError(f"ViSQOL model not found: {explicit}")
        return explicit

    candidates = [
        tools_dir / "model" / "libsvm_nu_svr_model.txt",
        REPO_ROOT / "model" / "libsvm_nu_svr_model.txt",
        REPO_ROOT.parent / "visqol" / "model" / "libsvm_nu_svr_model.txt",
        REPO_ROOT.parents[1] / "visqol" / "model" / "libsvm_nu_svr_model.txt",
    ]
    for candidate in candidates:
        if candidate.exists():
            return candidate
    return None


def parse_snr(output: str) -> dict[str, str]:
    patterns = {
        "offset_samples": r"Best alignment offset:\s+(-?\d+)\s+samples",
        "correlation": r"Correlation:\s+([-+0-9.]+)",
        "snr_db": r"Round-trip SNR:\s+([-+0-9.]+)\s+dB",
        "gain_adjusted_snr_db": r"Gain-adjusted SNR:\s+([-+0-9.]+)\s+dB",
        "gain": r"Best gain:\s+([-+0-9.]+)",
    }
    parsed: dict[str, str] = {}
    for key, pattern in patterns.items():
        match = re.search(pattern, output)
        parsed[key] = match.group(1) if match else ""
    return parsed


def parse_visqol(output: str) -> str:
    patterns = [
        r"MOS-LQO:\s*([-+0-9.]+)",
        r"moslqo\s*[:=]\s*([-+0-9.]+)",
        r"MOSLQO\s*[:=]\s*([-+0-9.]+)",
    ]
    for pattern in patterns:
        matches = re.findall(pattern, output, flags=re.IGNORECASE)
        if matches:
            return matches[-1]
    return ""


def parse_peaq(output: str) -> tuple[str, str]:
    odg = ""
    distortion = ""
    number = r"([-+]?\d+(?:\.\d+)?)"
    odg_match = re.search(rf"^\s*(?:Objective Difference Grade|ODG):\s*{number}", output, re.IGNORECASE | re.MULTILINE)
    distortion_match = re.search(rf"^\s*(?:Distortion Index|DI):\s*{number}", output, re.IGNORECASE | re.MULTILINE)
    if odg_match:
        odg = odg_match.group(1)
    if distortion_match:
        distortion = distortion_match.group(1)
    return odg, distortion


def normalize_for_perceptual(src: Path, dst: Path, timeout: int) -> bool:
    ffmpeg = shutil.which("ffmpeg")
    if not ffmpeg:
        return False
    proc = run_command(
        [ffmpeg, "-y", "-loglevel", "error", "-i", str(src), "-ar", "48000", "-ac", "2", str(dst)],
        timeout=timeout,
    )
    return proc.returncode == 0 and dst.exists() and dst.stat().st_size > 0


def run_perceptual_metrics(
    reference_wav: Path,
    decoded_wav: Path,
    metrics_dir: Path,
    tools_dir: Path,
    visqol_model: Path | None,
    timeout: int,
) -> dict[str, str]:
    metrics: dict[str, str] = {
        "visqol_moslqo": "",
        "peaq_odg": "",
        "peaq_distortion_index": "",
        "perceptual_status": "skipped",
    }

    normalized_dir = metrics_dir / "normalized"
    normalized_dir.mkdir(parents=True, exist_ok=True)
    reference_48k = normalized_dir / "reference_48k.wav"
    decoded_48k = normalized_dir / "decoded_48k.wav"
    normalized = normalize_for_perceptual(reference_wav, reference_48k, timeout) and normalize_for_perceptual(
        decoded_wav, decoded_48k, timeout
    )
    metric_ref = reference_48k if normalized else reference_wav
    metric_decoded = decoded_48k if normalized else decoded_wav

    statuses: list[str] = []

    visqol = tools_dir / "visqol"
    visqol_log = metrics_dir / "visqol.txt"
    if visqol.exists():
        if visqol_model:
            cmd = [
                str(visqol),
                "--reference_file",
                str(metric_ref),
                "--degraded_file",
                str(metric_decoded),
                "--use_speech_mode=false",
                "--similarity_to_quality_model",
                str(visqol_model),
            ]
            proc = run_command(cmd, timeout=timeout)
            visqol_log.write_text(proc.stdout, encoding="utf-8")
            if proc.returncode == 0:
                metrics["visqol_moslqo"] = parse_visqol(proc.stdout)
                statuses.append("visqol_ok")
            else:
                statuses.append("visqol_failed")
        else:
            visqol_log.write_text(
                "ViSQOL model not found. Use --visqol-model or place "
                "model/libsvm_nu_svr_model.txt under the tools directory.\n",
                encoding="utf-8",
            )
            statuses.append("visqol_model_missing")
    else:
        statuses.append("visqol_missing")

    peaq = tools_dir / "PQevalAudio"
    peaq_log = metrics_dir / "peaq.txt"
    if peaq.exists():
        proc = run_command([str(peaq), str(metric_ref), str(metric_decoded)], timeout=timeout)
        peaq_log.write_text(proc.stdout, encoding="utf-8")
        if proc.returncode == 0:
            odg, distortion = parse_peaq(proc.stdout)
            metrics["peaq_odg"] = odg
            metrics["peaq_distortion_index"] = distortion
            statuses.append("peaq_ok")
        else:
            statuses.append("peaq_failed")
    else:
        statuses.append("peaq_missing")

    if not normalized:
        statuses.append("ffmpeg_normalization_missing_or_failed")

    metrics["perceptual_status"] = ";".join(statuses)
    return metrics


def write_report(output_dir: Path, rows: list[dict[str, str]], meta: dict[str, str]) -> None:
    report_path = output_dir / "report.html"
    grouped = group_rows_by_fixture(rows)
    comparison_rows = build_comparison_rows(grouped)
    plots = build_inline_plots(rows)

    matrix_cells = matrix_cells_for_rows(rows)
    comparison_html = "".join(
        "<tr>"
        + "".join(
            f"<td>{html.escape(cell)}</td>"
            for cell in [row["fixture"]]
            + [row[f"{cell}_status"] for cell in matrix_cells]
            + [row[f"{cell}_snr_db"] for cell in matrix_cells]
            + [row[f"{cell}_visqol_moslqo"] for cell in matrix_cells]
            + [row[f"{cell}_peaq_odg"] for cell in matrix_cells]
        )
        + "</tr>"
        for row in comparison_rows
    )
    status_headers = "".join(f"<th>{html.escape(cell)} Status</th>" for cell in matrix_cells)
    snr_headers = "".join(f"<th>{html.escape(cell)} SNR</th>" for cell in matrix_cells)
    visqol_headers = "".join(f"<th>{html.escape(cell)} ViSQOL</th>" for cell in matrix_cells)
    peaq_headers = "".join(f"<th>{html.escape(cell)} PEAQ ODG</th>" for cell in matrix_cells)

    summary_rows = []
    for row in rows:
        cells = [
            row.get("input", ""),
            row.get("encoder", ""),
            row.get("decoder", ""),
            row.get("status", ""),
            row.get("snr_db", ""),
            row.get("gain_adjusted_snr_db", ""),
            row.get("visqol_moslqo", ""),
            row.get("peaq_odg", ""),
            row.get("peaq_distortion_index", ""),
            row.get("decoded_path", ""),
            row.get("error", ""),
        ]
        summary_rows.append("<tr>" + "".join(f"<td>{html.escape(cell)}</td>" for cell in cells) + "</tr>")

    plots_html = "".join(
        f"""
  <section class="plot">
    <h3>{html.escape(title)}</h3>
    <img src="data:{mime};base64,{image}" alt="{html.escape(title)}">
  </section>
"""
        for title, mime, image in plots
    )
    if not plots_html:
        plots_html = "<p>No plots were generated. Install matplotlib and run with non-empty metric values.</p>"

    meta_items = "".join(
        f"<dt>{html.escape(key)}</dt><dd>{html.escape(value)}</dd>" for key, value in sorted(meta.items())
    )
    report_path.write_text(
        f"""<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>at3rs Evaluation Report</title>
  <style>
    body {{ font-family: ui-sans-serif, system-ui, sans-serif; margin: 2rem; line-height: 1.45; color: #1f2328; }}
    table {{ border-collapse: collapse; width: 100%; font-size: 0.86rem; margin: 1rem 0 2rem; }}
    th, td {{ border: 1px solid #d0d7de; padding: 0.45rem; text-align: left; vertical-align: top; }}
    th {{ background: #f6f8fa; }}
    dt {{ font-weight: 700; float: left; clear: left; width: 12rem; }}
    dd {{ margin: 0 0 0.35rem 13rem; }}
    code {{ background: #f6f8fa; padding: 0.1rem 0.25rem; }}
    .plot {{ margin: 1.5rem 0; }}
    .plot img {{ display: block; max-width: 100%; border: 1px solid #d0d7de; background: white; }}
    .note {{ color: #57606a; }}
  </style>
</head>
<body>
  <h1>at3rs Evaluation Report</h1>
  <dl>{meta_items}</dl>
  <h2>Side-by-Side Comparison</h2>
  <p class="note">Rows compare encoder/decoder cells. Higher is generally better for SNR, ViSQOL, and PEAQ ODG. Decoded WAV paths are listed in the raw rows table.</p>
  <table>
    <thead>
      <tr>
        <th>Fixture</th>
        {status_headers}
        {snr_headers}
        {visqol_headers}
        {peaq_headers}
      </tr>
    </thead>
    <tbody>{comparison_html}</tbody>
  </table>
  <h2>Plots</h2>
  {plots_html}
  <h2>Raw Round Trip Rows</h2>
  <table>
    <thead>
      <tr>
        <th>Input</th><th>Encoder</th><th>Decoder</th><th>Status</th><th>SNR dB</th><th>Gain-adjusted SNR dB</th>
        <th>ViSQOL MOS-LQO</th><th>PEAQ ODG</th><th>PEAQ DI</th><th>Decoded WAV</th><th>Error</th>
      </tr>
    </thead>
    <tbody>{''.join(summary_rows)}</tbody>
  </table>
  <p>Detailed logs and WAV files are next to this report under each fixture subdirectory.</p>
</body>
</html>
""",
        encoding="utf-8",
    )


def cell_name(encoder: str, decoder: str) -> str:
    return f"{encoder}->{decoder}"


def row_cell_name(row: dict[str, str]) -> str:
    return cell_name(row.get("encoder", ""), row.get("decoder", ""))


def matrix_cells_for_rows(rows: list[dict[str, str]]) -> list[str]:
    present = {row_cell_name(row) for row in rows}
    ordered = [cell_name(encoder, decoder) for encoder, decoder in MATRIX_CELLS if cell_name(encoder, decoder) in present]
    extras = sorted(present.difference(ordered))
    return ordered + extras


def group_rows_by_fixture(rows: list[dict[str, str]]) -> dict[str, dict[str, dict[str, str]]]:
    grouped: dict[str, dict[str, dict[str, str]]] = {}
    for row in rows:
        fixture = Path(row["input"]).name
        grouped.setdefault(fixture, {})[row_cell_name(row)] = row
    return grouped


def build_comparison_rows(grouped: dict[str, dict[str, dict[str, str]]]) -> list[dict[str, str]]:
    comparison_rows = []
    matrix_cells = matrix_cells_for_rows([row for cells in grouped.values() for row in cells.values()])
    for fixture in sorted(grouped):
        comparison_row = {"fixture": fixture}
        for cell in matrix_cells:
            row = grouped[fixture].get(cell, {})
            comparison_row[f"{cell}_status"] = row.get("status", "")
            comparison_row[f"{cell}_snr_db"] = row.get("snr_db", "")
            comparison_row[f"{cell}_visqol_moslqo"] = row.get("visqol_moslqo", "")
            comparison_row[f"{cell}_peaq_odg"] = row.get("peaq_odg", "")
        comparison_rows.append(comparison_row)
    return comparison_rows


def format_delta(left: str, right: str) -> str:
    left_float = parse_float(left)
    right_float = parse_float(right)
    if left_float is None or right_float is None:
        return ""
    return f"{left_float - right_float:+.2f}"


def parse_float(value: str) -> float | None:
    try:
        return float(value)
    except (TypeError, ValueError):
        return None


def build_inline_plots(rows: list[dict[str, str]]) -> list[tuple[str, str, str]]:
    try:
        import matplotlib

        matplotlib.use("Agg")
        import matplotlib.pyplot as plt
    except Exception as exc:
        log(f"report plots using SVG fallback: matplotlib unavailable ({exc})")
        plt = None

    plot_specs = [
        ("SNR dB", "snr_db"),
        ("Gain-adjusted SNR dB", "gain_adjusted_snr_db"),
        ("ViSQOL MOS-LQO", "visqol_moslqo"),
        ("PEAQ ODG", "peaq_odg"),
    ]
    grouped = group_rows_by_fixture(rows)
    matrix_cells = matrix_cells_for_rows(rows)
    plots = []
    for title, field in plot_specs:
        fixtures = []
        series_values = {cell: [] for cell in matrix_cells}
        for fixture in sorted(grouped):
            values = {
                cell: parse_float(grouped[fixture].get(cell, {}).get(field, ""))
                for cell in matrix_cells
            }
            if all(value is None for value in values.values()):
                continue
            fixtures.append(fixture)
            for cell in matrix_cells:
                series_values[cell].append(values[cell])

        if not fixtures:
            continue

        if plt is None:
            plots.append((title, "image/svg+xml", render_metric_plot_svg(title, fixtures, series_values)))
        else:
            plots.append(
                (
                    title,
                    "image/png",
                    render_metric_plot_png(plt, title, fixtures, series_values),
                )
            )
    return plots


def render_metric_plot_png(plt, title: str, fixtures: list[str], series_values: dict[str, list[float | None]]) -> str:
    x_values = list(range(len(fixtures)))
    series = list(series_values)
    width = min(0.18, 0.8 / max(1, len(series)))
    fig_width = max(9.0, len(fixtures) * 1.25)
    fig, ax = plt.subplots(figsize=(fig_width, 4.8), dpi=140)

    for series_index, cell in enumerate(series):
        offset = (series_index - (len(series) - 1) / 2) * width
        values = series_values[cell]
        ax.bar(
            [x + offset for x in x_values],
            [value if value is not None else 0.0 for value in values],
            width,
            label=cell,
            color=CELL_COLORS.get(cell, "#57606a"),
            alpha=0.86,
        )
        for x, value in zip(x_values, values):
            if value is None:
                ax.text(x + offset, 0.0, "n/a", rotation=90, ha="center", va="bottom", fontsize=7)

    ax.set_title(title)
    ax.set_ylabel(title)
    ax.set_xticks(x_values)
    ax.set_xticklabels(fixtures, rotation=35, ha="right")
    ax.grid(axis="y", color="#d8dee4", linewidth=0.7, alpha=0.8)
    ax.legend()
    fig.tight_layout()

    buffer = io.BytesIO()
    fig.savefig(buffer, format="png", bbox_inches="tight")
    plt.close(fig)
    return base64.b64encode(buffer.getvalue()).decode("ascii")


def render_metric_plot_svg(
    title: str,
    fixtures: list[str],
    series_values: dict[str, list[float | None]],
) -> str:
    series = list(series_values)
    values = [value for series_list in series_values.values() for value in series_list if value is not None]
    if not values:
        return ""
    min_value = min(0.0, min(values))
    max_value = max(0.0, max(values))
    if max_value == min_value:
        max_value += 1.0

    width = max(900, 120 * len(fixtures))
    height = 460
    left = 70
    right = 30
    top = 55
    bottom = 130
    plot_width = width - left - right
    plot_height = height - top - bottom
    group_width = plot_width / max(1, len(fixtures))
    bar_width = min(34, group_width * 0.28)
    zero_y = value_to_y(0.0, min_value, max_value, top, plot_height)

    parts = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}">',
        '<rect width="100%" height="100%" fill="white"/>',
        f'<text x="{width / 2:.1f}" y="28" text-anchor="middle" font-family="Arial, sans-serif" font-size="20" font-weight="700">{html.escape(title)}</text>',
        f'<line x1="{left}" y1="{zero_y:.1f}" x2="{width - right}" y2="{zero_y:.1f}" stroke="#57606a" stroke-width="1"/>',
        f'<line x1="{left}" y1="{top}" x2="{left}" y2="{top + plot_height}" stroke="#d0d7de" stroke-width="1"/>',
        f'<line x1="{left}" y1="{top + plot_height}" x2="{width - right}" y2="{top + plot_height}" stroke="#d0d7de" stroke-width="1"/>',
    ]
    legend_x = left
    for cell in series:
        color = CELL_COLORS.get(cell, "#57606a")
        parts.append(f'<rect x="{legend_x}" y="{height - 30}" width="14" height="14" fill="{color}" opacity="0.86"/>')
        parts.append(
            f'<text x="{legend_x + 18}" y="{height - 18}" font-family="Arial, sans-serif" font-size="12" fill="{color}">{html.escape(cell)}</text>'
        )
        legend_x += 18 + len(cell) * 7 + 22

    for tick in range(5):
        value = min_value + (max_value - min_value) * tick / 4
        y = value_to_y(value, min_value, max_value, top, plot_height)
        parts.append(f'<line x1="{left}" y1="{y:.1f}" x2="{width - right}" y2="{y:.1f}" stroke="#d8dee4" stroke-width="0.8"/>')
        parts.append(
            f'<text x="{left - 8}" y="{y + 4:.1f}" text-anchor="end" font-family="Arial, sans-serif" font-size="11" fill="#57606a">{value:.1f}</text>'
        )

    for i, fixture in enumerate(fixtures):
        center = left + group_width * (i + 0.5)
        for series_index, cell in enumerate(series):
            value = series_values[cell][i]
            offset = (series_index - (len(series) - 1) / 2) * bar_width * 1.1
            color = CELL_COLORS.get(cell, "#57606a")
            if value is None:
                parts.append(
                    f'<text x="{center + offset:.1f}" y="{zero_y - 4:.1f}" text-anchor="middle" font-family="Arial, sans-serif" font-size="10">n/a</text>'
                )
                continue
            y = value_to_y(value, min_value, max_value, top, plot_height)
            bar_y = min(y, zero_y)
            bar_h = max(1.0, abs(zero_y - y))
            parts.append(
                f'<rect x="{center + offset - bar_width / 2:.1f}" y="{bar_y:.1f}" width="{bar_width:.1f}" height="{bar_h:.1f}" fill="{color}" opacity="0.86"/>'
            )
            parts.append(
                f'<text x="{center + offset:.1f}" y="{bar_y - 5:.1f}" text-anchor="middle" font-family="Arial, sans-serif" font-size="10" fill="#24292f">{value:.1f}</text>'
            )
        parts.append(
            f'<text x="{center:.1f}" y="{top + plot_height + 18}" text-anchor="end" transform="rotate(-35 {center:.1f} {top + plot_height + 18})" font-family="Arial, sans-serif" font-size="11">{html.escape(fixture)}</text>'
        )

    parts.append("</svg>")
    return base64.b64encode("".join(parts).encode("utf-8")).decode("ascii")


def value_to_y(value: float, min_value: float, max_value: float, top: float, plot_height: float) -> float:
    return top + (max_value - value) / (max_value - min_value) * plot_height


def evaluate_fixture(
    wav: Path,
    index: int,
    total: int,
    output_dir: Path,
    bitrate: int,
    wine: Path,
    wine_env: dict[str, str],
    sony_tool: Path,
    tools_dir: Path,
    visqol_model: Path | None,
    skip_perceptual: bool,
    include_sony_reference: bool,
    timeout: int,
) -> list[dict[str, str]]:
    stem_dir = output_dir / wav.stem
    encoded_dir = stem_dir / "encoded"
    decoded_dir = stem_dir / "decoded"
    metrics_root = stem_dir / "metrics"
    encoded_dir.mkdir(parents=True, exist_ok=True)
    decoded_dir.mkdir(parents=True, exist_ok=True)
    metrics_root.mkdir(parents=True, exist_ok=True)
    for stale_metric in metrics_root.iterdir():
        if stale_metric.is_file():
            stale_metric.unlink()

    cells = MATRIX_CELLS if include_sony_reference else AT3RS_ONLY_CELLS
    rows = []
    for encoder, decoder in cells:
        rows.append(
            evaluate_round_trip(
                wav,
                index,
                total,
                encoder,
                decoder,
                encoded_dir / f"{encoder}.at3",
                decoded_dir / f"{encoder}_{decoder}.wav",
                metrics_root / f"{encoder}_{decoder}",
                bitrate,
                wine,
                wine_env,
                sony_tool,
                tools_dir,
                visqol_model,
                skip_perceptual,
                timeout,
            )
        )

    return rows


def evaluate_round_trip(
    wav: Path,
    index: int,
    total: int,
    encoder: str,
    decoder: str,
    encoded_at3: Path,
    decoded_wav: Path,
    metrics_dir: Path,
    bitrate: int,
    wine: Path,
    wine_env: dict[str, str],
    sony_tool: Path,
    tools_dir: Path,
    visqol_model: Path | None,
    skip_perceptual: bool,
    timeout: int,
) -> dict[str, str]:
    metrics_dir.mkdir(parents=True, exist_ok=True)
    error_path = metrics_dir / "error.txt"
    if error_path.exists():
        error_path.unlink()

    row = make_row(wav, encoder, decoder, encoded_at3, decoded_wav, metrics_dir)
    prefix = f"[{index}/{total}] {wav.name} {encoder}->{decoder}"

    try:
        log(f"{prefix}: encode")
        if encoder == "at3rs":
            encode = checked_command(
                [
                    str(REPO_ROOT / "target" / "release" / "at3rs"),
                    "-e",
                    str(wav),
                    str(encoded_at3),
                    str(bitrate),
                ],
                timeout=timeout,
            )
        elif encoder == "sony":
            encode = checked_command(
                [
                    str(wine),
                    str(sony_tool),
                    "-e",
                    "-br",
                    str(bitrate),
                    str(wav),
                    str(encoded_at3),
                ],
                env=wine_env,
                timeout=timeout,
            )
        else:
            raise EvalError(f"unsupported encoder: {encoder}")
        (metrics_dir / "encode.txt").write_text(encode.stdout, encoding="utf-8")

        log(f"{prefix}: decode")
        if decoder == "at3rs":
            decode = checked_command(
                [str(REPO_ROOT / "target" / "release" / "at3rs"), "-d", str(encoded_at3), str(decoded_wav)],
                timeout=timeout,
            )
        elif decoder == "sony":
            decode = checked_command(
                [str(wine), str(sony_tool), "-d", str(encoded_at3), str(decoded_wav)],
                env=wine_env,
                timeout=timeout,
            )
        else:
            raise EvalError(f"unsupported decoder: {decoder}")
        (metrics_dir / f"decode_{decoder}.txt").write_text(decode.stdout, encoding="utf-8")

        log(f"{prefix}: snr")
        snr = checked_command(
            [str(REPO_ROOT / "target" / "release" / "snr_test"), str(wav), str(decoded_wav)],
            timeout=timeout,
        )
        (metrics_dir / "snr.txt").write_text(snr.stdout, encoding="utf-8")
        row.update(parse_snr(snr.stdout))

        if not skip_perceptual:
            log(f"{prefix}: perceptual metrics")
            row.update(run_perceptual_metrics(wav, decoded_wav, metrics_dir, tools_dir, visqol_model, timeout))
        else:
            row["perceptual_status"] = "skipped_by_user"

        row["status"] = "ok"
        log(f"{prefix}: ok snr={row['snr_db']} visqol={row['visqol_moslqo'] or 'skipped'} peaq={row['peaq_odg'] or 'skipped'}")
    except Exception as exc:
        row["error"] = str(exc)
        error_path.write_text(row["error"], encoding="utf-8")
        log(f"{prefix}: failed")

    return row


def make_row(
    wav: Path,
    encoder: str,
    decoder: str,
    encoded_at3: Path,
    decoded_wav: Path,
    metrics_dir: Path,
) -> dict[str, str]:
    return {
        "input": str(wav),
        "encoder": encoder,
        "decoder": decoder,
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
        "encoded_path": str(encoded_at3),
        "decoded_path": str(decoded_wav),
        "metrics_dir": str(metrics_dir),
        "error": "",
    }


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input_dir", type=Path, help="directory containing flat .wav fixtures")
    parser.add_argument("--bitrate", type=int, default=132, help="ATRAC3 bitrate in kbps")
    parser.add_argument("--output-root", type=Path, default=REPO_ROOT / "output", help="output directory root")
    parser.add_argument("--sony-tool", type=Path, default=DEFAULT_SONY_TOOL, help="path to psp_at3tool.exe")
    parser.add_argument("--tools-dir", type=Path, help="directory containing visqol and PQevalAudio")
    parser.add_argument("--visqol-model", type=Path, help="optional ViSQOL model file")
    parser.add_argument("--skip-perceptual", action="store_true", help="skip ViSQOL and PEAQ")
    parser.add_argument(
        "--no-sony-reference",
        action="store_true",
        help="skip Sony reference encode; run at3rs encode with at3rs and Sony decode",
    )
    parser.add_argument("--no-build", action="store_true", help="do not build release tools first")
    parser.add_argument("--timeout", type=int, default=180, help="per-command timeout in seconds")
    parser.add_argument(
        "--jobs",
        type=int,
        default=int(os.environ.get("AT3RS_EVAL_JOBS", "1")),
        help="number of fixtures to evaluate concurrently",
    )
    return parser.parse_args(argv)


def run_fixture_evals(
    wavs: list[Path],
    output_dir: Path,
    args: argparse.Namespace,
    wine: Path,
    wine_env: dict[str, str],
    tools_dir: Path,
    visqol_model: Path | None,
) -> list[dict[str, str]]:
    if args.jobs < 1:
        raise EvalError("--jobs must be >= 1")

    total = len(wavs)
    jobs = min(args.jobs, total)
    if jobs == 1:
        return [
            row
            for index, wav in enumerate(wavs, start=1)
            for row in evaluate_fixture(
                wav,
                index,
                total,
                output_dir,
                args.bitrate,
                wine,
                wine_env,
                args.sony_tool,
                tools_dir,
                visqol_model,
                args.skip_perceptual,
                not args.no_sony_reference,
                args.timeout,
            )
        ]

    log(f"parallel jobs: {jobs}")
    results_by_index: dict[int, list[dict[str, str]]] = {}
    with concurrent.futures.ThreadPoolExecutor(max_workers=jobs) as executor:
        futures = {
            executor.submit(
                evaluate_fixture,
                wav,
                index,
                total,
                output_dir,
                args.bitrate,
                wine,
                wine_env,
                args.sony_tool,
                tools_dir,
                visqol_model,
                args.skip_perceptual,
                not args.no_sony_reference,
                args.timeout,
            ): index
            for index, wav in enumerate(wavs, start=1)
        }
        for future in concurrent.futures.as_completed(futures):
            index = futures[future]
            results_by_index[index] = future.result()

    return [row for index in sorted(results_by_index) for row in results_by_index[index]]


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    try:
        input_dir = args.input_dir.resolve()
        wavs = discover_wav_files(input_dir)
        if not args.sony_tool.exists():
            raise EvalError(f"Sony PSP decoder not found: {args.sony_tool}")
        build_release_tools(args.no_build, args.timeout)
        wine, wine_env = resolve_wine()
        tools_dir = resolve_tools_dir(args.tools_dir) if not args.skip_perceptual else Path("")
        visqol_model = (
            resolve_visqol_model(args.visqol_model, tools_dir) if not args.skip_perceptual else None
        )

        ref = git_ref_slug()
        output_dir = args.output_root / ref / "eval" / input_dir.name
        output_dir.mkdir(parents=True, exist_ok=True)
        log(f"evaluating {len(wavs)} WAV fixture(s)")
        log(f"sony reference: {'disabled' if args.no_sony_reference else 'enabled'}")
        log(f"jobs: {min(args.jobs, len(wavs))}")
        log(f"output: {output_dir}")

        meta = {
            "created_at": dt.datetime.now(dt.UTC).replace(microsecond=0).isoformat(),
            "git_ref": ref,
            "git_head": git_head(),
            "input_dir": str(input_dir),
            "bitrate_kbps": str(args.bitrate),
            "sony_tool": str(args.sony_tool),
            "wine": str(wine),
            "tools_dir": str(tools_dir) if tools_dir else "skipped",
            "visqol_model": str(visqol_model) if visqol_model else "",
            "sony_reference": "disabled" if args.no_sony_reference else "enabled",
            "jobs": str(min(args.jobs, len(wavs))),
        }
        (output_dir / "_meta.json").write_text(json.dumps(meta, indent=2) + "\n", encoding="utf-8")

        rows = run_fixture_evals(wavs, output_dir, args, wine, wine_env, tools_dir, visqol_model)

        summary_path = output_dir / "summary.csv"
        fieldnames = list(rows[0].keys())
        with summary_path.open("w", newline="", encoding="utf-8") as fh:
            writer = csv.DictWriter(fh, fieldnames=fieldnames)
            writer.writeheader()
            writer.writerows(rows)

        write_report(output_dir, rows, meta)
        print(f"wrote {output_dir}")
        failed = [row for row in rows if row["status"] != "ok"]
        return 1 if failed else 0
    except EvalError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
