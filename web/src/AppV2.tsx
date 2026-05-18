// v2 stack 完整 shell（v1 视觉等同）。
// - Sidebar：5 段分组 28 项 nav（migrated 项有 v2 徽标 + accent 色 icon）
// - Topbar：折叠 / theme 切换 / lang / operator avatar / logout
// - Routes：已迁移页（Status/Models/Logs）渲染真组件；其它路径 fallback 到
//   PendingPage 显示"该页待 F5 迁移，老 stack /admin 仍可用"+ 直链跳过去
// - basename '/admin/v2'、theme persistence 与老 stack 共享 localStorage key

import { useEffect, useState, type ReactNode } from "react";
import { ToastProvider } from "@/components/ToastBar";
import {
  BrowserRouter,
  Navigate,
  Route,
  Routes,
  useLocation,
  useNavigate,
} from "react-router-dom";
import {
  type AboutInfo,
  type Operator,
  type UsageInfo,
  type Conversation,
  type CronJob,
  type Task,
  type AuditEntry,
  currentOperator,
  fetchAbout,
  fetchUsage,
  fetchConversations,
  fetchAuditLogs,
  fetchCronJobs,
  fetchTasks,
  isAuthenticated,
  login,
} from "@/lib/api";
import { type Lang, getLang, setLang, t } from "@/lib/i18n";
import Sidebar, { getAllPaths } from "@/components/Sidebar";
import Topbar from "@/components/Topbar";
import OnboardingBanner from "@/components/OnboardingBanner";
import CommandPalette, { type PaletteItem } from "@/components/CommandPalette";
import ChatPage from "@/pages/ChatPage";
import OverviewPage from "@/pages/OverviewPage";
import DocsPage from "@/pages/DocsPage";
import UploadsPage from "@/pages/UploadsPage";
import { usePlugins } from "@/plugins/usePlugins";
import PluginRoute from "@/plugins/PluginRoute";
import LogsPage from "@/pages/LogsPage";
import ModelsPage from "@/pages/ModelsPage";
import AgentsPage from "@/pages/AgentsPage";
import SkillsPage from "@/pages/SkillsPage";
import MemoryPage from "@/pages/MemoryPage";
import TasksPage from "@/pages/TasksPage";
import ReviewsPage from "@/pages/ReviewsPage";
import AuditPage from "@/pages/AuditPage";
import CapabilitiesPage from "@/pages/CapabilitiesPage";
import ChannelsPage from "@/pages/ChannelsPage";
import SettingsPage from "@/pages/SettingsPage";
import ClarificationsPage from "@/pages/ClarificationsPage";
import NodesPage from "@/pages/NodesPage";
import LearningPage from "@/pages/LearningPage";
import ClusterPage from "@/pages/ClusterPage";
import AdminOpsPage from "@/pages/AdminOpsPage";
import ImPlatformsPage from "@/pages/ImPlatformsPage";
import CronPage from "@/pages/CronPage";
import PluginsPage from "@/pages/PluginsPage";
import SessionsPage from "@/pages/SessionsPage";
import MoaPage from "@/pages/MoaPage";
import ProfilesPage from "@/pages/ProfilesPage";
import MePage from "@/pages/MePage";
import SecurityPage from "@/pages/SecurityPage";
import TracePage from "@/pages/TracePage";

/// Theme presets — 4 options. Adding more: register a `.<name>` block
/// in index.css with the same CSS-var contract (`--bg`/`--fg`/etc), then
/// include the name here. The early head script in `web/index.html`
/// allow-lists these names to avoid first-paint flicker.
type Theme = "dark" | "light" | "midnight" | "sepia";
const THEMES: Theme[] = ["dark", "light", "midnight", "sepia"];
const TWEAKS_KEY = "cyberclaw.admin.tweaks";

