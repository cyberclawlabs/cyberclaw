// pages_clarifications.jsx — Govern · Clarifications tab (S15 Task #15)
// R4.3/R4.4: Read-only audit view for admin. No approve/reject buttons.
// Polls GET /api/v1/chat/clarify/all?since=<iso8601> every 30s.

const { useState: clS, useEffect: clE } = React;

function ClarificationsPage({ lang }) {
  const t = window.tFor ? window.tFor(lang || 'en') : (k, fb) => fb || k;

  const [rows, clSetRows] = clS([]);
  const [loading, clSetLoading] = clS(true);
  const [error, clSetError] = clS(null);
  const [filter, clSetFilter] = clS({ status: 'all', since: '24h' });
  const [selectedRow, clSetSelectedRow] = clS(null);

  clE(() => {
    let cancelled = false;

    const load = async () => {
      try {
        const sinceDate =
          filter.since === '7d'
            ? new Date(Date.now() - 7 * 86400000).toISOString()
            : filter.since === '24h'
            ? new Date(Date.now() - 86400000).toISOString()
            : '2020-01-01T00:00:00Z';
        const data = await window.cc.api.chat.fetchAllClarifications(sinceDate);
        if (cancelled) return;
        const arr = Array.isArray(data) ? data : [];
        const filtered =
          filter.status === 'all'
            ? arr
            : arr.filter((r) => r.status === filter.status);
        clSetRows(filtered);
        clSetError(null);
      } catch (e) {
        if (!cancelled) clSetError(String(e && e.message ? e.message : e));
      } finally {
        if (!cancelled) clSetLoading(false);
      }
    };

    clSetLoading(true);
    load();
    const timer = setInterval(load, 30000);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [filter]);

  const statusBadge = (status) => {
    const styles = {
      pending: {
        bg: 'var(--amber-soft,rgba(251,191,36,0.12))',
        color: 'var(--color-amber-400,#fbbf24)',
        label: t('clarify.govern.status.pending') || 'Pending',
        dot: '●',
      },
      resolved: {
        bg: 'var(--emerald-soft,rgba(52,211,153,0.12))',
        color: 'var(--color-emerald-400,#34d399)',
        label: t('clarify.govern.status.resolved') || 'Resolved',
        dot: '✓',
      },
      timeout: {
        bg: 'var(--bg-3)',
        color: 'var(--fg-3)',
        label: t('clarify.govern.status.timeout') || 'Timed out',
        dot: '⏳',
      },
    };
    const s = styles[status] || styles.timeout;
    return (
      <span
        style={{
          display: 'inline-flex',
          alignItems: 'center',
          gap: 4,
          padding: '2px 8px',
          borderRadius: 6,
          fontSize: 11,
          fontWeight: 600,
          background: s.bg,
          color: s.color,
          fontFamily: 'var(--font-mono, monospace)',
        }}
      >
        {s.dot} {s.label}
      </span>
    );
  };

  const fmtTime = (iso) => {
    if (!iso) return '—';
    try {
      return new Date(iso).toLocaleString();
    } catch {
      return iso;
    }
  };

  const shortId = (id) => {
    if (!id) return '—';
    return id.length > 8 ? id.slice(0, 8) + '…' : id;
  };

  const questionSnippet = (row) => {
    const q =
      row.questions &&
      row.questions[0] &&
      (row.questions[0].question || row.questions[0].text || '');
    return q ? q.slice(0, 60) + (q.length > 60 ? '…' : '') : '—';
  };

  return (
    <div>
      {/* Header row: title + read-only badge + filters */}
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 12,
          marginBottom: 16,
          flexWrap: 'wrap',
        }}
      >
        <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          <span
            style={{
              fontSize: 13,
              fontWeight: 600,
              color: 'var(--fg)',
            }}
          >
            {t('clarify.govern.title') || 'Clarifications'}
          </span>
          {/* R4.4 visual label */}
          <span
            style={{
              fontSize: 10,
              padding: '2px 7px',
              borderRadius: 5,
              background: 'var(--bg-3)',
              border: '1px solid var(--border)',
              color: 'var(--fg-3)',
              fontFamily: 'var(--font-mono, monospace)',
              fontWeight: 500,
            }}
          >
            {t('clarify.govern.readonly_label') || 'Clarify · read-only'}
          </span>
        </div>

        <div style={{ flex: 1 }} />

        {/* Since filter */}
        <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
          <span style={{ fontSize: 11, color: 'var(--fg-3)' }}>
            {t('clarify.govern.filter_since') || 'Since'}
          </span>
          <select
            value={filter.since}
            onChange={(e) =>
              clSetFilter((f) => ({ ...f, since: e.target.value }))
            }
            style={{
              fontSize: 12,
              padding: '3px 8px',
              borderRadius: 6,
              background: 'var(--bg-3)',
              border: '1px solid var(--border)',
              color: 'var(--fg)',
              cursor: 'pointer',
            }}
          >
            <option value="all">All</option>
            <option value="24h">Last 24h</option>
            <option value="7d">Last 7d</option>
          </select>
        </div>

        {/* Status filter */}
        <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
          <span style={{ fontSize: 11, color: 'var(--fg-3)' }}>
            {t('clarify.govern.filter_status') || 'Status'}
          </span>
          <select
            value={filter.status}
            onChange={(e) =>
              clSetFilter((f) => ({ ...f, status: e.target.value }))
            }
            style={{
              fontSize: 12,
              padding: '3px 8px',
              borderRadius: 6,
              background: 'var(--bg-3)',
              border: '1px solid var(--border)',
              color: 'var(--fg)',
              cursor: 'pointer',
            }}
          >
            <option value="all">All</option>
            <option value="pending">
              {t('clarify.govern.status.pending') || 'Pending'}
            </option>
            <option value="resolved">
              {t('clarify.govern.status.resolved') || 'Resolved'}
            </option>
            <option value="timeout">
              {t('clarify.govern.status.timeout') || 'Timed out'}
            </option>
          </select>
        </div>
      </div>

      {/* Error state */}
      {error && (
        <div
          style={{
            padding: '10px 14px',
            borderRadius: 8,
            background: 'rgba(248,113,113,0.08)',
            border: '1px solid rgba(248,113,113,0.2)',
            color: 'var(--color-rose-400,#f87171)',
            fontSize: 12,
            marginBottom: 12,
          }}
        >
          {error}
        </div>
      )}

      {/* Loading */}
      {loading && rows.length === 0 && (
        <div
          style={{
            padding: '40px 0',
            textAlign: 'center',
            fontSize: 12,
            color: 'var(--fg-4)',
          }}
        >
          …
        </div>
      )}

      {/* Empty state */}
      {!loading && rows.length === 0 && !error && (
        <div
          style={{
            padding: '48px 0',
            textAlign: 'center',
            color: 'var(--fg-4)',
            fontSize: 13,
          }}
        >
          {t('clarify.govern.empty') || 'No clarifications yet'}
        </div>
      )}

      {/* Table */}
      {rows.length > 0 && (
        <div
          style={{
            background: 'var(--bg-2)',
            border: '1px solid var(--border)',
            borderRadius: 10,
            overflow: 'hidden',
          }}
        >
          <table
            style={{
              width: '100%',
              borderCollapse: 'collapse',
              fontSize: 12,
            }}
          >
            <thead>
              <tr
                style={{
                  background: 'var(--bg-3)',
                  borderBottom: '1px solid var(--border)',
                }}
              >
                {[
                  'clarify_id',
                  'conversation_id',
                  'agent_id',
                  'question',
                  'status',
                  'created_at',
                  'resolved_at',
                ].map((col) => (
                  <th
                    key={col}
                    style={{
                      padding: '8px 12px',
                      textAlign: 'left',
                      fontWeight: 600,
                      color: 'var(--fg-3)',
                      fontFamily: 'var(--font-mono, monospace)',
                      fontSize: 10,
                      textTransform: 'uppercase',
                      letterSpacing: '0.08em',
                      whiteSpace: 'nowrap',
                    }}
                  >
                    {col}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {rows.map((row, idx) => (
                <tr
                  key={row.clarify_id || idx}
                  onClick={() => clSetSelectedRow(row)}
                  style={{
                    borderBottom:
                      idx < rows.length - 1
                        ? '1px solid var(--border)'
                        : 'none',
                    cursor: 'pointer',
                    transition: 'background 0.1s',
                  }}
                  onMouseEnter={(e) =>
                    (e.currentTarget.style.background = 'var(--hover)')
                  }
                  onMouseLeave={(e) =>
                    (e.currentTarget.style.background = 'transparent')
                  }
                >
                  <td
                    style={{ padding: '9px 12px', fontFamily: 'var(--font-mono, monospace)' }}
                    title={row.clarify_id || ''}
                  >
                    {shortId(row.clarify_id)}
                  </td>
                  <td
                    style={{ padding: '9px 12px', fontFamily: 'var(--font-mono, monospace)' }}
                    title={row.conversation_id || ''}
                  >
                    {shortId(row.conversation_id)}
                  </td>
                  <td
                    style={{ padding: '9px 12px', color: 'var(--fg-2)', fontFamily: 'var(--font-mono, monospace)' }}
                    title={row.agent_id || ''}
                  >
                    {row.agent_id ? row.agent_id.slice(0, 12) + (row.agent_id.length > 12 ? '…' : '') : '—'}
                  </td>
                  <td style={{ padding: '9px 12px', color: 'var(--fg-2)', maxWidth: 240 }}>
                    <span style={{ display: 'block', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                      {questionSnippet(row)}
                    </span>
                  </td>
                  <td style={{ padding: '9px 12px' }}>
                    {statusBadge(row.status)}
                  </td>
                  <td style={{ padding: '9px 12px', color: 'var(--fg-3)', whiteSpace: 'nowrap' }}>
                    {fmtTime(row.created_at)}
                  </td>
                  <td style={{ padding: '9px 12px', color: 'var(--fg-3)', whiteSpace: 'nowrap' }}>
                    {fmtTime(row.resolved_at)}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {/* Detail Dialog */}
      {selectedRow && (
        <ClarificationsDetailDialog
          row={selectedRow}
          onClose={() => clSetSelectedRow(null)}
          t={t}
          statusBadge={statusBadge}
          fmtTime={fmtTime}
        />
      )}
    </div>
  );
}

function ClarificationsDetailDialog({ row, onClose, t, statusBadge, fmtTime }) {
  // Close on backdrop click or Escape
  clE(() => {
    const onKey = (e) => { if (e.key === 'Escape') onClose(); };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  const questions = Array.isArray(row.questions) ? row.questions : [];
  const answers = row.answers || {};

  return (
    <div
      style={{
        position: 'fixed',
        inset: 0,
        zIndex: 60,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
      }}
    >
      {/* Backdrop */}
      <div
        style={{
          position: 'absolute',
          inset: 0,
          background: 'rgba(0,0,0,0.55)',
        }}
        onClick={onClose}
      />

      {/* Dialog */}
      <div
        style={{
          position: 'relative',
          background: 'var(--bg-2)',
          border: '1px solid var(--border)',
          borderRadius: 12,
          padding: '20px 24px',
          width: 560,
          maxWidth: 'calc(100vw - 32px)',
          maxHeight: '80vh',
          overflowY: 'auto',
          boxShadow: '0 24px 64px rgba(0,0,0,0.4)',
        }}
      >
        {/* Dialog header */}
        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 10,
            marginBottom: 16,
          }}
        >
          <span style={{ fontSize: 14, fontWeight: 600, color: 'var(--fg)' }}>
            {t('clarify.govern.title') || 'Clarification'}
          </span>
          {statusBadge(row.status)}
          {/* R4.4 read-only label */}
          <span
            style={{
              fontSize: 10,
              padding: '2px 7px',
              borderRadius: 5,
              background: 'var(--bg-3)',
              border: '1px solid var(--border)',
              color: 'var(--fg-4)',
              fontFamily: 'var(--font-mono, monospace)',
            }}
          >
            {t('clarify.govern.readonly_label') || 'Clarify · read-only'}
          </span>
          <div style={{ flex: 1 }} />
          <button
            onClick={onClose}
            style={{
              background: 'transparent',
              border: 'none',
              cursor: 'pointer',
              color: 'var(--fg-3)',
              fontSize: 18,
              lineHeight: 1,
              padding: '2px 6px',
            }}
          >
            ×
          </button>
        </div>

        {/* Meta */}
        <div
          style={{
            display: 'grid',
            gridTemplateColumns: '1fr 1fr',
            gap: '6px 16px',
            marginBottom: 16,
            fontSize: 11,
            color: 'var(--fg-3)',
            fontFamily: 'var(--font-mono, monospace)',
          }}
        >
          <span>clarify_id: <span style={{ color: 'var(--fg)' }}>{row.clarify_id || '—'}</span></span>
          <span>conversation_id: <span style={{ color: 'var(--fg)' }}>{row.conversation_id || '—'}</span></span>
          <span>agent_id: <span style={{ color: 'var(--fg)' }}>{row.agent_id || '—'}</span></span>
          <span>source: <span style={{ color: 'var(--fg)' }}>{row.source || '—'}</span></span>
          <span>created_at: <span style={{ color: 'var(--fg)' }}>{fmtTime(row.created_at)}</span></span>
          <span>resolved_at: <span style={{ color: 'var(--fg)' }}>{fmtTime(row.resolved_at)}</span></span>
          {row.expires_at && (
            <span>expires_at: <span style={{ color: 'var(--fg)' }}>{fmtTime(row.expires_at)}</span></span>
          )}
        </div>

        {/* Questions + Answers */}
        {questions.length === 0 && (
          <div style={{ color: 'var(--fg-4)', fontSize: 12, fontStyle: 'italic' }}>
            No questions recorded.
          </div>
        )}
        {questions.map((q, qi) => {
          const qText = q.question || q.text || '';
          const answerVal =
            answers[qText] ||
            (row.answer && row.answer.answers && row.answer.answers[qText]) ||
            null;
          return (
            <div
              key={qi}
              style={{
                marginBottom: 14,
                background: 'var(--bg-3)',
                border: '1px solid var(--border)',
                borderRadius: 8,
                padding: '12px 14px',
              }}
            >
              <div
                style={{
                  fontSize: 10,
                  color: 'var(--fg-4)',
                  fontFamily: 'var(--font-mono, monospace)',
                  marginBottom: 6,
                  textTransform: 'uppercase',
                  letterSpacing: '0.08em',
                }}
              >
                {t('clarify.govern.question_label') || 'Question'} {qi + 1}
              </div>
              <div style={{ fontSize: 13, color: 'var(--fg)', marginBottom: answerVal ? 10 : 0 }}>
                {qText || '—'}
              </div>
              {q.options && q.options.length > 0 && (
                <div style={{ display: 'flex', flexWrap: 'wrap', gap: 6, marginTop: 6 }}>
                  {q.options.map((opt, oi) => {
                    const label = typeof opt === 'string' ? opt : opt.label || '';
                    const isSelected = answerVal === label;
                    return (
                      <span
                        key={oi}
                        style={{
                          fontSize: 11,
                          padding: '2px 8px',
                          borderRadius: 5,
                          background: isSelected ? 'var(--accent-soft)' : 'var(--bg-2)',
                          border: isSelected
                            ? '1px solid var(--accent-ring)'
                            : '1px solid var(--border)',
                          color: isSelected ? 'var(--accent)' : 'var(--fg-2)',
                          fontWeight: isSelected ? 600 : 400,
                        }}
                      >
                        {label}
                      </span>
                    );
                  })}
                </div>
              )}
              {answerVal && (
                <div
                  style={{
                    marginTop: 8,
                    padding: '6px 10px',
                    background: 'var(--bg-2)',
                    borderRadius: 6,
                    fontSize: 12,
                    color: 'var(--fg)',
                  }}
                >
                  <span style={{ color: 'var(--fg-4)', fontSize: 10, fontFamily: 'var(--font-mono, monospace)', marginRight: 6 }}>
                    {t('clarify.govern.answer_label') || 'Answer'}:
                  </span>
                  {answerVal}
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

window.ClarificationsPage = ClarificationsPage;
