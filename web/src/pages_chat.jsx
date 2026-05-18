// ChatPage — conversation-driven approval interface
// Sprint 12 L2: main chat page + approval card rendering

const { useState: uS, useEffect: uE, useRef: uR, useMemo: uM, useCallback: uC } = React;

// ── localStorage helpers ────────────────────────────────────────────────────
const LS_KEY            = 'cyberclaw.admin.chat.conversations';
const LS_MIGRATED_KEY   = 'cyberclaw.admin.chat.conversations.migrated';
const LS_ACTIVE_AGENT   = 'cyberclaw.admin.chat.active_agent';
const LS_ACTIVE_MODEL   = 'cyberclaw.admin.chat.active_model';

function genId() {
  return 'conv_' + Date.now().toString(36) + '_' + Math.random().toString(36).slice(2, 7);
}

function newConversation() {
  return {
    conv_id: genId(),
    title: 'New conversation',
    created_at: Date.now(),
    messages: [],
  };
}

// Normalize a server conversation object to always have conv_id
function normalizeConv(sc) {
  if (!sc) return sc;
  return {
    ...sc,
    conv_id: sc.conv_id || sc.id,
    messages: sc.messages || [],
  };
}

// ── Approval card ────────────────────────────────────────────────────────────
const RISK_TONE = { low: 'emerald', medium: 'amber', high: 'rose', critical: 'rose' };
const RISK_BORDER = {
  low:      'border-emerald-500/30 bg-emerald-500/5',
  medium:   'border-amber-400/40 bg-amber-400/5',
  high:     'border-rose-500/40 bg-rose-500/6',
  critical: 'border-rose-600/60 bg-rose-600/8',
};

const ApprovalCard = ({ toolCall, convId, onResolved, lang }) => {
  const t = tFor(lang);
  const args = toolCall.args || {};
  const risk = (args.risk_level || 'medium').toLowerCase();
  const [state, setState] = uS('idle'); // idle | loading | approved | rejected
  const [showReasonInput, setShowReasonInput] = uS(false);
  const [reason, setReason] = uS('');

  const borderCls = RISK_BORDER[risk] || RISK_BORDER.medium;

  const submit = async (approved) => {
    setState('loading');
    const body = {
      request_id: toolCall.id || args.request_id || '',
      conversation_id: convId,
      approved,
      ...(reason.trim() ? { reason: reason.trim() } : {}),
    };
    try {
      const jwt = sessionStorage.getItem('cyberclaw.admin.jwt');
      const res = await fetch('/api/v1/chat/approval', {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          ...(jwt ? { Authorization: 'Bearer ' + jwt } : {}),
        },
        body: JSON.stringify(body),
      });
      if (!res.ok && res.status !== 404) throw new Error('HTTP ' + res.status);
      if (res.status === 404) {
        // Lane 1 not yet wired — mock success + toast
        window.cc && window.cc.toast && window.cc.toast(
          'warning',
          '/api/v1/chat/approval returned 404 — using mock response',
          { duration: 4000 }
        );
      }
      setState(approved ? 'approved' : 'rejected');
      onResolved && onResolved(
        approved
          ? (t('chat.approve_card.approve') || 'Approved')
          : (t('chat.approve_card.reject') || 'Rejected') + (reason.trim() ? ': ' + reason.trim() : '')
      );
    } catch (err) {
      setState('idle');
      window.cc && window.cc.toast && window.cc.toast('error', String(err));
    }
  };

  if (state === 'approved') {
    return (
      <div className="mt-2 rounded-lg border border-emerald-500/40 bg-emerald-500/8 px-4 py-3 flex items-center gap-2">
        <I.Check size={14} className="text-emerald-400 shrink-0" />
        <span className="text-[13px] text-emerald-400 font-medium">{t('chat.approve_card.approve')}</span>
      </div>
    );
  }
  if (state === 'rejected') {
    return (
      <div className="mt-2 rounded-lg border border-rose-500/40 bg-rose-500/6 px-4 py-3 flex items-center gap-2">
        <I.Close size={14} className="text-rose-400 shrink-0" />
        <span className="text-[13px] text-rose-400 font-medium">{t('chat.approve_card.reject')}</span>
      </div>
    );
  }

  return (
    <div className={`mt-2 rounded-lg border ${borderCls} px-4 py-3 space-y-2.5`}>
      {/* Title row */}
      <div className="flex items-start justify-between gap-3">
        <div className="flex items-center gap-2">
          <I.Shield size={14} className="text-fg-3 shrink-0 mt-0.5" />
          <span className="text-[13px] font-semibold text-fg">
            {t('chat.approve_card.title') || 'Needs approval'}{args.action ? ': ' : ''}
            {args.action && <span className="mono text-accent">{args.action}</span>}
          </span>
        </div>
        {risk && <Badge tone={RISK_TONE[risk] || 'amber'}>{risk}</Badge>}
      </div>

      {/* Rationale */}
      {args.rationale && (
        <p className="text-[12px] text-fg-2 leading-relaxed">{args.rationale}</p>
      )}

      {/* Reason input (shown when rejecting) */}
      {showReasonInput && (
        <textarea
          className="w-full bg-bg-3 border border-border rounded-md px-3 py-2 text-[12px] text-fg placeholder:text-fg-4 outline-none focus-ring resize-none"
          rows={2}
          placeholder={t('reason_placeholder') || 'Reason (optional)…'}
          value={reason}
          onChange={(e) => setReason(e.target.value)}
        />
      )}

      {/* Action buttons */}
      <div className="flex items-center gap-2 pt-0.5">
        {state === 'loading' ? (
          <span className="text-[12px] text-fg-3 mono">{t('chat.approve_card.awaiting') || 'Processing…'}</span>
        ) : (
          <>
            <button
              onClick={() => submit(true)}
              className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-md bg-emerald-500/15 border border-emerald-500/30 text-emerald-400 text-[12px] font-medium hover:bg-emerald-500/25 transition-colors"
            >
              <I.Check size={12} />
              {t('chat.approve_card.approve') || 'Approve'}
            </button>
            <button
              onClick={() => {
                if (showReasonInput) {
                  submit(false);
                } else {
                  setShowReasonInput(true);
                }
              }}
              className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-md bg-rose-500/10 border border-rose-500/25 text-rose-400 text-[12px] font-medium hover:bg-rose-500/20 transition-colors"
            >
              <I.Close size={12} />
              {showReasonInput ? (t('confirm') || 'Confirm') : (t('chat.approve_card.reject') || 'Reject')}
            </button>
            {showReasonInput && (
              <button
                onClick={() => { setShowReasonInput(false); setReason(''); }}
                className="text-[12px] text-fg-4 hover:text-fg"
              >
                {t('cancel') || 'Cancel'}
              </button>
            )}
          </>
        )}
      </div>
    </div>
  );
};

