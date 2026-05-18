import { useEffect, useState } from "react";
import { type ImPlatform, fetchImPlatforms, putImPlatformConfig, testImPlatform } from "@/lib/api";
import { type Lang } from "@/lib/i18n";
import { useToast } from "@/components/ToastBar";
import TableSkeleton from "@/components/TableSkeleton";
import EmptyState from "@/components/EmptyState";
import Modal from "@/components/Modal";
import Field, { TextInput, TextArea } from "@/components/Field";
import { MessageCircle } from "@/components/icons";

const KIND_TONE: Record<string, string> = {
  discord: "bg-violet-500/15 text-violet-300",
  slack: "bg-amber-500/15 text-amber-300",
  telegram: "bg-cyan-500/15 text-cyan-300",
  whatsapp: "bg-emerald-500/15 text-emerald-300",
  signal: "bg-white/10 text-fg-3",
  webhook: "bg-orange-500/15 text-orange-300",
};
const STATUS_TONE: Record<string, string> = {
  active: "bg-emerald-500/15 text-emerald-300",
  unconfigured: "bg-amber-500/15 text-amber-300",
  error: "bg-rose-500/15 text-rose-300",
  idle: "bg-white/10 text-fg-3",
};

function dict(lang: Lang) {
  return lang === "zh-CN"
    ? {
        title: "IM 入站（IM Platforms）",
        subtitle:
          "把 Telegram / Slack / Discord / 飞书 等 IM 平台的入站消息绑定到 Agent。与「Channels」（Agent 出站推送）相反。",
        totalFmt: (n: number) => `共 ${n} 个`,
        noImPlatforms: "未配置 IM 平台",
        noImPlatformsBody: "在 /admin → IM 平台 中添加。",
        colName: "名称",
        colKind: "类型",
        colConfigured: "已配置",
        colStatus: "状态",
        colLastError: "最后错误",
        configure: "配置",
        configureTitle: (name: string) => `配置 ${name}`,
        createTitle: "新增 IM 平台",
        addPlatform: "+ 新增",
        nameLabel: "名称",
        kindLabel: "类型",
        namePlaceholder: "例：my-slack-prod / telegram-team-a",
        nameRequired: "名称不能为空",
        nameInvalid: "名称仅支持小写字母 / 数字 / 横线 / 下划线",
        nameAlreadyExists: "该名称已存在，请换一个",
        testSend: "测试发送",
        cancel: "取消",
        save: "保存",
        configuredYes: "是",
        configuredNo: "未配置",
        testSavFirst: "请先保存后再测试",
        configuredFmt: (name: string) => `已配置 ${name}`,
        source: "来源",
      }
    : {
        title: "IM Inbound (IM Platforms)",
        subtitle:
          "Bind inbound messages from Telegram / Slack / Discord / Lark / etc. into the Agent. Contrast with Channels (Agent → outbound notifications).",
        totalFmt: (n: number) => `${n} total`,
        noImPlatforms: "No IM platforms configured",
        noImPlatformsBody: "Add them in /admin → IM Platforms.",
        colName: "name",
        colKind: "kind",
        colConfigured: "configured",
        colStatus: "status",
        colLastError: "last_error",
        configure: "Configure",
        configureTitle: (name: string) => `Configure ${name}`,
        createTitle: "New IM platform",
        addPlatform: "+ New",
        nameLabel: "Name",
        kindLabel: "Kind",
        namePlaceholder: "e.g. my-slack-prod / telegram-team-a",
        nameRequired: "Name is required",
        nameInvalid: "Name must be lowercase letters / digits / dash / underscore only",
        nameAlreadyExists: "A platform with this name already exists",
        testSend: "Test send",
        cancel: "Cancel",
        save: "Save",
        configuredYes: "yes",
        configuredNo: "not configured",
        testSavFirst: "Save first to enable test",
        configuredFmt: (name: string) => `Configured ${name}`,
        source: "Source",
      };
}

// Per-platform config fields state
type PlatformFields =
  | { kind: "telegram"; bot_token: string; allowed_users: string }
  | { kind: "slack"; signing_secret: string; bot_token: string; allowed_channels: string }
  | { kind: "discord"; bot_token: string; guild_id: string }
  | { kind: "lark"; app_id: string; app_secret: string; verification_token: string }
  | { kind: "other"; config: string };

