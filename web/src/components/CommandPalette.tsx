// CommandPalette — Cmd+K global search overlay
// z-[60] (above Modal z-50), centered 20vh from top, width 480

import { useEffect, useRef, useState, type ComponentType } from "react";
import { type Lang } from "@/lib/i18n";

export interface PaletteItem {
  id: string;
  label: string;
  sub?: string;
  icon?: ComponentType<{ size?: number; className?: string }>;
  onSelect: () => void;
}

function fuzzyMatch(query: string, item: PaletteItem): boolean {
  if (!query) return true;
  const q = query.toLowerCase();
  const haystack = ((item.label ?? "") + " " + (item.sub ?? "")).toLowerCase();
  // substring match across label+sub
  return haystack.includes(q);
}

export default function CommandPalette({
  open,
  onClose,
  items,
  lang,
}: {
  open: boolean;
  onClose: () => void;
  items: PaletteItem[];
  lang?: Lang;
}) {
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLUListElement>(null);

  // Reset query + active on open
  useEffect(() => {
    if (open) {
      setQuery("");
      setActive(0);
      // autoFocus via ref after paint
      requestAnimationFrame(() => inputRef.current?.focus());
    }
  }, [open]);

  const filtered = items.filter((item) => fuzzyMatch(query, item)).slice(0, 12);

  // Clamp active when filtered list changes
  useEffect(() => {
    setActive((a) => Math.min(a, Math.max(0, filtered.length - 1)));
  }, [filtered.length]);

  // Scroll active row into view
  useEffect(() => {
    const list = listRef.current;
    if (!list) return;
    const row = list.children[active] as HTMLElement | undefined;
    row?.scrollIntoView({ block: "nearest" });
  }, [active]);

  const selectActive = () => {
    const item = filtered[active];
    if (item) {
      item.onSelect();
      onClose();
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((a) => Math.min(a + 1, filtered.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((a) => Math.max(a - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      selectActive();
    } else if (e.key === "Escape") {
      e.preventDefault();
      onClose();
    }
  };

  if (!open) return null;

  return (
    // Backdrop
    <div
      className="fixed inset-0 z-[60] flex justify-center"
      style={{ paddingTop: "20vh" }}
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        className="w-[480px] max-h-[420px] rounded-xl border border-border bg-bg-2 shadow-2xl flex flex-col overflow-hidden"
        onMouseDown={(e) => e.stopPropagation()}
      >
        {/* Search input */}
        <div className="border-b border-border px-3 py-2 flex items-center gap-2">
          <span className="text-fg-4 text-[13px]">⌘</span>
          <input
            ref={inputRef}
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setActive(0);
            }}
            onKeyDown={handleKeyDown}
            placeholder={lang === "zh-CN" ? "输入以搜索…" : "Type to search…"}
            className="flex-1 bg-transparent text-[13px] text-fg outline-none placeholder:text-fg-4"
          />
          {query && (
            <button
              onClick={() => setQuery("")}
              className="text-fg-4 hover:text-fg text-[11px]"
            >
              ✕
            </button>
          )}
        </div>

        {/* Results list */}
        <ul ref={listRef} className="flex-1 overflow-auto py-1">
          {filtered.length === 0 ? (
            <li className="px-4 py-3 text-[12px] text-fg-4">{lang === "zh-CN" ? "无匹配结果。" : "No results."}</li>
          ) : (
            filtered.map((item, idx) => {
              const isActive = idx === active;
              return (
                <li key={item.id}>
                  <button
                    onMouseEnter={() => setActive(idx)}
                    onClick={() => {
                      item.onSelect();
                      onClose();
                    }}
                    className={`w-full flex items-center gap-3 px-4 py-2 text-left transition-colors ${
                      isActive ? "bg-accent-soft text-accent" : "text-fg-2 hover:bg-hover"
                    }`}
                  >
                    {item.icon && (
                      <item.icon
                        size={15}
                        className={isActive ? "text-accent" : "text-fg-4"}
                      />
                    )}
                    <div className="flex-1 min-w-0">
                      <div className="text-[13px] truncate">{item.label}</div>
                      {item.sub && (
                        <div className="text-[10px] mono text-fg-4 truncate">{item.sub}</div>
                      )}
                    </div>
                  </button>
                </li>
              );
            })
          )}
        </ul>

        {/* Footer hint */}
        <div className="border-t border-border px-4 py-1.5 flex gap-3 text-[10px] mono text-fg-4">
          <span>↑↓ {lang === "zh-CN" ? "导航" : "navigate"}</span>
          <span>↵ {lang === "zh-CN" ? "打开" : "open"}</span>
          <span>Esc {lang === "zh-CN" ? "关闭" : "close"}</span>
        </div>
      </div>
    </div>
  );
}