function loadTweaks(): { theme: Theme; collapsed: boolean } {
  try {
    const t = JSON.parse(localStorage.getItem(TWEAKS_KEY) || "{}") as {
      theme?: string;
      collapsed?: boolean;
    };
    // P1-A fix: validate against the full THEMES array. Previously hard-coded
    // to "light" else "dark", so midnight/sepia silently collapsed to dark on
    // every page reload — user-visible theme change appeared not to persist.
    const theme: Theme = (THEMES as readonly string[]).includes(t.theme ?? "")
      ? (t.theme as Theme)
      : "dark";
    return { theme, collapsed: !!t.collapsed };
  } catch {
    return { theme: "dark", collapsed: false };
  }
}
function saveTweak(patch: Partial<{ theme: Theme; collapsed: boolean }>): void {
  try {
    const cur = JSON.parse(localStorage.getItem(TWEAKS_KEY) || "{}");
    localStorage.setItem(TWEAKS_KEY, JSON.stringify({ ...cur, ...patch }));
  } catch {}
}

export default function AppV2() {
  const [lang, setLangState] = useState<Lang>(getLang());
  const init = loadTweaks();
  const [theme, setTheme] = useState<Theme>(init.theme);
  const [collapsed, setCollapsed] = useState<boolean>(init.collapsed);
  // Plugin manifests — loaded once on mount. Errors are swallowed inside
  // the hook (returns empty list) so the shell still renders for users
  // who hit a 401 or whose backend is offline.
  const { mountable: pluginItems } = usePlugins();

  // 应用 theme class —— body 由 React mount 后管理，html 由 head 早脚本管理；
  // 这里同时管两处保证 React 切换主题时整页生效（head script 不会重跑）
  useEffect(() => {
    document.documentElement.classList.remove(...THEMES);
    document.documentElement.classList.add(theme);
    document.body.classList.remove(...THEMES);
    document.body.classList.add(theme);
    saveTweak({ theme });
  }, [theme]);

  const onLangChange = (l: Lang) => {
    setLang(l);
    setLangState(l);
  };
  const onCollapse = () => {
    const next = !collapsed;
    setCollapsed(next);
    saveTweak({ collapsed: next });
  };

  return (
    <ToastProvider>
    <BrowserRouter basename="/admin/v2">
      <Routes>
        <Route
          path="/login"
          element={<LoginPage lang={lang} onLangChange={onLangChange} />}
        />
        <Route
          path="*"
          element={
            <RequireAuth>
              <ShellLayout
                lang={lang}
                onLangChange={onLangChange}
                theme={theme}
                onThemeChange={setTheme}
                collapsed={collapsed}
                onCollapse={onCollapse}
                pluginItems={pluginItems.map((p) => ({
                  path: p.tab_path,
                  label: p.label,
                  icon: p.icon,
                }))}
              >
                <Routes>
                  <Route path="/" element={<StatusPage lang={lang} />} />
                  <Route path="/chat" element={<ChatPage lang={lang} />} />
                  <Route path="/models" element={<ModelsPage lang={lang} />} />
                  <Route path="/logs" element={<LogsPage lang={lang} />} />
                  <Route path="/agents" element={<AgentsPage lang={lang} />} />
                  <Route path="/skills" element={<SkillsPage lang={lang} />} />
                  <Route path="/memory" element={<MemoryPage lang={lang} />} />
                  <Route path="/tasks" element={<TasksPage lang={lang} />} />
                  <Route path="/reviews" element={<ReviewsPage lang={lang} />} />
                  <Route path="/audit" element={<AuditPage lang={lang} />} />
                  <Route path="/capabilities" element={<CapabilitiesPage lang={lang} />} />
                  <Route path="/channels" element={<ChannelsPage lang={lang} />} />
                  <Route path="/settings" element={<SettingsPage lang={lang} />} />
                  <Route path="/sessions" element={<SessionsPage lang={lang} />} />
                  <Route path="/clarifications" element={<ClarificationsPage lang={lang} />} />
                  <Route path="/nodes" element={<NodesPage lang={lang} />} />
                  <Route path="/learning" element={<LearningPage lang={lang} />} />
                  <Route path="/cluster" element={<ClusterPage lang={lang} />} />
                  <Route path="/plugins" element={<PluginsPage lang={lang} />} />
                  <Route path="/admin-ops" element={<AdminOpsPage lang={lang} />} />
                  <Route path="/im-platforms" element={<ImPlatformsPage lang={lang} />} />
                  <Route path="/cron" element={<CronPage lang={lang} />} />
                  {/* Aliases for merged/killed pages — no 404s */}
                  <Route path="/capability-monitor" element={<Navigate to="/capabilities" replace />} />
                  <Route path="/tools" element={<Navigate to="/capabilities" replace />} />
                  <Route path="/handoffs" element={<Navigate to="/agents" replace />} />
                  <Route path="/executions" element={<Navigate to="/tasks" replace />} />
                  <Route path="/kanban" element={<Navigate to="/tasks" replace />} />
                  <Route path="/curator" element={<Navigate to="/learning" replace />} />
                  <Route path="/browser-console" element={<Navigate to="/" replace />} />
                  <Route path="/multimodal" element={<Navigate to="/" replace />} />
                  <Route path="/workbench" element={<Navigate to="/" replace />} />
                  <Route path="/moa" element={<MoaPage lang={lang} />} />
                  <Route path="/profiles" element={<ProfilesPage lang={lang} />} />
                  <Route path="/me" element={<MePage lang={lang} />} />
                  <Route path="/overview" element={<OverviewPage lang={lang} />} />
                  <Route path="/docs" element={<DocsPage lang={lang} />} />
                  <Route path="/uploads" element={<UploadsPage lang={lang} />} />
                  <Route path="/security" element={<SecurityPage lang={lang} />} />
                  <Route path="/trace" element={<TracePage lang={lang} />} />
                  {/* Dynamic Platform Plugin routes — one per mountable manifest.
                      A new plugin appears here + in the sidebar without any code
                      change; the only contract is the manifest's tab_path. */}
                  {pluginItems.map((p) => (
                    <Route
                      key={`plugin:${p.name}`}
                      path={p.tab_path}
                      element={<PluginRoute plugin={p} lang={lang} />}
                    />
                  ))}
                  <Route path="*" element={<Navigate to="/" replace />} />
                </Routes>
              </ShellLayout>
            </RequireAuth>
          }
        />
      </Routes>
    </BrowserRouter>
    </ToastProvider>
  );
}

