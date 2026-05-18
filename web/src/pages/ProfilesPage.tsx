// ProfilesPage — user-level personality / SOUL preference CRUD.
// Backend: GET/POST/PATCH/DELETE /api/v1/profiles. Profile is NOT a 6th
// platform object; it's a thin preference layer (system_prompt + LLM tuning)
// stored at ~/.cyberclaw/profiles.toml chmod 600.

import { useEffect, useRef, useState } from "react";
import { useModalA11y } from "@/components/useModalA11y";
import {
  type Profile,
  createProfile,
  deleteProfile,
  fetchProfiles,
  updateProfile,
} from "@/lib/api";
import { type Lang } from "@/lib/i18n";
import TableSkeleton from "@/components/TableSkeleton";
import EmptyState from "@/components/EmptyState";
import { MessageSquare } from "@/components/icons";
import PageHeader from "@/components/PageHeader";
import Modal from "@/components/Modal";

function dict(lang: Lang) {
  return lang === "zh-CN"
    ? {
        title: "配置档案",
        subtitle: (n: number) => `${n} 个 SOUL 预设`,
        newProfile: "+ 新建配置",
        searchLabel: "搜索",
        searchPlaceholder: "名称 / 提示词",
        dialogNew: "新建配置",
        dialogEdit: "编辑配置",
        name: "名称",
        soul: "SOUL / 系统提示词",
        defaultAgent: "默认 agent",
        defaultModel: "默认 model",
        temperature: "温度",
        maxTokens: "最大 token",
        agentPlaceholder: "agent-id（可选）",
        modelPlaceholder: "claude-opus-4-7（可选）",
        tempPlaceholder: "0.0 – 2.0（可选）",
        maxTokensPlaceholder: "4096（可选）",
        soulPlaceholder: "你是一位资深 Rust 工程师，简洁回答，注明文件行号，不要编造 API。",
        namePlaceholder: "编码模式",
        cancel: "取消",
        create: "创建",
        saveChanges: "保存修改",
        saving: "保存中…",
        nameRequired: "名称必填",
        soulRequired: "SOUL / 系统提示词必填",
        tempRange: "温度需在 0–2 之间",
        maxTokensPositive: "最大 token 必须为正整数",
        failedSave: "保存失败",
        emptyTitle: "暂无配置",
        emptyFiltered: "没有匹配的配置",
        emptyBody: '点击"+ 新建配置"创建一个 SOUL 预设。',
        colName: "名称",
        colSoul: "SOUL 预览",
        colModel: "model",
        colUpdated: "更新时间",
        colActions: "操作",
        actionEdit: "编辑",
        actionDelete: "删除",
        confirmDelete: (n: string) => `确认删除配置 "${n}"？`,
        sourceNote:
          "存储于 ~/.cyberclaw/profiles.toml（chmod 600）。Profile 是用户偏好层，用于调整 Agent 的行为；它不是另一种 Agent。",
      }
    : {
        title: "Profiles",
        subtitle: (n: number) => `${n} SOUL preset${n === 1 ? "" : "s"}`,
        newProfile: "+ New profile",
        searchLabel: "search",
        searchPlaceholder: "name / prompt",
        dialogNew: "New profile",
        dialogEdit: "Edit profile",
        name: "name",
        soul: "SOUL / system prompt",
        defaultAgent: "default agent",
        defaultModel: "default model",
        temperature: "temperature",
        maxTokens: "max tokens",
        agentPlaceholder: "agent-id (optional)",
        modelPlaceholder: "claude-opus-4-7 (optional)",
        tempPlaceholder: "0.0 – 2.0 (optional)",
        maxTokensPlaceholder: "4096 (optional)",
        soulPlaceholder:
          "You are a senior Rust engineer. Be terse, cite line numbers, never invent APIs.",
        namePlaceholder: "Coding mode",
        cancel: "Cancel",
        create: "Create",
        saveChanges: "Save changes",
        saving: "Saving…",
        nameRequired: "Name is required",
        soulRequired: "SOUL / system prompt is required",
        tempRange: "Temperature must be 0–2",
        maxTokensPositive: "Max tokens must be a positive integer",
        failedSave: "Failed to save profile",
        emptyTitle: "No profiles yet",
        emptyFiltered: "No profiles match the filter",
        emptyBody: 'Click "+ New profile" to create a SOUL preset.',
        colName: "name",
        colSoul: "SOUL preview",
        colModel: "model",
        colUpdated: "updated",
        colActions: "actions",
        actionEdit: "edit",
        actionDelete: "Delete",
        confirmDelete: (n: string) => `Delete profile "${n}"?`,
        sourceNote:
          "Stored at ~/.cyberclaw/profiles.toml (chmod 600). Profiles are user preference layers that bias Agent behavior; they do not replace an Agent.",
      };
}

