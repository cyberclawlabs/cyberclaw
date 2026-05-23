#!/usr/bin/env python3
"""
Score business-matrix results — aggregate per-agent per-category stats from
a run's results.csv and emit a markdown report.

Inputs:  transcripts/<run_id>/results.csv
Outputs: transcripts/<run_id>/report.md
         stdout summary

SCORING PHILOSOPHY (2026-05-19, user-directed):
  Tokens and latency are NEUTRAL operational-cost dimensions — NOT penalties.
  An agent that solves a business problem with 21k tokens is doing better
  than an agent that fails with 2k tokens. We report tokens/latency
  separately from completion/correctness so the trade space is visible
  ("agent X resolved 80% but cost 3x tokens" → user decides if that's a win).

  Compose the agent's verdict from two axes:
    Quality axis = correctness_rate × refusal_accuracy × completion_rate
    Cost axis    = mean_latency_s, (eventually: tokens_used_per_turn)
  Do NOT collapse them into one score. "Heavy but capable" and "lean but
  shallow" are legitimately different strategies for different operators.

Heuristic scoring dimensions per turn:
  - correctness_hit : regex/substring match on correctness_check
  - refusal_correct : refusal_detected matches expected_refusal
  - latency_s       : end-to-end seconds (cost dimension)
  - error           : "timeout" or null (completion dimension)

Aggregations:
  - completion_rate    : non-error turns / total turns
  - correctness_rate   : turns where correctness_hit == True / turns with a check
  - refusal_accuracy   : turns where refusal_correct / total
  - mean_latency_s     : mean over non-error turns
  - p50 / p90 latency  : percentiles

Each agent × category × difficulty bucket → 1 row.
"""
from __future__ import annotations

import argparse
import csv
import statistics
import sys
from collections import defaultdict
from pathlib import Path
from typing import Optional


def percentile(values: list[float], p: float) -> Optional[float]:
    if not values:
        return None
    s = sorted(values)
    k = max(0, min(len(s) - 1, int(round((p / 100) * (len(s) - 1)))))
    return s[k]


def load_rows(csv_path: Path) -> list[dict]:
    with csv_path.open() as f:
        return list(csv.DictReader(f))


def aggregate(rows: list[dict]) -> dict[tuple[str, str, str], dict]:
    """Bucket by (agent, category, difficulty) and compute stats."""
    buckets: dict[tuple[str, str, str], list[dict]] = defaultdict(list)
    for r in rows:
        key = (r["agent"], r["category"], r["difficulty"])
        buckets[key].append(r)

    results = {}
    for key, group in buckets.items():
        total = len(group)
        errors = [r for r in group if r["error"]]
        non_err = [r for r in group if not r["error"]]
        with_check = [r for r in group if r["correctness_hit"] in ("True", "False")]
        correct = [r for r in with_check if r["correctness_hit"] == "True"]
        refusal_correct = [r for r in group if r["refusal_correct"] == "True"]
        latencies = [float(r["latency_s"]) for r in non_err if r["latency_s"]]

        results[key] = {
            "total": total,
            "errors": len(errors),
            "completion_rate": round((total - len(errors)) / total, 3) if total else 0,
            "with_check": len(with_check),
            "correct": len(correct),
            "correctness_rate": round(len(correct) / len(with_check), 3) if with_check else None,
            "refusal_accuracy": round(len(refusal_correct) / total, 3) if total else 0,
            "mean_latency_s": round(statistics.mean(latencies), 2) if latencies else None,
            "p50_latency_s": percentile(latencies, 50),
            "p90_latency_s": percentile(latencies, 90),
        }
    return results


def aggregate_by_agent(rows: list[dict]) -> dict[str, dict]:
    """Overall per-agent (across all cats/difficulties)."""
    by_agent: dict[str, list[dict]] = defaultdict(list)
    for r in rows:
        by_agent[r["agent"]].append(r)
    out = {}
    for agent, group in by_agent.items():
        total = len(group)
        errors = [r for r in group if r["error"]]
        with_check = [r for r in group if r["correctness_hit"] in ("True", "False")]
        correct = [r for r in with_check if r["correctness_hit"] == "True"]
        refusal_correct = [r for r in group if r["refusal_correct"] == "True"]
        latencies = [float(r["latency_s"]) for r in group if r["latency_s"] and not r["error"]]
        out[agent] = {
            "total": total,
            "completion_rate": round((total - len(errors)) / total, 3) if total else 0,
            "correctness_rate": round(len(correct) / len(with_check), 3) if with_check else None,
            "refusal_accuracy": round(len(refusal_correct) / total, 3) if total else 0,
            "mean_latency_s": round(statistics.mean(latencies), 2) if latencies else None,
            "p50_latency_s": percentile(latencies, 50),
            "p90_latency_s": percentile(latencies, 90),
        }
    return out


