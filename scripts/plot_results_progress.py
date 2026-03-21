#!/usr/bin/env python3

import argparse
import csv
import html
import re
from pathlib import Path


CORPORA = ["all_zeros/3", "repetitive/3", "binary_structured/3", "random/1"]
PLOTTED_STATUSES = {"baseline", "keep"}
FAST_CASE_INPUT_SIZE = 65536
UNIT_SCALE = {"MiB/s": 1.0, "GiB/s": 1024.0}
STATUS_COLORS = {"baseline": "#1f77b4", "keep": "#2ca02c", "discard": "#d62728"}

ABSOLUTE_CASE_RE = re.compile(r"([a-z_]+/\d)\s*=\s*([0-9.]+)-([0-9.]+)\s*(MiB/s|GiB/s)")
ARROW_CASE_RE = re.compile(r"([a-z_]+/\d)[^;]*?->\s*([0-9.]+)-([0-9.]+)\s*(MiB/s|GiB/s)")
PERCENT_RANGE_RE = re.compile(r"([+-]?\d+(?:\.\d+)?)%\s*to\s*([+-]?\d+(?:\.\d+)?)%")
PERCENT_SINGLE_RE = re.compile(r"([+-]?\d+(?:\.\d+)?)%")
RATIO_PAIR_RE = re.compile(r"([a-z_]+)=([0-9]+)B")


def midpoint(lo: str, hi: str, unit: str) -> float:
    return ((float(lo) + float(hi)) / 2.0) * UNIT_SCALE[unit]


def parse_absolute_cases(text: str) -> dict[str, float]:
    cases: dict[str, float] = {}
    for match in ABSOLUTE_CASE_RE.finditer(text):
        cases[match.group(1)] = midpoint(match.group(2), match.group(3), match.group(4))
    for match in ARROW_CASE_RE.finditer(text):
        cases[match.group(1)] = midpoint(match.group(2), match.group(3), match.group(4))
    return cases


def parse_relative_change(segment: str) -> float | None:
    lowered = segment.lower()
    if "flat" in lowered and not PERCENT_RANGE_RE.search(lowered) and not PERCENT_SINGLE_RE.search(lowered):
        return 0.0
    match = PERCENT_RANGE_RE.search(lowered)
    if match:
        return (float(match.group(1)) + float(match.group(2))) / 2.0 / 100.0
    values = [float(value) for value in PERCENT_SINGLE_RE.findall(lowered)]
    if values:
        return sum(values) / len(values) / 100.0
    return None


def parse_ratio(text: str, previous_ratio: float | None) -> float | None:
    pairs = RATIO_PAIR_RE.findall(text)
    if pairs:
        total_compressed = sum(int(value) for _, value in pairs)
        total_input = FAST_CASE_INPUT_SIZE * len(pairs)
        return total_input / total_compressed
    if "ratio unchanged" in text.lower():
        return previous_ratio
    return previous_ratio


def load_points(results_path: Path) -> list[dict[str, object]]:
    rows = list(csv.DictReader(results_path.open(), delimiter="\t"))
    state: dict[str, float] = {}
    ratio: float | None = None
    points: list[dict[str, object]] = []

    for index, row in enumerate(rows, start=1):
        if row["status"] not in PLOTTED_STATUSES:
            continue

        notes = row["compress_notes"]
        absolute_cases = parse_absolute_cases(notes)
        current = dict(state)
        current.update(absolute_cases)

        if not absolute_cases and state:
            for segment in notes.split(";"):
                segment = segment.strip()
                if not segment:
                    continue
                corpus = next((name for name in CORPORA if name in segment), None)
                if corpus is None:
                    continue
                delta = parse_relative_change(segment)
                if delta is None:
                    continue
                if corpus in current:
                    current[corpus] = current[corpus] * (1.0 + delta)

        if any(corpus not in current for corpus in CORPORA):
            continue

        ratio = parse_ratio(row["ratio_notes"], ratio)
        tp = sum(current[corpus] for corpus in CORPORA) / len(CORPORA)
        points.append(
            {
                "step": index,
                "status": row["status"],
                "tp": tp,
                "ratio": ratio,
                "description": row["description"],
            }
        )
        state = current

    return points


