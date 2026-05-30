// ChatPage — v2 stack 的 chat 页面。
// 功能：conv 列表 / 选择 / 创建 / 删除 / SSE 流式响应 / markdown 渲染 /
//       think 折叠 / compress 按钮 / token 用量环 / slash commands / copy-code-btn。

import { useEffect, useRef, useState } from "react";
import { marked } from "marked";
import DOMPurify from "dompurify";
import {
  type ChatMessage,
  type Conversation,
  type CompressResult,
  appendMessage,
  compressConversation,
  createConversation,
  deleteConversation,
  fetchConversation,
  fetchConversations,
  streamChatCompletion,
  uploadFile,
  fetchModelsCatalog,
  type ModelEntry,
} from "@/lib/api";
import { type Lang } from "@/lib/i18n";
import PtyTerminal from "@/components/PtyTerminal";
import { useToast } from "@/components/ToastBar";
import { Paperclip } from "@/components/icons";
import Modal from "@/components/Modal";

function dict(lang: Lang) {
  return lang === "zh-CN"
    ? {
        tabChat: "对话",
        tabTerminal: "终端",
        newConversation: "+ 新建对话",
        noConversationsYet: "暂无对话",
        pickOrStart: "选择或开始一个对话",
        compress: "⇩ 压缩",
        compressing: "…",
        compressTitle: "压缩对话上下文",
        deleteConvTitle: "删除对话",
        deleteConvConfirmFmt: (title: string) => `删除对话"${title}"？`,
        confirmDeleteBtn: "删除",
        confirmCancelBtn: "取消",
        createFailed: (status: number | undefined) => `创建失败：HTTP ${status}`,
        deleteFailed: (status: number | undefined) => `删除失败：HTTP ${status}`,
        compressFailed: (status: number | undefined) => `压缩失败：HTTP ${status}`,
        compressedFmt: (orig: number, comp: number) => `已压缩 ${orig} → ${comp} 条消息`,
        unknownCmd: (cmd: string) => `未知命令：${cmd}。请输入 /help`,
        thinkingLabel: "思考中…",
        placeholderDisabled: "请先选择或新建对话",
        placeholderActive: "有问题尽管问…（↵ 发送 · ⇧↵ 换行 · /help 查看命令 · 点 📎 或拖入上传文件）",
        attachFile: "上传文件 (≤25MB)",
        dropToUpload: "松开上传",
        uploadedFmt: (name: string) => `已上传 ${name}，路径已插入消息`,
        uploadFailedFmt: (msg: string) => `上传失败：${msg}`,
        ctxTooltipFmt: (pct: number) => `~${pct}% 的上下文窗口已使用`,
        send: "发送",
        stop: "停止",
        emptyTitle: "CyberClaw Chat",
        emptyHint: (
          <>
            输入 <span className="mono text-accent">/help</span> 查看命令，或新建对话开始聊天。
          </>
        ),
        startChatting: "开始聊天",
        msgsCount: (n: number) => `${n} 条消息`,
      }
    : {
        tabChat: "Chat",
        tabTerminal: "Terminal",
        newConversation: "+ New conversation",
        noConversationsYet: "No conversations yet",
        pickOrStart: "Pick or start a conversation",
        compress: "⇩ compress",
        compressing: "…",
        compressTitle: "Compress conversation context",
        deleteConvTitle: "Delete conversation",
        deleteConvConfirmFmt: (title: string) => `Delete conversation "${title}"?`,
        confirmDeleteBtn: "Delete",
        confirmCancelBtn: "Cancel",
        createFailed: (status: number | undefined) => `Create failed: HTTP ${status}`,
        deleteFailed: (status: number | undefined) => `Delete failed: HTTP ${status}`,
        compressFailed: (status: number | undefined) => `Compress failed: HTTP ${status}`,
        compressedFmt: (orig: number, comp: number) => `Compressed ${orig} → ${comp} messages`,
        unknownCmd: (cmd: string) => `Unknown command: ${cmd}. Try /help`,
        thinkingLabel: "thinking…",
        placeholderDisabled: "Pick or start a conversation first",
        placeholderActive: "Ask anything…  (↵ send · ⇧↵ newline · /help · 📎 or drop file)",
        attachFile: "Upload file (≤25MB)",
        dropToUpload: "Drop to upload",
        uploadedFmt: (name: string) => `Uploaded ${name}, path inserted into message`,
        uploadFailedFmt: (msg: string) => `Upload failed: ${msg}`,
        ctxTooltipFmt: (pct: number) => `~${pct}% of context window used`,
        send: "Send",
        stop: "Stop",
        emptyTitle: "CyberClaw Chat",
        emptyHint: (
          <>
            Type <span className="mono text-accent">/help</span> for slash commands, or start a new conversation.
          </>
        ),
        startChatting: "Start chatting",
        msgsCount: (n: number) => `${n} msgs`,
      };
}

