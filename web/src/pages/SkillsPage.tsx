// SkillsPage — CyberClaw skill management console.
//
// Design principles (distinct from generic card UIs):
// 1. Trust visibility: every skill shows an explicit trust badge (high/medium/low)
//    derived from source — this is a CyberClaw governance signal, not decoration.
// 2. has_scripts warning: skills with scripts/ dir get an explicit risk indicator
//    (Skill 本身不执行代码；scripts 表示可执行风险，需沙箱意识).
// 3. Audit label: toggle writes to the audit chain — user sees governance in action.
// 4. Skill is a knowledge/method layer — no "Run skill" button. Execution goes
//    through Connector → Capability.

import { useEffect, useMemo, useState } from "react";
import { useToast } from "@/components/ToastBar";
import {
  type Skill,
  fetchSkills,
  installSkill,
  installSkillRemote,
  createSkill,
  uninstallSkill,
  toggleSkill,
} from "@/lib/api";
import { type Lang } from "@/lib/i18n";
import TableSkeleton from "@/components/TableSkeleton";
import Modal from "@/components/Modal";
import Field, { TextInput, TextArea, Select } from "@/components/Field";
import PageHeader from "@/components/PageHeader";

// ---------------------------------------------------------------------------
// i18n
// ---------------------------------------------------------------------------

function dict(lang: Lang) {
  return lang === "zh-CN"
    ? {
        title: "技能",
        subtitle: (n: number) => `${n} 个已安装`,
        installBtn: "+ 安装",
        searchLabel: "搜索",
        searchPlaceholder: "名称 / 描述",
        trustLabel: "信任级别",
        allTrust: "全部",
        noSkills: "暂无 skill 已安装。",
        noMatch: "无匹配结果。",
        tabHub: "Hub",
        tabUrl: "URL",
        tabCreate: "新建",
        installTitle: "安装 Skill",
        skillNameLabel: "Skill 名称",
        skillNamePlaceholder: "e.g. code-review",
        urlLabel: "URL",
        urlHint: "公开 git 仓库 URL 或 .skill.zip",
        urlPlaceholder: "https://github.com/org/my-skill",
        createNameLabel: "名称",
        createNamePlaceholder: "my-skill",
        createCategoryLabel: "类别",
        createDescLabel: "描述",
        createDescPlaceholder: "这个 skill 做什么…",
        createContentLabel: "SKILL.md 内容",
        cancel: "取消",
        install: "安装",
        create: "新建",
        nameRequired: "Skill 名称必填",
        urlRequired: "URL 必填",
        uninstallTitle: "卸载 skill",
        uninstallMsg: (n: string) => `确定移除 skill "${n}"？此操作不可撤销。`,
        uninstall: "卸载",
        drawerClose: "关闭",
        trustHigh: "高可信",
        trustMedium: "中可信",
        trustLow: "低可信",
        scriptsWarning: "包含可执行 scripts/ — 仅在沙箱中运行",
        auditNote: "此 toggle 操作已记录到审计链",
        skillLayer: "Skill 是知识/方法层 — 执行经由 Connector → Capability",
        filesLabel: "文件",
        filesMeta: (bytes: number, files: number) =>
          `${fmtBytes(bytes)} · ${files} 个文件`,
        author: "作者",
        origin: "来源",
        importedAt: "导入时间",
        tags: "标签",
        installedAt: "安装时间",
        noExcerpt: "（无摘要）",
        enabledLabel: (on: boolean) => (on ? "停用" : "启用"),
        optionsLabel: "选项",
        optionUninstall: "卸载",
      }
    : {
        title: "Skills",
        subtitle: (n: number) => `${n} installed`,
        installBtn: "+ Install",
        searchLabel: "search",
        searchPlaceholder: "name / description",
        trustLabel: "trust",
        allTrust: "all",
        noSkills: "No skills installed.",
        noMatch: "No skills match your search.",
        tabHub: "Hub",
        tabUrl: "URL",
        tabCreate: "Create",
        installTitle: "Install Skill",
        skillNameLabel: "Skill name",
        skillNamePlaceholder: "e.g. code-review",
        urlLabel: "URL",
        urlHint: "Public git repo URL or .skill.zip",
        urlPlaceholder: "https://github.com/org/my-skill",
        createNameLabel: "Name",
        createNamePlaceholder: "my-skill",
        createCategoryLabel: "Category",
        createDescLabel: "Description",
        createDescPlaceholder: "What this skill does…",
        createContentLabel: "SKILL.md content",
        cancel: "Cancel",
        install: "Install",
        create: "Create",
        nameRequired: "Skill name required",
        urlRequired: "URL required",
        uninstallTitle: "Uninstall skill",
        uninstallMsg: (n: string) => `Remove skill "${n}"? This cannot be undone.`,
        uninstall: "Uninstall",
        drawerClose: "Close",
        trustHigh: "High trust",
        trustMedium: "Medium trust",
        trustLow: "Low trust",
        scriptsWarning: "Contains executable scripts/ — run in sandbox only",
        auditNote: "Toggle is recorded in the audit chain",
        skillLayer: "Skill is a knowledge layer — execution goes through Connector → Capability",
        filesLabel: "Files",
        filesMeta: (bytes: number, files: number) =>
          `${fmtBytes(bytes)} · ${files} files`,
        author: "Author",
        origin: "Imported from",
        importedAt: "Imported at",
        tags: "Tags",
        installedAt: "Installed",
        noExcerpt: "(no excerpt)",
        enabledLabel: (on: boolean) => (on ? "Disable" : "Enable"),
        optionsLabel: "Options",
        optionUninstall: "Uninstall",
      };
}

