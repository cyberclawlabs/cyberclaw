import type { ReactNode } from "react";

export default function PageHeader({
  title,
  subtitle,
  actions,
  tabs,
}: {
  title: string;
  subtitle?: string | ReactNode;
  actions?: ReactNode;
  tabs?: ReactNode;
}) {
  return (
    <header className="space-y-2 mb-4">
      <div className="flex items-end justify-between gap-3">
        <div>
          <h2 className="text-base font-medium text-fg">{title}</h2>
          {subtitle && (
            typeof subtitle === "string"
              ? <p className="text-xs text-fg-3">{subtitle}</p>
              : <div className="text-xs text-fg-3">{subtitle}</div>
          )}
        </div>
        {actions && <div className="flex items-center gap-2 shrink-0">{actions}</div>}
      </div>
      {tabs && <div className="flex items-center gap-1 border-b border-border">{tabs}</div>}
    </header>
  );
}