// Configure marked once at module level
marked.setOptions({ breaks: true, gfm: true });

// Vendor-specific tool-call markup detectors. Each entry: a pattern that
// signals "model intended to call a tool" without OpenAI tool_calls format
// (which we already dispatch). Order doesn't matter; first hit wins for cut.
const TOOL_INTENT_MARKERS: { name: string; marker: string }[] = [
  { name: "deepseek-dsml", marker: "<｜｜DSML｜｜" },
  { name: "openai-function-text", marker: "<function_call>" },
  { name: "anthropic-xml", marker: "<invoke>" },
  { name: "react-action", marker: "\nAction:" },
];

function renderMarkdown(text: string): string {
  // Cut at first vendor-specific tool-intent marker (model emitted intent but
  // platform didn't execute → strip the markup + flag hallucination risk).
  let cutAt = text.length;
  let triggered: string | null = null;
  for (const { name, marker } of TOOL_INTENT_MARKERS) {
    const i = text.indexOf(marker);
    if (i >= 0 && i < cutAt) {
      cutAt = i;
      triggered = name;
    }
  }
  let stripped = text.slice(0, cutAt);
  if (triggered) {
    stripped +=
      "\n\n> ⚠ 模型尝试调用工具（" +
      triggered +
      "），但平台尚未执行此格式的 dispatch；以上回复中涉及「今天/最新/具体数字」的事实部分可能为模型估计，请勿直接采信（v1.2 backlog #29）。";
  }
  const raw = marked.parse(stripped) as string;
  return DOMPurify.sanitize(raw);
}

// Models 现从 /api/v1/admin/llm/models 动态加载（catalog 单一来源 = ~/.cyberclaw/models.json）。
// Header 子组件自己 fetch，避免 ChatPage 顶层注入；初次加载前 dropdown 为空。

// Token context window estimate (chars/4 ≈ tokens, 200k ctx assumed)
const CTX_TOKENS = 200_000;

function computeTokenPct(input: string, messages: ChatMessage[] = []): number {
  const recent = messages.slice(-8);
  const chars = recent.reduce((s, m) => s + m.content.length, 0) + input.length;
  const tokens = chars / 4;
  return Math.min(100, (tokens / CTX_TOKENS) * 100);
}

// ── TokenRing ────────────────────────────────────────────────────────────────
function TokenRing({ pct, tooltip }: { pct: number; tooltip: string }) {
  const r = 10, circ = 2 * Math.PI * r;
  const used = circ * (1 - pct / 100);
  const tone = pct > 80 ? "rose-400" : pct > 50 ? "amber-400" : "accent";
  return (
    <div
      title={tooltip}
      className="relative flex items-center justify-center cursor-default shrink-0"
      style={{ width: 28, height: 28 }}
    >
      <svg width="28" height="28" viewBox="0 0 28 28">
        <circle cx="14" cy="14" r={r} fill="none" stroke="var(--border)" strokeWidth="2.5" />
        <circle
          cx="14" cy="14" r={r} fill="none"
          stroke={`var(--color-${tone}, var(--accent))`}
          strokeWidth="2.5"
          strokeDasharray={circ}
          strokeDashoffset={used}
          strokeLinecap="round"
          transform="rotate(-90 14 14)"
          style={{ transition: "stroke-dashoffset 0.4s" }}
        />
      </svg>
      <span className="absolute text-[8px] mono text-fg-3 leading-none">{Math.round(pct)}</span>
    </div>
  );
}

// ── splitThink ───────────────────────────────────────────────────────────────
function splitThink(content: string): Array<{ kind: "think" | "text"; body: string }> {
  const parts: Array<{ kind: "think" | "text"; body: string }> = [];
  const re = /<think>([\s\S]*?)(?:<\/think>|$)/g;
  let last = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(content)) !== null) {
    if (m.index > last) parts.push({ kind: "text", body: content.slice(last, m.index) });
    parts.push({ kind: "think", body: m[1] ?? "" });
    last = m.index + m[0].length;
  }
  if (last < content.length) parts.push({ kind: "text", body: content.slice(last) });
  return parts;
}