// ── Message bubble ────────────────────────────────────────────────────────────
const MessageBubble = ({ msg, convId, onApprovalResolved, lang }) => {
  const isUser = msg.role === 'user';
  const toolCalls = msg.tool_calls || [];
  const approvalCalls = toolCalls.filter(tc => tc.name === 'request_approval');

  // GFM markdown with Prism syntax highlighting (Sprint 14 S14-6)
  // Falls back to plain text if marked CDN failed to load.
  const renderMarkdown = (text) => {
    if (!text) return '';
    try {
      if (!window.marked) return text;
      return window.marked.parse(text);
    } catch {
      return text;
    }
  };

  // Inject copy button into each <pre> block produced by marked/Prism
  const postProcessMarkdown = (html) => {
    return html.replace(
      /<pre>/g,
      '<pre style="position:relative;">' +
      '<button class="copy-code-btn" onclick="' +
        "var c=this.parentElement.querySelector('code');if(c){navigator.clipboard.writeText(c.innerText).then(()=>{this.textContent='copied';setTimeout(()=>this.textContent='copy',1200)})}" +
      '">copy</button>'
    );
  };

  if (isUser) {
    return (
      <div className="flex justify-end mb-3">
        <div className="max-w-[75%] bg-accent text-white rounded-2xl rounded-tr-sm px-4 py-2.5 text-[13px] leading-relaxed shadow-sm">
          {msg.content}
        </div>
      </div>
    );
  }

  return (
    <div className="flex gap-3 mb-3 items-start">
      <div className="h-7 w-7 rounded-full bg-accent-soft flex items-center justify-center shrink-0 mt-0.5">
        <I.Bot size={13} className="text-accent" />
      </div>
      <div className="flex-1 min-w-0">
        <div className="bg-bg-2 border border-border rounded-2xl rounded-tl-sm px-4 py-2.5 text-[13px] text-fg leading-relaxed shadow-sm">
          <div
            className="markdown-body"
            dangerouslySetInnerHTML={{ __html: postProcessMarkdown(renderMarkdown(msg.content)) }}
          />
          {/* Approval cards for any request_approval tool_calls */}
          {approvalCalls.map((tc, i) => (
            <ApprovalCard
              key={tc.id || i}
              toolCall={tc}
              convId={convId}
              lang={lang}
              onResolved={(resolvedText) => onApprovalResolved && onApprovalResolved(resolvedText)}
            />
          ))}
        </div>
        {msg.ts && (
          <div className="mt-1 px-1 text-[10px] mono text-fg-4">{new Date(msg.ts).toLocaleTimeString()}</div>
        )}
      </div>
    </div>
  );
};

// ── Conversation sidebar ──────────────────────────────────────────────────────
const ConvSidebar = ({ convs, activeId, onSelect, onNew, onDelete, lang }) => {
  const t = tFor(lang);
  return (
    <div className="w-[220px] border-r border-border flex flex-col shrink-0 bg-bg-2 h-full">
      <div className="p-3 border-b border-border">
        <button
          onClick={onNew}
          className="w-full flex items-center justify-center gap-2 h-8 rounded-md bg-accent text-white text-[12px] font-medium hover:opacity-90 transition-opacity"
        >
          <I.Plus size={13} />
          {t('chat.conversation.new') || 'New conversation'}
        </button>
      </div>
      <div className="flex-1 overflow-auto py-1">
        {convs.length === 0 && (
          <div className="px-4 py-8 text-center text-[11px] text-fg-4">
            {t('empty_title') || 'Nothing here yet'}
          </div>
        )}
        {convs.map(c => (
          <div
            key={c.conv_id}
            className={`group relative mx-1 rounded-md transition-colors ${
              c.conv_id === activeId
                ? 'bg-bg-3 text-fg border border-border'
                : 'text-fg-2 hover:bg-[var(--hover)] hover:text-fg'
            }`}
            style={{ width: 'calc(100% - 8px)' }}
          >
            <button
              onClick={() => onSelect(c.conv_id)}
              className="w-full text-left px-3 py-2 text-[12px] pr-7"
            >
              <div className="truncate font-medium">{c.title}</div>
              <div className="text-[10px] mono text-fg-4 mt-0.5">
                {c.messages.length} {c.messages.length === 1 ? 'msg' : 'msgs'}
              </div>
            </button>
            {onDelete && (
              <button
                onClick={(e) => { e.stopPropagation(); onDelete(c.conv_id, c.title); }}
                title={t('chat.conversation.delete') || 'Delete conversation'}
                className="absolute top-1.5 right-1.5 h-5 w-5 rounded inline-flex items-center justify-center text-fg-4 opacity-0 group-hover:opacity-100 hover:text-rose-400 hover:bg-rose-500/10 transition-opacity"
              >
                <I.Trash size={11} />
              </button>
            )}
          </div>
        ))}
      </div>
    </div>
  );
};

