import { useEffect, useState } from "react";
import { useToast } from "@/components/ToastBar";
import { type Channel, fetchChannels, updateChannel } from "@/lib/api";
import { type Lang } from "@/lib/i18n";
import TableSkeleton from "@/components/TableSkeleton";
import EmptyState from "@/components/EmptyState";
import Modal from "@/components/Modal";
import Field, { TextInput, TextArea } from "@/components/Field";
import { Radio } from "@/components/icons";

const STATUS_TONE: Record<string, string> = {
  active: "bg-emerald-500/15 text-emerald-300",
  unconfigured: "bg-amber-500/15 text-amber-300",
  disabled: "bg-zinc-500/15 text-zinc-400",
  error: "bg-rose-500/15 text-rose-300",
};

function dict(lang: Lang) {
  return lang === "zh-CN"
    ? {
        title: "出站推送（Channels）",
        subtitle:
          "Agent 发送通知到外部接收方的通用 webhook 队列。与「IM 平台」（入站 IM 用户消息）相反。",
        totalFmt: (n: number) => `共 ${n} 个`,
        noChannels: "未配置频道",
        noChannelsBody: "在 /admin → 频道 中添加。",
        colChannelId: "channel_id",
        colName: "名称",
        colStatus: "状态",
        colEnabled: "已启用",
        colSecret: "密钥",
        colWebhookUrl: "webhook_url",
        edit: "编辑",
        editTitle: (name: string) => `编辑频道 ${name}`,
        createTitle: "新增频道",
        addChannel: "+ 新增",
        idLabel: "channel_id",
        idPlaceholder: "例：my-pagerduty-prod",
        idRequired: "channel_id 不能为空",
        idInvalid: "channel_id 仅支持小写字母 / 数字 / 横线 / 下划线",
        idAlreadyExists: "该 channel_id 已存在",
        nameLabel: "显示名称",
        cancel: "取消",
        save: "保存",
        fieldEnabled: "已启用",
        fieldWebhookUrl: "Webhook URL",
        fieldSecretConfigured: "已配置密钥",
        fieldSecretHint: "由后端管理 — 通过环境变量更新",
        secretConfigured: "✓ 已配置",
        secretNotConfigured: "未配置",
        fieldConfigJson: "Config (JSON)",
        invalidJson: "Config 必须是合法 JSON",
        yes: "是",
        no: "否",
        updatedFmt: (name: string) => `已更新 ${name}`,
        source: "来源",
      }
    : {
        title: "Outbound (Channels)",
        subtitle:
          "Generic webhook queues for Agent → external receivers. Contrast with IM Platforms (inbound IM-user messages).",
        totalFmt: (n: number) => `${n} total`,
        noChannels: "No channels configured",
        noChannelsBody: "Add channels in /admin → Channels.",
        colChannelId: "channel_id",
        colName: "name",
        colStatus: "status",
        colEnabled: "enabled",
        colSecret: "secret",
        colWebhookUrl: "webhook_url",
        edit: "Edit",
        editTitle: (name: string) => `Edit channel ${name}`,
        createTitle: "New channel",
        addChannel: "+ New",
        idLabel: "channel_id",
        idPlaceholder: "e.g. my-pagerduty-prod",
        idRequired: "channel_id is required",
        idInvalid: "channel_id must be lowercase letters / digits / dash / underscore only",
        idAlreadyExists: "channel_id already exists",
        nameLabel: "Display name",
        cancel: "Cancel",
        save: "Save",
        fieldEnabled: "Enabled",
        fieldWebhookUrl: "Webhook URL",
        fieldSecretConfigured: "Secret configured",
        fieldSecretHint: "Managed by backend — update via env vars",
        secretConfigured: "✓ configured",
        secretNotConfigured: "not configured",
        fieldConfigJson: "Config (JSON)",
        invalidJson: "Config must be valid JSON",
        yes: "Yes",
        no: "No",
        updatedFmt: (name: string) => `Updated ${name}`,
        source: "Source",
      };
}