// ---------------------------------------------------------------------------
// Style constants — match AgentsPage TRUST_TONE pattern
// ---------------------------------------------------------------------------

const TRUST_TONE: Record<string, string> = {
  high: "bg-emerald-500/15 text-emerald-300 border-emerald-500/30",
  medium: "bg-amber-500/15 text-amber-300 border-amber-500/30",
  low: "bg-rose-500/15 text-rose-300 border-rose-500/30",
};

const TRUST_DOT: Record<string, string> = {
  high: "bg-emerald-400",
  medium: "bg-amber-400",
  low: "bg-rose-400",
};

const SOURCE_TONE: Record<string, string> = {
  builtin: "bg-accent-soft text-accent",
  hub: "bg-violet-500/15 text-violet-300",
  local: "bg-cyan-500/15 text-cyan-300",
};

const CATEGORIES = ["AI", "DevOps", "Security", "Data", "Integration", "Other"];

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function fmtDate(v: string | number | undefined): string {
  if (v == null) return "—";
  let d: Date;
  if (typeof v === "number") {
    d = new Date(v * 1000);
  } else if (/^\d{9,10}$/.test(v)) {
    d = new Date(Number(v) * 1000);
  } else {
    d = new Date(v);
  }
  if (Number.isNaN(d.getTime())) return String(v);
  return d.toLocaleDateString();
}

function fmtBytes(b: number): string {
  if (b < 1024) return `${b} B`;
  if (b < 1024 * 1024) return `${(b / 1024).toFixed(1)} KB`;
  return `${(b / 1024 / 1024).toFixed(1)} MB`;
}

// ---------------------------------------------------------------------------
// Skill detail drawer
// ---------------------------------------------------------------------------

