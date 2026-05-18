// OnboardingBanner — 轻量、可跳过、不打扰的首次配置提示
//
// 设计原则（用户明示）:
// 1. 不要每次都弹出 — 完成 / 跳过后永不再问；"稍后" 可设置 7 天冷却
// 2. 可以跳过 — 主操作"设置"旁有"稍后"和"✕ 不再提示"
// 3. 仅填最核心的 — 只问 LLM key（必填）+ display name（可选），其他配置后置到对应页
// 4. 非阻断 — 顶部 banner 不是 modal，用户随时可以忽略继续工作

import { useEffect, useRef, useState } from "react";
import { useModalA11y } from "./useModalA11y";
import { type Lang } from "@/lib/i18n";
import {
  fetchOnboardingStatus,
  postOnboardingLlmConfig,
  postOnboardingComplete,
} from "@/lib/api";

const LS_DISMISS_UNTIL = "cyberclaw.onboarding.dismissed_until";
const LS_DISMISS_FOREVER = "cyberclaw.onboarding.dismissed_forever";

interface BannerProps {
  /** Kept for API compatibility with the AppV2 mount site; the wrappers in
   *  lib/api.ts pull the JWT from sessionStorage centrally, so we no longer
   *  need to thread it through props. Empty-string still gates the visibility
   *  effect. */
  jwt: string;
  lang: Lang;
}

const T = {
  zh: {
    banner: "几步配置即可开始使用 CyberClaw",
    setup: "设置",
    later: "稍后再说",
    never: "不再提示",
    title: "快速设置",
    subtitle: "只需 LLM 接入即可使用 chat。其它配置可在对应页按需添加。",
    llmProvider: "LLM Provider",
    llmKey: "API Key",
    llmKeyPlaceholder: "sk-...",
    displayName: "显示名（可选）",
    displayNamePlaceholder: "留空将使用账号 ID",
    cancel: "取消",
    save: "保存并完成",
    saving: "保存中…",
    success: "已完成 — 可以开始使用了",
    error: "保存失败",
  },
  en: {
    banner: "A few quick steps to start using CyberClaw",
    setup: "Set up",
    later: "Later",
    never: "Don't show again",
    title: "Quick Setup",
    subtitle: "Just LLM access to start chatting. Other config can be added per-page as needed.",
    llmProvider: "LLM Provider",
    llmKey: "API Key",
    llmKeyPlaceholder: "sk-...",
    displayName: "Display name (optional)",
    displayNamePlaceholder: "Defaults to user ID if blank",
    cancel: "Cancel",
    save: "Save & finish",
    saving: "Saving…",
    success: "Done — ready to use",
    error: "Save failed",
  },
};