function RequireAuth({ children }: { children: ReactNode }) {
  const location = useLocation();
  if (!isAuthenticated() || !currentOperator()) {
    return <Navigate to="/login" replace state={{ from: location }} />;
  }
  return <>{children}</>;
}

function ShellLayout({
  lang,
  onLangChange,
  theme,
  onThemeChange,
  collapsed,
  onCollapse,
  children,
  pluginItems = [],
}: {
  lang: Lang;
  onLangChange: (l: Lang) => void;
  theme: Theme;
  onThemeChange: (t: Theme) => void;
  collapsed: boolean;
  onCollapse: () => void;
  children: ReactNode;
  pluginItems?: { path: string; label: string; icon?: string }[];
}) {
  const navigate = useNavigate();
  const [about, setAbout] = useState<AboutInfo | null>(null);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [convs, setConvs] = useState<Conversation[]>([]);
  const [auditEntries, setAuditEntries] = useState<AuditEntry[]>([]);

  useEffect(() => {
    fetchAbout().then(setAbout, () => {});
    fetchConversations().then((cs) => setConvs(cs.slice(0, 5)), () => {});
    fetchAuditLogs({ limit: 5 }).then((r) => setAuditEntries(r.entries ?? []), () => {});
  }, []);

  // Global Cmd+K listener
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      const isCommandKey = (e.metaKey || e.ctrlKey) && e.key === "k";
      if (isCommandKey) {
        e.preventDefault();
        setPaletteOpen(true);
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  // Build palette items
  const paletteItems: PaletteItem[] = [
    // All nav paths
    ...getAllPaths().map((p) => ({
      id: `nav:${p.path}`,
      label: p.label,
      sub: p.path,
      onSelect: () => navigate(p.path),
    })),
    // Recent conversations
    ...convs.map((c) => ({
      id: `conv:${c.id}`,
      label: c.title || c.id,
      sub: `chat · ${c.message_count ?? 0} msg`,
      onSelect: () => navigate(`/chat?conv=${c.id}`),
    })),
    // Recent audit logs
    ...auditEntries.map((e, i) => ({
      id: `audit:${i}`,
      label: e.action,
      sub: `${e.kind ?? ""} · ${e.actor}`,
      onSelect: () => navigate("/audit"),
    })),
  ];

  return (
    <div className="h-screen flex bg-bg text-fg">
      <Sidebar
        collapsed={collapsed}
        version={about?.version || "—"}
        lang={lang}
        pluginItems={pluginItems}
      />
      <div className="flex-1 flex flex-col min-w-0">
        <Topbar
          lang={lang}
          onLangChange={onLangChange}
          theme={theme}
          onThemeChange={onThemeChange}
          collapsed={collapsed}
          onCollapse={onCollapse}
          operator={currentOperator()}
          onOpenPalette={() => setPaletteOpen(true)}
        />
        <OnboardingBanner
          jwt={sessionStorage.getItem("cyberclaw.admin.jwt") || ""}
          lang={lang}
        />
        <main className="flex-1 overflow-auto">
          <div className="mx-auto max-w-[1440px] px-6 py-5 anim-fade-in">
            {children}
          </div>
          <Footer about={about} />
        </main>
      </div>
      <CommandPalette
        open={paletteOpen}
        onClose={() => setPaletteOpen(false)}
        items={paletteItems}
        lang={lang}
      />
    </div>
  );
}

function Footer({ about }: { about: AboutInfo | null }) {
  return (
    <div className="px-6 py-3 border-t border-border text-[10px] mono text-fg-4 flex items-center justify-between max-w-[1440px] mx-auto w-full">
      <span>
        cyberclaw v{about?.version ?? "—"} · {about?.commit ?? "—"}
      </span>
      <span>
        {about ? `uptime ${about.uptime_secs}s` : "loading…"}
      </span>
    </div>
  );
}

// === Pages inline（小，不分文件） ===

function LoginPage({
  lang,
  onLangChange,
}: {
  lang: Lang;
  onLangChange: (l: Lang) => void;
}) {
  const navigate = useNavigate();
  const location = useLocation();
  const [userId, setUserId] = useState("cyberclaw");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    if (isAuthenticated() && currentOperator()) navigate("/", { replace: true });
  }, [navigate]);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setErr(null);
    try {
      await login(userId, password);
      const from = (location.state as { from?: { pathname: string } } | null)
        ?.from?.pathname;
      navigate(from && from !== "/login" ? from : "/", { replace: true });
    } catch (e: unknown) {
      const eobj = e as { status?: number; body?: string };
      setErr(
        `${t("auth.login.failed", lang)}: HTTP ${eobj.status ?? "?"} ${eobj.body ?? ""}`,
      );
    } finally {
      setBusy(false);
    }
  };

  return (
    <main className="min-h-screen flex items-center justify-center p-6 bg-bg text-fg">
      <form
        onSubmit={submit}
        className="w-full max-w-sm space-y-3 rounded-lg border border-border bg-bg-2 p-6"
      >
        <header className="space-y-1 mb-3">
          <h1 className="text-xl font-semibold">{t("app.brand", lang)}</h1>
          <p className="text-xs text-fg-3">{t("app.subtitle", lang)}</p>
        </header>
        <label className="block">
          <span className="text-xs text-fg-3">{t("auth.login.user_id", lang)}</span>
          <input
            value={userId}
            onChange={(e) => setUserId(e.target.value)}
            autoComplete="username"
            className="mt-1 w-full px-3 py-2 rounded-md bg-bg-3 border border-border text-sm outline-none focus-ring"
            required
          />
        </label>
        <label className="block">
          <span className="text-xs text-fg-3">{t("auth.login.password", lang)}</span>
          <input
            type="password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            autoComplete="current-password"
            className="mt-1 w-full px-3 py-2 rounded-md bg-bg-3 border border-border text-sm outline-none focus-ring"
          />
        </label>
        {err && <p className="text-xs text-rose-400">{err}</p>}
        <button
          type="submit"
          disabled={busy || !userId.trim()}
          className="w-full py-2 rounded-md bg-accent text-accent-fg text-sm hover:opacity-90 disabled:opacity-50"
        >
          {busy ? t("common.loading", lang) : t("auth.login.submit", lang)}
        </button>
        <select
          value={lang}
          onChange={(e) => onLangChange(e.target.value as Lang)}
          className="w-full text-xs px-2 py-1.5 rounded-md bg-bg-3 border border-border text-fg-3"
        >
          <option value="en">English</option>
          <option value="zh-CN">中文</option>
        </select>
      </form>
    </main>
  );
}