function SkillDrawer({
  skill,
  lang,
  onClose,
}: {
  skill: Skill;
  lang: Lang;
  onClose: () => void;
}) {
  const L = dict(lang);
  const trust = skill.trust_level ?? "low";

  return (
    <div
      className="fixed inset-0 z-40 flex justify-end"
      onClick={onClose}
    >
      {/* Backdrop */}
      <div className="absolute inset-0 bg-black/40" />
      {/* Drawer panel */}
      <div
        className="relative z-50 w-full max-w-md bg-bg-2 border-l border-border h-full overflow-y-auto shadow-2xl flex flex-col"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="flex items-start justify-between gap-3 px-5 pt-5 pb-4 border-b border-border">
          <div className="min-w-0">
            <div className="flex items-center gap-2 flex-wrap">
              <h2 className="text-sm font-semibold truncate">{skill.name}</h2>
              {skill.version && (
                <span className="px-1.5 py-0.5 text-[10px] mono rounded bg-white/10 text-fg-3">
                  v{skill.version}
                </span>
              )}
            </div>
            <p className="text-[11px] text-fg-3 mt-0.5">{skill.category ?? "Other"}</p>
          </div>
          <button
            onClick={onClose}
            className="text-fg-4 hover:text-fg text-xs px-2 py-1 rounded hover:bg-hover shrink-0"
          >
            {L.drawerClose}
          </button>
        </div>

        <div className="flex-1 px-5 py-4 space-y-4 text-xs">
          {/* Trust badge — CyberClaw governance signal */}
          <div className="flex items-center gap-2 flex-wrap">
            <span
              className={`inline-flex items-center gap-1.5 px-2 py-1 rounded-full border text-[11px] font-medium ${TRUST_TONE[trust] ?? TRUST_TONE.low}`}
            >
              <span className={`w-1.5 h-1.5 rounded-full ${TRUST_DOT[trust] ?? TRUST_DOT.low}`} />
              {trust === "high" ? L.trustHigh : trust === "medium" ? L.trustMedium : L.trustLow}
            </span>
            {skill.source && (
              <span className={`px-1.5 py-0.5 rounded text-[10px] mono ${SOURCE_TONE[skill.source] ?? "bg-white/10"}`}>
                {skill.source}
              </span>
            )}
            {skill.has_scripts && (
              <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded border border-rose-500/40 bg-rose-500/10 text-rose-400 text-[10px]">
                <span className="text-[9px]">!</span>
                {L.scriptsWarning}
              </span>
            )}
          </div>

          {/* Description — pick zh when available + UI lang is zh-CN.
              Falls back to English description so we never show empty. */}
          {(() => {
            const desc = lang === "zh-CN" && skill.description_zh
              ? skill.description_zh
              : skill.description;
            return desc ? (
              <p className="text-fg-2 leading-relaxed">{desc}</p>
            ) : null;
          })()}

          {/* Readme excerpt */}
          <div className="rounded-md bg-bg-3 border border-border p-3">
            <p className="text-[10px] text-fg-4 uppercase tracking-wide font-medium mb-1.5">SKILL.md</p>
            <p className="text-fg-3 leading-relaxed whitespace-pre-wrap text-[11px]">
              {skill.readme_excerpt || L.noExcerpt}
            </p>
          </div>

          {/* Tags */}
          {skill.tags && skill.tags.length > 0 && (
            <div>
              <p className="text-[10px] text-fg-4 uppercase tracking-wide font-medium mb-1.5">{L.tags}</p>
              <div className="flex flex-wrap gap-1.5">
                {skill.tags.map((tag) => (
                  <span key={tag} className="px-2 py-0.5 rounded-full bg-white/8 text-fg-3 text-[10px]">
                    {tag}
                  </span>
                ))}
              </div>
            </div>
          )}

          {/* Meta grid */}
          <div className="grid grid-cols-2 gap-x-4 gap-y-2 text-[11px]">
            {skill.author && (
              <>
                <span className="text-fg-4">{L.author}</span>
                <span className="text-fg-2 truncate">{skill.author}</span>
              </>
            )}
            {skill.origin && (
              <>
                <span className="text-fg-4">{L.origin}</span>
                <span className="text-fg-2 mono truncate" title={skill.origin}>
                  {skill.origin}
                </span>
              </>
            )}
            {skill.imported_at_origin && (
              <>
                <span className="text-fg-4">{L.importedAt}</span>
                <span className="text-fg-2 mono">{skill.imported_at_origin}</span>
              </>
            )}
            <span className="text-fg-4">{L.installedAt}</span>
            <span className="text-fg-2 mono">{fmtDate(skill.installed_at)}</span>
            {(skill.size_bytes != null || skill.file_count != null) && (
              <>
                <span className="text-fg-4">{L.filesLabel}</span>
                <span className="text-fg-2 mono">
                  {L.filesMeta(skill.size_bytes ?? 0, skill.file_count ?? 0)}
                </span>
              </>
            )}
          </div>

          {/* Audit footnote — governance visibility */}
          <div className="rounded-md bg-emerald-500/8 border border-emerald-500/20 px-3 py-2 text-[10px] text-emerald-400">
            {L.auditNote}
          </div>

          {/* Architectural note — Skill is NOT an executor */}
          <p className="text-[10px] text-fg-4 border-t border-border pt-3">
            {L.skillLayer}
          </p>
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Skill card
// ---------------------------------------------------------------------------

function SkillCard({
  s,
  lang,
  onToggle,
  onUninstall,
  onOpen,
}: {
  s: Skill;
  lang: Lang;
  onToggle: (s: Skill, enabled: boolean) => void;
  onUninstall: (s: Skill) => void;
  onOpen: (s: Skill) => void;
}) {
  const L = dict(lang);
  const [menuOpen, setMenuOpen] = useState(false);
  const trust = s.trust_level ?? "low";

  return (
    <div
      className="relative rounded-lg border border-border bg-bg-2 p-3 hover:border-border-strong transition-colors cursor-pointer"
      onClick={() => { setMenuOpen(false); onOpen(s); }}
    >
      {/* Top row: name + controls */}
      <div className="flex items-start justify-between gap-2 mb-1.5">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-1.5 flex-wrap">
            <span className="font-medium text-sm truncate">{s.name}</span>
            {s.version && (
              <span className="text-[10px] mono text-fg-4 shrink-0">v{s.version}</span>
            )}
          </div>
        </div>

        <div className="flex items-center gap-1.5 shrink-0" onClick={(e) => e.stopPropagation()}>
          {/* Toggle */}
          <label
            className="relative inline-flex items-center cursor-pointer"
            title={s.enabled !== false ? L.enabledLabel(true) : L.enabledLabel(false)}
          >
            <input
              type="checkbox"
              className="sr-only"
              checked={s.enabled !== false}
              onChange={(e) => onToggle(s, e.target.checked)}
            />
            <span className={`w-7 h-4 rounded-full transition-colors ${s.enabled !== false ? "bg-accent" : "bg-fg-4/40"}`} />
            <span className={`absolute left-0.5 top-0.5 w-3 h-3 rounded-full bg-white transition-transform ${s.enabled !== false ? "translate-x-3" : "translate-x-0"}`} />
          </label>

          {/* Source badge */}
          {s.source && (
            <span className={`px-1.5 py-0.5 rounded text-[10px] mono ${SOURCE_TONE[s.source] ?? "bg-white/10"}`}>
              {s.source}
            </span>
          )}

          {/* Options menu */}
          <div className="relative">
            <button
              onClick={(e) => { e.stopPropagation(); setMenuOpen((v) => !v); }}
              className="px-1.5 py-0.5 rounded text-fg-3 hover:text-fg hover:bg-hover text-[11px]"
              title={L.optionsLabel}
            >
              ···
            </button>
            {menuOpen && (
              <div
                className="absolute right-0 top-full mt-1 z-20 bg-bg-2 border border-border rounded-md shadow-lg min-w-[120px]"
                onClick={(e) => e.stopPropagation()}
              >
                <button
                  onClick={() => { setMenuOpen(false); onUninstall(s); }}
                  className="w-full text-left px-3 py-2 text-xs text-rose-400 hover:bg-hover"
                >
                  {L.optionUninstall}
                </button>
              </div>
            )}
          </div>
        </div>
      </div>

      {/* Trust badge row — CyberClaw governance signal */}
      <div className="flex items-center gap-1.5 mb-2 flex-wrap">
        <span
          className={`inline-flex items-center gap-1 px-1.5 py-0.5 rounded-full border text-[10px] font-medium ${TRUST_TONE[trust] ?? TRUST_TONE.low}`}
        >
          <span className={`w-1.5 h-1.5 rounded-full ${TRUST_DOT[trust] ?? TRUST_DOT.low}`} />
          {trust === "high" ? L.trustHigh : trust === "medium" ? L.trustMedium : L.trustLow}
        </span>

        {s.has_scripts && (
          <span className="inline-flex items-center gap-0.5 px-1.5 py-0.5 rounded border border-rose-500/40 bg-rose-500/10 text-rose-400 text-[10px]">
            <span className="font-bold">!</span> scripts
          </span>
        )}

        {/* Tags (first 2) */}
        {s.tags?.slice(0, 2).map((tag) => (
          <span key={tag} className="px-1.5 py-0.5 rounded-full bg-white/8 text-fg-4 text-[10px]">
            {tag}
          </span>
        ))}
        {(s.tags?.length ?? 0) > 2 && (
          <span className="text-[10px] text-fg-4">+{(s.tags?.length ?? 0) - 2}</span>
        )}
      </div>

      {/* Description */}
      {s.description ? (
        <p className="text-[12px] text-fg-3 line-clamp-2 leading-relaxed">{s.description}</p>
      ) : s.readme_excerpt ? (
        <p className="text-[12px] text-fg-3 line-clamp-2 leading-relaxed italic">{s.readme_excerpt}</p>
      ) : (
        <p className="text-[12px] text-fg-4 italic leading-relaxed">No description provided</p>
      )}

      {/* Footer */}
      <div className="mt-2 pt-2 border-t border-border flex items-center justify-between text-[10px] mono text-fg-4">
        <span className="truncate">{s.skill_id}</span>
        <span>{fmtDate(s.installed_at)}</span>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Main page
// ---------------------------------------------------------------------------

export default function SkillsPage({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [skills, setSkills] = useState<Skill[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [q, setQ] = useState("");
  const [trustFilter, setTrustFilter] = useState<string>("all");
  const [showInstall, setShowInstall] = useState(false);
  const [tab, setTab] = useState<"hub" | "url" | "create">("hub");
  const [busy, setBusy] = useState(false);
  const [formErr, setFormErr] = useState<string | null>(null);
  const [drawerSkill, setDrawerSkill] = useState<Skill | null>(null);
  const [uninstallTarget, setUninstallTarget] = useState<Skill | null>(null);
  const toast = useToast();

  // Hub tab
  const [hubName, setHubName] = useState("");
  // URL tab
  const [remoteUrl, setRemoteUrl] = useState("");
  // Create tab
  const [cName, setCName] = useState("");
  const [cCategory, setCCategory] = useState("Other");
  const [cDesc, setCDesc] = useState("");
  const [cContent, setCContent] = useState("");

  const refetch = () => {
    setLoading(true);
    fetchSkills().then(
      (s) => { setSkills(s); setLoading(false); },
      (e) => { setErr(`HTTP ${e?.status} ${e?.body ?? ""}`); setLoading(false); },
    );
  };

  useEffect(() => { refetch(); }, []);

  const groups = useMemo(() => {
    const filtered = skills.filter((s) => {
      const textMatch = !q || (s.name + " " + (s.description ?? "") + " " + (s.readme_excerpt ?? ""))
        .toLowerCase()
        .includes(q.toLowerCase());
      const trustMatch = trustFilter === "all" || s.trust_level === trustFilter;
      return textMatch && trustMatch;
    });
    const byCategory: Record<string, Skill[]> = {};
    for (const s of filtered) {
      const c = s.category || "Other";
      (byCategory[c] ??= []).push(s);
    }
    return Object.entries(byCategory).sort(([a], [b]) => a.localeCompare(b));
  }, [skills, q, trustFilter]);

  const resetInstall = () => {
    setHubName(""); setRemoteUrl(""); setCName(""); setCCategory("Other");
    setCDesc(""); setCContent(""); setFormErr(null);
  };

  const handleInstall = async () => {
    setFormErr(null);
    setBusy(true);
    try {
      let skill: Skill;
      if (tab === "hub") {
        if (!hubName.trim()) { setFormErr(L.nameRequired); setBusy(false); return; }
        skill = await installSkill("hub", hubName.trim());
      } else if (tab === "url") {
        if (!remoteUrl.trim()) { setFormErr(L.urlRequired); setBusy(false); return; }
        skill = await installSkillRemote(remoteUrl.trim());
      } else {
        if (!cName.trim()) { setFormErr(L.nameRequired); setBusy(false); return; }
        skill = await createSkill({ name: cName.trim(), category: cCategory, description: cDesc, content: cContent });
      }
      setShowInstall(false);
      resetInstall();
      refetch();
      toast({ tone: "success", msg: `Installed: ${skill.name}` });
    } catch (e: unknown) {
      const ae = e as { status?: number; body?: string };
      setFormErr(`Error ${ae?.status ?? ""}: ${ae?.body ?? "unknown"}`);
    } finally {
      setBusy(false);
    }
  };

  const handleToggle = async (s: Skill, enabled: boolean) => {
    setSkills((prev) => prev.map((sk) => sk.skill_id === s.skill_id ? { ...sk, enabled } : sk));
    try {
      await toggleSkill(s.skill_id, enabled);
      toast({ tone: "success", msg: `${s.name} ${enabled ? "enabled" : "disabled"}` });
    } catch (e: unknown) {
      setSkills((prev) => prev.map((sk) => sk.skill_id === s.skill_id ? { ...sk, enabled: !enabled } : sk));
      const ae = e as { status?: number; body?: string };
      toast({ tone: "error", msg: `Toggle failed: ${ae?.body ?? "unknown"}` });
    }
  };

  const handleUninstall = async () => {
    if (!uninstallTarget) return;
    setBusy(true);
    try {
      await uninstallSkill(uninstallTarget.skill_id);
      const name = uninstallTarget.name;
      setUninstallTarget(null);
      if (drawerSkill?.skill_id === uninstallTarget.skill_id) setDrawerSkill(null);
      refetch();
      toast({ tone: "success", msg: `Uninstalled: ${name}` });
    } catch (e: unknown) {
      const ae = e as { status?: number; body?: string };
      toast({ tone: "error", msg: `Error ${ae?.status ?? ""}: ${ae?.body ?? "unknown"}` });
      setUninstallTarget(null);
    } finally {
      setBusy(false);
    }
  };

  const TAB_CLS = (active: boolean) =>
    `px-3 py-1.5 text-xs rounded-md transition-colors ${active ? "bg-accent text-white" : "text-fg-3 hover:text-fg hover:bg-hover"}`;

  return (
    <section className="space-y-4" onClick={() => {}}>
      <PageHeader
        title={L.title}
        subtitle={L.subtitle(skills.length)}
        actions={
          <button
            onClick={() => setShowInstall(true)}
            className="px-3 h-8 rounded-md bg-accent text-white text-xs font-medium hover:opacity-90 transition-opacity"
          >
            {L.installBtn}
          </button>
        }
      />

      {/* Filters */}
      <div className="flex flex-wrap items-end gap-2 text-xs">
        <label className="flex flex-col gap-0.5 flex-1 min-w-[200px] max-w-sm">
          <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{L.searchLabel}</span>
          <input
            value={q}
            onChange={(e) => setQ(e.target.value)}
            placeholder={L.searchPlaceholder}
            className="h-8 px-3 rounded-md bg-bg-3 border border-border outline-none focus-ring"
          />
        </label>

        {/* Trust filter — CyberClaw governance filter */}
        <label className="flex flex-col gap-0.5">
          <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{L.trustLabel}</span>
          <select
            value={trustFilter}
            onChange={(e) => setTrustFilter(e.target.value)}
            className="h-8 px-2 rounded-md bg-bg-3 border border-border text-xs outline-none focus-ring"
          >
            <option value="all">{L.allTrust}</option>
            <option value="high">{L.trustHigh}</option>
            <option value="medium">{L.trustMedium}</option>
            <option value="low">{L.trustLow}</option>
          </select>
        </label>
      </div>

      {err && <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>}
      {loading && <TableSkeleton cols={3} rows={6} />}

      {!loading && groups.length === 0 && (
        <div className="rounded-lg border border-border bg-bg-2 p-10 text-center text-fg-3 text-sm">
          {skills.length === 0 ? L.noSkills : L.noMatch}
        </div>
      )}

      {groups.map(([cat, items]) => (
        <div key={cat} className="space-y-2">
          <div className="flex items-baseline gap-2">
            <h3 className="text-sm font-medium">{cat}</h3>
            <span className="text-[10px] text-fg-4 mono">{items.length}</span>
          </div>
          <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
            {items.map((s) => (
              <SkillCard
                key={s.skill_id}
                s={s}
                lang={lang}
                onToggle={handleToggle}
                onUninstall={setUninstallTarget}
                onOpen={setDrawerSkill}
              />
            ))}
          </div>
        </div>
      ))}

      {/* Detail drawer */}
      {drawerSkill && (
        <SkillDrawer
          skill={drawerSkill}
          lang={lang}
          onClose={() => setDrawerSkill(null)}
        />
      )}

      {/* Install Modal */}
      <Modal
        open={showInstall}
        onClose={() => { setShowInstall(false); resetInstall(); }}
        title={L.installTitle}
        width={520}
        footer={
          <>
            <button onClick={() => { setShowInstall(false); resetInstall(); }} className="px-3 h-8 rounded-md text-xs text-fg-3 hover:text-fg hover:bg-hover">
              {L.cancel}
            </button>
            <button onClick={handleInstall} disabled={busy} className="px-3 h-8 rounded-md bg-accent text-white text-xs font-medium hover:opacity-90 disabled:opacity-50">
              {tab === "create" ? L.create : L.install}
            </button>
          </>
        }
      >
        <div className="space-y-4">
          <div className="flex gap-1">
            <button className={TAB_CLS(tab === "hub")} onClick={() => { setTab("hub"); setFormErr(null); }}>{L.tabHub}</button>
            <button className={TAB_CLS(tab === "url")} onClick={() => { setTab("url"); setFormErr(null); }}>{L.tabUrl}</button>
            <button className={TAB_CLS(tab === "create")} onClick={() => { setTab("create"); setFormErr(null); }}>{L.tabCreate}</button>
          </div>

          {tab === "hub" && (
            <Field label={L.skillNameLabel} required>
              <TextInput value={hubName} onChange={setHubName} placeholder={L.skillNamePlaceholder} />
            </Field>
          )}

          {tab === "url" && (
            <Field label={L.urlLabel} hint={L.urlHint} required>
              <TextInput value={remoteUrl} onChange={setRemoteUrl} placeholder={L.urlPlaceholder} />
            </Field>
          )}

          {tab === "create" && (
            <div className="space-y-3">
              <Field label={L.createNameLabel} required>
                <TextInput value={cName} onChange={setCName} placeholder={L.createNamePlaceholder} />
              </Field>
              <Field label={L.createCategoryLabel}>
                <Select value={cCategory} onChange={setCCategory}>
                  {CATEGORIES.map((c) => <option key={c} value={c}>{c}</option>)}
                </Select>
              </Field>
              <Field label={L.createDescLabel}>
                <TextArea value={cDesc} onChange={setCDesc} placeholder={L.createDescPlaceholder} rows={2} />
              </Field>
              <Field label={L.createContentLabel}>
                <TextArea value={cContent} onChange={setCContent} placeholder="# My Skill&#10;…" rows={12} className="font-mono text-[12px]" />
              </Field>
            </div>
          )}

          {formErr && <p className="text-xs text-rose-400">{formErr}</p>}
        </div>
      </Modal>

      {/* Uninstall confirm Modal */}
      <Modal
        open={!!uninstallTarget}
        onClose={() => setUninstallTarget(null)}
        title={L.uninstallTitle}
        width={400}
        footer={
          <>
            <button onClick={() => setUninstallTarget(null)} className="px-3 h-8 rounded-md text-xs text-fg-3 hover:text-fg hover:bg-hover">
              {L.cancel}
            </button>
            <button onClick={handleUninstall} disabled={busy} className="px-3 h-8 rounded-md bg-rose-600 text-white text-xs font-medium hover:opacity-90 disabled:opacity-50">
              {L.uninstall}
            </button>
          </>
        }
      >
        <p className="text-sm text-fg-2">
          {L.uninstallMsg(uninstallTarget?.name ?? "")}
        </p>
      </Modal>
    </section>
  );
}