export default function OnboardingBanner({ jwt, lang }: BannerProps) {
  const t = T[lang === "zh-CN" ? "zh" : "en"];
  const [show, setShow] = useState(false);
  const [modalOpen, setModalOpen] = useState(false);
  const dialogRef = useRef<HTMLDivElement>(null);
  useModalA11y(dialogRef, modalOpen, () => setModalOpen(false));
  const [provider, setProvider] = useState("generic");
  const [apiKey, setApiKey] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // 显示决策：未完成 + 未永久关闭 + 未在冷却期
  useEffect(() => {
    if (!jwt) return;
    if (localStorage.getItem(LS_DISMISS_FOREVER) === "1") return;
    const until = parseInt(localStorage.getItem(LS_DISMISS_UNTIL) || "0", 10);
    if (until > Date.now()) return;

    let cancelled = false;
    fetchOnboardingStatus()
      .then((s) => {
        if (!cancelled && s?.needs_onboarding) setShow(true);
      })
      .catch(() => {
        // Status endpoint may 401 for unauthenticated users; that's not
        // an error to surface — just leave the banner hidden.
      });
    return () => {
      cancelled = true;
    };
  }, [jwt]);

  const dismissLater = () => {
    localStorage.setItem(LS_DISMISS_UNTIL, String(Date.now() + 7 * 24 * 60 * 60 * 1000));
    setShow(false);
  };

  const dismissForever = () => {
    localStorage.setItem(LS_DISMISS_FOREVER, "1");
    setShow(false);
  };

  const submit = async () => {
    setSaving(true);
    setError(null);
    try {
      // 1. 保存 LLM 配置
      await postOnboardingLlmConfig({
        provider,
        api_key: apiKey.trim(),
        display_name: displayName.trim() || null,
      });
      // 2. 标记完成（onboarded_at = now）— server endpoint is idempotent so
      //    if (1) succeeded but (2) fails the next retry of (2) alone is safe.
      await postOnboardingComplete();

      setModalOpen(false);
      setShow(false);
    } catch (e) {
      // Show a generic error to the user; log the detail to the console for
      // debugging without leaking server internals into the UI.
      console.error("onboarding submit failed:", e);
      setError(t.error);
    } finally {
      setSaving(false);
    }
  };

  if (!show && !modalOpen) return null;

  return (
    <>
      {show && !modalOpen && (
        <div className="flex items-center justify-between gap-3 px-4 py-2 border-b border-border bg-bg-2 text-[12px]">
          <span className="text-fg-2 truncate">
            <span className="text-accent mr-2">◐</span>
            {t.banner}
          </span>
          <div className="flex items-center gap-2 shrink-0">
            <button
              onClick={() => setModalOpen(true)}
              className="px-3 py-1 rounded-md bg-accent text-accent-fg hover:opacity-90"
            >
              {t.setup}
            </button>
            <button onClick={dismissLater} className="px-2 py-1 text-fg-3 hover:text-fg">
              {t.later}
            </button>
            <button
              onClick={dismissForever}
              className="px-2 py-1 text-fg-4 hover:text-fg-2"
              title={t.never}
              aria-label={t.never}
            >
              ✕
            </button>
          </div>
        </div>
      )}

      {modalOpen && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm"
          onClick={(e) => e.target === e.currentTarget && !saving && setModalOpen(false)}
        >
          <div
            ref={dialogRef}
            role="dialog"
            aria-modal="true"
            aria-label={t.title}
            tabIndex={-1}
            className="w-full max-w-md rounded-lg border border-border bg-bg-2 p-6 space-y-4 outline-none"
          >
            <header className="space-y-1">
              <h2 className="text-base font-semibold">{t.title}</h2>
              <p className="text-[12px] text-fg-3">{t.subtitle}</p>
            </header>

            <div className="space-y-3">
              <label className="block space-y-1">
                <span className="text-[11px] text-fg-3">{t.llmProvider}</span>
                <select
                  value={provider}
                  onChange={(e) => setProvider(e.target.value)}
                  disabled={saving}
                  className="w-full px-2 py-1.5 rounded border border-border bg-bg-2 text-[12px]"
                >
                  <option value="generic">generic (MiniMax / OpenAI-compatible)</option>
                  <option value="anthropic">anthropic</option>
                  <option value="openai">openai</option>
                  <option value="minimax">minimax</option>
                </select>
              </label>

              <label className="block space-y-1">
                <span className="text-[11px] text-fg-3">{t.llmKey} <span className="text-accent">*</span></span>
                <input
                  type="password"
                  value={apiKey}
                  onChange={(e) => setApiKey(e.target.value)}
                  placeholder={t.llmKeyPlaceholder}
                  disabled={saving}
                  className="w-full px-2 py-1.5 rounded border border-border bg-bg-2 text-[12px] mono"
                />
              </label>

              <label className="block space-y-1">
                <span className="text-[11px] text-fg-3">{t.displayName}</span>
                <input
                  type="text"
                  value={displayName}
                  onChange={(e) => setDisplayName(e.target.value)}
                  placeholder={t.displayNamePlaceholder}
                  disabled={saving}
                  className="w-full px-2 py-1.5 rounded border border-border bg-bg-2 text-[12px]"
                />
              </label>

              {error && (
                <p className="text-[11px] text-red-400" role="alert">
                  {error}
                </p>
              )}
            </div>

            <footer className="flex items-center justify-end gap-2 pt-2 border-t border-border">
              <button
                onClick={() => setModalOpen(false)}
                disabled={saving}
                className="px-3 py-1.5 text-[12px] text-fg-3 hover:text-fg"
              >
                {t.cancel}
              </button>
              <button
                onClick={submit}
                disabled={saving || !apiKey.trim()}
                className="px-3 py-1.5 rounded-md bg-accent text-accent-fg text-[12px] hover:opacity-90 disabled:opacity-50"
              >
                {saving ? t.saving : t.save}
              </button>
            </footer>
          </div>
        </div>
      )}
    </>
  );
}