// ── Model / Agent options ─────────────────────────────────────────────────────
const MODEL_OPTIONS = [
  { value: 'claude-sonnet-4-6',        label: 'Sonnet 4.6',       provider: 'anthropic' },
  { value: 'claude-opus-4-7',          label: 'Opus 4.7',         provider: 'anthropic' },
  { value: 'MiniMax-M2.7-HighSpeed',   label: 'MiniMax M2.7',     provider: 'minimax'   },
  { value: 'gpt-4o-mini',              label: 'GPT-4o mini',      provider: 'openai'    },
  { value: 'ollama-local',             label: 'Ollama (local)',    provider: 'ollama'    },
];

// ── Token ring SVG ────────────────────────────────────────────────────────────
const TokenRing = ({ pct, tooltip }) => {
  const r = 10, circ = 2 * Math.PI * r;
  const used = circ * (1 - pct / 100);
  return (
    <div title={tooltip} className="relative flex items-center justify-center cursor-default shrink-0" style={{ width: 28, height: 28 }}>
      <svg width="28" height="28" viewBox="0 0 28 28">
        <circle cx="14" cy="14" r={r} fill="none" stroke="var(--border)" strokeWidth="2.5" />
        <circle
          cx="14" cy="14" r={r} fill="none"
          stroke={pct > 80 ? 'var(--color-rose-400, #f87171)' : pct > 50 ? 'var(--color-amber-400, #fbbf24)' : 'var(--accent)'}
          strokeWidth="2.5"
          strokeDasharray={circ}
          strokeDashoffset={used}
          strokeLinecap="round"
          transform="rotate(-90 14 14)"
          style={{ transition: 'stroke-dashoffset 0.4s' }}
        />
      </svg>
      <span className="absolute text-[8px] mono text-fg-3 leading-none">{Math.round(pct)}</span>
    </div>
  );
};

// ── Workspace modal hint ───────────────────────────────────────────────────────
const WorkspaceModal = ({ cwd, onClose, t }) => (
  <div
    className="fixed inset-0 z-50 flex items-center justify-center"
    style={{ background: 'rgba(0,0,0,0.45)' }}
    onClick={onClose}
  >
    <div
      className="bg-bg-2 border border-border rounded-xl px-6 py-5 max-w-sm w-full shadow-xl"
      onClick={e => e.stopPropagation()}
    >
      <div className="flex items-center gap-2 mb-3">
        <I.Folder size={14} className="text-accent" />
        <span className="text-[13px] font-semibold text-fg">{t('composer.workspace') || 'Workspace'}</span>
      </div>
      <p className="text-[12px] mono text-fg-2 mb-3 break-all">{cwd}</p>
      <p className="text-[12px] text-fg-3 leading-relaxed">
        {t('composer.workspace_hint') || 'Workspace editing lives in /workbench (Govern mode).'}
      </p>
      <button
        onClick={onClose}
        className="mt-4 w-full h-8 rounded-md bg-bg-3 border border-border text-[12px] text-fg hover:border-[var(--border-strong)] transition-colors"
      >
        {t('close') || 'Close'}
      </button>
    </div>
  </div>
);

