// PluginRoute — generic page mounted at a plugin's tab_path.
//
// Shows the plugin's name + description + invoke button. Each plugin gets
// its own route by virtue of being in `mountable`, but the visible component
// is shared (no per-plugin React code is bundled at build time). To add
// real interactivity, future plugins ship a `widget` field in their
// manifest pointing at a server-side renderer.

import { useState } from "react";
import { type MountablePlugin } from "./usePlugins";
import { invokePlugin } from "@/lib/api";
import { type Lang } from "@/lib/i18n";

interface Props {
  plugin: MountablePlugin;
  lang: Lang;
}

function dict(lang: Lang) {
  return lang === "zh-CN"
    ? {
        version: "版本",
        invoke: "执行插件",
        invoking: "执行中…",
        inputLabel: "输入（JSON，留空发送 {})",
        result: "结果",
        error: "错误",
        notImplemented: "该插件的 .dylib 还未编译加载，调用会返回 404 (NotFound)。",
      }
    : {
        version: "Version",
        invoke: "Invoke plugin",
        invoking: "Invoking…",
        inputLabel: "Input (JSON, blank sends {})",
        result: "Result",
        error: "Error",
        notImplemented:
          "Plugin .dylib not compiled/loaded yet. Invoke will return 404 (NotFound) until a binary is registered.",
      };
}

export default function PluginRoute({ plugin, lang }: Props) {
  const t = dict(lang);
  const [input, setInput] = useState("");
  const [output, setOutput] = useState<unknown>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async () => {
    setBusy(true);
    setError(null);
    setOutput(null);
    try {
      const payload = input.trim() ? JSON.parse(input) : {};
      const res = await invokePlugin(plugin.name, payload);
      setOutput(res);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="space-y-4 max-w-3xl">
      <header className="space-y-1">
        <div className="flex items-center gap-2">
          {plugin.icon && <span className="text-lg">{plugin.icon}</span>}
          <h1 className="text-lg font-semibold">{plugin.label}</h1>
          <span className="text-[11px] mono text-fg-4">
            {t.version} {plugin.version}
          </span>
        </div>
        {plugin.description && (
          <p className="text-[12px] text-fg-3 leading-relaxed">{plugin.description}</p>
        )}
        <p className="text-[11px] text-fg-4">{t.notImplemented}</p>
      </header>

      <div className="rounded-lg border border-border bg-bg-2 p-4 space-y-3">
        <label className="block space-y-1">
          <span className="text-[11px] text-fg-3">{t.inputLabel}</span>
          <textarea
            value={input}
            onChange={(e) => setInput(e.target.value)}
            disabled={busy}
            rows={6}
            placeholder='{}'
            className="w-full px-2 py-1.5 rounded border border-border bg-bg-1 text-[12px] mono"
          />
        </label>
        <button
          onClick={submit}
          disabled={busy}
          className="px-3 py-1.5 rounded-md bg-accent text-accent-fg text-[12px] hover:opacity-90 disabled:opacity-50"
        >
          {busy ? t.invoking : t.invoke}
        </button>
      </div>

      {output !== null && (
        <div className="rounded-lg border border-border bg-bg-2 p-4 space-y-2">
          <h2 className="text-[12px] font-semibold">{t.result}</h2>
          <pre className="text-[11px] mono text-fg-1 whitespace-pre-wrap overflow-auto max-h-96">
            {JSON.stringify(output, null, 2)}
          </pre>
        </div>
      )}

      {error && (
        <div className="rounded-lg border border-red-500/50 bg-red-500/10 p-3" role="alert">
          <h2 className="text-[12px] font-semibold text-red-400 mb-1">{t.error}</h2>
          <p className="text-[11px] mono text-fg-1 whitespace-pre-wrap">{error}</p>
        </div>
      )}
    </section>
  );
}
