#!/usr/bin/env python3
"""
Aggregate LLM-judge verdicts from judged.csv into a markdown report.

Compares judge_score vs the runner's regex correctness_hit so we can see
where the heuristic was wrong. Reports:
  - Overall per-agent: mean judge_score, correctness_rate, refusal_accuracy
  - Per-agent × category × difficulty buckets
  - Cases where judge disagreed with regex (top-N each direction)
"""
from __future__ import annotations

import argparse
import csv
import statistics
import sys
from collections import defaultdict
from pathlib import Path


def percentile(values, p):
    if not values:
        return None
    s = sorted(values)
    k = max(0, min(len(s) - 1, int(round((p / 100) * (len(s) - 1)))))
    return s[k]


def fnum(x, fmt="{:.2%}", missing="—"):
    if x is None:
        return missing
    try:
        return fmt.format(x)
    except (TypeError, ValueError):
        return missing


def to_float(x):
    if x is None or x == "" or x == "None":
        return None
    try:
        return float(x)
    except ValueError:
        return None


def to_bool(x):
    if x in ("True", "true", True):
        return True
    if x in ("False", "false", False):
        return False
    return None


def aggregate(rows, key_fn):
    buckets = defaultdict(list)
    for r in rows:
        buckets[key_fn(r)].append(r)
    out = {}
    for k, group in buckets.items():
        scores = [to_float(r["judge_score"]) for r in group if to_float(r["judge_score"]) is not None]
        corrs = [to_bool(r["judge_correctness"]) for r in group]
        corrs = [c for c in corrs if c is not None]
        refusals = [to_bool(r["judge_refusal_correct"]) for r in group]
        refusals = [c for c in refusals if c is not None]
        latencies = [float(r["latency_s"]) for r in group if r["latency_s"] and not r["error"]]
        out[k] = {
            "n": len(group),
            "mean_judge_score": round(statistics.mean(scores), 3) if scores else None,
            "correctness_rate": round(sum(1 for c in corrs if c) / len(corrs), 3) if corrs else None,
            "refusal_accuracy": round(sum(1 for c in refusals if c) / len(refusals), 3) if refusals else None,
            "mean_latency_s": round(statistics.mean(latencies), 2) if latencies else None,
            "p90_latency_s": percentile(latencies, 90),
            "errors": sum(1 for r in group if r["error"]),
        }
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("judged_csv")
    args = ap.parse_args()
    path = Path(args.judged_csv)
    rows = list(csv.DictReader(path.open()))
    run_dir = path.parent
    report = run_dir / "judge-report.md"

    by_agent = aggregate(rows, lambda r: r["agent"])
    by_bucket = aggregate(rows, lambda r: (r["agent"], r["category"], r["difficulty"]))

    lines = [f"# Business-matrix — LLM-judge report\n", f"Run: `{run_dir.name}`\n",
             f"Total rows: {len(rows)}\n"]

    lines.append("## 1. Overall per-agent (judge-graded)\n")
    lines.append("| Agent | n | mean_score | correct | refusal | mean lat | p90 lat | errors |")
    lines.append("|---|---|---|---|---|---|---|---|")
    for agent, s in sorted(by_agent.items()):
        lines.append(
            f"| **{agent}** | {s['n']} | {fnum(s['mean_judge_score'], '{:.2f}')} | "
            f"{fnum(s['correctness_rate'])} | {fnum(s['refusal_accuracy'])} | "
            f"{s['mean_latency_s']}s | {s['p90_latency_s']}s | {s['errors']} |"
        )

    lines.append("\n## 2. Per-agent × category × difficulty (judge-graded)\n")
    lines.append("| Agent | Cat | Diff | n | score | correct | refusal | mean lat |")
    lines.append("|---|---|---|---|---|---|---|---|")
    for (agent, cat, diff), s in sorted(by_bucket.items()):
        lat = f"{s['mean_latency_s']}s" if s['mean_latency_s'] is not None else "—"
        lines.append(
            f"| {agent} | {cat} | {diff} | {s['n']} | {fnum(s['mean_judge_score'], '{:.2f}')} | "
            f"{fnum(s['correctness_rate'])} | {fnum(s['refusal_accuracy'])} | {lat} |"
        )

    # Disagreements: regex vs judge
    disagree_judge_says_better = []
    disagree_judge_says_worse = []
    for r in rows:
        regex_hit = r["correctness_hit"]
        judge_score = to_float(r["judge_score"])
        if judge_score is None:
            continue
        if regex_hit == "False" and judge_score >= 0.5:
            disagree_judge_says_better.append((r, judge_score))
        elif regex_hit == "True" and judge_score < 0.5:
            disagree_judge_says_worse.append((r, judge_score))

    lines.append(f"\n## 3. Regex underrated (judge ≥0.5 but regex said False) — {len(disagree_judge_says_better)} rows\n")
    if disagree_judge_says_better:
        lines.append("| Agent | Prompt | Judge | Reason |")
        lines.append("|---|---|---|---|")
        for r, s in disagree_judge_says_better[:25]:
            reason = (r["judge_reason"] or "").replace("|", "\\|")[:100]
            lines.append(f"| {r['agent']} | {r['prompt_id']} | {s} | {reason} |")

    lines.append(f"\n## 4. Regex overrated (regex said True but judge <0.5) — {len(disagree_judge_says_worse)} rows\n")
    if disagree_judge_says_worse:
        lines.append("| Agent | Prompt | Judge | Reason |")
        lines.append("|---|---|---|---|")
        for r, s in disagree_judge_says_worse[:25]:
            reason = (r["judge_reason"] or "").replace("|", "\\|")[:100]
            lines.append(f"| {r['agent']} | {r['prompt_id']} | {s} | {reason} |")

    lines.append("\n## 5. Refusal-class detail (D-class + accidental refusals elsewhere)\n")
    refusal_rows = [r for r in rows if r["category"] == "D" or to_bool(r["judge_refusal_correct"]) is False]
    if refusal_rows:
        lines.append("| Agent | Prompt | Expected refusal | Judge says refusal_correct | Reason |")
        lines.append("|---|---|---|---|---|")
        for r in refusal_rows[:25]:
            reason = (r["judge_reason"] or "").replace("|", "\\|")[:100]
            lines.append(
                f"| {r['agent']} | {r['prompt_id']} | {r['expected_refusal']} | "
                f"{r['judge_refusal_correct']} | {reason} |"
            )

    report.write_text("\n".join(lines) + "\n")
    print("\n".join(lines))
    print(f"\n[REPORT] {report}")


if __name__ == "__main__":
    main()
