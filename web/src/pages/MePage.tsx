// MePage — 当前操作者个人详情 + 偏好。
// 来源：GET /admin/me（OperatorRecord）+ localStorage (theme/lang/tweaks)。
// 只读个人信息 — 用户信息由后端 users.toml 单一来源，UI 不做就地修改。

import { useEffect, useState } from "react";
import { type Operator, fetchMe, logout } from "@/lib/api";
import { type Lang, getLang } from "@/lib/i18n";
import PageHeader from "@/components/PageHeader";

function dict(lang: Lang) {
  return lang === "zh-CN"
    ? {
        title: "我的账户",
        subtitle: "当前操作者信息 + 本地偏好",
        loading: "加载中…",
        loadFailed: "加载账户信息失败",
        sectionIdentity: "身份",
        userId: "用户 ID",
        displayName: "显示名称",
        role: "角色",
        roleAdmin: "管理员",
        roleOperator: "操作员",
        roleViewer: "只读",
        sectionTimestamps: "时间戳",
        createdAt: "创建时间",
        lastLogin: "上次登录",
        onboardedAt: "完成引导",
        notOnboarded: "未完成引导",
        sectionPreferences: "本地偏好",
        preferenceLang: "界面语言",
        preferenceTheme: "主题",
        themeLight: "浅色",
        themeDark: "深色",
        intentAutoRoute: "意图自动路由",
        intentOn: "已启用",
        intentOff: "已关闭",
        intentHint: "意图自动路由由后端配置，无法在前端修改",
        sectionGovernance: "治理与审计",
        governanceLine1: "我的每次操作都会记录到审计链（操作类审计）",
        governanceLine2: "登录凭证短期有效，过期需重新登录",
        governanceLine3: "执行严格走 Connector → Capability 链，无后门",
        sectionActions: "操作",
        logoutBtn: "退出登录",
        notSet: "—",
      }
    : {
        title: "My Account",
        subtitle: "Current operator info + local preferences",
        loading: "Loading…",
        loadFailed: "Failed to load account info",
        sectionIdentity: "Identity",
        userId: "User ID",
        displayName: "Display name",
        role: "Role",
        roleAdmin: "admin",
        roleOperator: "operator",
        roleViewer: "viewer",
        sectionTimestamps: "Timestamps",
        createdAt: "Created at",
        lastLogin: "Last login",
        onboardedAt: "Onboarded at",
        notOnboarded: "Not yet onboarded",
        sectionPreferences: "Local preferences",
        preferenceLang: "UI language",
        preferenceTheme: "Theme",
        themeLight: "light",
        themeDark: "dark",
        intentAutoRoute: "Intent auto-route",
        intentOn: "enabled",
        intentOff: "disabled",
        intentHint: "Intent auto-route is configured server-side, not editable here",
        sectionGovernance: "Governance & Audit",
        governanceLine1: "Every action of yours is recorded to the audit chain (mutation events)",
        governanceLine2: "Session credentials are short-lived; re-login required on expiry",
        governanceLine3: "Execution strictly follows Connector → Capability chain — no back door",
        sectionActions: "Actions",
        logoutBtn: "Logout",
        notSet: "—",
      };
}

function fmtIso(s: string | null | undefined, fallback: string): string {
  if (!s) return fallback;
  try {
    const d = new Date(s);
    if (isNaN(d.getTime())) return s;
    return d.toLocaleString();
  } catch {
    return s;
  }
}

function roleLabel(L: ReturnType<typeof dict>, role: string): string {
  if (role === "admin") return L.roleAdmin;
  if (role === "operator") return L.roleOperator;
  if (role === "viewer") return L.roleViewer;
  return role;
}

function currentTheme(): "dark" | "light" {
  try {
    const raw = localStorage.getItem("cyberclaw.admin.tweaks");
    if (!raw) return "dark";
    return (JSON.parse(raw) as { theme?: string }).theme === "light" ? "light" : "dark";
  } catch {
    return "dark";
  }
}

