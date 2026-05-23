#!/usr/bin/env python3
"""Re-extract cyberclaw assistant text from on-disk transcripts.

Original extract relied on `◆ assistant` / `▶ you` markers that scrolled
out of the captured pane when replies were long (>200 lines). This script
opens each empty-cb row's transcript file, applies a more tolerant
extractor (strip box-drawing, drop status bar + input area, keep main
conversation content), and writes a `judged.csv` precursor with the
recovered text — overwriting only `assistant_text` so the rest of the
row stays untouched.

Usage:
    reextract.py <run-dir>
"""
from __future__ import annotations

import argparse
import csv
import re
import sys
from pathlib import Path

# Markers to strip from pane content
BOX_CHARS = re.compile(r"[│┃┌┐└┘├┤┬┴┼┄┅┈┉─━┊]+")
STATUS_LINE = re.compile(r"\s*model:\s+[^\n]+\n", re.M)
INPUT_BLOCK = re.compile(r"┌ Enter 发送.*", re.S)
TUI_HEADER = re.compile(r" ██████.+?键位：[^\n]*\n.*?斜杠：[^\n]*\n", re.S)
EMPTY_LINES = re.compile(r"\n\s*\n\s*\n+", re.M)
LEADING_SPACES = re.compile(r"^\s+", re.M)


def extract_from_pane(pane: str, user_text: str) -> str:
    """Robust fallback: strip TUI chrome, keep conversation body."""
    # First try marker-based extraction (original logic)
    user_marker = user_text.strip()[:40]
    user_idx = pane.rfind(user_marker)
    if user_idx >= 0:
        m = re.search(
            r"◆ assistant[^\n]*\n(.*?)(?:\n\s*model:|\n\s*─ Enter 发送|\Z)",
            pane[user_idx:],
            re.S,
        )
        if m:
            body = BOX_CHARS.sub(" ", m.group(1))
            return body.strip()[:4000]

    # Fallback: ◆ assistant block anywhere
    matches = list(re.finditer(
        r"◆ assistant[^\n]*\n(.*?)(?:\n\s*model:|\n\s*─ Enter 发送|\Z)",
        pane, re.S,
    ))
    if matches:
        body = BOX_CHARS.sub(" ", matches[-1].group(1))
        return body.strip()[:4000]

    # ULTIMATE FALLBACK (this is the new bit):
    # Marker scrolled out. Take all visible pane content, strip the
    # TUI chrome (header logo, status bar, input area), and return
    # whatever's left in the conversation area.
    txt = pane
    txt = TUI_HEADER.sub("", txt)
    txt = INPUT_BLOCK.sub("", txt)
    txt = STATUS_LINE.sub("", txt)
    txt = BOX_CHARS.sub(" ", txt)
    # Trim leading whitespace per-line
    txt = LEADING_SPACES.sub("", txt)
    # Collapse triple+ blank lines
    txt = EMPTY_LINES.sub("\n\n", txt)
    # If we still see "Type your message…" it means input box wasn't fully stripped
    txt = txt.replace("Type your message…", "").replace("Type your message...", "")
    # Trim conversation marker if present
    txt = txt.replace("conversation", "", 1)
    return txt.strip()[:4000]


def load_transcript(path: Path) -> str:
    """Read transcript file, return just the pane section."""
    txt = path.read_text(encoding="utf-8", errors="replace")
    sep = txt.find("--- FULL PANE ---")
    return txt[sep + len("--- FULL PANE ---"):] if sep >= 0 else txt


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("run_dir", help="path to transcripts/run-<ts>/")
    ap.add_argument("--inplace", action="store_true", help="overwrite results.csv (backup .bak)")
    args = ap.parse_args()

    run_dir = Path(args.run_dir)
    csv_path = run_dir / "results.csv"
    if not csv_path.exists():
        sys.exit(f"no results.csv at {csv_path}")

    rows = list(csv.DictReader(csv_path.open()))
    print(f"loaded {len(rows)} rows", file=sys.stderr)

    recovered = 0
    still_empty = 0
    for row in rows:
        if row["agent"] != "cyberclaw":
            continue
        if row["assistant_text"].strip():
            continue
        # Find transcript file
        prompt_id = row["prompt_id"]
        rnd = row["round"]
        turn_idx = row["turn_idx"]
        t_path = run_dir / "cyberclaw" / prompt_id / f"round-{rnd}-turn-{turn_idx}.txt"
        if not t_path.exists():
            still_empty += 1
            continue
        pane = load_transcript(t_path)
        new_text = extract_from_pane(pane, row["user_text"])
        if new_text:
            row["assistant_text"] = new_text
            recovered += 1
        else:
            still_empty += 1

    print(f"recovered {recovered}, still_empty {still_empty}", file=sys.stderr)

    if args.inplace:
        backup = csv_path.with_suffix(".csv.bak")
        backup.write_text(csv_path.read_text())
        with csv_path.open("w", newline="") as f:
            w = csv.DictWriter(f, fieldnames=rows[0].keys())
            w.writeheader()
            w.writerows(rows)
        print(f"wrote updated CSV (backup at {backup})", file=sys.stderr)
    else:
        out = run_dir / "results.reextracted.csv"
        with out.open("w", newline="") as f:
            w = csv.DictWriter(f, fieldnames=rows[0].keys())
            w.writeheader()
            w.writerows(rows)
        print(f"wrote {out}", file=sys.stderr)


if __name__ == "__main__":
    main()