export default function ChannelsPage({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [channels, setChannels] = useState<Channel[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [editing, setEditing] = useState<Channel | null>(null);
  const [busy, setBusy] = useState(false);
  const toast = useToast();

  // Form state
  const [enabled, setEnabled] = useState(false);
  const [webhookUrl, setWebhookUrl] = useState("");
  const [configRaw, setConfigRaw] = useState("{}");
  const [formErr, setFormErr] = useState<string | null>(null);
  // Create-mode state: when isCreating, modal renders extra id+name inputs
  // and handleSave PUTs to the user-supplied id (PUT acts as upsert in
  // the backend, so create = PUT to a fresh id).
  const [isCreating, setIsCreating] = useState(false);
  const [newId, setNewId] = useState("");
  const [newName, setNewName] = useState("");

  const refetch = () => {
    setLoading(true);
    setErr(null);
    fetchChannels().then(
      (ch) => { setChannels(ch); setLoading(false); },
      (e) => { setErr(`HTTP ${e?.status} ${e?.body ?? ""}`); setLoading(false); },
    );
  };

  useEffect(() => { refetch(); }, []);

  const openEdit = (ch: Channel) => {
    setEditing(ch);
    setEnabled(ch.enabled);
    setWebhookUrl(ch.webhook_url ?? "");
    setConfigRaw(ch.config ? JSON.stringify(ch.config, null, 2) : "{}");
    setFormErr(null);
    setIsCreating(false);
    setNewId("");
    setNewName("");
  };

  const openCreate = () => {
    setEditing({ id: "", name: "", enabled: true, webhook_url: "", status: "disabled", secret_configured: false, config: {} });
    setEnabled(true);
    setWebhookUrl("");
    setConfigRaw("{}");
    setFormErr(null);
    setIsCreating(true);
    setNewId("");
    setNewName("");
  };

  const closeModal = () => {
    setEditing(null);
    setIsCreating(false);
    setNewId("");
    setNewName("");
  };

  const handleSave = async () => {
    if (!editing) return;
    // Create-mode validation: id is the channel_id slug.
    let targetId = editing.id;
    let targetName = editing.name ?? editing.id;
    if (isCreating) {
      const id = newId.trim();
      if (!id) { setFormErr(L.idRequired); return; }
      if (!/^[a-z0-9_-]+$/.test(id)) { setFormErr(L.idInvalid); return; }
      if (channels.some((c) => c.id === id)) { setFormErr(L.idAlreadyExists); return; }
      targetId = id;
      targetName = newName.trim() || id;
    }
    let parsedConfig: Record<string, unknown>;
    try {
      parsedConfig = JSON.parse(configRaw);
    } catch {
      setFormErr(L.invalidJson);
      return;
    }
    setFormErr(null);
    setBusy(true);
    try {
      await updateChannel(targetId, {
        enabled,
        webhook_url: webhookUrl || undefined,
        config: parsedConfig,
        ...(isCreating ? { name: targetName } : {}),
      });
      closeModal();
      refetch();
      toast({ tone: "success", msg: L.updatedFmt(targetName) });
    } catch (e: unknown) {
      const ae = e as { status?: number; body?: string };
      setFormErr(`Error ${ae?.status ?? ""}: ${ae?.body ?? "unknown"}`);
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
            {L.addChannel}
          </button>
        </div>
        <p className="text-xs text-fg-3">{L.totalFmt(channels.length)}</p>
      </header>

      {err && <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>}
      {loading && <TableSkeleton cols={7} />}

      {!loading && channels.length === 0 && (
        <EmptyState icon={Radio} title={L.noChannels} body={L.noChannelsBody} />
      )}

      {!loading && channels.length > 0 && (
        <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
          <table className="w-full text-xs">
            <thead className="bg-bg-3">
              <tr className="text-left">
                <th className="px-3 py-2 font-medium text-fg-3">{L.colChannelId}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colName}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colStatus}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colEnabled}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colSecret}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colWebhookUrl}</th>
                <th className="px-3 py-2 font-medium text-fg-3"></th>
              </tr>
            </thead>
            <tbody>
              {channels.map((ch) => (
                <tr key={ch.id} className="border-t border-border hover:bg-hover">
                  <td className="px-3 py-2 mono text-fg-4">{ch.id}</td>
                  <td className="px-3 py-2 text-fg-2">{ch.name ?? "—"}</td>
                  <td className="px-3 py-2">
                    <span className={`px-1.5 py-0.5 rounded text-[10px] mono ${STATUS_TONE[ch.status ?? ""] ?? "bg-white/10 text-fg-3"}`}>
                      {ch.status ?? "—"}
                    </span>
                  </td>
                  <td className="px-3 py-2 mono text-fg-4">{ch.enabled ? L.yes : L.no}</td>
                  <td className="px-3 py-2 mono text-fg-4">{ch.secret_configured ? "✓" : "—"}</td>
                  <td className="px-3 py-2 mono text-fg-3 truncate max-w-xs">{ch.webhook_url ?? "—"}</td>
                  <td className="px-3 py-2">
                    <button
                      onClick={() => openEdit(ch)}
                      className="px-2 h-6 rounded text-[11px] text-fg-3 hover:text-fg hover:bg-hover border border-border"
                    >
                      {L.edit}
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <Modal
        open={!!editing}
        onClose={closeModal}
        title={isCreating ? L.createTitle : L.editTitle(editing?.name ?? editing?.id ?? "")}
        width={480}
        footer={
          <>
            <button onClick={closeModal} className="px-3 h-8 rounded-md text-xs text-fg-3 hover:text-fg hover:bg-hover">
              {L.cancel}
            </button>
            <button onClick={handleSave} disabled={busy} className="px-3 h-8 rounded-md bg-accent text-white text-xs font-medium hover:opacity-90 disabled:opacity-50">
              {L.save}
            </button>
          </>
        }
      >
        <div className="space-y-4">
          {isCreating && (
            <div className="space-y-3 pb-3 border-b border-border">
              <Field label={L.idLabel} required hint={L.idPlaceholder}>
                <TextInput value={newId} onChange={setNewId} placeholder="my-pagerduty-prod" />
              </Field>
              <Field label={L.nameLabel}>
                <TextInput value={newName} onChange={setNewName} placeholder={newId || L.idPlaceholder} />
              </Field>
            </div>
          )}
          <Field label={L.fieldEnabled}>
            <label className="flex items-center gap-2 cursor-pointer">
              <input
                type="checkbox"
                checked={enabled}
                onChange={(e) => setEnabled(e.target.checked)}
                className="w-4 h-4 accent-accent"
              />
              <span className="text-[13px] text-fg">{enabled ? L.yes : L.no}</span>
            </label>
          </Field>
          <Field label={L.fieldWebhookUrl}>
            <TextInput value={webhookUrl} onChange={setWebhookUrl} placeholder="https://…" />
          </Field>
          <Field label={L.fieldSecretConfigured} hint={L.fieldSecretHint}>
            <div className="h-9 px-3 flex items-center rounded-md bg-bg-3 border border-border text-[13px] text-fg-4">
              {editing?.secret_configured ? L.secretConfigured : L.secretNotConfigured}
            </div>
          </Field>
          <Field label={L.fieldConfigJson}>
            <TextArea value={configRaw} onChange={setConfigRaw} rows={6} className="font-mono text-[12px]" />
          </Field>
          {formErr && <p className="text-xs text-rose-400">{formErr}</p>}
        </div>
      </Modal>

      <p className="text-[10px] text-fg-4">{L.source}: <code>/api/v1/channels</code>.</p>
    </section>
  );
}