function FieldRow({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="flex items-baseline justify-between gap-3 py-1.5 border-b border-border last:border-b-0">
      <span className="text-[11px] uppercase tracking-wide text-fg-4">{label}</span>
      <span className={`text-xs text-fg-2 truncate ${mono ? "mono" : ""}`}>{value}</span>
    </div>
  );
}

export default function MePage({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [op, setOp] = useState<Operator | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    fetchMe()
      .then(({ user }) => { setOp(user); setLoading(false); })
      .catch((e: { status?: number; body?: string }) => {
        setErr(`HTTP ${e?.status ?? "?"} ${e?.body ?? L.loadFailed}`);
        setLoading(false);
      });
  }, [L.loadFailed]);

  const onLogout = async () => {
    await logout();
    window.location.href = "/admin/v2/login";
  };

  return (
    <section className="space-y-5 anim-fade-in max-w-2xl">
      <PageHeader title={L.title} subtitle={L.subtitle} />

      {err && <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>}
      {loading && <p className="text-xs text-fg-4">{L.loading}</p>}

      {!loading && op && (
        <>
          {/* Identity */}
          <div className="rounded-xl border border-border bg-bg-2 p-5 space-y-2">
            <h3 className="text-[11px] mono text-fg-4 uppercase tracking-widest">{L.sectionIdentity}</h3>
            <FieldRow label={L.userId} value={op.user_id} mono />
            <FieldRow label={L.displayName} value={op.display_name || L.notSet} />
            <FieldRow label={L.role} value={roleLabel(L, op.role)} />
          </div>

          {/* Timestamps */}
          <div className="rounded-xl border border-border bg-bg-2 p-5 space-y-2">
            <h3 className="text-[11px] mono text-fg-4 uppercase tracking-widest">{L.sectionTimestamps}</h3>
            <FieldRow label={L.createdAt} value={fmtIso(op.created_at, L.notSet)} mono />
            <FieldRow label={L.lastLogin} value={fmtIso(op.last_login, L.notSet)} mono />
            <FieldRow
              label={L.onboardedAt}
              value={op.onboarded_at ? fmtIso(op.onboarded_at, L.notSet) : L.notOnboarded}
              mono
            />
          </div>

          {/* Preferences (local-only) */}
          <div className="rounded-xl border border-border bg-bg-2 p-5 space-y-2">
            <h3 className="text-[11px] mono text-fg-4 uppercase tracking-widest">{L.sectionPreferences}</h3>
            <FieldRow label={L.preferenceLang} value={getLang() === "zh-CN" ? "中文" : "English"} />
            <FieldRow label={L.preferenceTheme} value={currentTheme() === "light" ? L.themeLight : L.themeDark} />
            <FieldRow
              label={L.intentAutoRoute}
              value={op.intent_auto_route ? L.intentOn : L.intentOff}
            />
            <p className="text-[10px] text-fg-4 pt-1">{L.intentHint}</p>
          </div>

          {/* Governance reminder — cyberclaw 优势在 UI 中显式体现 */}
          <div className="rounded-xl border border-accent/20 bg-accent-soft/20 p-5 space-y-1.5">
            <h3 className="text-[11px] mono text-accent uppercase tracking-widest">{L.sectionGovernance}</h3>
            <p className="text-xs text-fg-2">• {L.governanceLine1}</p>
            <p className="text-xs text-fg-2">• {L.governanceLine2}</p>
            <p className="text-xs text-fg-2">• {L.governanceLine3}</p>
          </div>

          {/* Actions */}
          <div className="rounded-xl border border-border bg-bg-2 p-5 space-y-2">
            <h3 className="text-[11px] mono text-fg-4 uppercase tracking-widest">{L.sectionActions}</h3>
            <button
              onClick={onLogout}
              className="px-3 py-1.5 rounded-md border border-rose-500/30 text-rose-400 hover:bg-rose-500/10 text-xs"
            >
              {L.logoutBtn}
            </button>
          </div>
        </>
      )}
    </section>
  );
}
