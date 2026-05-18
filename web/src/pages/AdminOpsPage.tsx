// AdminOpsPage — 4-tab admin console.
// Tab 1: MCP Servers (existing CRUD view, read-only status here)
// Tab 2: Browser Console (Browser CDP status + targets read-only)
// Tab 3: Multimodal Workbench (architecture display, no triggering)
// Tab 4: MCP Bridge Tools (amber placeholder — endpoint is POST-only, no list API)

import { useEffect, useState } from "react";
import {
  type BrowserStatusResponse,
  type McpServer,
  fetchBrowserStatus,
  fetchMcpServers,
} from "@/lib/api";
import { type Lang } from "@/lib/i18n";
import TableSkeleton from "@/components/TableSkeleton";
import EmptyState from "@/components/EmptyState";
import { Server } from "@/components/icons";

// ─── i18n ─────────────────────────────────────────────────────────────────────

function dict(lang: Lang) {
  const zh = lang === "zh-CN";
  return {
    title: zh ? "管理操作" : "Admin Ops",

    // Tabs
    tabMcp: zh ? "MCP 服务器" : "MCP Servers",
    tabBrowser: zh ? "Browser Console" : "Browser Console",
    tabMultimodal: zh ? "多模态 Workbench" : "Multimodal Workbench",
    tabBridge: zh ? "MCP Bridge Tools" : "MCP Bridge Tools",

    // Tab 1 — MCP Servers
    totalFmt: (n: number) => zh ? `${n} 个 MCP 服务器已注册` : `${n} MCP servers registered`,
    noServers: zh ? "未注册 MCP 服务器" : "No MCP servers registered",
    noServersBody: zh ? "在 /admin → 管理操作 中注册。" : "Use /admin → Admin Ops to register one.",
    colName: zh ? "名称" : "Name",
    colConnectorId: "connector_id",
    colCapabilities: zh ? "能力数" : "Capabilities",
    mcpBanner: zh
      ? "MCP 服务器注册/注销在 /admin → 管理操作 中管理。本 Tab 仅展示只读状态。"
      : "MCP server register/unregister is managed in /admin → Admin Ops. This tab shows read-only status.",
    source: zh ? "来源" : "Source",

    // Tab 2 — Browser Console
    browserTitle: zh ? "Browser CDP 自动化（v2 占位）— 端点待暴露" : "Browser CDP Automation (v2 placeholder) — endpoint pending exposure",
    browserDesc: zh
      ? "通过 Chrome DevTools Protocol 让 Agent 直接操作浏览器。cyberclaw 的 BrowserConnector 经由 Connector → Capability 链接入，所有操作经过 PolicyEngine / DangerousCapabilityFilter 治理。"
      : "Lets Agents directly operate a browser via Chrome DevTools Protocol. cyberclaw's BrowserConnector is wired through the Connector → Capability chain; every action is governed by PolicyEngine / DangerousCapabilityFilter.",
    browserEnabled: zh ? "已启用" : "Enabled",
    browserDisabled: zh ? "未启用（设置 CYBERCLAW_BROWSER_ENABLED=1）" : "Disabled (set CYBERCLAW_BROWSER_ENABLED=1)",
    browserAttached: zh ? "已连接" : "Attached",
    browserDetached: zh ? "未连接" : "Detached",
    browserWsUrl: zh ? "WebSocket URL" : "WebSocket URL",
    browserDebugUrl: zh ? "Debug URL" : "Debug URL",
    browserTargets: zh ? "CDP Targets（只读）" : "CDP Targets (read-only)",
    browserNoTargets: zh ? "暂无 CDP 目标" : "No CDP targets",
    browserNoTargetsBody: zh ? "启用 Browser CDP 后此处将列出页面 targets。" : "Page targets will appear here once Browser CDP is attached.",
    colTargetId: "ID",
    colTargetUrl: "URL",
    colTargetTitle: zh ? "标题" : "Title",
    colTargetType: zh ? "类型" : "Type",
    browserArch: zh
      ? "架构路径：BrowserConnector → browser.navigate / browser.click / browser.fill / browser.evaluate / browser.screenshot → PolicyEngine → DangerousCapabilityFilter → 浏览器 CDP"
      : "Architecture path: BrowserConnector → browser.navigate / browser.click / browser.fill / browser.evaluate / browser.screenshot → PolicyEngine → DangerousCapabilityFilter → Browser CDP",

    // Tab 3 — Multimodal Workbench
    mmTitle: zh ? "多模态能力（架构展示）" : "Multimodal Capabilities (Architecture Display)",
    mmDesc: zh
      ? "通过 Connector → Capability 链调用多模态能力，不绕过 Governance Gate。所有调用经由 OrchestratorGateway 的 build_governing_gateway，PolicyEngine 在每次分发前介入。"
      : "Multimodal capabilities are invoked through the Connector → Capability chain, never bypassing the Governance Gate. All calls flow through OrchestratorGateway's build_governing_gateway; PolicyEngine intervenes before every dispatch.",
    mmNotice: zh
      ? "本 Tab 不实际触发生成（避免消耗 token + 治理顾虑）。"
      : "This tab does not trigger actual generation (to avoid token consumption and governance concerns).",
    mmEndpoints: zh ? "后端端点" : "Backend Endpoints",
    mmArch: zh
      ? "底层连接器：local connector（LOCAL_CONNECTOR_ID）。工作区沙盒：/tmp/admin-multimodal（0700 权限）。图像 fetch 限 20 MB + https 外网 only；音频 base64 inline upload only（防止路径穿越）。"
      : "Underlying connector: local connector (LOCAL_CONNECTOR_ID). Workspace sandbox: /tmp/admin-multimodal (0700 perms). Image fetch capped at 20 MB + https external only; audio accepts base64 inline upload only (path-traversal prevention).",

    // Tab 4 — MCP Bridge Tools
    bridgeTitle: zh ? "MCP Bridge Tools" : "MCP Bridge Tools",
    bridgeAmber: zh
      ? "GET 列表端点未就绪 — /api/v1/mcp/bridge_tool 当前为 POST（按需桥接单个工具），无 list API 暴露。"
      : "GET list endpoint not ready — /api/v1/mcp/bridge_tool is currently POST-only (bridge individual tools on demand); no list API is exposed.",
    bridgeDesc: zh
      ? "McpToolBridge 将 MCP 服务器工具桥接为 BridgedTool 投影（带命名空间 + 风险分类）。桥接结果存储在 AppState，不持久化。风险分类基于工具名 pattern 规则（Critical/High/Medium/Low）。"
      : "McpToolBridge bridges MCP server tools into BridgedTool projections (namespaced + risk-classified). Bridge results are stored in AppState, not persisted. Risk classification uses tool-name pattern rules (Critical/High/Medium/Low).",
    bridgeArch: zh
      ? "架构路径：MCP Server → McpToolBridge.bridge_tool() → BridgedTool（namespace:tool_name, risk_level）→ AppState → ToolDescriptionBridge → LLM 可见面"
      : "Architecture path: MCP Server → McpToolBridge.bridge_tool() → BridgedTool (namespace:tool_name, risk_level) → AppState → ToolDescriptionBridge → LLM visibility surface",
  };
}

