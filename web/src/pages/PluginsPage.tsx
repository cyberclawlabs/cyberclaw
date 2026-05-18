// PluginsPage — lists installed Platform Plugins discovered from
// ~/.cyberclaw/plugins/*/plugin.json via GET /api/v1/plugins.

import { useEffect, useState } from "react";
import { type Plugin, fetchPlugins } from "@/lib/api";
import { type Lang } from "@/lib/i18n";

function dict(lang: Lang) {
  return lang === "zh-CN"
    ? {
        title: "平台插件",
        subtitle: "已安装的插件，从 ~/.cyberclaw/plugins/*/plugin.json 发现。",
        loading: "加载中…",
        noPlugins: "未安装任何插件",
        noPluginsHint: (
          <>
            将 <code className="text-accent">plugin.json</code> + 资源文件放入{" "}
            <code className="text-accent">~/.cyberclaw/plugins/&#123;name&#125;/</code>{" "}
            即可注册 Platform Plugin。Manifest 格式见{" "}
            <code className="text-accent">docs/architecture/plugins/</code>。
          </>
        ),
        colIcon: "图标",
        colName: "名称",
        colVersion: "版本",
        colTabPath: "页面路径",
        colEnabled: "已启用",
        colDescription: "描述",
        enabledYes: "是",
        enabledNo: "否",
        sourceFmt: (n: number) => `来源：/api/v1/plugins。已安装 ${n} 个插件。`,
      }
    : {
        title: "Platform Plugins",
        subtitle: (
          <>
            Installed plugins discovered from{" "}
            <code>~/.cyberclaw/plugins/*/plugin.json</code>.
          </>
        ),
        loading: "Loading…",
        noPlugins: "No plugins installed.",
        noPluginsHint: (
          <>
            Drop a <code className="text-accent">plugin.json</code> + assets into{" "}
            <code className="text-accent">~/.cyberclaw/plugins/&#123;name&#125;/</code>{" "}
            to register a Platform Plugin. See{" "}
            <code className="text-accent">docs/architecture/plugins/</code> for the manifest schema.
          </>
        ),
        colIcon: "icon",
        colName: "name",
        colVersion: "version",
        colTabPath: "tab path",
        enabledYes: "yes",
        enabledNo: "no",
        colEnabled: "enabled",
        colDescription: "description",
        sourceFmt: (n: number) =>
          `Source: /api/v1/plugins. ${n} plugin${n !== 1 ? "s" : ""} installed.`,
      };
}

export default function PluginsPage({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [plugins, setPlugins] = useState<Plugin[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setErr(null);
    fetchPlugins().then(
      (ps) => { if (!cancelled) setPlugins(ps); },
      (e) => { if (!cancelled) setErr(`HTTP ${e?.status} ${e?.body ?? ""}`); },
    ).finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, []);

  return (
    <section className="space-y-3">
      <header className="space-y-1">
        <h2 className="text-base font-medium">{L.title}</h2>
        <p className="text-xs text-fg-3">{L.subtitle}</p>
      </header>

      {err && (
        <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>
      )}

      <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
        <table className="w-full text-xs table-fixed">
          <thead className="bg-bg-3">
            <tr className="text-left">
              <th className="px-3 py-2 font-medium text-fg-3 w-8">{L.colIcon}</th>
              <th className="px-3 py-2 font-medium text-fg-3 w-40">{L.colName}</th>
              <th className="px-3 py-2 font-medium text-fg-3 w-20">{L.colVersion}</th>
              <th className="px-3 py-2 font-medium text-fg-3 w-32">{L.colTabPath}</th>
              <th className="px-3 py-2 font-medium text-fg-3 w-16">{L.colEnabled}</th>
              <th className="px-3 py-2 font-medium text-fg-3">{L.colDescription}</th>
            </tr>
          </thead>
          <tbody>
            {loading && (
              <tr>
                <td colSpan={6} className="px-3 py-6 text-center text-fg-4">{L.loading}</td>
              </tr>
            )}
            {!loading && plugins.length === 0 && (
              <tr>
                <td colSpan={6} className="px-3 py-10 text-center text-fg-4">
                  <div className="space-y-2">
                    <div>{L.noPlugins}</div>
                    <div className="text-[11px] text-fg-4 max-w-md mx-auto leading-relaxed">
                      {L.noPluginsHint}
                    </div>
                  </div>
                </td>
              </tr>
            )}
            {plugins.map((p) => (
              <tr key={p.name} className="border-t border-border hover:bg-hover">
                <td className="px-3 py-2 text-base leading-none">{p.icon ?? "—"}</td>
                <td className="px-3 py-2 mono font-medium truncate">{p.label ?? p.name}</td>
                <td className="px-3 py-2 mono text-fg-4">{p.version}</td>
                <td className="px-3 py-2 mono text-fg-4 truncate">{p.tab_path ?? "—"}</td>
                <td className="px-3 py-2">
                  <span className={`px-1.5 py-0.5 rounded text-[10px] mono ${
                    p.enabled
                      ? "bg-emerald-500/15 text-emerald-300"
                      : "bg-white/10 text-fg-4"
                  }`}>
                    {p.enabled ? L.enabledYes : L.enabledNo}
                  </span>
                </td>
                <td className="px-3 py-2 text-fg-3 truncate">{p.description ?? "—"}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {!loading && (
        <p className="text-[10px] text-fg-4">{L.sourceFmt(plugins.length)}</p>
      )}
    </section>
  );
}