def render_svg(points: list[dict[str, object]], output_path: Path) -> None:
    width, height = 1100, 760
    left, right, top, bottom = 90, 40, 70, 110
    plot_w = width - left - right
    plot_h = height - top - bottom

    xs = [float(point["tp"]) for point in points]
    ys = [float(point["ratio"]) for point in points if point["ratio"] is not None]
    min_x, max_x = min(xs), max(xs)
    min_y, max_y = min(ys), max(ys)

    if max_x == min_x:
        max_x += 1.0
    if max_y == min_y:
        pad = max(0.05, min_y * 0.002)
        min_y -= pad
        max_y += pad
    else:
        ypad = (max_y - min_y) * 0.08
        min_y -= ypad
        max_y += ypad

    xpad = (max_x - min_x) * 0.05
    min_x -= xpad
    max_x += xpad

    def sx(value: float) -> float:
        return left + (value - min_x) / (max_x - min_x) * plot_w

    def sy(value: float) -> float:
        return top + plot_h - (value - min_y) / (max_y - min_y) * plot_h

    parts: list[str] = []
    parts.append(
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}">'
    )
    parts.append(
        '<defs><marker id="arrow" markerWidth="10" markerHeight="10" refX="8" refY="3" orient="auto" markerUnits="strokeWidth"><path d="M0,0 L0,6 L9,3 z" fill="#aaaaaa"/></marker></defs>'
    )
    parts.append('<rect width="100%" height="100%" fill="white"/>')
    parts.append(
        f'<text x="{width / 2}" y="34" text-anchor="middle" font-family="sans-serif" font-size="24">Baseline and Keep Progress from results.tsv</text>'
    )

    for index in range(6):
        xv = left + plot_w * index / 5
        yv = top + plot_h * index / 5
        parts.append(
            f'<line x1="{xv:.1f}" y1="{top}" x2="{xv:.1f}" y2="{top + plot_h}" stroke="#e5e5e5" stroke-width="1"/>'
        )
        parts.append(
            f'<line x1="{left}" y1="{yv:.1f}" x2="{left + plot_w}" y2="{yv:.1f}" stroke="#e5e5e5" stroke-width="1"/>'
        )

    parts.append(
        f'<line x1="{left}" y1="{top + plot_h}" x2="{left + plot_w}" y2="{top + plot_h}" stroke="#222" stroke-width="2"/>'
    )
    parts.append(f'<line x1="{left}" y1="{top}" x2="{left}" y2="{top + plot_h}" stroke="#222" stroke-width="2"/>')

    for index in range(6):
        xv = left + plot_w * index / 5
        xval = min_x + (max_x - min_x) * index / 5
        parts.append(
            f'<text x="{xv:.1f}" y="{top + plot_h + 28}" text-anchor="middle" font-family="sans-serif" font-size="14">{xval:.0f}</text>'
        )
        yv = top + plot_h - plot_h * index / 5
        yval = min_y + (max_y - min_y) * index / 5
        parts.append(
            f'<text x="{left - 12}" y="{yv + 5:.1f}" text-anchor="end" font-family="sans-serif" font-size="14">{yval:.3f}</text>'
        )

    parts.append(
        f'<text x="{width / 2}" y="{height - 30}" text-anchor="middle" font-family="sans-serif" font-size="18">TP: average compress throughput across fast cases (MiB/s)</text>'
    )
    parts.append(
        f'<text x="26" y="{height / 2}" text-anchor="middle" font-family="sans-serif" font-size="18" transform="rotate(-90 26 {height / 2})">Compression ratio (input bytes / compressed bytes)</text>'
    )

    if len({round(y, 9) for y in ys}) == 1:
        parts.append(
            f'<text x="{width / 2}" y="58" text-anchor="middle" font-family="sans-serif" font-size="13" fill="#666666">Ratio is flat because all plotted baseline and keep rows in results.tsv record the same compressed sizes.</text>'
        )

    for first, second in zip(points, points[1:]):
        parts.append(
            f'<line x1="{sx(float(first["tp"])):.1f}" y1="{sy(float(first["ratio"])):.1f}" x2="{sx(float(second["tp"])):.1f}" y2="{sy(float(second["ratio"])):.1f}" stroke="#aaaaaa" stroke-width="2" marker-end="url(#arrow)"/>'
        )

    for point in points:
        x = sx(float(point["tp"]))
        y = sy(float(point["ratio"]))
        color = STATUS_COLORS.get(str(point["status"]), "#333333")
        tooltip = html.escape(
            f'step {point["step"]}: {point["status"]} | TP={float(point["tp"]):.2f} MiB/s | ratio={float(point["ratio"]):.3f}% | {point["description"]}'
        )
        parts.append(
            f'<g><title>{tooltip}</title><circle cx="{x:.1f}" cy="{y:.1f}" r="7" fill="{color}" stroke="white" stroke-width="1.5"/><text x="{x + 10:.1f}" y="{y - 10:.1f}" font-family="sans-serif" font-size="12">{point["step"]}</text></g>'
        )

    legend_x = width - 200
    legend_y = 95
    parts.append(
        f'<rect x="{legend_x - 20}" y="{legend_y - 28}" width="180" height="92" fill="white" stroke="#cccccc"/>'
    )
    for index, (name, color) in enumerate(
        [("baseline", STATUS_COLORS["baseline"]), ("keep", STATUS_COLORS["keep"]), ("discard", STATUS_COLORS["discard"])]
    ):
        yy = legend_y + index * 26
        parts.append(f'<circle cx="{legend_x}" cy="{yy}" r="6" fill="{color}"/>')
        parts.append(f'<text x="{legend_x + 16}" y="{yy + 5}" font-family="sans-serif" font-size="14">{name}</text>')

    parts.append("</svg>")
    output_path.write_text("\n".join(parts))


def main() -> None:
    parser = argparse.ArgumentParser(description="Plot optimization progress from results.tsv as an SVG chart.")
    parser.add_argument(
        "results",
        nargs="?",
        default="results.tsv",
        help="Path to the input TSV file. Defaults to results.tsv in the current directory.",
    )
    parser.add_argument(
        "-o",
        "--output",
        default="results_progress_tp_vs_ratio.svg",
        help="Path to the output SVG file. Defaults to results_progress_tp_vs_ratio.svg.",
    )
    args = parser.parse_args()

    results_path = Path(args.results)
    output_path = Path(args.output)
    points = load_points(results_path)
    if not points:
        raise SystemExit(f"no plottable progress points found in {results_path}")

    render_svg(points, output_path)
    print(f"wrote {output_path}")
    ratios = [float(point["ratio"]) for point in points]
    if len({round(value, 9) for value in ratios}) == 1:
        print("note: ratio is constant because all plotted baseline and keep rows in results.tsv have identical ratio_notes")


if __name__ == "__main__":
    main()