// ── MessageBubble ────────────────────────────────────────────────────────────
function MessageBubble({ msg, streaming }: { msg: ChatMessage; streaming: boolean }) {
  const isUser = msg.role === "user";
  const segments = isUser ? [{ kind: "text" as const, body: msg.content }] : splitThink(msg.content);
  const visibleEmpty = segments.every((s) => s.kind === "think" || !s.body.trim());
  // P1-B fix: ref must wrap the whole bubble, not each segment. Previously
  // `ref={ref}` was assigned inside `.map()` to every non-user text segment
  // → only the LAST segment retained the ref, so copy-code-btn injection
  // only ran on the last segment's <pre> blocks. Move ref to outer bubble.
  const bubbleRef = useRef<HTMLDivElement>(null);

  // Inject copy-code-btn into every <pre> block after render
  useEffect(() => {
    if (!bubbleRef.current) return;
    bubbleRef.current.querySelectorAll("pre").forEach((pre) => {
      if (pre.dataset.hasCopy) return;
      pre.style.position = "relative";
      pre.dataset.hasCopy = "1";
      const btn = document.createElement("button");
      btn.textContent = "copy";
      btn.className = "copy-code-btn";
      btn.style.cssText =
        "position:absolute;top:6px;right:6px;font-size:10px;padding:2px 6px;" +
        "border-radius:4px;background:var(--bg-2);border:1px solid var(--border-strong);" +
        "color:var(--fg-3);cursor:pointer;opacity:0.7";
      btn.onclick = () =>
        navigator.clipboard.writeText(pre.querySelector("code")?.textContent ?? "");
      pre.appendChild(btn);
    });
  }, [msg.content]);

  return (
    <div className={`flex ${isUser ? "justify-end" : "justify-start"}`}>
      <div
        ref={bubbleRef}
        className={`max-w-[85%] rounded-lg px-3 py-2 text-[13px] leading-relaxed break-words ${
          isUser ? "bg-accent text-accent-fg" : "bg-bg-3 text-fg border border-border"
        }`}
      >
        {segments.map((seg, i) =>
          seg.kind === "think" ? (
            <details
              key={i}
              className="my-1 rounded border border-border bg-bg-2 px-2 py-1 text-[11px] text-fg-3 mono"
            >
              <summary className="cursor-pointer select-none opacity-80 hover:opacity-100">
                ⋯ thinking ({seg.body.length} chars)
              </summary>
              <div className="whitespace-pre-wrap pt-1.5 text-fg-4">{seg.body.trim()}</div>
            </details>
          ) : isUser ? (
            <span key={i} className="whitespace-pre-wrap">
              {seg.body}
            </span>
          ) : (
            <div
              key={i}
              className="markdown-body"
              // eslint-disable-next-line react/no-danger
              dangerouslySetInnerHTML={{ __html: renderMarkdown(seg.body) }}
            />
          ),
        )}
        {visibleEmpty && streaming && (
          <span className="text-fg-4 italic">thinking…</span>
        )}
        {streaming && msg.role === "assistant" && !visibleEmpty && (
          <span className="inline-block w-1.5 h-3 ml-1 bg-accent animate-pulse align-middle" />
        )}
      </div>
    </div>
  );
}