function defaultFields(platform: ImPlatform): PlatformFields {
  const cfg = (platform.config ?? {}) as Record<string, string>;
  switch (platform.kind) {
    case "telegram":
      return { kind: "telegram", bot_token: cfg.bot_token ?? "", allowed_users: JSON.stringify(cfg.allowed_users ?? [], null, 2) };
    case "slack":
      return { kind: "slack", signing_secret: cfg.signing_secret ?? "", bot_token: cfg.bot_token ?? "", allowed_channels: JSON.stringify(cfg.allowed_channels ?? [], null, 2) };
    case "discord":
      return { kind: "discord", bot_token: cfg.bot_token ?? "", guild_id: cfg.guild_id ?? "" };
    case "lark":
      return { kind: "lark", app_id: cfg.app_id ?? "", app_secret: cfg.app_secret ?? "", verification_token: cfg.verification_token ?? "" };
    default:
      return { kind: "other", config: JSON.stringify(platform.config ?? {}, null, 2) };
  }
}

function parseJson(value: string, field: string): unknown {
  try { return JSON.parse(value); } catch { throw new Error(`Field '${field}' must be valid JSON`); }
}

function fieldsToPayload(f: PlatformFields): Record<string, unknown> {
  if (f.kind === "telegram") {
    return { bot_token: f.bot_token, allowed_users: parseJson(f.allowed_users || "[]", "allowed_users") };
  }
  if (f.kind === "slack") {
    return { signing_secret: f.signing_secret, bot_token: f.bot_token, allowed_channels: parseJson(f.allowed_channels || "[]", "allowed_channels") };
  }
  if (f.kind === "discord") {
    return { bot_token: f.bot_token, guild_id: f.guild_id };
  }
  if (f.kind === "lark") {
    return { app_id: f.app_id, app_secret: f.app_secret, verification_token: f.verification_token };
  }
  return parseJson(f.config || "{}", "config") as Record<string, unknown>;
}

function PlatformForm({ fields, onChange }: { fields: PlatformFields; onChange: (f: PlatformFields) => void }) {
  if (fields.kind === "telegram") {
    return (
      <div className="space-y-3">
        <Field label="Bot token" required>
          <TextInput type="password" value={fields.bot_token} onChange={(v) => onChange({ ...fields, bot_token: v })} placeholder="123456:ABC…" />
        </Field>
        <Field label="Allowed users (JSON array)" hint='e.g. [123456789, 987654321]'>
          <TextArea value={fields.allowed_users} onChange={(v) => onChange({ ...fields, allowed_users: v })} rows={3} className="font-mono text-[12px]" />
        </Field>
      </div>
    );
  }
  if (fields.kind === "slack") {
    return (
      <div className="space-y-3">
        <Field label="Signing secret" required>
          <TextInput type="password" value={fields.signing_secret} onChange={(v) => onChange({ ...fields, signing_secret: v })} />
        </Field>
        <Field label="Bot token" required>
          <TextInput type="password" value={fields.bot_token} onChange={(v) => onChange({ ...fields, bot_token: v })} placeholder="xoxb-…" />
        </Field>
        <Field label="Allowed channels (JSON array)" hint='e.g. ["C01234567"]'>
          <TextArea value={fields.allowed_channels} onChange={(v) => onChange({ ...fields, allowed_channels: v })} rows={3} className="font-mono text-[12px]" />
        </Field>
      </div>
    );
  }
  if (fields.kind === "discord") {
    return (
      <div className="space-y-3">
        <Field label="Bot token" required>
          <TextInput type="password" value={fields.bot_token} onChange={(v) => onChange({ ...fields, bot_token: v })} />
        </Field>
        <Field label="Guild ID" required>
          <TextInput value={fields.guild_id} onChange={(v) => onChange({ ...fields, guild_id: v })} placeholder="123456789012345678" />
        </Field>
      </div>
    );
  }
  if (fields.kind === "lark") {
    return (
      <div className="space-y-3">
        <Field label="App ID" required>
          <TextInput value={fields.app_id} onChange={(v) => onChange({ ...fields, app_id: v })} />
        </Field>
        <Field label="App secret" required>
          <TextInput type="password" value={fields.app_secret} onChange={(v) => onChange({ ...fields, app_secret: v })} />
        </Field>
        <Field label="Verification token">
          <TextInput value={fields.verification_token} onChange={(v) => onChange({ ...fields, verification_token: v })} />
        </Field>
      </div>
    );
  }
  return (
    <Field label="Config (JSON)">
      <TextArea value={fields.config} onChange={(v) => onChange({ ...fields, config: v })} rows={8} className="font-mono text-[12px]" />
    </Field>
  );
}