// ─── StatusPage ──────────────────────────────────────────────────────────────

function fmtNum(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
}
function fmtUsd(n: number): string {
  return `$${n.toFixed(4)}`;
}
function fmtUptime(secs: number): string {
  if (secs < 60) return `${secs}s`;
  const m = Math.floor(secs / 60) % 60;
  const h = Math.floor(secs / 3600) % 24;
  const d = Math.floor(secs / 86400);
  if (d > 0) return `${d}d ${h}h`;
  if (h > 0) return `${h}h ${m}m`;
  return `${m}m`;
}
function relTime(iso: string | undefined | null): string {
  if (!iso) return "—";
  const diff = (Date.now() - new Date(iso).getTime()) / 1000;
  if (diff < 60) return "just now";
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
  return `${Math.floor(diff / 86400)}d ago`;
}

type HealthStatus = "ok" | "error" | "loading";

function useDashboard() {
  const [usage, setUsage] = useState<UsageInfo | null>(null);
  const [about, setAbout] = useState<AboutInfo | null>(null);
  const [convs, setConvs] = useState<Conversation[]>([]);
  const [auditEntries, setAuditEntries] = useState<AuditEntry[]>([]);
  const [cronJobs, setCronJobs] = useState<CronJob[]>([]);
  const [tasks, setTasks] = useState<Task[]>([]);
  const [health, setHealth] = useState<Record<string, HealthStatus>>({
    api: "loading", audit: "loading", cron: "loading", plugins: "loading",
  });
  const [now, setNow] = useState(() => new Date());

  useEffect(() => {
    fetchAbout().then(setAbout, () => {});
    fetchUsage().then(setUsage, () => {});
    fetchConversations().then((cs) => setConvs(cs.slice(0, 3)), () => {});
    fetchAuditLogs({ limit: 5 }).then((r) => setAuditEntries(r.entries ?? []), () => {});
    fetchCronJobs().then(setCronJobs, () => {});
    fetchTasks("all", 20).then(setTasks, () => {});

    // health probes
    const probe = (key: string, url: string) => {
      const jwt = sessionStorage.getItem("cyberclaw.admin.jwt");
      const headers: Record<string, string> = jwt ? { Authorization: `Bearer ${jwt}` } : {};
      fetch(url, { headers })
        .then((r) => setHealth((h) => ({ ...h, [key]: r.ok ? "ok" : "error" } as Record<string, HealthStatus>)))
        .catch(() => setHealth((h) => ({ ...h, [key]: "error" } as Record<string, HealthStatus>)));
    };
    probe("api", "/healthz");
    probe("audit", "/api/v1/audit/logs?limit=1");
    probe("cron", "/api/v1/cron");
    probe("plugins", "/api/v1/plugins");

    const tick = setInterval(() => setNow(new Date()), 30_000);
    return () => clearInterval(tick);
  }, []);

  return { usage, about, convs, auditEntries, cronJobs, tasks, health, now };
}