// ── Main ChatPage ─────────────────────────────────────────────────────────────
function ChatPage({ lang }) {
  const t = tFor(lang);
  const bottomRef = uR(null);

  // Conversations state (server-backed, S14-3b)
  const [convs, setConvs] = uS([]);
  const [activeId, setActiveId] = uS(null);
  const [convsLoaded, setConvsLoaded] = uS(false);

  // Input state
  const [input, setInput] = uS('');
  const [model, setModel] = uS(() => {
    try { return localStorage.getItem(LS_ACTIVE_MODEL) || MODEL_OPTIONS[0].value; } catch { return MODEL_OPTIONS[0].value; }
  });
  const [sending, setSending] = uS(false);
  const textareaRef = uR(null);

  // S14-4: SSE streaming state.
  //   streamingText — the accumulating delta text for the in-flight
  //                   assistant reply (rendered as a "live" bubble).
  //   streamConvId  — the conversation id the stream is bound to; used
  //                   so switching conversations mid-stream doesn't paint
  //                   partial text on the wrong thread.
  //   abortRef      — AbortController returned by subscribeChatCompletions,
  //                   invoked by the Stop button.
  const [streamingText, setStreamingText] = uS('');
  const [streamConvId, setStreamConvId] = uS(null);
  const abortRef = uR(null);

  // S15-T14: Clarify card state
  //   activeClarify   — the currently displayed clarify request (oldest pending)
  //   clarifyQueue    — additional pending clarifies waiting behind activeClarify
  //   composerLocked  — true while any clarify is pending (disables input)
  const [activeClarify, setActiveClarify] = uS(null);
  const [clarifyQueue, setClarifyQueue] = uS([]);
  const [composerLocked, setComposerLocked] = uS(false);

  // S16-T6: Compress state
  const [compressing, setCompressing] = uS(false);
  const [compressLockoutUntil, setCompressLockoutUntil] = uS(0);

  // Composer: agent list + selection
  const [agents, setAgents] = uS([]);
  const [activeAgent, setActiveAgent] = uS(() => {
    try { return localStorage.getItem(LS_ACTIVE_AGENT) || ''; } catch { return ''; }
  });

  // Composer: workspace CWD
  const [cwd, setCwd] = uS('~');
  const [showWorkspaceModal, setShowWorkspaceModal] = uS(false);

  // Persist model selection
  uE(() => { try { localStorage.setItem(LS_ACTIVE_MODEL, model); } catch {} }, [model]);

  // Persist agent selection
  uE(() => { try { if (activeAgent) localStorage.setItem(LS_ACTIVE_AGENT, activeAgent); } catch {} }, [activeAgent]);

  // Fetch agents list
  uE(() => {
    if (window.cc && window.cc.api && window.cc.api.agents && window.cc.api.agents.list) {
      window.cc.api.agents.list().then(list => {
        if (!Array.isArray(list)) return;
        setAgents(list);
        // Default: use stored selection if valid, else first in list
        const stored = (() => { try { return localStorage.getItem(LS_ACTIVE_AGENT) || ''; } catch { return ''; } })();
        const valid = list.find(a => a.id === stored || a.agent_id === stored);
        if (!valid && list.length > 0) {
          const def = list.find(a => a.is_default) || list[0];
          setActiveAgent(def.id || def.agent_id || '');
        }
      }).catch(() => {});
    }
  }, []);

  // Fetch workspace CWD
  uE(() => {
    if (window.cc && window.cc.api && window.cc.api.settings && window.cc.api.settings.config) {
      window.cc.api.settings.config().then(cfg => {
        if (cfg && cfg.cwd) setCwd(cfg.cwd);
        else if (cfg && cfg.workspace) setCwd(cfg.workspace);
      }).catch(() => {});
    }
  }, []);

  // Active conversation
  const activeConv = uM(() => convs.find(c => c.conv_id === activeId) || null, [convs, activeId]);

  // Load conversations from server on mount + one-shot migration (S14-3b)
  uE(() => {
    const chatApi = window.cc && window.cc.api && window.cc.api.chat && window.cc.api.chat.conversations;
    if (!chatApi) { setConvsLoaded(true); return; }

    chatApi.list().then(async (serverList) => {
      const list = Array.isArray(serverList) ? serverList : (serverList && Array.isArray(serverList.conversations) ? serverList.conversations : []);

      // One-shot migration: if local has data and server is empty and not already migrated
      const alreadyMigrated = (() => { try { return localStorage.getItem(LS_MIGRATED_KEY) === 'true'; } catch { return false; } })();
      const localRaw = (() => { try { const r = localStorage.getItem(LS_KEY); return r ? JSON.parse(r) : []; } catch { return []; } })();

      if (!alreadyMigrated && list.length === 0 && localRaw.length > 0) {
        let migrated = 0;
        const created = [];
        for (const lc of localRaw) {
          try {
            const payload = { title: lc.title || 'Imported conversation' };
            const sc = await chatApi.create(payload);
            const newId = sc && (sc.id || sc.conv_id);
            if (newId && Array.isArray(lc.messages)) {
              for (const msg of lc.messages) {
                try { await chatApi.appendMessage(newId, { role: msg.role, content: msg.content || '' }); } catch {}
              }
            }
            if (sc) { created.push(sc); migrated++; }
          } catch {}
        }
        try { localStorage.setItem(LS_MIGRATED_KEY, 'true'); } catch {}
        try { localStorage.removeItem(LS_KEY); } catch {}
        if (migrated > 0 && window.cc && window.cc.toast) {
          window.cc.toast('info', 'Migrated ' + migrated + ' local conversation' + (migrated !== 1 ? 's' : '') + ' to server');
        }
        // Re-fetch after migration
        try {
          const refreshed = await chatApi.list();
          const rList = (Array.isArray(refreshed) ? refreshed : (refreshed && Array.isArray(refreshed.conversations) ? refreshed.conversations : [])).map(normalizeConv);
          setConvs(rList);
          if (rList.length > 0) setActiveId(rList[0].conv_id);
        } catch {
          const normCreated = created.map(normalizeConv);
          setConvs(normCreated);
          if (normCreated.length > 0) setActiveId(normCreated[0].conv_id);
        }
        setConvsLoaded(true);
        return;
      }

      try { localStorage.setItem(LS_MIGRATED_KEY, 'true'); } catch {}
      const normList = list.map(normalizeConv);
      setConvs(normList);
      if (normList.length > 0) setActiveId(normList[0].conv_id);
      setConvsLoaded(true);
    }).catch(() => {
      setConvsLoaded(true);
    });
  }, []);

  // Scroll to bottom on new messages
  uE(() => {
    if (bottomRef.current) {
      bottomRef.current.scrollIntoView({ behavior: 'smooth' });
    }
  }, [activeConv && activeConv.messages && activeConv.messages.length]);

  // Create first conversation if none (after load completes)
  uE(() => {
    if (!convsLoaded) return;
    if (convs.length === 0) {
      const chatApi = window.cc && window.cc.api && window.cc.api.chat && window.cc.api.chat.conversations;
      if (chatApi) {
        chatApi.create({ title: 'New conversation' }).then(sc => {
          if (!sc) return;
          const c = normalizeConv(sc);
          setConvs([c]);
          setActiveId(c.conv_id);
        }).catch(() => {});
      }
    }
  }, [convsLoaded]);

  const updateConv = uC((convId, updater) => {
    setConvs(prev => prev.map(c => c.conv_id === convId ? updater(c) : c));
  }, []);

  const handleNew = () => {
    const chatApi = window.cc && window.cc.api && window.cc.api.chat && window.cc.api.chat.conversations;
    if (!chatApi) return;
    chatApi.create({ title: 'New conversation' }).then(sc => {
      if (!sc) return;
      const c = normalizeConv(sc);
      setConvs(prev => [c, ...prev]);
      setActiveId(c.conv_id);
    }).catch(err => {
      if (window.cc && window.cc.toast) window.cc.toast('error', 'Failed to create conversation: ' + String(err));
    });
  };

  const handleSelect = (id) => setActiveId(id);

  // S-polish: 让用户清理 chat 列表里残留的测试 / 旧会话。
  // 后端 DELETE /api/v1/chat/conversations/:id 已支持（chat_conversations.rs:679）。
  const handleDeleteConv = async (convId, title) => {
    if (!window.confirm(`Delete conversation "${title || convId}"? This cannot be undone.`)) return;
    const chatApi = window.cc && window.cc.api && window.cc.api.chat && window.cc.api.chat.conversations;
    if (!chatApi || !chatApi.delete) return;
    try {
      await chatApi.delete(convId);
      setConvs(prev => prev.filter(c => c.conv_id !== convId));
      if (activeId === convId) setActiveId(null);
      if (window.cc && window.cc.toast) window.cc.toast('success', `Deleted: ${title || convId}`);
    } catch (err) {
      if (window.cc && window.cc.toast) window.cc.toast('error', `Delete failed: ${err && err.message}`);
    }
  };

  const appendMessage = (convId, msg) => {
    const ts = Date.now();
    const msgWithTs = { ...msg, ts };
    updateConv(convId, c => {
      const messages = [...c.messages, msgWithTs];
      // Auto-title from first user message
      const title = c.title === 'New conversation' && msg.role === 'user'
        ? msg.content.slice(0, 40) + (msg.content.length > 40 ? '…' : '')
        : c.title;
      // Server-side rename if title changed
      if (title !== c.title) {
        const chatApi = window.cc && window.cc.api && window.cc.api.chat && window.cc.api.chat.conversations;
        if (chatApi) chatApi.rename(convId, title).catch(() => {});
      }
      return { ...c, messages, title };
    });
    // Mirror to server
    const chatApi = window.cc && window.cc.api && window.cc.api.chat && window.cc.api.chat.conversations;
    if (chatApi) {
      chatApi.appendMessage(convId, { role: msg.role, content: msg.content || '' })
        .catch(() => {});
    }
  };

  const handleApprovalResolved = (convId, resolvedText) => {
    appendMessage(convId, { role: 'user', content: resolvedText });
    // Trigger assistant reply with the resolution
    sendToApi(convId, resolvedText, true);
  };

  const sendToApi = async (convId, text, isResolution = false) => {
    setSending(true);
    const jwt = sessionStorage.getItem('cyberclaw.admin.jwt');
    const conv = convs.find(c => c.conv_id === convId);
    const history = (conv ? conv.messages : []).map(m => ({ role: m.role, content: m.content || '' }));
    if (!isResolution) {
      history.push({ role: 'user', content: text });
    }
    try {
      const res = await fetch('/api/v1/chat/message', {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          ...(jwt ? { Authorization: 'Bearer ' + jwt } : {}),
        },
        body: JSON.stringify({
          conversation_id: convId,
          model,
          messages: history,
        }),
      });

      if (res.status === 404 || res.status === 405) {
        // API not yet wired — mock assistant echo
        const mockContent = `[mock] I received: "${text}". The chat API endpoint (\`/api/v1/chat/message\`) is not yet wired — Lane 1 will provide it. Your message has been recorded in conversation history.`;
        appendMessage(convId, { role: 'assistant', content: mockContent });
        setSending(false);
        return;
      }

      if (!res.ok) throw new Error('HTTP ' + res.status);

      const data = await res.json();
      // Expected shape: { message: { role, content, tool_calls? } }
      const msg = data.message || { role: 'assistant', content: data.content || '' };
      appendMessage(convId, msg);
    } catch (err) {
      appendMessage(convId, {
        role: 'assistant',
        content: `[error] ${String(err)}`,
      });
    } finally {
      setSending(false);
    }
  };

  // S15-T14: Refresh recovery — restore pending clarify on conversation load
  uE(() => {
    if (!activeId) return;
    const chatApi = window.cc && window.cc.api && window.cc.api.chat;
    if (!chatApi || !chatApi.fetchPendingClarifies) return;
    chatApi.fetchPendingClarifies(activeId).then(pending => {
      if (!Array.isArray(pending) || pending.length === 0) return;
      // Normalize: backend may return flat ClarifyRequest objects
      const first = pending[0];
      const clarify = {
        id: first.id || first.clarify_id,
        conversation_id: activeId,
        questions: first.questions || [],
        source: first.source || null,
        expires_at: first.expires_at || null,
      };
      setActiveClarify(clarify);
      setClarifyQueue(pending.slice(1).map(p => ({
        id: p.id || p.clarify_id,
        conversation_id: activeId,
        questions: p.questions || [],
        source: p.source || null,
        expires_at: p.expires_at || null,
      })));
      setComposerLocked(true);
    }).catch(() => {});
  }, [activeId]);

  // S15-T14: Handle clarify submit
  const handleClarifySubmit = uC(async (answer) => {
    if (!activeClarify) return;
    const chatApi = window.cc && window.cc.api && window.cc.api.chat;
    if (chatApi && chatApi.submitClarifyResponse) {
      await chatApi.submitClarifyResponse(activeClarify.id, answer);
    }
    // Optimistic: transition to resolved; SSE will confirm
    setActiveClarify(prev => prev ? { ...prev, status: 'resolved', resolvedAnswer: answer } : prev);
  }, [activeClarify]);

  // S15-T14: Advance clarify queue after a card is resolved
  const advanceClarifyQueue = uC(() => {
    if (clarifyQueue.length > 0) {
      const [next, ...rest] = clarifyQueue;
      setActiveClarify(next);
      setClarifyQueue(rest);
    } else {
      setActiveClarify(null);
      setTimeout(() => {
        setComposerLocked(false);
        if (textareaRef.current) textareaRef.current.focus();
      }, 300);
    }
  }, [clarifyQueue]);

  // S14-4: Send via SSE streaming when the browser supports it and the
  // backend speaks `text/event-stream`. On abort the partial buffer is
  // committed with a gray `[aborted]` suffix. On any non-SSE response
  // (e.g. 406 or a JSON body) we transparently fall back to the
  // non-stream `sendToApi` path so UX never regresses.
  const streamAssistantReply = (convId, history) => {
    if (!window.cc || !window.cc.subscribeChatCompletions) {
      // Helper absent — fall back immediately.
      return sendToApi(convId, history[history.length - 1].content);
    }
    setSending(true);
    setStreamConvId(convId);
    setStreamingText('');
    let acc = '';

    const finalize = (suffix) => {
      const content = (acc + (suffix || '')).trim();
      if (content.length > 0) {
        appendMessage(convId, { role: 'assistant', content });
      }
      setStreamingText('');
      setStreamConvId(null);
      setSending(false);
      abortRef.current = null;
    };

    abortRef.current = window.cc.subscribeChatCompletions(
      { model, messages: history, conversation_id: convId },
      {
        onDelta(delta) {
          acc += delta;
          setStreamingText(acc);
        },
        onClarify(clarify) {
          // S15-T14: SSE clarify frame — mount card + lock composer
          if (activeClarify) {
            // Queue behind existing card
            setClarifyQueue(prev => [...prev, clarify]);
          } else {
            setActiveClarify(clarify);
            setComposerLocked(true);
          }
        },
        onClarifyResolved(id, answer) {
          // S15-T14: SSE clarify_resolved — transition resolved card then advance queue
          setActiveClarify(prev => {
            if (prev && prev.id === id) {
              return { ...prev, status: 'resolved', resolvedAnswer: answer };
            }
            return prev;
          });
          setTimeout(() => advanceClarifyQueue(), 300);
        },
        onEnd() { finalize(); },
        onError(err) {
          // Fallback path: if the server didn't give us SSE, re-run the
          // non-stream flow so the user still gets a reply.
          if (err && err.notSse) {
            setStreamingText('');
            setStreamConvId(null);
            abortRef.current = null;
            sendToApi(convId, history[history.length - 1].content);
            return;
          }
          if (err && err.aborted) {
            finalize(' [aborted]');
            return;
          }
          finalize(`\n[error] ${String(err && err.message ? err.message : err)}`);
        },
      }
    );
  };

  const handleStop = () => {
    if (abortRef.current) {
      try { abortRef.current.abort(); } catch {}
    }
  };

  const handleSend = async () => {
    const text = input.trim();
    // S15-T14: Block send while clarify is pending
    if (!text || sending || !activeId || composerLocked) return;

    // Ensure active conv exists
    let convId = activeId;
    if (!activeConv) {
      const c = newConversation();
      setConvs(prev => [c, ...prev]);
      setActiveId(c.conv_id);
      convId = c.conv_id;
    }

    appendMessage(convId, { role: 'user', content: text });
    setInput('');

    // Build history snapshot (including the message we just appended).
    const conv = convs.find(c => c.conv_id === convId);
    const history = (conv ? conv.messages : []).map(m => ({ role: m.role, content: m.content || '' }));
    history.push({ role: 'user', content: text });

    streamAssistantReply(convId, history);
  };

  const handleKeyDown = (e) => {
    // S15-T14: Ignore send shortcuts while composer is locked (clarify pending)
    if (composerLocked) return;
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  const messages = activeConv ? activeConv.messages : [];

  // S16-T6: compress disabled reason
  const compressDisabled = uM(() => {
    if (messages.length < 5) return { disabled: true, reason: t('compress.tooltip.too_short') };
    if (composerLocked) return { disabled: true, reason: t('compress.tooltip.locked_by_clarify') };
    if (Date.now() < compressLockoutUntil) return { disabled: true, reason: t('compress.tooltip.cooldown') };
    return { disabled: false, reason: '' };
  }, [messages.length, composerLocked, compressLockoutUntil]);

  // S16-T6: compress handler
  const handleCompress = async () => {
    if (compressDisabled.disabled || compressing || !activeId) return;
    setCompressing(true);
    try {
      const result = await window.cc.api.chat.compress(activeId);
      // Reload conversation messages from server
      const chatApi = window.cc && window.cc.api && window.cc.api.chat && window.cc.api.chat.conversations;
      if (chatApi) {
        const fresh = await chatApi.get(activeId);
        if (fresh) {
          const normalized = normalizeConv(fresh);
          setConvs(prev => prev.map(c => c.conv_id === activeId ? normalized : c));
        }
      }
      if (window.cc && window.cc.toast) {
        window.cc.toast('success', t('compress.success', {
          original: result.original_count,
          compressed: result.compressed_count,
        }));
      }
    } catch (err) {
      const msg = (err && err.message) ? err.message : 'unknown';
      if (window.cc && window.cc.toast) {
        window.cc.toast('error', t('compress.failed', { reason: msg }));
      }
      if (/circuit.?breaker/i.test(msg)) {
        setCompressLockoutUntil(Date.now() + 10 * 60 * 1000);
      }
    } finally {
      setCompressing(false);
    }
  };

  return (
    <div
      className="flex h-full rounded-lg border border-border overflow-hidden bg-bg"
      style={{ height: 'calc(100vh - 160px)', minHeight: 480 }}
    >
      {/* Left: conversation list */}
      <ConvSidebar
        convs={convs}
        activeId={activeId}
        onSelect={handleSelect}
        onNew={handleNew}
        onDelete={handleDeleteConv}
        lang={lang}
      />

      {/* Right: chat area */}
      <div className="flex-1 flex flex-col min-w-0">
        {/* Chat header */}
        <div className="h-11 border-b border-border flex items-center px-4 gap-3 shrink-0 bg-bg-2">
          <I.MessageSquare size={14} className="text-accent" />
          <span className="text-[13px] font-medium truncate">
            {activeConv ? activeConv.title : (t('chat.conversation.new') || 'New conversation')}
          </span>
          <div className="flex-1" />
        </div>

        {/* Messages area */}
        <div className="flex-1 overflow-auto px-5 py-4">
          {messages.length === 0 && (
            <div className="h-full flex flex-col items-center justify-center text-center gap-4">
              <div className="h-14 w-14 rounded-2xl bg-accent-soft flex items-center justify-center">
                <I.MessageSquare size={24} className="text-accent" />
              </div>
              <div>
                <div className="text-[16px] font-semibold text-fg">CyberClaw Chat</div>
                <div className="text-[13px] text-fg-3 mt-1 max-w-[320px]">
                  {t('chat.placeholder') || 'Send a message to start. Approval requests will appear inline.'}
                </div>
              </div>
            </div>
          )}
          {messages.map((msg, i) => {
            // S21-T11: route handoff messages to HandoffCard
            if (msg.role === 'handoff') {
              const payload = msg.metadata || null;
              return window.HandoffCard
                ? React.createElement(window.HandoffCard, {
                    key: msg.id || i,
                    payload,
                    lang,
                  })
                : null;
            }
            // S16-T7: route summary messages to CompressSummary card
            if (msg.role === 'summary') {
              const meta = msg.metadata || {};
              return window.CompressSummary
                ? React.createElement(window.CompressSummary, {
                    key: msg.id || i,
                    originalCount: meta.original_count || 0,
                    compressedAt: meta.compressed_at || msg.created_at || '',
                    stagesApplied: meta.stages_applied || [],
                    summaryText: msg.content || '',
                    lang,
                  })
                : null;
            }
            return (
              <MessageBubble
                key={i}
                msg={msg}
                convId={activeId}
                lang={lang}
                onApprovalResolved={(text) => handleApprovalResolved(activeId, text)}
              />
            );
          })}
          {/* S14-4: live streaming assistant reply bubble */}
          {streamingText && streamConvId === activeId && (
            <div className="flex gap-3 mb-3 items-start">
              <div className="h-7 w-7 rounded-full bg-accent-soft flex items-center justify-center shrink-0 mt-0.5">
                <I.Bot size={13} className="text-accent" />
              </div>
              <div className="flex-1 min-w-0">
                <div className="bg-bg-2 border border-border rounded-2xl rounded-tl-sm px-4 py-2.5 text-[13px] text-fg leading-relaxed shadow-sm whitespace-pre-wrap">
                  {streamingText}
                  <span className="inline-block w-1.5 h-3 ml-0.5 align-middle bg-accent/70 animate-pulse" />
                </div>
              </div>
            </div>
          )}
          {sending && !streamingText && (
            <div className="flex gap-3 mb-3 items-start">
              <div className="h-7 w-7 rounded-full bg-accent-soft flex items-center justify-center shrink-0">
                <I.Bot size={13} className="text-accent" />
              </div>
              <div className="bg-bg-2 border border-border rounded-2xl rounded-tl-sm px-4 py-2.5">
                <div className="flex gap-1 items-center h-5">
                  <span className="inline-block w-1.5 h-1.5 rounded-full bg-fg-4 animate-bounce" style={{ animationDelay: '0ms' }} />
                  <span className="inline-block w-1.5 h-1.5 rounded-full bg-fg-4 animate-bounce" style={{ animationDelay: '150ms' }} />
                  <span className="inline-block w-1.5 h-1.5 rounded-full bg-fg-4 animate-bounce" style={{ animationDelay: '300ms' }} />
                </div>
              </div>
            </div>
          )}
          <div ref={bottomRef} />
        </div>

        {/* Composer footer */}
        <div className="border-t border-border bg-bg-2 shrink-0">
          {/* S15-T14: ClarifyCard flyout — slides out above composer */}
          {activeClarify && (() => {
            const q = activeClarify.questions && activeClarify.questions[0];
            const question = q ? (q.question || '') : '';
            const options = q ? (q.options || []) : [];
            return (
              <div
                className="clarify-flyout-wrapper"
                style={{
                  overflow: 'hidden',
                  borderBottom: '1px solid var(--border)',
                  animation: 'clarify-slide-in 0.18s ease',
                }}
              >
                <style>{`
                  @keyframes clarify-slide-in {
                    from { transform: translateY(-100%); opacity: 0; }
                    to   { transform: translateY(0);    opacity: 1; }
                  }
                `}</style>
                <div style={{ padding: '10px 14px' }}>
                  {window.ClarifyCard
                    ? React.createElement(window.ClarifyCard, {
                        clarifyId: activeClarify.id,
                        // S19-C: pass full clarify object for multi-question / multiSelect / preview
                        clarify: activeClarify,
                        // Legacy compat props (used by ClarifyCard when clarify.questions absent)
                        question,
                        options,
                        allowFreeform: false,
                        status: activeClarify.status || 'pending',
                        resolvedAnswer: activeClarify.resolvedAnswer || null,
                        onSubmit: handleClarifySubmit,
                        lang,
                      })
                    : null}
                </div>
              </div>
            );
          })()}

          {/* S15-T14: Composer lock hint bar */}
          {composerLocked && (
            <div style={{
              padding: '6px 14px',
              background: 'var(--bg-3)',
              borderBottom: '1px solid var(--border)',
              fontSize: 11,
              color: 'var(--fg-3)',
              display: 'flex',
              alignItems: 'center',
              gap: 6,
            }}>
              <span>⏸</span>
              <span>{t('clarify.composer_lock_hint') || 'Waiting for your response to the clarification above'}</span>
            </div>
          )}

          {/* Footer controls bar — flex-nowrap + overflow scroll；select 给 min-w 强制不压 */}
          <div className="flex flex-nowrap items-center gap-1.5 px-3 pt-2.5 pb-1 overflow-x-auto">
            {/* Agent dropdown — agents.length === 0 时显式告知用户为何点不动 */}
            <select
              value={activeAgent}
              onChange={e => setActiveAgent(e.target.value)}
              disabled={agents.length === 0}
              title={agents.length === 0
                ? (t('composer.agent.empty.tooltip') || 'No agents configured. Create one in Govern → Agents.')
                : (t('composer.agent') || 'Agent')}
              className="h-7 text-[11px] mono px-1.5 rounded-md bg-bg-3 border border-border text-fg-2 outline-none focus-ring hover:border-[var(--border-strong)] cursor-pointer shrink-0 disabled:cursor-not-allowed disabled:opacity-60"
              style={{ minWidth: 100, maxWidth: 160 }}
            >
              {agents.length === 0 && (
                <option value="">{t('composer.agent.empty') || 'No agents'}</option>
              )}
              {agents.map(a => (
                <option key={a.id || a.agent_id} value={a.id || a.agent_id}>
                  {a.display_name || a.name || a.id || a.agent_id}
                </option>
              ))}
            </select>

            {/* Model dropdown */}
            <select
              value={model}
              onChange={e => setModel(e.target.value)}
              title={t('composer.model') || 'Model'}
              className="h-7 text-[11px] mono px-1.5 rounded-md bg-bg-3 border border-border text-fg-2 outline-none focus-ring hover:border-[var(--border-strong)] cursor-pointer shrink-0"
              style={{ minWidth: 140, maxWidth: 200 }}
            >
              {MODEL_OPTIONS.map(m => (
                <option key={m.value} value={m.value}>{m.label}</option>
              ))}
            </select>

            {/* Workspace chip */}
            <button
              onClick={() => setShowWorkspaceModal(true)}
              title={t('composer.workspace') || 'Workspace'}
              className="h-7 flex items-center gap-1 px-2 rounded-md bg-bg-3 border border-border text-fg-3 text-[11px] mono hover:border-[var(--border-strong)] hover:text-fg transition-colors shrink-0 max-w-[120px] whitespace-nowrap"
            >
              <I.Folder size={11} className="shrink-0" />
              <span className="truncate">{cwd}</span>
            </button>

            {/* S16-T6: Compress button */}
            <button
              className="composer-compress-btn"
              disabled={compressDisabled.disabled || compressing}
              title={compressDisabled.reason || t('compress.tooltip.ready')}
              onClick={handleCompress}
              aria-label={t('compress.button.label')}
              style={{
                height: 28,
                display: 'flex',
                alignItems: 'center',
                gap: 4,
                padding: '0 8px',
                borderRadius: 6,
                border: '1px solid var(--border)',
                background: 'var(--bg-3)',
                color: 'var(--fg-3)',
                fontSize: 11,
                fontFamily: 'var(--font-mono, monospace)',
                cursor: compressDisabled.disabled || compressing ? 'not-allowed' : 'pointer',
                opacity: compressDisabled.disabled || compressing ? 0.5 : 1,
                whiteSpace: 'nowrap',
                flexShrink: 0,
              }}
            >
              {compressing
                ? <span style={{ display: 'inline-flex', gap: 2 }}>
                    <span style={{ width: 4, height: 4, borderRadius: '50%', background: 'currentColor', animation: 'bounce 1s infinite', animationDelay: '0ms' }} />
                    <span style={{ width: 4, height: 4, borderRadius: '50%', background: 'currentColor', animation: 'bounce 1s infinite', animationDelay: '150ms' }} />
                    <span style={{ width: 4, height: 4, borderRadius: '50%', background: 'currentColor', animation: 'bounce 1s infinite', animationDelay: '300ms' }} />
                  </span>
                : <span>⚡ {t('compress.button.label')}</span>}
            </button>

            {/* Spacer → textarea + ring + send */}
            <div className="flex-1" />
          </div>

          {/* Textarea + ring + send row */}
          <div className="flex gap-2 items-end px-3 pb-3">
            <textarea
              ref={textareaRef}
              value={input}
              onChange={(e) => {
                setInput(e.target.value);
                e.target.style.height = 'auto';
                e.target.style.height = Math.min(e.target.scrollHeight, 160) + 'px';
              }}
              onKeyDown={handleKeyDown}
              disabled={sending || composerLocked}
              placeholder={t('chat.placeholder') || 'Message CyberClaw… (Enter to send, Shift+Enter for newline)'}
              rows={1}
              className="flex-1 bg-bg-3 border border-border rounded-xl px-4 py-2.5 text-[13px] text-fg placeholder:text-fg-4 outline-none focus-ring resize-none leading-relaxed disabled:opacity-50"
              style={{ minHeight: 40, maxHeight: 160 }}
            />

            {/* Token ring */}
            <TokenRing
              pct={Math.min(100, messages.length * 2)}
              tooltip={`${t('composer.token_ring') || 'Token usage'}: ~${messages.length * 400} tokens used / ~200k budget`}
            />

            {/* S14-4: Stop button — visible while a stream is in flight */}
            {sending && abortRef.current && (
              <button
                onClick={handleStop}
                className="h-10 w-10 rounded-xl bg-rose-500/15 border border-rose-500/40 flex items-center justify-center text-rose-400 hover:bg-rose-500/25 transition-colors shrink-0"
                title={t('chat.stop') || 'Stop'}
                aria-label="Stop streaming"
              >
                <svg width="12" height="12" viewBox="0 0 12 12" fill="currentColor" aria-hidden="true">
                  <rect x="2" y="2" width="8" height="8" rx="1" />
                </svg>
              </button>
            )}

            {/* Send button */}
            <button
              onClick={handleSend}
              disabled={!input.trim() || sending || composerLocked}
              className="h-10 w-10 rounded-xl bg-accent flex items-center justify-center text-white hover:opacity-90 transition-opacity disabled:opacity-40 disabled:cursor-not-allowed shrink-0"
              title={t('chat.send') || 'Send (↵)'}
            >
              <I.ArrowRight size={16} />
            </button>
          </div>

          {/* Hint row */}
          <div className="px-4 pb-2 flex items-center gap-2 text-[10px] mono text-fg-4">
            <span>↵ send</span>
            <span className="text-fg-4/50">·</span>
            <span>⇧↵ newline</span>
          </div>
        </div>

        {/* Workspace modal */}
        {showWorkspaceModal && (
          <WorkspaceModal cwd={cwd} onClose={() => setShowWorkspaceModal(false)} t={t} />
        )}
      </div>
    </div>
  );
}