// ── ChatPage (main) ──────────────────────────────────────────────────────────
export default function ChatPage({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [convs, setConvs] = useState<Conversation[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [active, setActive] = useState<Conversation | null>(null);
  const [model, setModel] = useState("");
  const [input, setInput] = useState("");
  const [sending, setSending] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [slashHint, setSlashHint] = useState<string | null>(null);
  const [compressing, setCompressing] = useState(false);
  // Bug J: 会话列表走旧 /api/v1（conv_ 格式 id），但 /v2/agent/chat/completions
  // 校验 conversation_id 要 UUID。首轮以 conv_ 为 key 不带 id 发 v2，服务端新建
  // 并在 X-Conversation-Id 回 UUID，存这里；后续轮用 UUID 续传同一服务端 session。
  const [convV2IdMap, setConvV2IdMap] = useState<Record<string, string>>({});
  const abortRef = useRef<AbortController | null>(null);
  const scrollerRef = useRef<HTMLDivElement>(null);

  // 初始加载会话列表
  useEffect(() => {
    fetchConversations().then(
      (list) => {
        setConvs(list);
        if (list.length > 0) setActiveId(list[0]!.id);
      },
      (e) => setErr(`HTTP ${e?.status} ${e?.body ?? ""}`),
    );
  }, []);

  // 切换 active conv → 拉详情 + 重置 per-conv UI 状态
  // (v1.7.2 fix: sending / err / slashHint / compressing 都是 component-level
  //  state, 之前没在 activeId 变化时 reset → 用户切换会话时上一 chat 的错误
  //  toast + 停止按钮进度条会"跨会话残留", 看起来像 UI bug.)
  useEffect(() => {
    setSending(false);
    setErr(null);
    setSlashHint(null);
    setCompressing(false);
    if (!activeId) {
      setActive(null);
      return;
    }
    let cancelled = false;
    fetchConversation(activeId).then(
      (c) => !cancelled && setActive(c),
      (e) => !cancelled && setErr(`HTTP ${e?.status} ${e?.body ?? ""}`),
    );
    return () => {
      cancelled = true;
    };
  }, [activeId]);

  // 自动滚到底
  useEffect(() => {
    if (scrollerRef.current) {
      scrollerRef.current.scrollTop = scrollerRef.current.scrollHeight;
    }
  }, [active?.messages?.length, sending]);

  const handleNew = async () => {
    try {
      const c = await createConversation("New chat");
      setConvs((prev) => [c, ...prev]);
      setActiveId(c.id);
    } catch (e: unknown) {
      const eobj = e as { status?: number; body?: string };
      setErr(L.createFailed(eobj.status));
    }
  };

  // P1-J pt2 fix: window.confirm is unstyled / unthemed and on some
  // environments (PWA, Electron, certain browser policies) returns false
  // without showing a dialog. Use the existing Modal pattern for parity
  // with the rest of the app.
  const [pendingDelete, setPendingDelete] = useState<{ id: string; title: string } | null>(null);
  const handleDelete = (id: string, title: string) => {
    setPendingDelete({ id, title });
  };
  const confirmDelete = async () => {
    if (!pendingDelete) return;
    const { id } = pendingDelete;
    setPendingDelete(null);
    try {
      await deleteConversation(id);
      setConvs((prev) => prev.filter((c) => c.id !== id));
      // Bug J: 丢弃该 conv_ 的 v2 UUID 映射，避免 id 复用时续到旧 session。
      setConvV2IdMap((m) => {
        if (!m[id]) return m;
        const next = { ...m };
        delete next[id];
        return next;
      });
      if (activeId === id) setActiveId(null);
    } catch (e: unknown) {
      const eobj = e as { status?: number; body?: string };
      setErr(L.deleteFailed(eobj.status));
    }
  };

  const handleCompress = async () => {
    if (!activeId || compressing) return;
    setCompressing(true);
    try {
      const r: CompressResult = await compressConversation(activeId);
      setSlashHint(L.compressedFmt(r.original_count, r.compressed_count));
      setTimeout(() => setSlashHint(null), 4000);
      const fresh = await fetchConversation(activeId);
      setActive(fresh);
    } catch (e: unknown) {
      const eobj = e as { status?: number };
      setErr(L.compressFailed(eobj.status));
    } finally {
      setCompressing(false);
    }
  };

  const handleSlashCommand = async (raw: string) => {
    const cmd = raw.trim().toLowerCase();
    if (cmd === "/clear" && activeId) {
      await handleDelete(activeId, active?.title ?? activeId);
      return;
    }
    if (cmd === "/new") { await handleNew(); return; }
    if (cmd === "/compress" && activeId) { await handleCompress(); return; }
    if (cmd === "/help") {
      setSlashHint("/clear · /new · /compress · /help");
      setTimeout(() => setSlashHint(null), 5000);
      return;
    }
    setSlashHint(L.unknownCmd(cmd));
    setTimeout(() => setSlashHint(null), 4000);
  };

  const handleSend = async () => {
    const text = input.trim();
    if (!text || !activeId || sending) return;
    if (text.startsWith("/")) {
      setInput("");
      await handleSlashCommand(text);
      return;
    }
    setInput("");
    setErr(null);
    setSending(true);

    // optimistic UI
    const userMsg: ChatMessage = { role: "user", content: text };
    const assistantMsg: ChatMessage = { role: "assistant", content: "" };
    // Bug J: 点「+ 新建对话」后 activeId 立即就绪（textarea enabled），但
    // fetchConversation 可能尚未返回 → active 仍为 null。旧逻辑 `cur ? … : cur`
    // 在 cur=null 时丢弃这两条消息，随后 onToken 也因 `!cur` 直接 return，
    // 导致 v2 token 永不渲染（webui 看起来"没回复"）。这里在 cur=null 时合成
    // 一个最小会话容器，保证 optimistic 气泡与流式渲染不依赖加载时序。
    setActive((cur) =>
      cur
        ? { ...cur, messages: [...(cur.messages ?? []), userMsg, assistantMsg] }
        : {
            id: activeId,
            title: convs.find((c) => c.id === activeId)?.title ?? "",
            messages: [userMsg, assistantMsg],
          },
    );

    const ac = new AbortController();
    abortRef.current = ac;
    let collected = "";

    try {
      // P1-D fix: surface persist failures instead of swallowing. Optimistic
      // UI still shows the user message immediately, but if the server fails
      // we set `err` so the operator knows history is truncated.
      void appendMessage(activeId, userMsg).catch((e: unknown) => {
        const eobj = e as { status?: number; body?: string };
        if (eobj.status === 403) {
          setErr(
            lang === "zh-CN"
              ? "此会话属于其他用户，无法写入。请点击「+ 新会话」创建一条自己的对话。"
              : "This conversation belongs to another user — click \"+ New chat\" to start your own.",
          );
        } else {
          setErr(`Warning: failed to persist user message (HTTP ${eobj.status ?? "?"})`);
        }
      });

      // Bug J: 优先用已映射的 v2 UUID。首轮还没映射时传 activeId（conv_ 或本就
      // 是 UUID）——streamChatCompletion 内部判定 conv_ 不透传给 v2，改由服务端
      // 新建并经 onConversationId 回 UUID 存入 convV2IdMap，后续轮即命中映射。
      const v2ConvId = convV2IdMap[activeId] ?? activeId;
      await streamChatCompletion({
        conversationId: v2ConvId,
        model,
        messages: [...(active?.messages ?? []), userMsg],
        signal: ac.signal,
        onConversationId: (uuid) => {
          if (!convV2IdMap[activeId]) {
            setConvV2IdMap((m) => (m[activeId] ? m : { ...m, [activeId]: uuid }));
          }
        },
        onToken: (chunk) => {
          collected += chunk;
          setActive((cur) => {
            if (!cur || !cur.messages) return cur;
            const next = [...cur.messages];
            const last = next[next.length - 1];
            if (last && last.role === "assistant") {
              next[next.length - 1] = { ...last, content: last.content + chunk };
            }
            return { ...cur, messages: next };
          });
        },
      });

      if (collected.trim()) {
        // P1-D fix: same as above — surface assistant message persist failures.
        void appendMessage(activeId, { role: "assistant", content: collected }).catch(
          (e: unknown) => {
            const eobj = e as { status?: number; body?: string };
            if (eobj.status === 403) {
              setErr(
                lang === "zh-CN"
                  ? "此会话属于其他用户，无法保存回复。请点击「+ 新会话」创建一条自己的对话。"
                  : "This conversation belongs to another user — start a new chat to save replies.",
              );
            } else {
              setErr(`Warning: failed to persist assistant message (HTTP ${eobj.status ?? "?"})`);
            }
          },
        );
      }
      setConvs((prev) =>
        prev.map((c) =>
          c.id === activeId ? { ...c, message_count: (c.message_count ?? 0) + 2 } : c,
        ),
      );
    } catch (e: unknown) {
      if (ac.signal.aborted) return;
      const eobj = e as { status?: number; body?: string };
      setErr(`Stream failed: HTTP ${eobj.status} ${eobj.body ?? ""}`);
    } finally {
      setSending(false);
      abortRef.current = null;
    }
  };

  const handleStop = () => {
    abortRef.current?.abort();
    setSending(false);
  };

  const tokenPct = computeTokenPct(input, active?.messages ?? []);
  const canCompress = (active?.messages?.length ?? 0) >= 5 && !compressing;

  const [view, setView] = useState<"chat" | "terminal">("chat");

  // P1-J pt1 fix: derive ws:// vs wss:// from the current page protocol.
  // Hardcoded ws:// triggered mixed-content blocks on HTTPS deployments and
  // sent the JWT (currently in query string — see security advisory for the
  // ticket-exchange follow-up) in cleartext on the wire when HTTPS was off.
  const wsScheme = location.protocol === "https:" ? "wss" : "ws";
  const ptyWsUrl = `${wsScheme}://${location.host}/api/v1/pty/ws?token=${sessionStorage.getItem("cyberclaw.admin.jwt") ?? ""}`;

  return (
    <div className="flex flex-col h-[calc(100vh-160px)] min-h-[420px] rounded-lg border border-border overflow-hidden bg-bg">
      {/* Tab switcher */}
      <div className="border-b border-border px-3 py-1 flex items-center gap-1 bg-bg-2 shrink-0">
        <button
          onClick={() => setView("chat")}
          className={`px-3 py-1 rounded text-[12px] font-medium transition-colors ${
            view === "chat"
              ? "bg-accent text-accent-fg"
              : "text-fg-3 hover:text-fg hover:bg-hover"
          }`}
        >
          {L.tabChat}
        </button>
        <button
          onClick={() => setView("terminal")}
          className={`px-3 py-1 rounded text-[12px] font-medium transition-colors ${
            view === "terminal"
              ? "bg-accent text-accent-fg"
              : "text-fg-3 hover:text-fg hover:bg-hover"
          }`}
        >
          {L.tabTerminal}
        </button>
      </div>

      {view === "chat" ? (
        <div className="flex flex-1 min-h-0 overflow-hidden">
          <ConvSidebar
            convs={convs}
            activeId={activeId}
            onSelect={setActiveId}
            onNew={handleNew}
            onDelete={handleDelete}
            lang={lang}
          />
          <div className="flex-1 flex flex-col min-w-0 bg-bg-2">
            <ChatHeader
              conv={active}
              model={model}
              setModel={setModel}
              onCompress={handleCompress}
              canCompress={canCompress}
              compressing={compressing}
              lang={lang}
            />
            <div ref={scrollerRef} className="flex-1 overflow-auto px-5 py-4 space-y-3">
              {!active && <EmptyState lang={lang} onNew={handleNew} />}
              {active?.messages?.map((m, i) => (
                <MessageBubble
                  key={i}
                  msg={m}
                  streaming={sending && i === (active.messages?.length ?? 0) - 1}
                />
              ))}
              {err && (
                <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>
              )}
            </div>
            <Composer
              input={input}
              setInput={setInput}
              onSend={handleSend}
              onStop={handleStop}
              sending={sending}
              disabled={!activeId}
              lang={lang}
              tokenPct={tokenPct}
              slashHint={slashHint}
            />
          </div>
        </div>
      ) : (
        <div className="flex-1 min-h-0 overflow-hidden p-1">
          <PtyTerminal wsUrl={ptyWsUrl} />
        </div>
      )}

      {/* P1-J pt2: themed delete confirm modal (replaces window.confirm) */}
      <Modal
        open={!!pendingDelete}
        onClose={() => setPendingDelete(null)}
        title={L.deleteConvTitle}
        width={420}
        footer={
          <>
            <button
              onClick={() => setPendingDelete(null)}
              className="px-3 h-8 rounded-md text-xs text-fg-3 hover:text-fg hover:bg-hover"
            >
              {L.confirmCancelBtn}
            </button>
            <button
              onClick={confirmDelete}
              className="px-3 h-8 rounded-md bg-rose-600 text-white text-xs font-medium hover:opacity-90"
            >
              {L.confirmDeleteBtn}
            </button>
          </>
        }
      >
        <p className="text-sm text-fg-2">
          {pendingDelete ? L.deleteConvConfirmFmt(pendingDelete.title) : ""}
        </p>
      </Modal>
    </div>
  );
}

// ── ConvSidebar ──────────────────────────────────────────────────────────────
function ConvSidebar({
  convs,
  activeId,
  onSelect,
  onNew,
  onDelete,
  lang,
}: {
  convs: Conversation[];
  activeId: string | null;
  onSelect: (id: string) => void;
  onNew: () => void;
  onDelete: (id: string, title: string) => void;
  lang: Lang;
}) {
  const L = dict(lang);
  return (
    <div className="w-[220px] border-r border-border bg-bg-2 flex flex-col shrink-0">
      <div className="p-3 border-b border-border">
        <button
          onClick={onNew}
          className="w-full h-8 rounded-md bg-accent text-accent-fg text-[12px] font-medium hover:opacity-90"
        >
          {L.newConversation}
        </button>
      </div>
      <div className="flex-1 overflow-auto py-1">
        {convs.length === 0 && (
          <div className="px-4 py-8 text-center text-[11px] text-fg-4">
            {L.noConversationsYet}
          </div>
        )}
        {convs.map((c) => (
          <div
            key={c.id}
            className={`group relative mx-1 rounded-md transition-colors ${
              c.id === activeId
                ? "bg-bg-3 text-fg border border-border"
                : "text-fg-2 hover:bg-hover hover:text-fg"
            }`}
          >
            <button
              onClick={() => onSelect(c.id)}
              className="w-full text-left px-3 py-2 text-[12px] pr-7"
            >
              <div className="truncate font-medium">{c.title}</div>
              <div className="text-[10px] mono text-fg-4 mt-0.5">{L.msgsCount(c.message_count ?? 0)}</div>
            </button>
            <button
              onClick={(e) => { e.stopPropagation(); onDelete(c.id, c.title); }}
              title={L.deleteConvTitle}
              className="absolute top-1.5 right-1.5 h-5 w-5 rounded inline-flex items-center justify-center text-fg-4 opacity-0 group-hover:opacity-100 hover:text-rose-400 hover:bg-rose-500/10 transition-opacity"
            >
              ×
            </button>
          </div>
        ))}
      </div>
    </div>
  );
}

// ── ChatHeader ───────────────────────────────────────────────────────────────
function ChatHeader({
  conv,
  model,
  setModel,
  onCompress,
  canCompress,
  compressing,
  lang,
}: {
  conv: Conversation | null;
  model: string;
  setModel: (m: string) => void;
  onCompress: () => void;
  canCompress: boolean;
  compressing: boolean;
  lang: Lang;
}) {
  const L = dict(lang);
  return (
    <div className="h-11 border-b border-border flex items-center px-4 gap-3 shrink-0 bg-bg-2">
      <span className="text-accent">▢</span>
      <span className="text-[13px] font-medium truncate flex-1">
        {conv?.title ?? L.pickOrStart}
      </span>
      <button
        onClick={onCompress}
        disabled={!canCompress}
        title={L.compressTitle}
        className="h-7 px-2 text-[11px] mono rounded-md bg-bg-3 border border-border text-fg-3 hover:text-fg disabled:opacity-30 shrink-0"
      >
        {compressing ? L.compressing : L.compress}
      </button>
      <ModelSelect model={model} setModel={setModel} />
    </div>
  );
}

// Model dropdown — fetches catalog once, sets default if parent model is empty.
function ModelSelect({ model, setModel }: { model: string; setModel: (m: string) => void }) {
  const [models, setModels] = useState<ModelEntry[]>([]);
  const [defaultId, setDefaultId] = useState<string>("");
  useEffect(() => {
    fetchModelsCatalog()
      .then((c) => {
        setModels(c.models);
        setDefaultId(c.current_default);
        if (!model && c.current_default) setModel(c.current_default);
      })
      .catch(() => {
        // backend 不可用时不再硬塞列表，让 setMonel 保持空白由用户输入或后端 default 解析
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  return (
    <select
      value={model}
      onChange={(e) => setModel(e.target.value)}
      className="h-7 px-2 text-[11px] mono rounded-md bg-bg-3 border border-border text-fg-2 outline-none focus-ring shrink-0"
      style={{ minWidth: 140 }}
    >
      {models.length === 0 && <option value="">—</option>}
      {models.map((m) => (
        <option key={m.id} value={m.id}>
          {(m.label ?? m.id) + (m.id === defaultId ? "  ★" : "")}
        </option>
      ))}
    </select>
  );
}

// ── EmptyState ───────────────────────────────────────────────────────────────
function EmptyState({ lang, onNew }: { lang: Lang; onNew: () => void }) {
  const L = dict(lang);
  return (
    <div className="h-full flex flex-col items-center justify-center text-center gap-3 py-10">
      <div className="h-14 w-14 rounded-2xl bg-accent-soft flex items-center justify-center text-accent text-xl">
        ▢
      </div>
      <div>
        <div className="text-[15px] font-semibold text-fg">{L.emptyTitle}</div>
        <div className="text-[12px] text-fg-3 mt-1 max-w-[320px]">
          {L.emptyHint}
        </div>
      </div>
      <button
        onClick={onNew}
        className="mt-3 px-4 py-2 rounded-md bg-accent text-accent-fg text-[12px]"
      >
        {L.startChatting}
      </button>
    </div>
  );
}

// ── Composer ─────────────────────────────────────────────────────────────────
function Composer({
  input,
  setInput,
  onSend,
  onStop,
  sending,
  disabled,
  lang,
  tokenPct,
  slashHint,
}: {
  input: string;
  // React.Dispatch so handleFiles (multi-file upload) can call the
  // functional-updater form to avoid stale-closure overwrites.
  setInput: React.Dispatch<React.SetStateAction<string>>;
  onSend: () => void;
  onStop: () => void;
  sending: boolean;
  disabled: boolean;
  lang: Lang;
  tokenPct: number;
  slashHint: string | null;
}) {
  const L = dict(lang);
  const toast = useToast();
  const fileInputRef = useRef<HTMLInputElement>(null);
  const [uploading, setUploading] = useState(false);
  const [dragOver, setDragOver] = useState(false);

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    // IME 兼容：中文输入法在 candidate 选词阶段会发 Enter；
    // 此时 isComposing=true 或 keyCode=229，必须忽略避免误发送。
    if (e.nativeEvent.isComposing || e.keyCode === 229) return;
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      onSend();
    }
  };

  // Shared upload pipeline for both paperclip-button and drag-drop. On success
  // we splice the server-canonical path into the textarea so the agent (which
  // sees the same workspace via cmd_run mount) can reference it.
  const handleFiles = async (files: FileList | File[]) => {
    const arr = Array.from(files);
    if (arr.length === 0) return;
    setUploading(true);
    try {
      // P1 fix: previously `setInput((input ? input : "") + tag)` captured
      // `input` from initial render — second+ iteration of a multi-file
      // upload overwrote the first file's tag. Accumulate all tags first,
      // then setInput once via functional updater.
      const tags: string[] = [];
      for (const f of arr) {
        const resp = await uploadFile(f);
        tags.push(`\n[file] ${resp.path}`);
        toast({ tone: "success", msg: L.uploadedFmt(resp.original_filename ?? f.name) });
      }
      const appended = tags.join("");
      setInput((prev) => (prev ?? "") + appended);
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      toast({ tone: "error", msg: L.uploadFailedFmt(msg) });
    } finally {
      setUploading(false);
      if (fileInputRef.current) fileInputRef.current.value = "";
    }
  };

  const tooltip = L.ctxTooltipFmt(Math.round(tokenPct));
  return (
    <div
      className={`border-t border-border p-3 bg-bg-2 transition-colors ${dragOver ? "bg-accent/5 outline outline-2 outline-accent/40" : ""}`}
      onDragOver={(e) => {
        if (e.dataTransfer.types.includes("Files")) {
          e.preventDefault();
          setDragOver(true);
        }
      }}
      onDragLeave={() => setDragOver(false)}
      onDrop={(e) => {
        e.preventDefault();
        setDragOver(false);
        if (e.dataTransfer.files.length > 0) {
          void handleFiles(e.dataTransfer.files);
        }
      }}
    >
      {slashHint && (
        <div className="mb-2 px-2 py-1 rounded bg-bg-3 border border-border text-[11px] mono text-fg-3">
          {slashHint}
        </div>
      )}
      {dragOver && (
        <div className="mb-2 px-2 py-1.5 rounded bg-accent/10 border border-accent/30 text-[11px] text-accent text-center">
          {L.dropToUpload}
        </div>
      )}
      <div className="flex items-end gap-2">
        <input
          ref={fileInputRef}
          type="file"
          multiple
          hidden
          onChange={(e) => {
            if (e.target.files && e.target.files.length > 0) {
              void handleFiles(e.target.files);
            }
          }}
        />
        <button
          type="button"
          onClick={() => fileInputRef.current?.click()}
          disabled={uploading || disabled}
          title={L.attachFile}
          className="h-10 w-10 flex items-center justify-center rounded-md text-fg-3 hover:text-fg hover:bg-hover disabled:opacity-40"
        >
          <Paperclip size={16} className={uploading ? "animate-pulse" : ""} />
        </button>
        <textarea
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={onKeyDown}
          disabled={disabled}
          rows={2}
          placeholder={disabled ? L.placeholderDisabled : L.placeholderActive}
          className="flex-1 px-3 py-2 rounded-md bg-bg-3 border border-border text-[13px] outline-none focus-ring resize-none disabled:opacity-50"
        />
        <TokenRing pct={tokenPct} tooltip={tooltip} />
        {sending ? (
          <button
            onClick={onStop}
            className="h-10 px-3 rounded-md bg-rose-500/15 text-rose-300 text-[12px]"
          >
            {L.stop}
          </button>
        ) : (
          <button
            onClick={onSend}
            disabled={!input.trim() || disabled}
            className="h-10 px-4 rounded-md bg-accent text-accent-fg text-[13px] hover:opacity-90 disabled:opacity-40"
          >
            {L.send}
          </button>
        )}
      </div>
    </div>
  );
}