function PulseDot({ status }: { status: HealthStatus }) {
  const color =
    status === "ok" ? "bg-emerald-500" :
    status === "error" ? "bg-rose-500" :
    "bg-amber-400";
  return (
    <span className="relative inline-flex h-2 w-2">
      <span className={`absolute inline-flex h-full w-full animate-ping rounded-full opacity-60 ${color}`} />
      <span className={`relative inline-flex h-2 w-2 rounded-full ${color}`} />
    </span>
  );
}

function KpiCard({
  label, value, sub,
}: { label: string; value: string; sub?: string }) {
  return (
    <div className="rounded-xl border border-border bg-bg-2 p-4 flex flex-col gap-1 min-w-0">
      <span className="text-[10px] mono text-fg-4 uppercase tracking-widest">{label}</span>
      <span className="text-2xl mono font-semibold text-fg truncate">{value}</span>
      {sub && <span className="text-[11px] text-fg-3 truncate">{sub}</span>}
    </div>
  );
}

function PanelShell({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="rounded-xl border border-border bg-bg-2 flex flex-col min-h-[180px]">
      <div className="px-4 py-2.5 border-b border-border flex items-center">
        <span className="text-[11px] mono text-fg-3 uppercase tracking-widest">{title}</span>
      </div>
      <div className="flex-1 overflow-hidden">{children}</div>
    </div>
  );
}

