// ContextRing — Hermes-parity token-usage indicator. Sits in the Topbar
// and shows current day's total LLM token consumption as a circular ring
// with a tooltip breakdown (in / out / by-model).
//
// Refreshes every 30s. Errors are silent (component renders nothing if
// the /api/v1/usage probe fails).

import { useEffect, useState } from "react";
import { fetchUsage, type UsageInfo } from "@/lib/api";
import { type Lang } from "@/lib/i18n";

interface Props {
  lang: Lang;
  /** Soft token budget for the ring's full-circumference reading.
   *  Default 1_000_000 matches typical mid-tier daily allowances. */
  budget?: number;
}

function fmtNum(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return `${n}`;
}

export default function ContextRing({ lang, budget = 1_000_000 }: Props) {
  const [usage, setUsage] = useState<UsageInfo | null>(null);

  useEffect(() => {
    let cancelled = false;
    const load = async () => {
      try {
        const u = await fetchUsage();
        if (!cancelled) setUsage(u);
      } catch {
        // Silent — Ring just doesn't appear if probe fails.
      }
    };
    load();
    const id = setInterval(load, 30_000);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, []);

  if (!usage) return null;

  const total = usage.total_input_tokens + usage.total_output_tokens;
  const ratio = Math.min(1, total / budget);
  const R = 11; // SVG radius
  const C = 2 * Math.PI * R; // circumference
  const offset = C * (1 - ratio);

  // Color tier: <50% accent, 50-80% yellow, >80% red.
  const stroke = ratio < 0.5 ? "currentColor" : ratio < 0.8 ? "#facc15" : "#ef4444";

  const tooltip =
    lang === "zh-CN"
      ? `今日 token 用量：${fmtNum(total)} / ${fmtNum(budget)} (${(ratio * 100).toFixed(1)}%)\n输入：${fmtNum(usage.total_input_tokens)}\n输出：${fmtNum(usage.total_output_tokens)}`
      : `Today's tokens: ${fmtNum(total)} / ${fmtNum(budget)} (${(ratio * 100).toFixed(1)}%)\nInput: ${fmtNum(usage.total_input_tokens)}\nOutput: ${fmtNum(usage.total_output_tokens)}`;

  return (
    <div
      className="flex items-center gap-1.5 text-[11px] mono text-fg-3 px-2 py-1 rounded hover:bg-bg-2 cursor-help shrink-0"
      title={tooltip}
      aria-label={tooltip}
    >
      <svg width={26} height={26} viewBox="0 0 26 26">
        <circle
          cx={13}
          cy={13}
          r={R}
          fill="none"
          stroke="currentColor"
          strokeWidth={2}
          opacity={0.2}
        />
        <circle
          cx={13}
          cy={13}
          r={R}
          fill="none"
          stroke={stroke}
          strokeWidth={2}
          strokeDasharray={C}
          strokeDashoffset={offset}
          strokeLinecap="round"
          transform="rotate(-90 13 13)"
          style={{ transition: "stroke-dashoffset .3s ease, stroke .2s ease" }}
        />
      </svg>
      <span>{fmtNum(total)}</span>
    </div>
  );
}
