import type { ComponentType } from "react";

export default function EmptyState({
  icon: IconComp,
  title,
  body,
  cta,
}: {
  icon?: ComponentType<{ size?: number; className?: string }>;
  title: string;
  body?: string;
  cta?: { label: string; onClick: () => void };
}) {
  return (
    <div className="py-12 text-center rounded-lg border border-border bg-bg-2">
      {IconComp && <div className="mx-auto mb-3 text-fg-4"><IconComp size={32} /></div>}
      <div className="text-sm text-fg font-medium">{title}</div>
      {body && <p className="text-xs text-fg-3 mt-1 max-w-md mx-auto">{body}</p>}
      {cta && (
        <button onClick={cta.onClick} className="mt-4 px-3 py-1.5 rounded-md bg-accent text-accent-fg text-[12px] hover:opacity-90">
          {cta.label}
        </button>
      )}
    </div>
  );
}
