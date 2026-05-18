// Topbar — port v1 shell.jsx 顶栏：search / theme / lang / operator avatar / logout

import { useNavigate } from "react-router-dom";
import { type Lang, t } from "@/lib/i18n";
import { type Operator, logout } from "@/lib/api";
import ContextRing from "./ContextRing";

type Theme = "dark" | "light" | "midnight" | "sepia";

export default function Topbar({
  lang,
  onLangChange,
  theme,
  onThemeChange,
  collapsed,
  onCollapse,
  operator,
  onOpenPalette,
}: {
  lang: Lang;
  onLangChange: (l: Lang) => void;
  theme: Theme;
  onThemeChange: (t: Theme) => void;
  collapsed: boolean;
  onCollapse: () => void;
  operator: Operator | null;
  onOpenPalette: () => void;
}) {
  const navigate = useNavigate();
  const handleLogout = () => {
    logout();
    navigate("/login", { replace: true });
  };

  const initials = (operator?.display_name || operator?.user_id || "OP")
    .split(/\s+/)
    .map((s) => s[0] ?? "")
    .join("")
    .slice(0, 2)
    .toUpperCase();

  return (
    <header className="h-11 border-b border-border bg-bg-2 flex items-center px-3 gap-2 shrink-0">
      <button
        onClick={onCollapse}
        aria-label={lang === "zh-CN"
          ? (collapsed ? "展开侧边栏" : "收起侧边栏")
          : (collapsed ? "Expand sidebar" : "Collapse sidebar")}
        title={lang === "zh-CN"
          ? (collapsed ? "展开侧边栏" : "收起侧边栏")
          : (collapsed ? "Expand sidebar" : "Collapse sidebar")}
        className="h-7 w-7 flex items-center justify-center rounded-md text-fg-3 hover:text-fg hover:bg-hover transition-colors"
      >
        <span aria-hidden="true">{collapsed ? "›" : "‹"}</span>
      </button>
      <div className="flex-1" />
      {/* Token usage ring — refreshes every 30s, hover for breakdown */}
      <ContextRing lang={lang} />
      {/* Cmd+K hint button */}
      <button
        onClick={onOpenPalette}
        title={lang === "zh-CN" ? "命令面板 (⌘K)" : "Command Palette (⌘K)"}
        className="h-7 px-2 text-[11px] mono rounded-md bg-bg-3 border border-border text-fg-3 hover:text-fg hover:bg-hover transition-colors"
      >
        ⌘K
      </button>
      <select
        value={theme}
        onChange={(e) => onThemeChange(e.target.value as Theme)}
        title={lang === "zh-CN" ? "主题预设" : "Theme preset"}
        className="h-7 px-2 text-[11px] mono rounded-md bg-bg-3 border border-border text-fg-2 outline-none focus-ring shrink-0"
        style={{ minWidth: 86 }}
      >
        <option value="dark">{lang === "zh-CN" ? "暗色" : "Dark"}</option>
        <option value="light">{lang === "zh-CN" ? "亮色" : "Light"}</option>
        <option value="midnight">{lang === "zh-CN" ? "深黑" : "Midnight"}</option>
        <option value="sepia">{lang === "zh-CN" ? "护眼" : "Sepia"}</option>
      </select>
      <select
        value={lang}
        onChange={(e) => onLangChange(e.target.value as Lang)}
        title="Language"
        className="h-7 px-2 text-[11px] mono rounded-md bg-bg-3 border border-border text-fg-2 outline-none focus-ring shrink-0"
        style={{ minWidth: 56 }}
      >
        <option value="en">EN</option>
        <option value="zh-CN">中文</option>
      </select>
      {operator && (
        <button
          type="button"
          onClick={() => navigate("/me")}
          title={lang === "zh-CN" ? "查看我的账户" : "View my account"}
          className="flex items-center gap-2 ml-1 px-1 py-0.5 rounded-md hover:bg-hover transition-colors"
        >
          <div className="h-7 w-7 rounded-full bg-accent-soft flex items-center justify-center text-[10px] mono font-semibold text-accent">
            {initials}
          </div>
          <div className="text-[11px] leading-tight text-left">
            <div className="text-fg">{operator.display_name || operator.user_id}</div>
            <div className="text-fg-4 mono text-[9px]">{operator.role}</div>
          </div>
        </button>
      )}
      <button
        onClick={handleLogout}
        className="h-7 px-2 text-[11px] mono rounded-md bg-bg-3 border border-border text-fg-3 hover:text-fg hover:bg-hover transition-colors"
      >
        {t("auth.logout", lang)}
      </button>
    </header>
  );
}