function statusDict(lang: Lang) {
  return lang === "zh-CN"
    ? {
        dashboardTitle: "CyberClaw 控制台",
        serverUp: "服务在线",
        serverDown: "服务离线",
        kpiRequests: "请求数 24h",
        kpiTokens: "token 数",
        kpiTokensSub: (out: string) => `输入 · ${out} 输出`,
        kpiEstCost: "预估费用",
        kpiActiveModels: "活跃模型",
        kpiModelSub: (n: number) => n === 1 ? "模型" : "模型",
        kpiUptime: "运行时长",
        kpiSessions: "今日会话",
        kpiSessionsSub: "会话",
        panelRecentChats: "最近对话",
        noConversations: "暂无对话。",
        panelAuditEvents: "审计事件",
        noAuditEvents: "暂无审计事件。",
        panelJobs: "任务",
        tasksRunning: "运行中任务",
        tasksPending: "等待中任务",
        cronEnabled: "定时任务（已启用）",
        lastCronRun: "最后定时运行",
        systemHealth: "系统健康",
        healthLabels: { api: "API", audit: "审计", cron: "定时", plugins: "插件" } as Record<string, string>,
        llmConfigured: "已配置",
        llmUnknown: "未知",
        quickLinks: [
          { href: "/admin/v2/models", label: "模型" },
          { href: "/admin/v2/logs", label: "审计日志" },
          { href: "/admin/v2/tasks", label: "任务" },
        ],
      }
    : {
        dashboardTitle: "CyberClaw Dashboard",
        serverUp: "server up",
        serverDown: "server down",
        kpiRequests: "requests 24h",
        kpiTokens: "tokens",
        kpiTokensSub: (out: string) => `in · ${out} out`,
        kpiEstCost: "est. cost",
        kpiActiveModels: "active models",
        kpiModelSub: (n: number) => n === 1 ? "model" : "models",
        kpiUptime: "uptime",
        kpiSessions: "sessions today",
        kpiSessionsSub: "sessions",
        panelRecentChats: "Recent chats",
        noConversations: "No conversations yet.",
        panelAuditEvents: "Audit events",
        noAuditEvents: "No audit events.",
        panelJobs: "Jobs",
        tasksRunning: "Tasks running",
        tasksPending: "Tasks pending",
        cronEnabled: "Cron jobs (enabled)",
        lastCronRun: "last cron run",
        systemHealth: "System health",
        healthLabels: { api: "API", audit: "Audit", cron: "Cron", plugins: "Plugins" } as Record<string, string>,
        llmConfigured: "configured",
        llmUnknown: "unknown",
        quickLinks: [
          { href: "/admin/v2/models", label: "Models" },
          { href: "/admin/v2/logs", label: "Audit Logs" },
          { href: "/admin/v2/tasks", label: "Tasks" },
        ],
      };
}

