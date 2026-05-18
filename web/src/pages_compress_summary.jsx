// S16-T7: CompressSummary card — renders inline in message list for role="summary" messages.
// Registered as window.CompressSummary.
//
// Props:
//   originalCount  : number   — how many messages were compressed
//   compressedAt   : string   — ISO 8601 timestamp
//   stagesApplied  : string[] — e.g. ['PruneToolResults', 'SummarizeEarly']
//   summaryText    : string   — markdown body of the summary message
//   lang           : string   — current UI locale (passed from ChatPage)

(function () {
  const { useState, useMemo } = React;

  function escapeHtml(str) {
    return String(str)
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;');
  }

  function CompressSummary({ originalCount, compressedAt, stagesApplied, summaryText, lang }) {
    const t = window.tFor ? window.tFor(lang || 'en') : (k, vars) => {
      let s = k;
      if (vars) {
        for (const [vk, vv] of Object.entries(vars)) {
          s = s.replace(new RegExp('\\{' + vk + '\\}', 'g'), vv);
        }
      }
      return s;
    };

    const [expanded, setExpanded] = useState(false);

    const renderedHtml = useMemo(() => {
      if (!summaryText) return '';
      if (typeof window.marked === 'object' && typeof window.marked.parse === 'function') {
        try { return window.marked.parse(summaryText); } catch {}
      }
      // Fallback: escape + line-break conversion
      return escapeHtml(summaryText).replace(/\n/g, '<br/>');
    }, [summaryText]);

    const timestampReadable = useMemo(() => {
      if (!compressedAt) return '';
      try { return new Date(compressedAt).toLocaleString(); } catch { return compressedAt; }
    }, [compressedAt]);

    const stagesText = useMemo(() => {
      return (stagesApplied || [])
        .map(s => s.replace(/([A-Z])/g, ' $1').trim())
        .join(' • ');
    }, [stagesApplied]);

    const longBody = summaryText && summaryText.length > 200;

    return (
      <div
        className="compress-summary-card"
        role="note"
        aria-label={t('compress.summary.header', { count: originalCount || 0 })}
        style={{
          background: 'rgba(250, 204, 21, 0.08)',
          border: '1px solid rgba(250, 204, 21, 0.25)',
          borderLeft: '3px solid rgba(250, 204, 21, 0.6)',
          borderRadius: 12,
          padding: '10px 14px',
          margin: '8px 0',
          fontSize: 13,
        }}
      >
        {/* Header */}
        <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 6 }}>
          <span style={{ fontSize: 15 }}>⚡</span>
          <span style={{ fontWeight: 600, color: 'var(--fg)', flex: 1 }}>
            {t('compress.summary.header', { count: originalCount || 0 })}
          </span>
          {timestampReadable && (
            <span
              style={{ fontSize: 11, color: 'var(--fg-4)', whiteSpace: 'nowrap' }}
              title={timestampReadable}
            >
              {timestampReadable}
            </span>
          )}
        </div>

        {/* Body */}
        <div
          className={'compress-summary-body markdown-body' + (expanded ? ' expanded' : ' collapsed')}
          dangerouslySetInnerHTML={{
            __html: renderedHtml || '<p><em>' + escapeHtml(t('compress.summary.no_text')) + '</em></p>',
          }}
          style={
            !expanded && longBody
              ? {
                  maxHeight: '4.5em',
                  overflow: 'hidden',
                  WebkitMaskImage: 'linear-gradient(to bottom, black 60%, transparent 100%)',
                  maskImage: 'linear-gradient(to bottom, black 60%, transparent 100%)',
                }
              : {}
          }
        />

        {/* Expand / collapse toggle */}
        {longBody && (
          <button
            className="compress-summary-toggle"
            onClick={() => setExpanded(e => !e)}
            style={{
              marginTop: 4,
              background: 'none',
              border: 'none',
              padding: 0,
              fontSize: 11,
              color: 'var(--accent)',
              cursor: 'pointer',
              textDecoration: 'underline',
            }}
          >
            {expanded ? t('compress.summary.collapse') : t('compress.summary.expand')}
          </button>
        )}

        {/* Stages row */}
        {stagesText && (
          <div style={{ marginTop: 6 }}>
            <small style={{ fontSize: 11, color: 'var(--fg-4)' }}>
              {t('compress.summary.stages_label')}: {stagesText}
            </small>
          </div>
        )}
      </div>
    );
  }

  window.CompressSummary = CompressSummary;
})();