// ─── Tab button ───────────────────────────────────────────────────────────────

type TabId = "mcp" | "browser" | "multimodal" | "bridge";

function TabBtn({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      className={[
        "px-3 py-1.5 text-[12px] border-b-2 -mb-px transition-colors whitespace-nowrap",
        active
          ? "border-accent text-fg font-medium"
          : "border-transparent text-fg-3 hover:text-fg",
      ].join(" ")}
    >
      {children}
    </button>
  );
}

// ─── Architecture hint block ──────────────────────────────────────────────────

function ArchHint({ children }: { children: React.ReactNode }) {
  return (
    <div className="rounded-lg border border-blue-500/20 bg-blue-500/5 px-3 py-2.5 text-[11px] text-blue-300/80 font-mono leading-relaxed">
      {children}
    </div>
  );
}

// ─── Tab 1: MCP Servers ───────────────────────────────────────────────────────

function McpServersTab({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [servers, setServers] = useState<McpServer[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setErr(null);
    fetchMcpServers().then(
      (s) => !cancelled && setServers(s),
      (e) => !cancelled && setErr(`HTTP ${e?.status} ${e?.body ?? ""}`),
    ).finally(() => !cancelled && setLoading(false));
    return () => { cancelled = true; };
  }, []);

  return (
    <div className="space-y-3">
      <p className="text-xs text-fg-3">{L.totalFmt(servers.length)}</p>

      {err && (
        <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>
      )}

      <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
        {loading && <div className="p-3"><TableSkeleton cols={3} rows={3} /></div>}

        {!loading && servers.length === 0 && (
          <EmptyState icon={Server} title={L.noServers} body={L.noServersBody} />
        )}

        {!loading && servers.length > 0 && (
          <table className="w-full text-xs">
            <thead className="bg-bg-3 border-b border-border">
              <tr className="text-left">
                <th className="px-3 py-2 font-medium text-fg-3">{L.colName}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colConnectorId}</th>
                <th className="px-3 py-2 font-medium text-fg-3 text-right">{L.colCapabilities}</th>
              </tr>
            </thead>
            <tbody>
              {servers.map((s) => (
                <tr key={s.connector_id} className="border-t border-border hover:bg-hover">
                  <td className="px-3 py-2 mono font-medium">{s.name}</td>
                  <td className="px-3 py-2 mono text-fg-4">{s.connector_id}</td>
                  <td className="px-3 py-2 mono text-right text-fg-3">{s.capabilities ?? "—"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      <div className="rounded-lg border border-amber-500/30 bg-amber-500/5 px-3 py-2.5 text-xs text-amber-300">
        {L.mcpBanner}
      </div>
      <p className="text-[10px] text-fg-4">
        {L.source}: <code>/api/v1/admin/mcp/servers</code>
      </p>
    </div>
  );
}

// ─── Tab 2: Browser Console ───────────────────────────────────────────────────

function BrowserConsoleTab({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [status, setStatus] = useState<BrowserStatusResponse | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setErr(null);
    fetchBrowserStatus().then(
      (s) => !cancelled && setStatus(s),
      (e) => !cancelled && setErr(`HTTP ${e?.status} ${e?.body ?? ""}`),
    ).finally(() => !cancelled && setLoading(false));
    return () => { cancelled = true; };
  }, []);

  return (
    <div className="space-y-3">
      <div className="rounded-lg border border-border bg-bg-2 px-3 py-2.5 space-y-1">
        <p className="text-xs font-medium text-fg">{L.browserTitle}</p>
        <p className="text-[11px] text-fg-3 leading-relaxed">{L.browserDesc}</p>
      </div>

      {err && (
        <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>
      )}

      {loading && <div className="p-3"><TableSkeleton cols={2} rows={2} /></div>}

      {!loading && status && (
        <div className="space-y-3">
          {/* Status badges */}
          <div className="flex flex-wrap gap-2">
            <span className={[
              "text-[11px] px-2 py-0.5 rounded-full font-medium",
              status.enabled
                ? "bg-green-500/15 text-green-400"
                : "bg-fg-4/15 text-fg-4",
            ].join(" ")}>
              {status.enabled ? L.browserEnabled : L.browserDisabled}
            </span>
            <span className={[
              "text-[11px] px-2 py-0.5 rounded-full font-medium",
              status.attached
                ? "bg-green-500/15 text-green-400"
                : "bg-amber-500/15 text-amber-400",
            ].join(" ")}>
              {status.attached ? L.browserAttached : L.browserDetached}
            </span>
          </div>

          {/* URL info */}
          {(status.debug_url || status.ws_url) && (
            <div className="rounded-lg border border-border bg-bg-2 overflow-hidden text-xs">
              {status.debug_url && (
                <div className="flex items-center gap-2 px-3 py-2 border-b border-border">
                  <span className="text-fg-4 w-24 shrink-0">{L.browserDebugUrl}</span>
                  <code className="text-fg-3 mono">{status.debug_url}</code>
                </div>
              )}
              {status.ws_url && (
                <div className="flex items-center gap-2 px-3 py-2">
                  <span className="text-fg-4 w-24 shrink-0">{L.browserWsUrl}</span>
                  <code className="text-fg-3 mono">{status.ws_url || "—"}</code>
                </div>
              )}
            </div>
          )}

          {/* Targets table */}
          <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
            <div className="px-3 py-2 bg-bg-3 border-b border-border">
              <span className="text-xs font-medium text-fg-3">{L.browserTargets}</span>
            </div>
            {status.targets.length === 0 ? (
              <EmptyState
                icon={Server}
                title={L.browserNoTargets}
                body={L.browserNoTargetsBody}
              />
            ) : (
              <table className="w-full text-xs">
                <thead className="bg-bg-3 border-t border-border">
                  <tr className="text-left">
                    <th className="px-3 py-2 font-medium text-fg-3">{L.colTargetTitle}</th>
                    <th className="px-3 py-2 font-medium text-fg-3">{L.colTargetUrl}</th>
                    <th className="px-3 py-2 font-medium text-fg-3">{L.colTargetType}</th>
                  </tr>
                </thead>
                <tbody>
                  {status.targets.map((t) => (
                    <tr key={t.id} className="border-t border-border hover:bg-hover">
                      <td className="px-3 py-2 text-fg max-w-[200px] truncate">{t.title || "—"}</td>
                      <td className="px-3 py-2 mono text-fg-4 max-w-[260px] truncate">{t.url}</td>
                      <td className="px-3 py-2 text-fg-3">{t.type}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>
        </div>
      )}

      <ArchHint>{L.browserArch}</ArchHint>
      <p className="text-[10px] text-fg-4">
        {L.source}: <code>/api/v1/admin/browser/status</code>
      </p>
    </div>
  );
}

// ─── Tab 3: Multimodal Workbench ──────────────────────────────────────────────

const MM_ENDPOINTS = [
  {
    method: "POST",
    path: "/api/v1/admin/multimodal/vision/analyze",
    cap: "local::vision.analyze_image",
    desc: { zh: "图像分析（VQA）— 接受 base64 或 https URL", en: "Image analysis (VQA) — accepts base64 or https URL" },
  },
  {
    method: "POST",
    path: "/api/v1/admin/multimodal/image/generate",
    cap: "local::image.generate",
    desc: { zh: "图像生成 — 返回 base64 + 沙盒路径", en: "Image generation — returns base64 + sandbox path" },
  },
  {
    method: "POST",
    path: "/api/v1/admin/multimodal/audio/transcribe",
    cap: "local::audio.transcribe",
    desc: { zh: "语音转文字（Whisper）— 仅接受 audio_b64 inline", en: "Speech-to-text (Whisper) — accepts audio_b64 inline only" },
  },
  {
    method: "POST",
    path: "/api/v1/admin/multimodal/audio/synthesize",
    cap: "local::audio.synthesize",
    desc: { zh: "文字转语音 — 返回 base64 音频", en: "Text-to-speech — returns base64 audio" },
  },
];

function MultimodalWorkbenchTab({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const zh = lang === "zh-CN";

  return (
    <div className="space-y-3">
      <div className="rounded-lg border border-border bg-bg-2 px-3 py-2.5 space-y-1">
        <p className="text-xs font-medium text-fg">{L.mmTitle}</p>
        <p className="text-[11px] text-fg-3 leading-relaxed">{L.mmDesc}</p>
      </div>

      <div className="rounded-lg border border-amber-500/30 bg-amber-500/5 px-3 py-2 text-[11px] text-amber-300">
        {L.mmNotice}
      </div>

      <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
        <div className="px-3 py-2 bg-bg-3 border-b border-border">
          <span className="text-xs font-medium text-fg-3">{L.mmEndpoints}</span>
        </div>
        <table className="w-full text-xs">
          <thead className="bg-bg-3 border-t border-border">
            <tr className="text-left">
              <th className="px-3 py-2 font-medium text-fg-3 w-14">{zh ? "方法" : "Method"}</th>
              <th className="px-3 py-2 font-medium text-fg-3">{zh ? "路径" : "Path"}</th>
              <th className="px-3 py-2 font-medium text-fg-3 hidden md:table-cell">Capability</th>
              <th className="px-3 py-2 font-medium text-fg-3">{zh ? "说明" : "Description"}</th>
            </tr>
          </thead>
          <tbody>
            {MM_ENDPOINTS.map((ep) => (
              <tr key={ep.path} className="border-t border-border hover:bg-hover">
                <td className="px-3 py-2">
                  <span className="text-[10px] font-medium bg-blue-500/15 text-blue-400 px-1.5 py-0.5 rounded mono">
                    {ep.method}
                  </span>
                </td>
                <td className="px-3 py-2 mono text-fg-4 text-[10px]">{ep.path}</td>
                <td className="px-3 py-2 mono text-fg-4 text-[10px] hidden md:table-cell">{ep.cap}</td>
                <td className="px-3 py-2 text-fg-3 text-[11px]">{zh ? ep.desc.zh : ep.desc.en}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      <ArchHint>{L.mmArch}</ArchHint>
    </div>
  );
}

// ─── Tab 4: MCP Bridge Tools ──────────────────────────────────────────────────

function McpBridgeToolsTab({ lang }: { lang: Lang }) {
  const L = dict(lang);

  return (
    <div className="space-y-3">
      <div className="rounded-lg border border-border bg-bg-2 px-3 py-2.5 space-y-1">
        <p className="text-xs font-medium text-fg">{L.bridgeTitle}</p>
        <p className="text-[11px] text-fg-3 leading-relaxed">{L.bridgeDesc}</p>
      </div>

      <div className="rounded-lg border border-amber-500/30 bg-amber-500/5 px-3 py-2.5 text-xs text-amber-300">
        {L.bridgeAmber}
      </div>

      <ArchHint>{L.bridgeArch}</ArchHint>
      <p className="text-[10px] text-fg-4">
        {L.source}: <code>POST /api/v1/mcp/bridge_tool</code>
      </p>
    </div>
  );
}

// ─── Main Page ────────────────────────────────────────────────────────────────

export default function AdminOpsPage({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [tab, setTab] = useState<TabId>("mcp");

  return (
    <section className="space-y-4">
      <header>
        <h2 className="text-base font-medium text-fg">{L.title}</h2>
      </header>

      {/* Tab bar */}
      <div className="flex gap-0 border-b border-border overflow-x-auto">
        <TabBtn active={tab === "mcp"} onClick={() => setTab("mcp")}>{L.tabMcp}</TabBtn>
        <TabBtn active={tab === "browser"} onClick={() => setTab("browser")}>{L.tabBrowser}</TabBtn>
        <TabBtn active={tab === "multimodal"} onClick={() => setTab("multimodal")}>{L.tabMultimodal}</TabBtn>
        <TabBtn active={tab === "bridge"} onClick={() => setTab("bridge")}>{L.tabBridge}</TabBtn>
      </div>

      {/* Tab panels */}
      {tab === "mcp" && <McpServersTab lang={lang} />}
      {tab === "browser" && <BrowserConsoleTab lang={lang} />}
      {tab === "multimodal" && <MultimodalWorkbenchTab lang={lang} />}
      {tab === "bridge" && <McpBridgeToolsTab lang={lang} />}
    </section>
  );
}
