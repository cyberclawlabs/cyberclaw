// pages_handoffs.jsx — Govern · Handoffs tab (Sprint 21 T13)
// Read-only audit view of agent→agent conversation transfers.
// Polls GET /api/v1/chat/handoff?limit=50 every 30s.
// Click row → detail dialog with full briefing_text.

const { useState: hdS, useEffect: hdE } = React;

function HandoffsPage({ lang }) {
  const t = window.tFor ? window.tFor(lang || 'en') : (k, fb) => fb || k;

  const [rows, hdSetRows] = hdS([]);
  const [loading, hdSetLoading] = hdS(true);
  const [error, hdSetError] = hdS(null);
  const [selectedRow, hdSetSelectedRow] = hdS(null);

  hdE(() => {
    let cancelled = false;

    const load = async () => {
      try {
        const data = await window.cc.api.handoff.list({ limit: 50 });
        if (cancelled) return;
        hdSetRows(Array.isArray(data.handoffs) ? data.handoffs : []);
        hdSetError(null);
      } catch (e) {
        if (!cancelled) hdSetError(String(e && e.message ? e.message : e));
      } finally {
        if (!cancelled) hdSetLoading(false);
      }
    };

    hdSetLoading(true);
    load();
    const timer = setInterval(load, 30000);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, []);

  const statusBadge = (status) => {
    const styles = {
      initiated: {
        bg: 'rgba(99,102,241,0.12)',
        color: 'var(--color-indigo-400,#818cf8)',
        label: t('handoffs.status.initiated') || 'Initiated',
        dot: '◎',
      },
      authorized: {
        bg: 'var(--amber-soft,rgba(251,191,36,0.12))',
        color: 'var(--color-amber-400,#fbbf24)',
        label: t('handoffs.status.authorized') || 'Authorized',
        dot: '●',
      },
      accepted: {
        bg: 'var(--emerald-soft,rgba(52,211,153,0.12))',
        color: 'var(--color-emerald-400,#34d399)',
        label: t('handoffs.status.accepted') || 'Accepted',
        dot: '✓',
      },
      declined: {
        bg: 'rgba(248,113,113,0.12)',
        color: 'var(--color-rose-400,#f87171)',
        label: t('handoffs.status.declined') || 'Declined',
        dot: '✕',
      },
      expired: {
        bg: 'var(--bg-3)',
        color: 'var(--fg-3)',
        label: t('handoffs.status.expired') || 'Expired',
        dot: '⏳',
      },
    };
    const s = styles[status] || styles.expired;
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
    return id.length > 12 ? id.slice(0, 12) + '…' : id;
  };

  return (
    <div>
      {/* Header row */}
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
          <span style={{ fontSize: 13, fontWeight: 600, color: 'var(--fg)' }}>
            {t('handoffs.page.title') || 'Handoffs'}
          </span>
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
            {t('handoffs.page.subtitle') || 'read-only'}
          </span>
        </div>

        <div style={{ flex: 1 }} />

        <button
          onClick={() => {
            // Trigger a manual refresh by remounting via key change isn't
            // available here; instead we reload rows directly.
            window.cc.api.handoff
              .list({ limit: 50 })
              .then((data) => {
                hdSetRows(Array.isArray(data.handoffs) ? data.handoffs : []);
                hdSetError(null);
              })
              .catch((e) => hdSetError(String(e && e.message ? e.message : e)));
          }}
          style={{
            fontSize: 11,
            padding: '4px 10px',
            borderRadius: 6,
            background: 'var(--bg-3)',
            border: '1px solid var(--border)',
            color: 'var(--fg-2)',
            cursor: 'pointer',
            fontFamily: 'var(--font-mono, monospace)',
          }}
        >
          ↻ {t('refresh') || 'Refresh'}
        </button>
      </div>

      {/* Error banner */}
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
          {t('handoffs.empty') || 'No handoffs recorded yet.'}
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
                  { key: 'icon', label: '🔀' },
                  { key: 'from', label: t('handoffs.col.from') || 'From' },
                  { key: 'to', label: t('handoffs.col.to') || 'To' },
                  { key: 'reason', label: t('handoffs.col.reason') || 'Reason' },
                  { key: 'status', label: t('handoffs.col.status') || 'Status' },
                  { key: 'initiated_at', label: t('handoffs.col.initiated_at') || 'Initiated' },
                ].map((col) => (
                  <th
                    key={col.key}
                    style={{
                      padding: '8px 12px',
                      textAlign: 'left',
                      fontWeight: 600,
                      color: 'var(--fg-3)',
                      fontFamily: 'var(--font-mono, monospace)',
                      fontSize: 10,
                      textTransform: col.key === 'icon' ? 'none' : 'uppercase',
                      letterSpacing: '0.08em',
                      whiteSpace: 'nowrap',
                    }}
                  >
                    {col.label}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {rows.map((row, idx) => (
                <tr
                  key={row.handoff_id || idx}
                  onClick={() => hdSetSelectedRow(row)}
                  style={{
                    borderBottom:
                      idx < rows.length - 1 ? '1px solid var(--border)' : 'none',
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
                  <td style={{ padding: '9px 12px', color: 'var(--fg-4)', fontSize: 14 }}>
                    🔀
                  </td>
                  <td
                    style={{
                      padding: '9px 12px',
                      fontFamily: 'var(--font-mono, monospace)',
                      color: 'var(--fg-2)',
                    }}
                    title={row.from_agent_id || ''}
                  >
                    {shortId(row.from_agent_id)}
                  </td>
                  <td
                    style={{
                      padding: '9px 12px',
                      fontFamily: 'var(--font-mono, monospace)',
                      color: 'var(--fg-2)',
                    }}
                    title={row.to_agent_id || ''}
                  >
                    {shortId(row.to_agent_id)}
                  </td>
                  <td
                    style={{
                      padding: '9px 12px',
                      color: 'var(--fg-2)',
                      maxWidth: 220,
                    }}
                  >
                    <span
                      style={{
                        display: 'block',
                        overflow: 'hidden',
                        textOverflow: 'ellipsis',
                        whiteSpace: 'nowrap',
                      }}
                    >
                      {row.reason
                        ? row.reason.slice(0, 60) + (row.reason.length > 60 ? '…' : '')
                        : '—'}
                    </span>
                  </td>
                  <td style={{ padding: '9px 12px' }}>
                    {statusBadge(row.status)}
                  </td>
                  <td
                    style={{
                      padding: '9px 12px',
                      color: 'var(--fg-3)',
                      whiteSpace: 'nowrap',
                    }}
                  >
                    {fmtTime(row.initiated_at)}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {/* Detail Dialog */}
      {selectedRow && (
        <HandoffDetailDialog
          row={selectedRow}
          onClose={() => hdSetSelectedRow(null)}
          t={t}
          statusBadge={statusBadge}
          fmtTime={fmtTime}
        />
      )}
    </div>
  );
}

function HandoffDetailDialog({ row, onClose, t, statusBadge, fmtTime }) {
  hdE(() => {
    const onKey = (e) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

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
          width: 600,
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
            {t('handoffs.detail.title') || 'Handoff detail'}
          </span>
          {statusBadge(row.status)}
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

        {/* Meta fields */}
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
          <span>
            handoff_id:{' '}
            <span style={{ color: 'var(--fg)' }}>{row.handoff_id || '—'}</span>
          </span>
          <span>
            status:{' '}
            <span style={{ color: 'var(--fg)' }}>{row.status || '—'}</span>
          </span>
          <span>
            from:{' '}
            <span style={{ color: 'var(--fg)' }}>{row.from_agent_id || '—'}</span>
          </span>
          <span>
            to:{' '}
            <span style={{ color: 'var(--fg)' }}>{row.to_agent_id || '—'}</span>
          </span>
          <span>
            initiated_at:{' '}
            <span style={{ color: 'var(--fg)' }}>{fmtTime(row.initiated_at)}</span>
          </span>
          <span>
            decided_at:{' '}
            <span style={{ color: 'var(--fg)' }}>{fmtTime(row.decided_at)}</span>
          </span>
          {row.reason && (
            <span style={{ gridColumn: '1 / -1' }}>
              reason:{' '}
              <span style={{ color: 'var(--fg)' }}>{row.reason}</span>
            </span>
          )}
        </div>

        {/* Briefing text */}
        <div
          style={{
            marginBottom: 16,
          }}
        >
          <div
            style={{
              fontSize: 10,
              color: 'var(--fg-4)',
              fontFamily: 'var(--font-mono, monospace)',
              textTransform: 'uppercase',
              letterSpacing: '0.08em',
              marginBottom: 8,
            }}
          >
            {t('handoffs.detail.briefing_label') || 'Briefing'}
          </div>
          {row.briefing_text ? (
            <pre
              style={{
                margin: 0,
                padding: '12px 14px',
                background: 'var(--bg-3)',
                border: '1px solid var(--border)',
                borderRadius: 8,
                fontSize: 12,
                color: 'var(--fg)',
                fontFamily: 'var(--font-mono, monospace)',
                whiteSpace: 'pre-wrap',
                wordBreak: 'break-word',
                overflowX: 'auto',
                lineHeight: 1.6,
              }}
            >
              {row.briefing_text}
            </pre>
          ) : (
            <div
              style={{
                padding: '12px 14px',
                background: 'var(--bg-3)',
                border: '1px solid var(--border)',
                borderRadius: 8,
                fontSize: 12,
                color: 'var(--fg-4)',
                fontStyle: 'italic',
              }}
            >
              —
            </div>
          )}
        </div>

        {/* Close button */}
        <div style={{ display: 'flex', justifyContent: 'flex-end' }}>
          <button
            onClick={onClose}
            style={{
              fontSize: 12,
              padding: '6px 16px',
              borderRadius: 6,
              background: 'var(--bg-3)',
              border: '1px solid var(--border)',
              color: 'var(--fg-2)',
              cursor: 'pointer',
              fontFamily: 'var(--font-mono, monospace)',
            }}
          >
            {t('handoffs.detail.close') || 'Close'}
          </button>
        </div>
      </div>
    </div>
  );
}

window.HandoffsPage = HandoffsPage;
