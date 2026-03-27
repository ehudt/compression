#!/usr/bin/env python3

import argparse
import csv
from pathlib import Path


LEGACY_RESULTS_COLUMNS = [
    "commit",
    "status",
    "ratio_notes",
    "compress_notes",
    "decompress_notes",
    "roundtrip_notes",
    "description",
]

RESULTS_COLUMNS = LEGACY_RESULTS_COLUMNS + [
    "campaign_id",
    "target",
    "axis",
    "levels",
    "rerun_status",
    "evidence_status",
    "per_file_notes",
]

CAMPAIGN_COLUMNS = [
    "campaign_id",
    "baseline_commit",
    "target",
    "axis",
    "levels",
    "status",
    "notes",
]

OUTCOME_PRIORITY = ["keep", "discard", "blocked", "inconclusive", "crash", "baseline"]


def read_tsv(path: Path) -> list[dict[str, str]]:
    if not path.exists():
        return []
    with path.open(newline="") as fh:
        reader = csv.DictReader(fh, delimiter="\t")
        return [dict(row) for row in reader]


def write_tsv(path: Path, fieldnames: list[str], rows: list[dict[str, str]]) -> None:
    with path.open("w", newline="") as fh:
        writer = csv.DictWriter(fh, fieldnames=fieldnames, delimiter="\t", lineterminator="\n")
        writer.writeheader()
        for row in rows:
            writer.writerow({name: row.get(name, "") for name in fieldnames})


def ensure_results(path: Path) -> str:
    if not path.exists():
        write_tsv(path, RESULTS_COLUMNS, [])
        return "created"

    rows = read_tsv(path)
    with path.open(newline="") as fh:
        reader = csv.DictReader(fh, delimiter="\t")
        fieldnames = list(reader.fieldnames or [])

    if fieldnames == RESULTS_COLUMNS:
        return "ok"

    if fieldnames == LEGACY_RESULTS_COLUMNS:
        write_tsv(path, RESULTS_COLUMNS, rows)
        return "migrated"

    missing = [name for name in RESULTS_COLUMNS if name not in fieldnames]
    merged = list(fieldnames)
    for required in RESULTS_COLUMNS:
        if required not in merged:
            merged.append(required)
    if missing:
        write_tsv(path, merged, rows)
        return "extended"
    return "ok"


def ensure_campaigns(path: Path) -> str:
    if not path.exists():
        write_tsv(path, CAMPAIGN_COLUMNS, [])
        return "created"

    rows = read_tsv(path)
    with path.open(newline="") as fh:
        reader = csv.DictReader(fh, delimiter="\t")
        fieldnames = list(reader.fieldnames or [])

    if fieldnames == CAMPAIGN_COLUMNS:
        return "ok"

    merged = list(fieldnames)
    for required in CAMPAIGN_COLUMNS:
        if required not in merged:
            merged.append(required)
    write_tsv(path, merged, rows)
    return "extended"


def primary_outcome(rows: list[dict[str, str]]) -> str:
    statuses = {row.get("status", "") for row in rows}
    for status in OUTCOME_PRIORITY:
        if status in statuses:
            return status
    return "unknown"


def summarize_commit(results_path: Path, commit: str) -> int:
    rows = read_tsv(results_path)
    matched = [row for row in rows if row.get("commit", "") and commit.startswith(row["commit"])]
    if not matched:
        print("found=0")
        print("primary_outcome=no_rows")
        print("status_counts=")
        print("campaign_ids=")
        print("detail=no matching results rows")
        return 0

    counts: dict[str, int] = {}
    campaign_ids: list[str] = []
    targets: list[str] = []
    for row in matched:
        status = row.get("status", "")
        counts[status] = counts.get(status, 0) + 1
        campaign_id = row.get("campaign_id", "")
        if campaign_id and campaign_id not in campaign_ids:
            campaign_ids.append(campaign_id)
        target = row.get("target", "")
        if target and target not in targets:
            targets.append(target)

    print("found=1")
    print(f"primary_outcome={primary_outcome(matched)}")
    print("status_counts=" + ",".join(f"{key}:{counts[key]}" for key in sorted(counts)))
    print("campaign_ids=" + ",".join(campaign_ids))
    detail = [f"{len(matched)} row(s)"]
    if campaign_ids:
        detail.append("campaigns=" + ",".join(campaign_ids))
    if targets:
        detail.append("targets=" + ",".join(targets))
    print("detail=" + "; ".join(detail))
    return 0


def main() -> None:
    parser = argparse.ArgumentParser(description="Maintain and summarize autoresearch logging files.")
    subparsers = parser.add_subparsers(dest="cmd", required=True)

    ensure_parser = subparsers.add_parser("ensure-files", help="Create or migrate logging TSV files.")
    ensure_parser.add_argument("--results", default="results.tsv")
    ensure_parser.add_argument("--campaigns", default="campaigns.tsv")

    summarize_parser = subparsers.add_parser("summarize-commit", help="Summarize results rows for one commit.")
    summarize_parser.add_argument("--results", default="results.tsv")
    summarize_parser.add_argument("--commit", required=True)

    args = parser.parse_args()

    if args.cmd == "ensure-files":
        print(f"results={ensure_results(Path(args.results))}")
        print(f"campaigns={ensure_campaigns(Path(args.campaigns))}")
        return

    if args.cmd == "summarize-commit":
        raise SystemExit(summarize_commit(Path(args.results), args.commit))


if __name__ == "__main__":
    main()