function StatusPage({ lang }: { lang: Lang }) {
  const SL = statusDict(lang);
  const op = currentOperator();
  const { usage, about, convs, auditEntries, cronJobs, tasks, health, now } = useDashboard();

  const activeModels = usage ? Object.keys(usage.by_model).length : 0;
  const activeTasks = tasks.filter((t) => t.status === "running").length;
  const pendingTasks = tasks.filter((t) => t.status === "pending").length;
  const cronEnabled = cronJobs.filter((j) => j.enabled).length;
  const lastCron = cronJobs.slice().sort((a, b) =>
    (b.last_run_at ?? "").localeCompare(a.last_run_at ?? "")
  )[0];

  const apiIsUp = health.api === "ok";
  const tz = Intl.DateTimeFormat().resolvedOptions().timeZone;
  const timeStr = now.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });

  return (
    <section className="space-y-5 anim-fade-in">
      {/* ── Header ─────────────────────────────────────────────────────── */}
      <header className="flex items-start justify-between gap-4 flex-wrap">
        <div className="space-y-0.5">
          <h1 className="text-xl font-semibold tracking-tight text-fg">
            {SL.dashboardTitle}
          </h1>
          <p className="text-[12px] text-fg-3">
            {op?.display_name || op?.user_id || "—"}
            <span className="mx-1.5 text-fg-4">·</span>
            <span className="mono">{op?.role || "—"}</span>
          </p>
        </div>
        <div className="flex items-center gap-3 text-[11px] text-fg-3">
          <span className="mono">{timeStr} {tz}</span>
          <span className="flex items-center gap-1.5">
            <PulseDot status={(health["api"] ?? "loading") as HealthStatus} />
            <span>{apiIsUp ? SL.serverUp : SL.serverDown}</span>
            {about && (
              <span className="text-fg-4 ml-1">· {fmtUptime(about.uptime_secs)}</span>
            )}
          </span>
        </div>
      </header>

      {/* ── KPI Strip ──────────────────────────────────────────────────── */}
      <div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-6 gap-3">
        <KpiCard
          label={SL.kpiRequests}
          value={usage ? fmtNum(usage.total_requests) : "…"}
        />
        <KpiCard
          label={SL.kpiTokens}
          value={usage ? `${fmtNum(usage.total_input_tokens)}` : "…"}
          sub={usage ? SL.kpiTokensSub(fmtNum(usage.total_output_tokens)) : undefined}
        />
        <KpiCard
          label={SL.kpiEstCost}
          value={usage ? fmtUsd(usage.estimated_cost_usd) : "…"}
          sub="USD"
        />
        <KpiCard
          label={SL.kpiActiveModels}
          value={usage ? String(activeModels) : "…"}
          sub={SL.kpiModelSub(activeModels)}
        />
        <KpiCard
          label={SL.kpiUptime}
          value={about ? fmtUptime(about.uptime_secs) : "…"}
          sub={about ? `v${about.version}` : undefined}
        />
        <KpiCard
          label={SL.kpiSessions}
          value={usage ? fmtNum(usage.total_sessions) : "…"}
          sub={SL.kpiSessionsSub}
        />
      </div>

      {/* ── Activity Panels ────────────────────────────────────────────── */}
      <div className="grid grid-cols-1 md:grid-cols-3 gap-3">
        {/* Recent chats */}
        <PanelShell title={SL.panelRecentChats}>
          {convs.length === 0 ? (
            <p className="px-4 py-3 text-[11px] text-fg-4">{SL.noConversations}</p>
          ) : (
            <ul className="divide-y divide-border">
              {convs.map((c) => (
                <li key={c.id}>
                  <a
                    href={`/admin/v2/chat?conv=${c.id}`}
                    className="flex items-start justify-between gap-2 px-4 py-2.5 hover:bg-bg-3 transition-colors"
                  >
                    <span className="text-[12px] text-fg-2 truncate flex-1">{c.title || c.id}</span>
                    <span className="text-[10px] mono text-fg-4 whitespace-nowrap shrink-0">
                      {c.message_count != null ? `${c.message_count}msg` : ""}
                      {c.updated_at ? ` · ${relTime(c.updated_at)}` : ""}
                    </span>
                  </a>
                </li>
              ))}
            </ul>
          )}
        </PanelShell>

        {/* Recent audit events */}
        <PanelShell title={SL.panelAuditEvents}>
          {auditEntries.length === 0 ? (
            <p className="px-4 py-3 text-[11px] text-fg-4">{SL.noAuditEvents}</p>
          ) : (
            <ul className="divide-y divide-border">
              {auditEntries.map((e, i) => (
                <li key={i} className="px-4 py-2 flex flex-col gap-0.5">
                  <div className="flex items-center justify-between gap-2">
                    <span className="text-[11px] mono text-accent truncate">{e.action}</span>
                    <span className="text-[10px] mono text-fg-4 shrink-0">{relTime(e.ts)}</span>
                  </div>
                  <div className="text-[10px] text-fg-3 truncate">
                    {e.actor}
                    {e.kind ? <span className="text-fg-4 ml-1">· {e.kind}</span> : null}
                  </div>
                </li>
              ))}
            </ul>
          )}
        </PanelShell>

        {/* Active jobs */}
        <PanelShell title={SL.panelJobs}>
          <div className="px-4 py-3 space-y-2">
            <div className="flex items-center justify-between">
              <span className="text-[11px] text-fg-3">{SL.tasksRunning}</span>
              <span className="mono text-[13px] text-fg">{activeTasks}</span>
            </div>
            <div className="flex items-center justify-between">
              <span className="text-[11px] text-fg-3">{SL.tasksPending}</span>
              <span className="mono text-[13px] text-fg">{pendingTasks}</span>
            </div>
            <div className="flex items-center justify-between">
              <span className="text-[11px] text-fg-3">{SL.cronEnabled}</span>
              <span className="mono text-[13px] text-fg">{cronEnabled}</span>
            </div>
            {lastCron && (
              <div className="pt-1 border-t border-border">
                <div className="text-[10px] text-fg-4 mb-0.5">{SL.lastCronRun}</div>
                <div className="text-[11px] text-fg-2 truncate">{lastCron.name}</div>
                <div className="text-[10px] mono text-fg-4">{relTime(lastCron.last_run_at)}</div>
              </div>
            )}
          </div>
        </PanelShell>
      </div>

      {/* ── System Health ───────────────────────────────────────────────── */}
      <div className="rounded-xl border border-border bg-bg-2 px-4 py-3 flex flex-wrap gap-4 items-center">
        <span className="text-[10px] mono text-fg-4 uppercase tracking-widest shrink-0">{SL.systemHealth}</span>
        {(["api", "audit", "cron", "plugins"] as const).map((key) => (
          <span key={key} className="flex items-center gap-1.5 text-[11px] text-fg-3">
            <PulseDot status={(health[key] ?? "loading") as HealthStatus} />
            <span>{SL.healthLabels[key] ?? key}</span>
          </span>
        ))}
        <span className="flex items-center gap-1.5 text-[11px] text-fg-3 ml-auto">
          <span className={`h-2 w-2 rounded-full ${about?.commit ? "bg-emerald-500" : "bg-fg-4"}`} />
          LLM {about?.commit ? SL.llmConfigured : SL.llmUnknown}
        </span>
      </div>

      {/* ── Quick links ────────────────────────────────────────────────── */}
      <div className="flex flex-wrap gap-2 text-[11px]">
        {SL.quickLinks.map(({ href, label }) => (
          <a
            key={href}
            href={href}
            className="px-3 py-1.5 rounded-md border border-border bg-bg-3 text-fg-2 hover:text-fg hover:border-border-strong transition-colors"
          >
            {label}
          </a>
        ))}
      </div>
    </section>
  );
}
