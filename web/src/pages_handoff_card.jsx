// Sprint 21 T11 — HandoffCard component
// Renders role="handoff" message as an inline chat card.
//
// Message payload shape (from ChatMessage::handoff_card in T2):
//   metadata: {
//     handoff_id, from_agent_id, to_agent_id, reason,
//     briefing_preview, briefing_truncated, initiated_at, decided_at
//   }

(function () {
  const { useState } = React;

  function ccInterp(str, params) {
    let out = str || '';
    Object.keys(params || {}).forEach(k => {
      out = out.replace('{' + k + '}', params[k]);
    });
    return out;
  }

  function fmtTsHandoff(ts) {
    if (!ts) return '';
    try {
      const d = typeof ts === 'string' ? new Date(ts) : new Date(ts * 1000);
      return d.toLocaleString();
    } catch { return ''; }
  }

  function HandoffCard({ payload, lang }) {
    const t = window.tFor ? window.tFor(lang || 'en') : (k) => k;
    const [expanded, setExpanded] = useState(false);

    if (!payload) return null;

    const hasBriefing = payload.briefing_preview && payload.briefing_preview.length > 0;

    return (
      <div
        role="note"
        aria-label="Agent handoff"
        style={{
          background: 'rgba(99, 102, 241, 0.06)',
          border: '1px solid rgba(99, 102, 241, 0.20)',
          borderLeft: '3px solid rgba(99, 102, 241, 0.50)',
          borderRadius: 12,
          padding: '10px 14px',
          margin: '8px 0',
          fontSize: 13,
        }}
      >
        {/* Header row */}
        <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: payload.reason ? 6 : 0 }}>
          <span style={{ fontSize: 15 }}>🔀</span>
          <span style={{ fontWeight: 600, color: 'var(--fg)', flex: 1 }}>
            {ccInterp(t('handoff.card.title'), {
              from: payload.from_agent_id || '?',
              to: payload.to_agent_id || '?',
            })}
          </span>
          {(payload.decided_at || payload.initiated_at) && (
            <span
              style={{ fontSize: 11, color: 'var(--fg-4)', whiteSpace: 'nowrap', fontFamily: 'var(--font-mono, monospace)' }}
              title={fmtTsHandoff(payload.decided_at || payload.initiated_at)}
            >
              {fmtTsHandoff(payload.decided_at || payload.initiated_at)}
            </span>
          )}
        </div>

        {/* Reason */}
        {payload.reason && (
          <div style={{ marginTop: 4, fontSize: 12, color: 'var(--fg-2)' }}>
            <span style={{ color: 'var(--fg-4)' }}>{t('handoff.card.reason')}:</span>{' '}
            {payload.reason}
          </div>
        )}

        {/* Collapsible briefing */}
        {hasBriefing && (
          <div style={{ marginTop: 6 }}>
            <button
              onClick={() => setExpanded(e => !e)}
              type="button"
              style={{
                background: 'none',
                border: 'none',
                padding: 0,
                fontSize: 11,
                color: 'var(--accent)',
                cursor: 'pointer',
                textDecoration: 'underline',
              }}
            >
              {expanded ? t('handoff.card.collapse') : t('handoff.card.expand')}
            </button>
            {expanded && (
              <div
                style={{
                  marginTop: 6,
                  fontSize: 11,
                  color: 'var(--fg-3)',
                  whiteSpace: 'pre-wrap',
                  fontFamily: 'var(--font-mono, monospace)',
                  background: 'var(--bg-2)',
                  border: '1px solid var(--border)',
                  borderRadius: 6,
                  padding: '8px 10px',
                  maxHeight: 200,
                  overflowY: 'auto',
                }}
              >
                {payload.briefing_preview}
                {payload.briefing_truncated && (
                  <div style={{ marginTop: 4, color: 'var(--fg-4)', fontStyle: 'italic' }}>
                    [truncated — full briefing available via audit]
                  </div>
                )}
              </div>
            )}
          </div>
        )}
      </div>
    );
  }

  window.HandoffCard = HandoffCard;
})();