export default function ImPlatformsPage({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [platforms, setPlatforms] = useState<ImPlatform[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [configuring, setConfiguring] = useState<ImPlatform | null>(null);
  const [fields, setFields] = useState<PlatformFields | null>(null);
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const [formErr, setFormErr] = useState<string | null>(null);
  const [testResult, setTestResult] = useState<string | null>(null);
  // Create-mode state. When `isCreating` is true the modal renders extra
  // name + kind inputs at the top and handleSave uses `newName` as the
  // platform identifier (PUT acts as upsert in the backend, so create =
  // PUT to a fresh name).
  const [isCreating, setIsCreating] = useState(false);
  const [newName, setNewName] = useState("");
  const toast = useToast();

  const refetch = () => {
    setLoading(true);
    setErr(null);
    fetchImPlatforms().then(
      (p) => { setPlatforms(p); setLoading(false); },
      (e) => { setErr(`HTTP ${e?.status} ${e?.body ?? ""}`); setLoading(false); },
    );
  };

  useEffect(() => { refetch(); }, []);

  const openConfigure = (p: ImPlatform) => {
    setConfiguring(p);
    setFields(defaultFields(p));
    setFormErr(null);
    setTestResult(null);
    setSaved(false);
    setIsCreating(false);
    setNewName("");
  };

  const openCreate = () => {
    // Seed with a benign stub so the Modal mounts with default empty fields.
    // The actual platform `name` comes from the `newName` input; `kind` is
    // user-selectable via the PlatformForm dropdown (defaults to telegram).
    const stub: ImPlatform = {
      name: "",
      kind: "telegram",
      configured: false,
      config: {},
    };
    setConfiguring(stub);
    setFields(defaultFields(stub));
    setFormErr(null);
    setTestResult(null);
    setSaved(false);
    setIsCreating(true);
    setNewName("");
  };

  const closeModal = () => {
    setConfiguring(null);
    setIsCreating(false);
    setNewName("");
  };

  const handleSave = async () => {
    if (!configuring || !fields) return;
    // When creating, validate the new name first.
    let targetName = configuring.name;
    if (isCreating) {
      const trimmed = newName.trim();
      if (!trimmed) { setFormErr(L.nameRequired); return; }
      if (!/^[a-z0-9_-]+$/.test(trimmed)) { setFormErr(L.nameInvalid); return; }
      if (platforms.some((p) => p.name === trimmed)) {
        setFormErr(L.nameAlreadyExists);
        return;
      }
      targetName = trimmed;
    }
    let payload: Record<string, unknown>;
    try {
      payload = fieldsToPayload(fields);
    } catch (e: unknown) {
      setFormErr(e instanceof Error ? e.message : "Invalid JSON in one of the fields");
      return;
    }
    setFormErr(null);
    setBusy(true);
    try {
      await putImPlatformConfig(targetName, payload);
      setSaved(true);
      refetch();
      toast({ tone: "success", msg: L.configuredFmt(targetName) });
      if (isCreating) {
        // After create-save, switch the open Modal from "creating" to
        // "editing the just-created one" so the user can immediately use
        // the Test Send button (which requires `saved=true`).
        setConfiguring({ ...configuring, name: targetName });
        setIsCreating(false);
      }
    } catch (e: unknown) {
      const ae = e as { status?: number; body?: string };
      setFormErr(`Error ${ae?.status ?? ""}: ${ae?.body ?? "unknown"}`);
    } finally {
      setBusy(false);
    }
  };

  const handleTest = async () => {
    if (!configuring) return;
    setBusy(true);
    setTestResult(null);
    try {
      const res = await testImPlatform(configuring.name);
      setTestResult(res.ok ? `✓ Test message sent${res.detail ? ": " + res.detail : ""}` : `✗ ${res.detail ?? "failed"}`);
    } catch (e: unknown) {
      const ae = e as { status?: number; body?: string };
      setTestResult(`✗ ${ae?.body ?? "unknown error"}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="space-y-4">
      <header className="space-y-1">
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0">
            <h2 className="text-base font-medium text-fg">{L.title}</h2>
            <p className="text-[11px] text-fg-3 leading-relaxed">{L.subtitle}</p>
          </div>
          <button
            onClick={openCreate}
            className="shrink-0 px-3 h-8 rounded-md text-xs bg-accent text-accent-fg hover:opacity-90"
          >
            {L.addPlatform}
          </button>
        </div>
        <p className="text-xs text-fg-3">{L.totalFmt(platforms.length)}</p>
      </header>

      {err && <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>}
      {loading && <TableSkeleton cols={6} />}

      {!loading && platforms.length === 0 && (
        <EmptyState icon={MessageCircle} title={L.noImPlatforms} body={L.noImPlatformsBody} />
      )}

      {!loading && platforms.length > 0 && (
        <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
          <table className="w-full text-xs">
            <thead className="bg-bg-3">
              <tr className="text-left">
                <th className="px-3 py-2 font-medium text-fg-3">{L.colName}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colKind}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colConfigured}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colStatus}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colLastError}</th>
                <th className="px-3 py-2 font-medium text-fg-3"></th>
              </tr>
            </thead>
            <tbody>
              {platforms.map((p) => (
                <tr key={p.name} className="border-t border-border hover:bg-hover">
                  <td className="px-3 py-2 font-medium">{p.name}</td>
                  <td className="px-3 py-2">
                    <span className={`px-1.5 py-0.5 rounded text-[10px] mono ${KIND_TONE[p.kind] ?? "bg-white/10 text-fg-3"}`}>
                      {p.kind}
                    </span>
                  </td>
                  <td className="px-3 py-2 mono">
                    {p.configured
                      ? <span className="text-emerald-400">{L.configuredYes}</span>
                      : <span className="text-fg-4">{L.configuredNo}</span>}
                  </td>
                  <td className="px-3 py-2">
                    <span className={`px-1.5 py-0.5 rounded text-[10px] mono ${STATUS_TONE[p.status ?? ""] ?? "bg-white/10 text-fg-3"}`}>
                      {p.status ?? "—"}
                    </span>
                  </td>
                  <td className="px-3 py-2 mono text-rose-400 truncate max-w-xs" title={p.last_error ?? ""}>
                    {p.last_error ?? "—"}
                  </td>
                  <td className="px-3 py-2">
                    <button
                      onClick={() => openConfigure(p)}
                      className="px-2 h-6 rounded text-[11px] text-fg-3 hover:text-fg hover:bg-hover border border-border"
                    >
                      {L.configure}
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <Modal
        open={!!configuring}
        onClose={closeModal}
        title={isCreating ? L.createTitle : L.configureTitle(configuring?.name ?? "")}
        width={500}
        footer={
          <div className="flex items-center gap-2 w-full">
            <button
              onClick={handleTest}
              disabled={!saved || busy || isCreating}
              className="px-3 h-8 rounded-md text-xs text-fg-3 hover:text-fg hover:bg-hover border border-border disabled:opacity-40"
              title={saved ? undefined : L.testSavFirst}
            >
              {L.testSend}
            </button>
            <div className="flex-1" />
            <button onClick={closeModal} className="px-3 h-8 rounded-md text-xs text-fg-3 hover:text-fg hover:bg-hover">
              {L.cancel}
            </button>
            <button onClick={handleSave} disabled={busy} className="px-3 h-8 rounded-md bg-accent text-white text-xs font-medium hover:opacity-90 disabled:opacity-50">
              {L.save}
            </button>
          </div>
        }
      >
        <div className="space-y-4">
          {isCreating && fields && (
            <div className="space-y-3 pb-3 border-b border-border">
              <Field label={L.nameLabel} required hint={L.namePlaceholder}>
                <TextInput value={newName} onChange={setNewName} placeholder="my-slack-prod" />
              </Field>
              <Field label={L.kindLabel}>
                <select
                  className="h-9 px-2 rounded-md bg-bg-3 border border-border text-[13px] outline-none focus-ring w-full"
                  value={fields.kind}
                  onChange={(e) => setFields(defaultFields({
                    ...(configuring as ImPlatform),
                    kind: e.target.value,
                    config: {},
                  }))}
                >
                  <option value="telegram">Telegram</option>
                  <option value="slack">Slack</option>
                  <option value="discord">Discord</option>
                  <option value="lark">Lark (飞书)</option>
                  <option value="other">Other (raw JSON)</option>
                </select>
              </Field>
            </div>
          )}
          {fields && <PlatformForm fields={fields} onChange={setFields} />}
          {formErr && <p className="text-xs text-rose-400">{formErr}</p>}
          {testResult && (
            <p className={`text-xs ${testResult.startsWith("✓") ? "text-emerald-400" : "text-rose-400"}`}>
              {testResult}
            </p>
          )}
        </div>
      </Modal>

      <p className="text-[10px] text-fg-4">{L.source}: <code>/api/v1/admin/im-platforms</code>.</p>
    </section>
  );
}