function relTime(lang: Lang, iso?: string): string {
  if (!iso) return "—";
  const diff = Date.now() - new Date(iso).getTime();
  if (isNaN(diff)) return iso;
  const zh = lang === "zh-CN";
  if (diff < 60_000) return zh ? "<1 分钟前" : "<1m ago";
  if (diff < 3_600_000) return zh ? `${Math.round(diff / 60_000)} 分钟前` : `${Math.round(diff / 60_000)}m ago`;
  if (diff < 86_400_000) return zh ? `${Math.round(diff / 3_600_000)} 小时前` : `${Math.round(diff / 3_600_000)}h ago`;
  return zh ? `${Math.round(diff / 86_400_000)} 天前` : `${Math.round(diff / 86_400_000)}d ago`;
}

// ---------------------------------------------------------------------------
// Profile form dialog
// ---------------------------------------------------------------------------

type FormState = {
  name: string;
  system_prompt: string;
  default_agent_id: string;
  default_model: string;
  temperature: string;
  max_tokens: string;
};

function ProfileDialog({
  initial,
  lang,
  onClose,
  onSaved,
}: {
  initial: Profile | null;
  lang: Lang;
  onClose: () => void;
  onSaved: (p: Profile) => void;
}) {
  const L = dict(lang);
  const dialogRef = useRef<HTMLDivElement>(null);
  useModalA11y(dialogRef, true, onClose);
  const [form, setForm] = useState<FormState>({
    name: initial?.name ?? "",
    system_prompt: initial?.system_prompt ?? "",
    default_agent_id: initial?.default_agent_id ?? "",
    default_model: initial?.default_model ?? "",
    temperature: initial?.temperature?.toString() ?? "",
    max_tokens: initial?.max_tokens?.toString() ?? "",
  });
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const set = (k: keyof FormState, v: string) =>
    setForm((f) => ({ ...f, [k]: v }));

  async function submit() {
    if (!form.name.trim()) { setErr(L.nameRequired); return; }
    if (!form.system_prompt.trim()) { setErr(L.soulRequired); return; }

    const temp = form.temperature.trim();
    const maxT = form.max_tokens.trim();
    let temperature: number | undefined;
    let max_tokens: number | undefined;
    if (temp) {
      const n = Number(temp);
      if (!Number.isFinite(n) || n < 0 || n > 2) { setErr(L.tempRange); return; }
      temperature = n;
    }
    if (maxT) {
      const n = Number(maxT);
      if (!Number.isInteger(n) || n <= 0) { setErr(L.maxTokensPositive); return; }
      max_tokens = n;
    }

    setBusy(true);
    setErr(null);
    const payload: Partial<Profile> = {
      name: form.name.trim(),
      system_prompt: form.system_prompt.trim(),
      default_agent_id: form.default_agent_id.trim() || undefined,
      default_model: form.default_model.trim() || undefined,
      temperature,
      max_tokens,
    };
    try {
      const saved = initial
        ? await updateProfile(initial.id, payload)
        : await createProfile(payload);
      onSaved(saved);
    } catch (e: unknown) {
      const apiErr = e as { body?: string; status?: number };
      setErr(apiErr?.body ?? L.failedSave);
    } finally {
      setBusy(false);
    }
  }

  // Field row helper: consistent label + input pair, no mono unless explicitly
  // technical (identifiers). System prompt is prose → never mono.
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60" onClick={onClose}>
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-label={initial ? L.dialogEdit : L.dialogNew}
        tabIndex={-1}
        className="bg-bg-2 border border-border rounded-xl p-5 w-full max-w-lg space-y-3 shadow-xl outline-none"
        onClick={(e) => e.stopPropagation()}
      >
        <h2 className="text-sm font-semibold">{initial ? L.dialogEdit : L.dialogNew}</h2>

        {err && <p className="text-xs text-rose-400 px-2 py-1 bg-rose-500/10 rounded">{err}</p>}

        <label className="flex flex-col gap-0.5 text-xs">
          <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{L.name}</span>
          <input
            className="px-2 py-1.5 rounded bg-bg-3 border border-border outline-none focus-ring"
            value={form.name}
            onChange={(e) => set("name", e.target.value)}
            placeholder={L.namePlaceholder}
          />
        </label>

        <label className="flex flex-col gap-0.5 text-xs">
          <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{L.soul}</span>
          <textarea
            rows={6}
            className="px-2 py-1.5 rounded bg-bg-3 border border-border outline-none focus-ring resize-y"
            value={form.system_prompt}
            onChange={(e) => set("system_prompt", e.target.value)}
            placeholder={L.soulPlaceholder}
          />
        </label>

        <div className="grid grid-cols-2 gap-2">
          <label className="flex flex-col gap-0.5 text-xs">
            <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{L.defaultAgent}</span>
            <input
              className="px-2 py-1.5 rounded bg-bg-3 border border-border outline-none focus-ring mono"
              value={form.default_agent_id}
              onChange={(e) => set("default_agent_id", e.target.value)}
              placeholder={L.agentPlaceholder}
            />
          </label>

          <label className="flex flex-col gap-0.5 text-xs">
            <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{L.defaultModel}</span>
            <input
              className="px-2 py-1.5 rounded bg-bg-3 border border-border outline-none focus-ring mono"
              value={form.default_model}
              onChange={(e) => set("default_model", e.target.value)}
              placeholder={L.modelPlaceholder}
            />
          </label>

          <label className="flex flex-col gap-0.5 text-xs">
            <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{L.temperature}</span>
            <input
              className="px-2 py-1.5 rounded bg-bg-3 border border-border outline-none focus-ring"
              value={form.temperature}
              onChange={(e) => set("temperature", e.target.value)}
              placeholder={L.tempPlaceholder}
            />
          </label>

          <label className="flex flex-col gap-0.5 text-xs">
            <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{L.maxTokens}</span>
            <input
              className="px-2 py-1.5 rounded bg-bg-3 border border-border outline-none focus-ring"
              value={form.max_tokens}
              onChange={(e) => set("max_tokens", e.target.value)}
              placeholder={L.maxTokensPlaceholder}
            />
          </label>
        </div>

        <div className="flex justify-end gap-2 pt-1">
          <button
            className="px-3 py-1.5 text-xs rounded border border-border hover:bg-hover"
            onClick={onClose}
            disabled={busy}
          >
            {L.cancel}
          </button>
          <button
            className="px-3 py-1.5 text-xs rounded bg-accent text-accent-fg hover:opacity-90 disabled:opacity-50"
            onClick={submit}
            disabled={busy}
          >
            {busy ? L.saving : initial ? L.saveChanges : L.create}
          </button>
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Main page
// ---------------------------------------------------------------------------

export default function ProfilesPage({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [q, setQ] = useState("");
  const [editing, setEditing] = useState<Profile | null | undefined>(undefined);
  const [busy, setBusy] = useState<string | null>(null);

  async function refresh() {
    try {
      setProfiles(await fetchProfiles());
      setErr(null);
    } catch (e: unknown) {
      const apiErr = e as { status?: number; body?: string };
      setErr(`HTTP ${apiErr?.status} ${apiErr?.body ?? ""}`);
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => { refresh(); }, []);

  const filtered = profiles.filter(
    (p) =>
      !q ||
      p.name.toLowerCase().includes(q.toLowerCase()) ||
      p.system_prompt.toLowerCase().includes(q.toLowerCase()),
  );

  // P1 fix: bare `confirm()` was silently dropped in PWA/Electron, and
  // the catch swallowed delete errors. State-driven Modal + surfaced err.
  const [pendingDelete, setPendingDelete] = useState<Profile | null>(null);
  function remove(p: Profile) {
    setPendingDelete(p);
  }
  async function confirmRemove() {
    const p = pendingDelete;
    if (!p) return;
    setPendingDelete(null);
    setBusy(p.id);
    try {
      await deleteProfile(p.id);
      setProfiles((prev) => prev.filter((x) => x.id !== p.id));
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setErr(`Failed to delete profile: ${msg}`);
    } finally {
      setBusy(null);
    }
  }

  return (
    <section className="space-y-4">
      {editing !== undefined && (
        <ProfileDialog
          initial={editing}
          lang={lang}
          onClose={() => setEditing(undefined)}
          onSaved={(saved) => {
            setProfiles((prev) => {
              const idx = prev.findIndex((p) => p.id === saved.id);
              return idx >= 0
                ? prev.map((p) => (p.id === saved.id ? saved : p))
                : [...prev, saved];
            });
            setEditing(undefined);
          }}
        />
      )}

      <PageHeader
        title={L.title}
        subtitle={L.subtitle(profiles.length)}
        actions={
          <button
            className="px-3 py-1.5 rounded-md bg-accent text-accent-fg text-[11px] hover:opacity-90"
            onClick={() => setEditing(null)}
          >
            {L.newProfile}
          </button>
        }
      />

      <div className="flex flex-wrap items-end gap-2 text-xs">
        <label className="flex flex-col gap-0.5">
          <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{L.searchLabel}</span>
          <input
            value={q}
            onChange={(e) => setQ(e.target.value)}
            placeholder={L.searchPlaceholder}
            className="h-8 px-3 rounded-md bg-bg-3 border border-border outline-none focus-ring w-56"
          />
        </label>
      </div>

      {err && <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>}

      {loading && <TableSkeleton cols={5} />}

      {!loading && filtered.length === 0 && (
        <EmptyState
          icon={MessageSquare}
          title={profiles.length === 0 ? L.emptyTitle : L.emptyFiltered}
          body={profiles.length === 0 ? L.emptyBody : undefined}
        />
      )}

      {!loading && filtered.length > 0 && (
        <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
          <table className="w-full text-xs table-fixed">
            <thead className="bg-bg-3">
              <tr className="text-left">
                <th className="px-3 py-2 font-medium text-fg-3 w-[18%]">{L.colName}</th>
                <th className="px-3 py-2 font-medium text-fg-3 w-[40%]">{L.colSoul}</th>
                <th className="px-3 py-2 font-medium text-fg-3 w-[14%] mono">{L.colModel}</th>
                <th className="px-3 py-2 font-medium text-fg-3 w-[14%]">{L.colUpdated}</th>
                <th className="px-3 py-2 font-medium text-fg-3 w-[14%]">{L.colActions}</th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((p) => (
                <tr key={p.id} className="border-t border-border hover:bg-hover">
                  <td className="px-3 py-2 font-medium truncate max-w-0 overflow-hidden">
                    {p.name}
                  </td>
                  <td className="px-3 py-2 text-fg-3 truncate max-w-0 overflow-hidden text-[11px]">
                    {p.system_prompt}
                  </td>
                  <td className="px-3 py-2 mono text-fg-3 truncate max-w-0 overflow-hidden">
                    {p.default_model ?? "—"}
                  </td>
                  <td className="px-3 py-2 text-fg-3">{relTime(lang, p.updated_at)}</td>
                  <td className="px-3 py-2">
                    <div className="flex items-center gap-1.5">
                      <button
                        disabled={busy === p.id}
                        onClick={() => setEditing(p)}
                        className="px-1.5 py-0.5 text-[10px] rounded border border-border hover:bg-hover disabled:opacity-40"
                      >
                        {L.actionEdit}
                      </button>
                      <button
                        disabled={busy === p.id}
                        onClick={() => remove(p)}
                        className="px-1.5 py-0.5 text-[10px] rounded border border-rose-500/30 text-rose-400 hover:bg-rose-500/10 disabled:opacity-40"
                      >
                        {L.actionDelete}
                      </button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <p className="text-[10px] text-fg-4">{L.sourceNote}</p>

      <Modal
        open={!!pendingDelete}
        onClose={() => setPendingDelete(null)}
        title={L.actionDelete}
        width={420}
        footer={
          <>
            <button
              onClick={() => setPendingDelete(null)}
              className="px-3 h-8 rounded-md text-xs text-fg-3 hover:text-fg hover:bg-hover"
            >
              {L.cancel}
            </button>
            <button
              onClick={confirmRemove}
              className="px-3 h-8 rounded-md bg-rose-600 text-white text-xs font-medium hover:opacity-90"
            >
              {L.actionDelete}
            </button>
          </>
        }
      >
        <p className="text-sm text-fg-2">
          {pendingDelete ? L.confirmDelete(pendingDelete.name) : ""}
        </p>
      </Modal>
    </section>
  );
}