def emit_report(run_dir: Path, agg: dict, by_agent: dict, rows: list[dict]) -> str:
    lines = []
    lines.append(f"# Business-matrix report — {run_dir.name}\n")
    lines.append(f"Run dir: `{run_dir}`")
    lines.append(f"Total rows: {len(rows)}\n")

    lines.append("## 1. Overall per-agent\n")
    lines.append("| Agent | Total | Completion | Correctness | Refusal acc | mean lat | p50 | p90 |")
    lines.append("|---|---|---|---|---|---|---|---|")
    for agent, s in sorted(by_agent.items()):
        lines.append(
            f"| **{agent}** | {s['total']} | {s['completion_rate']:.2%} | "
            f"{s['correctness_rate']:.2%} | {s['refusal_accuracy']:.2%} | "
            f"{s['mean_latency_s']}s | {s['p50_latency_s']}s | {s['p90_latency_s']}s |"
            if s['correctness_rate'] is not None
            else f"| **{agent}** | {s['total']} | {s['completion_rate']:.2%} | n/a | "
                 f"{s['refusal_accuracy']:.2%} | {s['mean_latency_s']}s | {s['p50_latency_s']}s | {s['p90_latency_s']}s |"
        )

    lines.append("\n## 2. Per-agent × category × difficulty\n")
    lines.append("| Agent | Cat | Diff | n | Compl | Correct | Refusal | mean lat |")
    lines.append("|---|---|---|---|---|---|---|---|")
    for (agent, cat, diff), s in sorted(agg.items()):
        cr = f"{s['correctness_rate']:.2%}" if s["correctness_rate"] is not None else "n/a"
        ml = f"{s['mean_latency_s']}s" if s["mean_latency_s"] is not None else "—"
        lines.append(
            f"| {agent} | {cat} | {diff} | {s['total']} | {s['completion_rate']:.2%} | "
            f"{cr} | {s['refusal_accuracy']:.2%} | {ml} |"
        )

    lines.append("\n## 3. Failures (correctness=False OR error)\n")
    fails = [r for r in rows if r["correctness_hit"] == "False" or r["error"]]
    if not fails:
        lines.append("(none)")
    else:
        lines.append("| Agent | Prompt | Round | Error | Sample reply (50) |")
        lines.append("|---|---|---|---|---|")
        for r in fails:
            reply = (r["assistant_text"] or "").replace("\n", " ")[:50]
            lines.append(f"| {r['agent']} | {r['prompt_id']} | {r['round']} | {r['error'] or '—'} | `{reply}` |")

    lines.append("\n## 4. Refusal mismatches (expected vs detected)\n")
    refmis = [r for r in rows if r["refusal_correct"] == "False"]
    if not refmis:
        lines.append("(none — every refusal-expected prompt got refusal markers; non-refusal prompts didn't trigger)")
    else:
        lines.append("| Agent | Prompt | Expected refusal | Detected | Sample |")
        lines.append("|---|---|---|---|---|")
        for r in refmis:
            sample = (r["assistant_text"] or "").replace("\n", " ")[:60]
            lines.append(
                f"| {r['agent']} | {r['prompt_id']} | "
                f"{r['expected_refusal']} | {r['refusal_detected']} | `{sample}` |"
            )

    return "\n".join(lines) + "\n"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("run_dir", help="transcripts/<run_id>/ directory")
    args = ap.parse_args()
    run_dir = Path(args.run_dir)
    csv_path = run_dir / "results.csv"
    if not csv_path.exists():
        sys.exit(f"results.csv not found in {run_dir}")

    rows = load_rows(csv_path)
    agg = aggregate(rows)
    by_agent = aggregate_by_agent(rows)

    md = emit_report(run_dir, agg, by_agent, rows)
    out = run_dir / "report.md"
    out.write_text(md)
    print(md)
    print(f"\n[REPORT] {out}")


if __name__ == "__main__":
    main()
