// Govern Memory Console — Sprint 17 Lane B / S19 F (Create + Delete)
// Admin-only page for inspecting, editing, tracing, creating, and deleting memory records.

const { useState, useEffect, useCallback, useRef } = React;

// ---------------------------------------------------------------------------
// MemoryConsolePage
// ---------------------------------------------------------------------------

function MemoryConsolePage({ lang }) {
  // i18n.jsx exposes `tFor` on window directly (Object.assign(window, ...)),
  // not on window.cc. Match what every other page does.
  const t = tFor(lang || 'en');

  const TAB_LEVELS = {
    working:   'L0',
    episodic:  'L1',
    procedural:'L2',
  };

  // Resolve current operator role for admin-only gating.
  const op = (() => { try { return JSON.parse(sessionStorage.getItem('cyberclaw.admin.op') || '{}'); } catch { return {}; } })();
  const isAdmin = op.role === 'admin';

  const [tab, setTab]     = useState('working');
  const [agentId, setAgentId]   = useState('');
  const [keyword, setKeyword]   = useState('');
  const [rows, setRows]   = useState([]);
  const [loading, setLoading]   = useState(false);
  const [error, setError] = useState(null);
  const [selected, setSelected] = useState(null);
  const [showCreate, setShowCreate] = useState(false);
  const [showSearch, setShowSearch] = useState(false);

  // Global Cmd+K / Ctrl+K opens search modal. Esc closes it (modal handles its own Esc).
  useEffect(() => {
    const onKey = (e) => {
      if ((e.metaKey || e.ctrlKey) && (e.key === 'k' || e.key === 'K')) {
        e.preventDefault();
        setShowSearch(true);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  const fetchRows = useCallback(() => {
    const level = TAB_LEVELS[tab];
    setLoading(true);
    setError(null);
    const params = { level };
    if (agentId.trim()) params.agent_id = agentId.trim();
    if (keyword.trim()) params.keyword   = keyword.trim();
    window.cc.api.memory
      .list(params)
      .then((data) => {
        setRows((data && data.records) ? data.records : []);
        setLoading(false);
      })
      .catch((err) => {
        setError(err.message || 'Failed to load memory records');
        setLoading(false);
      });
  }, [tab, agentId, keyword]);

  useEffect(() => { fetchRows(); }, [fetchRows]);

  return (
    <div className="flex flex-col h-full">
      {/* Page header */}
      <div className="px-6 pt-5 pb-3 border-b border-border shrink-0">
        <div className="text-[15px] font-semibold text-fg">{t('memory_console.tab.label')}</div>
        <div className="text-[12px] text-fg-3 mt-0.5">{t('memory_console.subtitle')}</div>
      </div>

      {/* Tab bar */}
      <div className="flex gap-1 px-6 pt-3 shrink-0">
        {Object.keys(TAB_LEVELS).map((key) => (
          <button
            key={key}
            onClick={() => { setTab(key); setSelected(null); }}
            className={`px-3 py-1.5 rounded-md text-[12px] font-medium transition-colors ${
              tab === key
                ? 'bg-[var(--bg-3)] text-fg border border-[var(--border)]'
                : 'text-fg-3 hover:text-fg hover:bg-[var(--hover)]'
            }`}
          >
            {t('memory_console.tab.' + key)}
          </button>
        ))}
      </div>

      {/* Filters */}
      <div className="flex gap-2 px-6 py-3 shrink-0">
        <input
          type="text"
          value={agentId}
          onChange={(e) => setAgentId(e.target.value)}
          placeholder={t('memory_console.filter_agent')}
          className="bg-bg-2 border border-border rounded-md px-3 py-1.5 text-[12px] text-fg placeholder:text-fg-4 outline-none focus:border-[var(--accent)] w-48"
        />
        <input
          type="text"
          value={keyword}
          onChange={(e) => setKeyword(e.target.value)}
          placeholder={t('memory_console.filter_keyword')}
          className="bg-bg-2 border border-border rounded-md px-3 py-1.5 text-[12px] text-fg placeholder:text-fg-4 outline-none focus:border-[var(--accent)] flex-1"
        />
        <button
          onClick={fetchRows}
          className="px-3 py-1.5 bg-[var(--accent)] text-[var(--accent-fg)] rounded-md text-[12px] font-medium hover:opacity-90 transition-opacity"
        >
          {t('refresh')}
        </button>
        <button
          onClick={() => setShowSearch(true)}
          title={t('memory_console.search.button') + ' (⌘K)'}
          className="px-3 py-1.5 bg-bg border border-border rounded-md text-[12px] font-medium text-fg-2 hover:text-fg hover:border-[var(--accent)] transition-colors ml-auto inline-flex items-center gap-2"
        >
          <span>🔍 {t('memory_console.search.button')}</span>
          <kbd className="text-[10px] text-fg-4 px-1 py-0.5 rounded bg-bg-2 border border-border">
            {t('memory_console.search.shortcut_hint')}
          </kbd>
        </button>
        {isAdmin && (
          <button
            onClick={() => setShowCreate(true)}
            className="px-3 py-1.5 bg-bg border border-border rounded-md text-[12px] font-medium text-fg-2 hover:text-fg hover:border-[var(--accent)] transition-colors"
          >
            {t('memory_console.action.new')}
          </button>
        )}
      </div>

      {/* Table */}
      <div className="flex-1 overflow-auto px-6 pb-6">
        {error && (
          <div className="text-[12px] text-rose-400 py-3">{error}</div>
        )}
        {!error && !loading && rows.length === 0 && (
          <div className="py-10 text-center text-[13px] text-fg-4">
            {t('memory_console.empty')}
          </div>
        )}
        {!error && (loading || rows.length > 0) && (
          <table className="w-full text-[12px] border-collapse">
            <thead>
              <tr className="border-b border-border text-left text-fg-4">
                <th className="py-2 pr-3 font-medium">{t('memory_console.col_agent')}</th>
                <th className="py-2 pr-3 font-medium">{t('memory_console.col_level')}</th>
                <th className="py-2 pr-3 font-medium">{t('memory_console.col_created')}</th>
                <th className="py-2 pr-3 font-medium">{t('memory_console.col_updated')}</th>
                <th className="py-2 pr-3 font-medium">{t('memory_console.col_size')}</th>
                <th className="py-2 font-medium">{t('memory_console.col_preview')}</th>
              </tr>
            </thead>
            <tbody>
              {loading && (
                <tr>
                  <td colSpan={6} className="py-8 text-center text-fg-4">Loading…</td>
                </tr>
              )}
              {rows.map((row) => (
                <tr
                  key={row.id}
                  onClick={() => setSelected(row)}
                  className="border-b border-border hover:bg-[var(--hover)] cursor-pointer transition-colors"
                >
                  <td className="py-2 pr-3 mono text-fg-2">{row.agent_id}</td>
                  <td className="py-2 pr-3">
                    <span className="px-1.5 py-0.5 rounded text-[10px] bg-[var(--accent-soft)] text-[var(--accent)] font-medium">
                      {row.level}
                    </span>
                  </td>
                  <td className="py-2 pr-3 text-fg-3">{fmtTs(row.created_at)}</td>
                  <td className="py-2 pr-3 text-fg-3">{fmtTs(row.updated_at)}</td>
                  <td className="py-2 pr-3 text-fg-3">{row.size_bytes}B</td>
                  <td className="py-2 text-fg-3 truncate max-w-[280px]">{row.preview}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      {/* Detail dialog */}
      {selected && (
        <MemoryDetailDialog
          summary={selected}
          lang={lang}
          onClose={() => setSelected(null)}
          onSaved={() => { setSelected(null); fetchRows(); }}
          onDeleted={() => { setSelected(null); fetchRows(); }}
        />
      )}

      {/* Create dialog (S19 F) */}
      {showCreate && (
        <CreateMemoryDialog
          lang={lang}
          defaultLevel={TAB_LEVELS[tab]}
          onClose={() => setShowCreate(false)}
          onCreated={() => { setShowCreate(false); fetchRows(); }}
        />
      )}

      {/* Global BM25 search modal (S20 P4-A frontend) */}
      {showSearch && (
        <MemoryGlobalSearchDialog
          lang={lang}
          onClose={() => setShowSearch(false)}
          onOpenRecord={(summary) => { setShowSearch(false); setSelected(summary); }}
        />
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// MemoryGlobalSearchDialog — Sprint 20 P4-A frontend activation
//   - Cmd+K opens; Esc closes
//   - 300ms debounced input, hits GET /api/v1/memory/search?q=&limit=20
//   - Cross-level (no level param); results show L0/L1/L2 badge + score + highlighted matched_tokens
//   - Click result -> opens existing MemoryDetailDialog (reuses full/edit/trace/delete path)
// ---------------------------------------------------------------------------

function MemoryGlobalSearchDialog({ lang, onClose, onOpenRecord }) {
  const t = tFor ? tFor(lang || 'en') : (k) => k;

  const [query, setQuery] = useState('');
  const [mode, setMode] = useState('keyword');
  const [results, setResults] = useState([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState(null);
  const [total, setTotal] = useState(0);
  const inputRef = useRef(null);
  const debounceRef = useRef(null);

  // Autofocus input on mount.
  useEffect(() => {
    if (inputRef.current) inputRef.current.focus();
  }, []);

  // Esc to close.
  useEffect(() => {
    const onKey = (e) => { if (e.key === 'Escape') onClose(); };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  // Debounced search on query or mode change.
  useEffect(() => {
    if (debounceRef.current) clearTimeout(debounceRef.current);
    const q = query.trim();
    if (!q) {
      setResults([]);
      setTotal(0);
      setError(null);
      setLoading(false);
      return;
    }
    setLoading(true);
    debounceRef.current = setTimeout(() => {
      window.cc.api.memory
        .search({ q, limit: 20, mode })
        .then((data) => {
          setResults((data && data.results) || []);
          setTotal((data && typeof data.total === 'number') ? data.total : 0);
          setError(null);
          setLoading(false);
        })
        .catch((err) => {
          if (err.message && err.message.includes('not configured')) {
            setError(t('memory_console.search.mode.disabled_error'));
          } else {
            setError(err.message || 'Search failed');
          }
          setResults([]);
          setTotal(0);
          setLoading(false);
        });
    }, 300);
    return () => { if (debounceRef.current) clearTimeout(debounceRef.current); };
  }, [query, mode]);

  // Highlight matched tokens within preview/content text (case-insensitive).
  const highlight = (text, tokens) => {
    if (!text || !tokens || tokens.length === 0) return text || '';
    const esc = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    const pattern = new RegExp('(' + tokens.map(esc).join('|') + ')', 'gi');
    const parts = text.split(pattern);
    return parts.map((p, i) =>
      pattern.test(p)
        ? <mark key={i} className="bg-[var(--accent-soft)] text-[var(--accent)] rounded px-0.5">{p}</mark>
        : p
    );
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center pt-[10vh] bg-black/60"
      onClick={onClose}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        className="w-[min(720px,92vw)] max-h-[70vh] bg-bg border border-border rounded-lg shadow-2xl overflow-hidden flex flex-col"
      >
        {/* Mode toggle */}
        <div className="flex gap-1 px-4 pt-3 pb-1 shrink-0">
          {['keyword', 'semantic', 'hybrid'].map((m) => (
            <button
              key={m}
              onClick={() => setMode(m)}
              className={`px-2.5 py-1 rounded text-[12px] font-medium transition-colors ${
                mode === m
                  ? 'bg-[var(--accent)] text-[var(--accent-fg)]'
                  : 'bg-bg-2 text-fg-3 hover:text-fg'
              }`}
            >
              {t(`memory_console.search.mode.${m}`)}
            </button>
          ))}
        </div>

        {/* Input */}
        <div className="flex items-center gap-2 px-4 py-3 border-b border-border">
          <span className="text-fg-3 text-[14px]">🔍</span>
          <input
            ref={inputRef}
            type="text"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={t('memory_console.search.placeholder')}
            className="flex-1 bg-transparent text-[13px] text-fg placeholder:text-fg-4 outline-none"
          />
          <button
            onClick={onClose}
            title={t('memory_console.search.close')}
            className="text-fg-4 hover:text-fg text-[12px] px-2 py-0.5 rounded border border-border hover:bg-[var(--hover)] transition-colors"
          >
            Esc
          </button>
        </div>

        {/* Results */}
        <div className="flex-1 overflow-auto">
          {error && (
            <div className="px-4 py-3 text-[12px] text-rose-400">{error}</div>
          )}
          {!error && !query.trim() && (
            <div className="px-4 py-10 text-center text-[13px] text-fg-4">
              {t('memory_console.search.empty_query')}
            </div>
          )}
          {!error && query.trim() && loading && (
            <div className="px-4 py-10 text-center text-[13px] text-fg-4">Loading…</div>
          )}
          {!error && query.trim() && !loading && results.length === 0 && (
            <div className="px-4 py-10 text-center text-[13px] text-fg-4">
              {t('memory_console.search.no_results').replace('{query}', query)}
            </div>
          )}
          {!error && results.length > 0 && (
            <>
              <div className="px-4 py-2 text-[11px] text-fg-4 border-b border-border">
                {t('memory_console.search.result_count').replace('{total}', total)}
              </div>
              <ul>
                {results.map((item) => {
                  const mem = item.memory || item;
                  const tokens = item.matched_tokens || [];
                  return (
                    <li
                      key={mem.id}
                      onClick={() => onOpenRecord(mem)}
                      className="px-4 py-3 border-b border-border hover:bg-[var(--hover)] cursor-pointer transition-colors"
                    >
                      <div className="flex items-center gap-2 text-[11px] mb-1">
                        <span className="px-1.5 py-0.5 rounded text-[10px] bg-[var(--accent-soft)] text-[var(--accent)] font-medium">
                          {mem.level}
                        </span>
                        <span className="mono text-fg-3">{mem.agent_id}</span>
                        <span className="text-fg-4">·</span>
                        <span className="text-fg-4">
                          {t('memory_console.search.score_label')} {typeof item.score === 'number' ? item.score.toFixed(2) : '—'}
                        </span>
                      </div>
                      <div className="text-[12px] text-fg-2 line-clamp-2">
                        {highlight(mem.preview || '', tokens)}
                      </div>
                    </li>
                  );
                })}
              </ul>
            </>
          )}
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// MemoryDetailDialog
// ---------------------------------------------------------------------------

function MemoryDetailDialog({ summary, lang, onClose, onSaved, onDeleted }) {
  const t = tFor ? tFor(lang || 'en') : (k) => k;

  const [record, setRecord]   = useState(null);
  const [trace, setTrace]     = useState(null);
  const [editing, setEditing] = useState(false);
  const [content, setContent] = useState('');
  const [saving, setSaving]   = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [loadErr, setLoadErr] = useState(null);

  // Load full record + trace in parallel.
  useEffect(() => {
    let cancelled = false;
    window.cc.api.memory.get(summary.id).then((r) => {
      if (cancelled) return;
      setRecord(r);
      setContent(r.content || '');
    }).catch((e) => {
      if (!cancelled) setLoadErr(e.message || 'Failed to load record');
    });
    window.cc.api.memory.trace(summary.id).then((tr) => {
      if (!cancelled) setTrace(tr);
    }).catch(() => {
      /* trace errors are non-fatal */
    });
    return () => { cancelled = true; };
  }, [summary.id]);

  const handleSave = async () => {
    if (!window.confirm(t('memory_console.detail.save_confirm'))) return;
    setSaving(true);
    try {
      await window.cc.api.memory.edit(summary.id, content);
      window.cc && window.cc.toast && window.cc.toast.success
        ? window.cc.toast.success('Memory edited (audit logged)')
        : alert('Memory edited (audit logged)');
      onSaved();
    } catch (e) {
      alert('Save failed: ' + (e.message || String(e)));
    } finally {
      setSaving(false);
    }
  };

  const handleDelete = async () => {
    if (!window.confirm(t('memory_console.delete.confirm'))) return;
    setDeleting(true);
    try {
      await window.cc.api.memory.delete(summary.id);
      window.cc && window.cc.toast && window.cc.toast.success
        ? window.cc.toast.success(t('memory_console.delete.success'))
        : alert(t('memory_console.delete.success'));
      onDeleted && onDeleted();
    } catch (e) {
      alert('Delete failed: ' + (e.message || String(e)));
    } finally {
      setDeleting(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      <div className="absolute inset-0 bg-black/60" onClick={onClose} />
      <div
        className="relative bg-bg-2 border border-border rounded-xl shadow-2xl flex flex-col"
        style={{ width: 680, maxWidth: 'calc(100vw - 48px)', maxHeight: 'calc(100vh - 80px)' }}
      >
        {/* Header */}
        <div className="px-5 py-4 border-b border-border flex items-center gap-3 shrink-0">
          <span className="px-1.5 py-0.5 rounded text-[10px] bg-[var(--accent-soft)] text-[var(--accent)] font-medium mono">
            {summary.level}
          </span>
          <span className="text-[13px] font-medium text-fg truncate">{summary.agent_id}</span>
          <span className="mono text-[11px] text-fg-4 truncate flex-1">{summary.id}</span>
          <button onClick={onClose} className="text-fg-4 hover:text-fg shrink-0">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
          </button>
        </div>

        {/* Body */}
        <div className="flex-1 overflow-auto px-5 py-4 space-y-4">
          {loadErr && <div className="text-[12px] text-rose-400">{loadErr}</div>}
          {!record && !loadErr && (
            <div className="text-[12px] text-fg-4">Loading…</div>
          )}
          {record && (
            <>
              {/* Metadata row */}
              <div className="flex gap-4 text-[11px] text-fg-4 mono">
                <span>created: {fmtTs(record.created_at)}</span>
                <span>updated: {fmtTs(record.updated_at)}</span>
                <span>{record.content ? record.content.length : 0}B</span>
              </div>

              {/* Content */}
              {editing ? (
                <textarea
                  value={content}
                  onChange={(e) => setContent(e.target.value)}
                  rows={12}
                  className="w-full bg-bg border border-border rounded-md px-3 py-2 text-[12px] text-fg mono outline-none focus:border-[var(--accent)] resize-y"
                />
              ) : (
                <pre className="bg-bg border border-border rounded-md px-3 py-2 text-[12px] text-fg-2 mono whitespace-pre-wrap break-words max-h-64 overflow-auto">
                  {record.content}
                </pre>
              )}

              {/* Trace section */}
              {trace && (
                <div>
                  <div className="text-[11px] font-medium text-fg-3 mb-1">{t('memory_console.detail.trace_label')}</div>
                  <div className="bg-bg border border-border rounded-md px-3 py-2 text-[11px] text-fg-4 mono space-y-1">
                    {trace.note && <div className="text-fg-3 italic">{trace.note}</div>}
                    {trace.read_by && trace.read_by.length > 0 && (
                      <div>read_by: {trace.read_by.join(', ')}</div>
                    )}
                    {trace.written_by && trace.written_by.length > 0 && (
                      <div>written_by: {trace.written_by.join(', ')}</div>
                    )}
                    {(!trace.read_by || trace.read_by.length === 0) &&
                     (!trace.written_by || trace.written_by.length === 0) && !trace.note && (
                      <div>No provenance data available.</div>
                    )}
                  </div>
                </div>
              )}
            </>
          )}
        </div>

        {/* Footer */}
        <div className="px-5 py-3 border-t border-border flex items-center gap-2 shrink-0">
          {editing ? (
            <>
              <button
                onClick={handleSave}
                disabled={saving}
                className="px-3 py-1.5 bg-[var(--accent)] text-[var(--accent-fg)] rounded-md text-[12px] font-medium hover:opacity-90 disabled:opacity-50 transition-opacity"
              >
                {saving ? 'Saving…' : t('memory_console.detail.save')}
              </button>
              <button
                onClick={() => { setEditing(false); setContent(record ? record.content : ''); }}
                className="px-3 py-1.5 bg-bg border border-border rounded-md text-[12px] text-fg-2 hover:text-fg transition-colors"
              >
                {t('cancel')}
              </button>
            </>
          ) : (
            <>
              {record && (
                <button
                  onClick={() => setEditing(true)}
                  className="px-3 py-1.5 bg-bg border border-border rounded-md text-[12px] text-fg-2 hover:text-fg transition-colors"
                >
                  {t('memory_console.detail.edit')}
                </button>
              )}
              {record && (
                <button
                  onClick={handleDelete}
                  disabled={deleting}
                  className="px-3 py-1.5 bg-bg border border-rose-500/50 rounded-md text-[12px] text-rose-400 hover:text-rose-300 hover:border-rose-400 disabled:opacity-50 transition-colors"
                >
                  {deleting ? '…' : t('memory_console.action.delete')}
                </button>
              )}
              <button
                onClick={onClose}
                className="px-3 py-1.5 bg-bg border border-border rounded-md text-[12px] text-fg-2 hover:text-fg transition-colors ml-auto"
              >
                {t('close')}
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function fmtTs(ts) {
  if (!ts) return '—';
  try {
    const d = new Date(ts);
    return d.toLocaleString(undefined, {
      month: 'short', day: 'numeric',
      hour: '2-digit', minute: '2-digit',
    });
  } catch {
    return String(ts);
  }
}

// ---------------------------------------------------------------------------
// CreateMemoryDialog (S19 F)
// ---------------------------------------------------------------------------

function CreateMemoryDialog({ lang, defaultLevel, onClose, onCreated }) {
  const t = tFor ? tFor(lang || 'en') : (k) => k;

  const [agentId, setAgentId] = useState('');
  const [level, setLevel]     = useState(defaultLevel || 'L1');
  const [content, setContent] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [validErr, setValidErr] = useState(null);

  const handleCreate = async () => {
    if (!agentId.trim() || !content.trim()) {
      setValidErr(t('memory_console.create.validation_failed'));
      return;
    }
    setValidErr(null);
    setSubmitting(true);
    try {
      await window.cc.api.memory.create({ agent_id: agentId.trim(), level, content: content.trim() });
      window.cc && window.cc.toast && window.cc.toast.success
        ? window.cc.toast.success(t('memory_console.create.success'))
        : alert(t('memory_console.create.success'));
      onCreated();
    } catch (e) {
      setValidErr('Create failed: ' + (e.message || String(e)));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      <div className="absolute inset-0 bg-black/60" onClick={onClose} />
      <div
        className="relative bg-bg-2 border border-border rounded-xl shadow-2xl flex flex-col"
        style={{ width: 520, maxWidth: 'calc(100vw - 48px)', maxHeight: 'calc(100vh - 80px)' }}
      >
        {/* Header */}
        <div className="px-5 py-4 border-b border-border flex items-center justify-between shrink-0">
          <span className="text-[13px] font-medium text-fg">{t('memory_console.create.title')}</span>
          <button onClick={onClose} className="text-fg-4 hover:text-fg">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
          </button>
        </div>

        {/* Body */}
        <div className="flex-1 overflow-auto px-5 py-4 space-y-4">
          {validErr && <div className="text-[12px] text-rose-400">{validErr}</div>}

          {/* Agent ID */}
          <div className="space-y-1">
            <label className="text-[11px] font-medium text-fg-3">{t('memory_console.create.agent_id')}</label>
            <input
              type="text"
              value={agentId}
              onChange={(e) => setAgentId(e.target.value)}
              placeholder="agent-ada"
              className="w-full bg-bg border border-border rounded-md px-3 py-1.5 text-[12px] text-fg outline-none focus:border-[var(--accent)] mono"
            />
          </div>

          {/* Level */}
          <div className="space-y-1">
            <label className="text-[11px] font-medium text-fg-3">{t('memory_console.create.level')}</label>
            <select
              value={level}
              onChange={(e) => setLevel(e.target.value)}
              className="w-full bg-bg border border-border rounded-md px-3 py-1.5 text-[12px] text-fg outline-none focus:border-[var(--accent)]"
            >
              <option value="L0">L0 — Working Memory</option>
              <option value="L1">L1 — Episodic Memory</option>
              <option value="L2">L2 — Procedural Memory</option>
            </select>
          </div>

          {/* Content */}
          <div className="space-y-1">
            <label className="text-[11px] font-medium text-fg-3">{t('memory_console.create.content')}</label>
            <textarea
              value={content}
              onChange={(e) => setContent(e.target.value)}
              rows={8}
              className="w-full bg-bg border border-border rounded-md px-3 py-2 text-[12px] text-fg mono outline-none focus:border-[var(--accent)] resize-y"
            />
          </div>
        </div>

        {/* Footer */}
        <div className="px-5 py-3 border-t border-border flex items-center gap-2 shrink-0">
          <button
            onClick={handleCreate}
            disabled={submitting}
            className="px-3 py-1.5 bg-[var(--accent)] text-[var(--accent-fg)] rounded-md text-[12px] font-medium hover:opacity-90 disabled:opacity-50 transition-opacity"
          >
            {submitting ? '…' : t('memory_console.create.submit')}
          </button>
          <button
            onClick={onClose}
            className="px-3 py-1.5 bg-bg border border-border rounded-md text-[12px] text-fg-2 hover:text-fg transition-colors"
          >
            {t('memory_console.create.cancel')}
          </button>
        </div>
      </div>
    </div>
  );
}

// Expose globally so app.jsx can reference it.
window.MemoryConsolePage = MemoryConsolePage;
