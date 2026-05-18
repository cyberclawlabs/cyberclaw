// SettingsPage v2 — 3-tab: Config / Env / About
// Config: raw TOML-like JSON edit; Env: key/value table with masking; About: existing info card.

import { useEffect, useRef, useState } from "react";
import { type AboutInfo, fetchAbout, fetchSettingsConfig, fetchSettingsEnv, updateSettingsConfig, updateSettingsEnv } from "@/lib/api";
import { type Lang } from "@/lib/i18n";
import Field, { TextArea, TextInput } from "@/components/Field";
import { useToast } from "@/components/ToastBar";

type Tab = "config" | "env" | "about";

function fmtUptime(secs: number): string {
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ${secs % 60}s`;
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  return `${h}h ${m}m`;
}
function fmtBuildTime(raw: string): string {
  const n = Number(raw);
  if (!Number.isNaN(n) && n > 1_000_000_000) {
    return new Date(n * 1000).toLocaleString();
  }
  return raw;
}
function isSensitive(key: string): boolean {
  return /token|key|secret|password/i.test(key);
}
function nowHMS(): string {
  return new Date().toLocaleTimeString("en", { hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false });
}

function dict(lang: Lang) {
  return lang === "zh-CN"
    ? {
        source: "来源",
        title: "设置",
        subtitle: "管理 CyberClaw 配置和环境变量",
        tabConfig: "配置",
        tabEnv: "环境变量",
        tabAbout: "关于",
        // Config tab
        configHint: "编辑配置文件内容。保存后立即生效。",
        refresh: "刷新",
        save: "保存",
        saving: "保存中…",
        savedAtFmt: (t: string) => `已在 ${t} 保存`,
        configPathFmt: (p: string) => `路径：${p}`,
        redactedHint: "已对敏感字段做脱敏显示（带 *** 的值）。仅修改其它字段后保存，密钥不会被覆盖。",
        // Env tab
        add: "+ 添加",
        saveAll: "保存全部",
        noEnvVars: "未配置环境变量",
        envVarsSavedFmt: (n: number) => `已保存 ${n} 个环境变量`,
        // About tab
        loading: "加载中…",
        colKey: "键",
        colValue: "值",
      }
    : {
        source: "Source",
        title: "Settings",
        subtitle: "Manage CyberClaw configuration and environment",
        tabConfig: "Config",
        tabEnv: "Env",
        tabAbout: "About",
        // Config tab
        configHint: "Edit the configuration file content. Changes are applied immediately on save.",
        refresh: "Refresh",
        save: "Save",
        saving: "Saving…",
        savedAtFmt: (t: string) => `Saved at ${t}`,
        configPathFmt: (p: string) => `Path: ${p}`,
        redactedHint: "Secret fields are shown redacted (values containing ***). Saving without modifying redacted lines preserves the real secret.",
        // Env tab
        add: "+ Add",
        saveAll: "Save all",
        noEnvVars: "No env vars configured",
        envVarsSavedFmt: (n: number) => `Saved ${n} env var${n !== 1 ? "s" : ""}`,
        // About tab
        loading: "Loading…",
        colKey: "Key",
        colValue: "Value",
      };
}

// ─── Config Tab ─────────────────────────────────────────────────────────────

function ConfigTab({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const toast = useToast();
  // The server stores raw TOML; we edit the text body directly. `meta` keeps
  // the path/redacted flags from the load response for UI hints (NOT mutated
  // on save — server is authoritative for those).
  const [meta, setMeta] = useState<{ path: string; redacted: boolean } | null>(null);
  const [draft, setDraft] = useState("");
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const load = () => {
    setLoading(true);
    setErr(null);
    fetchSettingsConfig().then(
      (data) => {
        setMeta({ path: data.path, redacted: !!data.redacted });
        setDraft(data.content ?? "");
      },
      (e) => setErr(`HTTP ${e?.status} ${e?.body ?? ""}`),
    ).finally(() => setLoading(false));
  };

  useEffect(() => { load(); }, []);

  const save = async () => {
    setSaving(true);
    setErr(null);
    try {
      await updateSettingsConfig(draft);
      toast({ tone: "success", msg: L.savedAtFmt(nowHMS()) });
    } catch (e: unknown) {
      const ae = e as { status?: number; body?: string };
      setErr(`HTTP ${ae?.status ?? ""} ${ae?.body ?? "save failed"}`);
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between gap-2">
        <p className="text-xs text-fg-3">{L.configHint}</p>
        <div className="flex items-center gap-2 shrink-0">
          <button
            onClick={load}
            disabled={loading}
            className="px-3 h-8 rounded-md text-xs bg-bg-3 border border-border hover:bg-hover disabled:opacity-50"
          >
            {L.refresh}
          </button>
          <button
            onClick={save}
            disabled={saving || loading}
            className="px-3 h-8 rounded-md text-xs bg-accent text-accent-fg hover:opacity-90 disabled:opacity-50"
          >
            {saving ? L.saving : L.save}
          </button>
        </div>
      </div>

      {err && <p className="text-[11px] text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>}

      {loading && <div className="h-64 rounded-md bg-bg-3 border border-border animate-pulse" />}

      {!loading && (
        <Field label="config.toml">
          <TextArea
            value={draft}
            onChange={setDraft}
            rows={24}
            className="font-mono text-[12px] leading-relaxed whitespace-pre"
            placeholder={L.loading}
          />
        </Field>
      )}

      {meta?.redacted && (
        <p className="text-[11px] text-amber-300/90 px-2 py-1.5 bg-amber-500/10 rounded">
          {L.redactedHint}
        </p>
      )}

      {meta && (
        <p className="text-[10px] text-fg-4">
          {L.configPathFmt(meta.path)} · {L.source}: <code>/api/v1/settings/config</code>
        </p>
      )}
    </div>
  );
}

// ─── Env Tab ────────────────────────────────────────────────────────────────

// P1-C fix: track `dirty` per row so we never PUT back a redacted "***"
// value over a real server-side secret. The server returns sensitive
// values redacted; if the operator hits "Save all" without touching a
// sensitive row, that row stays at its loaded redacted value and would
// silently overwrite the real secret. Skip undirtied sensitive rows.
type EnvRow = { key: string; value: string; visible: boolean; deleted: boolean; dirty: boolean };

function EnvTab({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const toast = useToast();
  const [rows, setRows] = useState<EnvRow[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const load = () => {
    setLoading(true);
    setErr(null);
    fetchSettingsEnv().then(
      (data) => {
        setRows(
          Object.entries(data).map(([key, value]) => ({
            key,
            value,
            visible: !isSensitive(key),
            deleted: false,
            dirty: false,
          })),
        );
      },
      (e) => setErr(`HTTP ${e?.status} ${e?.body ?? ""}`),
    ).finally(() => setLoading(false));
  };

  useEffect(() => { load(); }, []);

  const saveAll = async () => {
    setSaving(true);
    setErr(null);
    try {
      const record: Record<string, string> = {};
      for (const r of rows) {
        if (r.deleted || !r.key.trim()) continue;
        // SAFETY: skip sensitive rows the user never modified. Their `value`
        // currently holds whatever the server returned (potentially "***" or
        // redacted), so PUTting them back would corrupt the real secret.
        if (isSensitive(r.key) && !r.dirty) continue;
        record[r.key.trim()] = r.value;
      }
      await updateSettingsEnv(record);
      const count = Object.keys(record).length;
      toast({ tone: "success", msg: L.envVarsSavedFmt(count) });
      // After successful save, mark all dirty rows clean again.
      setRows((prev) => prev.map((r) => ({ ...r, dirty: false })));
    } catch (e: unknown) {
      const ae = e as { status?: number; body?: string };
      setErr(`HTTP ${ae?.status} ${ae?.body ?? ""}`);
    } finally {
      setSaving(false);
    }
  };

  const updateRow = (idx: number, patch: Partial<EnvRow>) => {
    setRows((prev) =>
      prev.map((r, i) => {
        if (i !== idx) return r;
        // Mark dirty when key or value is actually edited (not when just
        // toggling `visible` / `deleted` flags).
        const valueChanged = "value" in patch && patch.value !== r.value;
        const keyChanged = "key" in patch && patch.key !== r.key;
        const dirty = r.dirty || valueChanged || keyChanged;
        return { ...r, ...patch, dirty };
      }),
    );
  };

  const addRow = () => {
    setRows((prev) => [
      ...prev,
      { key: "", value: "", visible: true, deleted: false, dirty: true },
    ]);
  };

  const visibleRows = rows.map((r, idx) => ({ ...r, idx })).filter((r) => !r.deleted);

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between gap-2">
        <button
          onClick={addRow}
          className="px-3 h-8 rounded-md text-xs bg-bg-3 border border-border hover:bg-hover"
        >
          {L.add}
        </button>
        <div className="flex items-center gap-2">
          <button
            onClick={saveAll}
            disabled={saving || loading}
            className="px-3 h-8 rounded-md text-xs bg-accent text-accent-fg hover:opacity-90 disabled:opacity-50"
          >
            {saving ? L.saving : L.saveAll}
          </button>
        </div>
      </div>

      {err && <p className="text-[11px] text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>}

      {loading && <div className="h-40 rounded-md bg-bg-3 border border-border animate-pulse" />}

      {!loading && (
        <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
          <table className="w-full text-xs">
            <thead className="bg-bg-3">
              <tr className="text-left">
                <th className="px-3 py-2 font-medium text-fg-3 w-48">{L.colKey}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colValue}</th>
                <th className="px-3 py-2 font-medium text-fg-3 w-8" />
                <th className="px-3 py-2 font-medium text-fg-3 w-8" />
              </tr>
            </thead>
            <tbody>
              {visibleRows.length === 0 && (
                <tr>
                  <td colSpan={4} className="px-3 py-4 text-center text-fg-4">
                    {L.noEnvVars}
                  </td>
                </tr>
              )}
              {visibleRows.map(({ idx, key, value, visible }) => (
                <tr key={idx} className="border-t border-border">
                  <td className="px-2 py-1.5">
                    <TextInput
                      value={key}
                      onChange={(v) => updateRow(idx, { key: v })}
                      placeholder="KEY"
                      className="font-mono h-7 text-[11px]"
                    />
                  </td>
                  <td className="px-2 py-1.5">
                    <TextInput
                      value={visible ? value : "••••••••"}
                      onChange={(v) => updateRow(idx, { value: v })}
                      type={visible ? "text" : "password"}
                      placeholder="value"
                      className="font-mono h-7 text-[11px]"
                    />
                  </td>
                  <td className="px-2 py-1.5 text-center">
                    {isSensitive(key) && (
                      <button
                        onClick={() => updateRow(idx, { visible: !visible })}
                        title={visible ? "Hide" : "Show"}
                        className="text-fg-3 hover:text-fg"
                      >
                        {visible ? (
                          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8">
                            <path d="M1 12S5 4 12 4s11 8 11 8-4 8-11 8S1 12 1 12z" />
                            <circle cx="12" cy="12" r="3" />
                          </svg>
                        ) : (
                          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8">
                            <path d="M17.94 17.94A10.07 10.07 0 0112 20c-7 0-11-8-11-8a18.45 18.45 0 015.06-5.94" />
                            <path d="M9.9 4.24A9.12 9.12 0 0112 4c7 0 11 8 11 8a18.5 18.5 0 01-2.16 3.19" />
                            <line x1="1" y1="1" x2="23" y2="23" />
                          </svg>
                        )}
                      </button>
                    )}
                  </td>
                  <td className="px-2 py-1.5 text-center">
                    <button
                      onClick={() => updateRow(idx, { deleted: true })}
                      title="Delete"
                      className="text-fg-4 hover:text-rose-400"
                    >
                      <svg width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5">
                        <path d="M12 4L4 12M4 4l8 8" strokeLinecap="round" />
                      </svg>
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <p className="text-[10px] text-fg-4">
        {L.source}: <code>/api/v1/settings/env</code>
      </p>
    </div>
  );
}

// ─── About Tab ───────────────────────────────────────────────────────────────

function AboutTab({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [about, setAbout] = useState<AboutInfo | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    fetchAbout().then(
      (a) => !cancelled && setAbout(a),
      (e) => !cancelled && setErr(`HTTP ${e?.status} ${e?.body ?? ""}`),
    ).finally(() => !cancelled && setLoading(false));
    return () => { cancelled = true; };
  }, []);

  return (
    <div className="space-y-4">
      {err && <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>}
      {loading && <p className="text-xs text-fg-4">{L.loading}</p>}
      {!loading && about && (
        <div className="grid grid-cols-2 sm:grid-cols-3 gap-3">
          <InfoCard label="version" value={about.version} />
          <InfoCard label="commit" value={about.commit} mono />
          <InfoCard label="uptime" value={fmtUptime(about.uptime_secs)} />
          <InfoCard label="build_time" value={fmtBuildTime(about.build_time)} />
          {about.node_id && <InfoCard label="node_id" value={about.node_id} mono />}
        </div>
      )}
      <p className="text-[10px] text-fg-4">
        {L.source}: <code>/api/v1/settings/about</code>
      </p>
    </div>
  );
}

function InfoCard({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="rounded-lg border border-border bg-bg-2 p-3">
      <div className="text-[10px] mono text-fg-4 uppercase tracking-wide">{label}</div>
      <div className={`text-sm font-medium mt-1 truncate ${mono ? "mono" : ""}`}>{value}</div>
    </div>
  );
}

// ─── Page ────────────────────────────────────────────────────────────────────

export default function SettingsPage({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [tab, setTab] = useState<Tab>("config");

  const tabs: { id: Tab; label: string }[] = [
    { id: "config", label: L.tabConfig },
    { id: "env", label: L.tabEnv },
    { id: "about", label: L.tabAbout },
  ];

  return (
    <section className="space-y-4">
      <header className="space-y-0.5">
        <h2 className="text-base font-medium text-fg">{L.title}</h2>
        <p className="text-xs text-fg-3">{L.subtitle}</p>
      </header>

      {/* Tablist */}
      <div className="flex items-center gap-1 border-b border-border">
        {tabs.map(({ id, label }) => (
          <button
            key={id}
            onClick={() => setTab(id)}
            className={`px-3 py-1.5 text-xs rounded-t-md transition-colors ${
              tab === id
                ? "bg-accent-soft text-accent font-medium"
                : "text-fg-3 hover:text-fg"
            }`}
          >
            {label}
          </button>
        ))}
      </div>

      {/* Panel */}
      <div className="min-h-[200px]">
        {tab === "config" && <ConfigTab lang={lang} />}
        {tab === "env" && <EnvTab lang={lang} />}
        {tab === "about" && <AboutTab lang={lang} />}
      </div>
    </section>
  );
}
