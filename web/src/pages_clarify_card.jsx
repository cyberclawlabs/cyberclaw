// pages_clarify_card.jsx — ClarifyCard composer flyout component
// Sprint 15 S15-T13: Clarify card (hermes-webui composer flyout pattern)
// Sprint 19 S19-C: v1.1 — multi-question ordering, multiSelect checkbox, preview side-panel
//
// Exported as window.ClarifyCard for pages_chat.jsx to mount.
//
// Props:
//   clarifyId      {string}   — clarify request id
//   clarify        {object}   — full ClarifyRequest {id, questions: [{question, options, multi_select}]}
//   // Legacy v1 compat props (single-question mode):
//   question       {string}   — questions[0].question (ignored when clarify provided)
//   options        {Array}    — questions[0].options  (ignored when clarify provided)
//   allowFreeform  {boolean}  — v1 always false; Other button auto-appended
//   status         {string}   — 'pending' | 'resolved' | 'timeout'
//   resolvedAnswer {object}   — answer payload when status=resolved
//   onSubmit       {function} — async (answer) => void
//                               answer = {answers: {questionText: pickedLabel, ...}}

const { useState: ccS, useEffect: ccE, useRef: ccR, useCallback: ccC, useMemo: ccM } = React;

// i18n helper (same pattern as pages_chat.jsx tFor)
function ccT(lang) {
  const t = window.tFor ? window.tFor(lang) : (k, fb) => fb || k;
  return t;
}

// Simple template interpolation: "Question {current} of {total}" → "Question 1 of 3"
function ccInterp(str, vars) {
  if (!str || !vars) return str || '';
  return str.replace(/\{(\w+)\}/g, (_, k) => (vars[k] !== undefined ? vars[k] : '{' + k + '}'));
}

// Render markdown using window.marked (S14-6 CDN)
function renderMarkdown(text) {
  if (!text) return '';
  try {
    if (!window.marked) return text;
    return window.marked.parse(text);
  } catch {
    return text;
  }
}

// ── Option button (single-select) ──────────────────────────────────────────────
function SingleOptionButton({ opt, idx, isSelected, isSubmitting, isFocused, onPick, onFocus }) {
  const hasRecommended = idx === 0 && /\(recommended\)/i.test(opt.label);
  const labelText = hasRecommended
    ? opt.label.replace(/\s*\(recommended\)\s*/i, '').trim()
    : opt.label;

  return (
    <button
      disabled={isSubmitting}
      title={opt.description || undefined}
      onClick={() => onPick(idx)}
      onMouseEnter={() => onFocus && onFocus(idx)}
      onFocus={() => onFocus && onFocus(idx)}
      style={{
        display: 'flex', alignItems: 'center', gap: 10,
        padding: '8px 12px',
        background: isSelected ? 'var(--accent-soft)' : isFocused ? 'var(--bg-3)' : 'var(--bg-3)',
        border: isSelected
          ? '1px solid var(--accent-ring)'
          : isFocused
            ? '1px solid var(--accent-ring,var(--border))'
            : '1px solid var(--border)',
        borderRadius: 8,
        cursor: isSubmitting ? 'not-allowed' : 'pointer',
        opacity: isSubmitting ? 0.6 : 1,
        textAlign: 'left',
        transition: 'background 0.12s, border-color 0.12s',
        outline: 'none',
      }}
    >
      <span style={{
        minWidth: 22, height: 22, borderRadius: 6,
        background: 'var(--bg-2)', border: '1px solid var(--border)',
        display: 'flex', alignItems: 'center', justifyContent: 'center',
        fontSize: 11, fontFamily: 'var(--font-mono, monospace)',
        color: 'var(--fg-3)', flexShrink: 0,
      }}>
        {idx + 1}
      </span>
      <span style={{ flex: 1, fontSize: 13, color: 'var(--fg)', fontWeight: idx === 0 ? 500 : 400 }}>
        {labelText}
      </span>
      {hasRecommended && (
        <span style={{
          fontSize: 10, padding: '2px 6px', borderRadius: 4,
          background: 'var(--accent-soft)', color: 'var(--accent)',
          fontWeight: 600, flexShrink: 0,
        }}>
          Recommended
        </span>
      )}
      {opt.description && !hasRecommended && (
        <span
          title={opt.description}
          style={{
            width: 6, height: 6, borderRadius: '50%',
            background: 'var(--fg-4)', flexShrink: 0,
            cursor: 'help',
          }}
        />
      )}
    </button>
  );
}

// ── Single question panel ──────────────────────────────────────────────────────
// Handles both single-select (pill buttons) and multi-select (checkboxes).
// Also handles the preview side-panel layout when any option has a preview field.
function QuestionPanel({ question, isSubmitting, isLast, onAnswer, lang, t }) {
  const opts = question.options || [];
  const isMulti = !!question.multi_select;
  const hasPreview = !isMulti && opts.some(o => o.preview);

  // Single-select state
  const [selectedIdx, setSelectedIdx] = ccS(null);
  // Multi-select state
  const [checkedSet, setCheckedSet] = ccS(new Set());
  // Freeform state
  const [showFreeform, setShowFreeform] = ccS(false);
  const [freeformText, setFreeformText] = ccS('');
  const freeformRef = ccR(null);
  // Preview focus state (single-select + hasPreview only)
  const [focusedIdx, setFocusedIdx] = ccS(null);

  // Reset state when question changes
  ccE(() => {
    setSelectedIdx(null);
    setCheckedSet(new Set());
    setShowFreeform(false);
    setFreeformText('');
    setFocusedIdx(null);
  }, [question.question]);

  // Focus freeform textarea when shown
  ccE(() => {
    if (showFreeform && freeformRef.current) {
      freeformRef.current.focus();
    }
  }, [showFreeform]);

  // Keyboard: 1-4 picks option (single-select); Space toggles focused option (multi-select)
  ccE(() => {
    if (isSubmitting) return;

    function onKeyDown(e) {
      const tag = document.activeElement && document.activeElement.tagName;
      if (tag === 'TEXTAREA' || tag === 'INPUT') {
        if (e.key === 'Escape') { setShowFreeform(false); setFreeformText(''); }
        return;
      }
      if (!isMulti) {
        const num = parseInt(e.key, 10);
        if (num >= 1 && num <= opts.length) {
          e.preventDefault();
          handleSinglePick(num - 1);
        }
        if (e.key === 'ArrowDown') {
          e.preventDefault();
          setFocusedIdx(prev => (prev === null ? 0 : Math.min(prev + 1, opts.length - 1)));
        }
        if (e.key === 'ArrowUp') {
          e.preventDefault();
          setFocusedIdx(prev => (prev === null ? opts.length - 1 : Math.max(prev - 1, 0)));
        }
        if (e.key === 'Enter' && focusedIdx !== null) {
          e.preventDefault();
          handleSinglePick(focusedIdx);
        }
      }
    }
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [isSubmitting, isMulti, opts.length, focusedIdx]);

  // ── Single-select pick ─────────────────────────────────────────────────────
  function handleSinglePick(idx) {
    if (isSubmitting) return;
    const opt = opts[idx];
    if (!opt) return;
    setSelectedIdx(idx);
    onAnswer(opt.label);
  }

  // ── Multi-select toggle ────────────────────────────────────────────────────
  function handleCheckToggle(label) {
    setCheckedSet(prev => {
      const next = new Set(prev);
      if (next.has(label)) { next.delete(label); } else { next.add(label); }
      return next;
    });
  }

  function handleMultiSubmit() {
    if (isSubmitting || checkedSet.size === 0) return;
    const labels = opts.map(o => o.label).filter(l => checkedSet.has(l));
    onAnswer(labels.join(','));
  }

  // ── Freeform submit ────────────────────────────────────────────────────────
  function handleFreeformSubmit() {
    const text = freeformText.trim();
    if (!text || isSubmitting) return;
    onAnswer(text);
  }

  // ── Focused option preview ─────────────────────────────────────────────────
  const focusedPreviewHtml = ccM(() => {
    if (!hasPreview || focusedIdx === null) return null;
    const opt = opts[focusedIdx];
    if (!opt || !opt.preview) return null;
    return renderMarkdown(opt.preview);
  }, [hasPreview, focusedIdx, opts]);

  // ── Render: multi-select layout ────────────────────────────────────────────
  if (isMulti) {
    return (
      <div>
        <div style={{ fontSize: 11, color: 'var(--fg-4)', marginBottom: 8 }}>
          {t('clarify.card.multi_select_hint') || 'Select one or more, then Submit'}
        </div>
        {!showFreeform && (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
            {opts.map((opt, idx) => (
              <label
                key={idx}
                style={{
                  display: 'flex', alignItems: 'center', gap: 10,
                  padding: '8px 12px',
                  background: 'var(--bg-3)',
                  border: checkedSet.has(opt.label) ? '1px solid var(--accent-ring)' : '1px solid var(--border)',
                  borderRadius: 8,
                  cursor: isSubmitting ? 'not-allowed' : 'pointer',
                  opacity: isSubmitting ? 0.6 : 1,
                  userSelect: 'none',
                }}
              >
                <input
                  type="checkbox"
                  checked={checkedSet.has(opt.label)}
                  disabled={isSubmitting}
                  onChange={() => handleCheckToggle(opt.label)}
                  style={{ accentColor: 'var(--accent)', width: 14, height: 14, flexShrink: 0 }}
                />
                <span style={{
                  minWidth: 22, height: 22, borderRadius: 6,
                  background: 'var(--bg-2)', border: '1px solid var(--border)',
                  display: 'flex', alignItems: 'center', justifyContent: 'center',
                  fontSize: 11, fontFamily: 'var(--font-mono, monospace)',
                  color: 'var(--fg-3)', flexShrink: 0,
                }}>
                  {idx + 1}
                </span>
                <span style={{ flex: 1, fontSize: 13, color: 'var(--fg)' }}>{opt.label}</span>
                {opt.description && (
                  <span style={{ fontSize: 11, color: 'var(--fg-4)' }}>{opt.description}</span>
                )}
              </label>
            ))}

            {/* Other button — clears checkboxes and opens freeform */}
            <button
              disabled={isSubmitting}
              onClick={() => { setCheckedSet(new Set()); setShowFreeform(true); }}
              style={{
                display: 'flex', alignItems: 'center', gap: 10,
                padding: '8px 12px',
                background: 'var(--bg-3)', border: '1px dashed var(--border)',
                borderRadius: 8,
                cursor: isSubmitting ? 'not-allowed' : 'pointer',
                opacity: isSubmitting ? 0.6 : 1,
                textAlign: 'left',
              }}
            >
              <span style={{
                minWidth: 22, height: 22, borderRadius: 6,
                background: 'var(--bg-2)', border: '1px solid var(--border)',
                display: 'flex', alignItems: 'center', justifyContent: 'center',
                fontSize: 10, color: 'var(--fg-4)', flexShrink: 0,
              }}>…</span>
              <span style={{ fontSize: 13, color: 'var(--fg-3)' }}>
                {t('clarify.card.other_label') || 'Other…'}
              </span>
            </button>

            {/* Explicit submit button for multi-select */}
            <button
              disabled={isSubmitting || checkedSet.size === 0}
              onClick={handleMultiSubmit}
              style={{
                marginTop: 4,
                padding: '7px 14px', fontSize: 12, borderRadius: 6,
                background: 'var(--accent)', border: 'none',
                color: 'var(--accent-fg, #fff)',
                cursor: (isSubmitting || checkedSet.size === 0) ? 'not-allowed' : 'pointer',
                opacity: (isSubmitting || checkedSet.size === 0) ? 0.5 : 1,
                fontWeight: 500,
                alignSelf: 'flex-end',
              }}
            >
              {isSubmitting ? '…' : (isLast
                ? (t('clarify.card.submit_all') || 'Submit All')
                : (t('clarify.card.next') || 'Next →'))}
            </button>
          </div>
        )}

        {/* Freeform fallback */}
        {showFreeform && (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
            <textarea
              ref={freeformRef}
              rows={3}
              value={freeformText}
              onChange={e => setFreeformText(e.target.value)}
              placeholder={t('clarify.card.freeform_placeholder') || 'Type your answer…'}
              disabled={isSubmitting}
              className="focus-ring"
              style={{
                width: '100%', padding: '8px 12px', fontSize: 13,
                background: 'var(--bg-3)', border: '1px solid var(--border)',
                borderRadius: 8, color: 'var(--fg)', resize: 'vertical',
                minHeight: 72, outline: 'none', boxSizing: 'border-box',
              }}
            />
            <div style={{ display: 'flex', gap: 8, justifyContent: 'flex-end' }}>
              <button
                onClick={() => { setShowFreeform(false); setFreeformText(''); }}
                disabled={isSubmitting}
                style={{
                  padding: '6px 12px', fontSize: 12, borderRadius: 6,
                  background: 'transparent', border: '1px solid var(--border)',
                  color: 'var(--fg-3)', cursor: 'pointer',
                }}
              >
                {t('cancel') || 'Cancel'}
              </button>
              <button
                onClick={handleFreeformSubmit}
                disabled={isSubmitting || !freeformText.trim()}
                style={{
                  padding: '6px 14px', fontSize: 12, borderRadius: 6,
                  background: 'var(--accent)', border: 'none',
                  color: 'var(--accent-fg, #fff)',
                  cursor: (isSubmitting || !freeformText.trim()) ? 'not-allowed' : 'pointer',
                  opacity: (isSubmitting || !freeformText.trim()) ? 0.5 : 1,
                  fontWeight: 500,
                }}
              >
                {isSubmitting ? '…' : (isLast
                  ? (t('clarify.card.submit_all') || 'Submit All')
                  : (t('clarify.card.next') || 'Next →'))}
              </button>
            </div>
          </div>
        )}
      </div>
    );
  }

  // ── Render: single-select + preview layout ─────────────────────────────────
  if (hasPreview) {
    return (
      <div
        className="clarify-card-with-preview"
        style={{ display: 'flex', gap: 12 }}
      >
        {/* Left: option list */}
        <div style={{ flexShrink: 0, width: '50%', display: 'flex', flexDirection: 'column', gap: 6 }}>
          {!showFreeform && opts.map((opt, idx) => (
            <SingleOptionButton
              key={idx}
              opt={opt}
              idx={idx}
              isSelected={selectedIdx === idx}
              isSubmitting={isSubmitting}
              isFocused={focusedIdx === idx}
              onPick={handleSinglePick}
              onFocus={setFocusedIdx}
            />
          ))}
          {!showFreeform && (
            <button
              disabled={isSubmitting}
              onClick={() => setShowFreeform(true)}
              style={{
                display: 'flex', alignItems: 'center', gap: 10,
                padding: '8px 12px',
                background: 'var(--bg-3)', border: '1px dashed var(--border)',
                borderRadius: 8,
                cursor: isSubmitting ? 'not-allowed' : 'pointer',
                opacity: isSubmitting ? 0.6 : 1,
                textAlign: 'left',
              }}
            >
              <span style={{
                minWidth: 22, height: 22, borderRadius: 6,
                background: 'var(--bg-2)', border: '1px solid var(--border)',
                display: 'flex', alignItems: 'center', justifyContent: 'center',
                fontSize: 10, color: 'var(--fg-4)', flexShrink: 0,
              }}>…</span>
              <span style={{ fontSize: 13, color: 'var(--fg-3)' }}>
                {t('clarify.card.other_label') || 'Other…'}
              </span>
            </button>
          )}
          {showFreeform && (
            <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
              <textarea
                ref={freeformRef}
                rows={3}
                value={freeformText}
                onChange={e => setFreeformText(e.target.value)}
                placeholder={t('clarify.card.freeform_placeholder') || 'Type your answer…'}
                disabled={isSubmitting}
                className="focus-ring"
                style={{
                  width: '100%', padding: '8px 12px', fontSize: 13,
                  background: 'var(--bg-3)', border: '1px solid var(--border)',
                  borderRadius: 8, color: 'var(--fg)', resize: 'vertical',
                  minHeight: 72, outline: 'none', boxSizing: 'border-box',
                }}
              />
              <div style={{ display: 'flex', gap: 8, justifyContent: 'flex-end' }}>
                <button
                  onClick={() => { setShowFreeform(false); setFreeformText(''); }}
                  disabled={isSubmitting}
                  style={{
                    padding: '6px 12px', fontSize: 12, borderRadius: 6,
                    background: 'transparent', border: '1px solid var(--border)',
                    color: 'var(--fg-3)', cursor: 'pointer',
                  }}
                >
                  {t('cancel') || 'Cancel'}
                </button>
                <button
                  onClick={handleFreeformSubmit}
                  disabled={isSubmitting || !freeformText.trim()}
                  style={{
                    padding: '6px 14px', fontSize: 12, borderRadius: 6,
                    background: 'var(--accent)', border: 'none',
                    color: 'var(--accent-fg, #fff)',
                    cursor: (isSubmitting || !freeformText.trim()) ? 'not-allowed' : 'pointer',
                    opacity: (isSubmitting || !freeformText.trim()) ? 0.5 : 1,
                    fontWeight: 500,
                  }}
                >
                  {isSubmitting ? '…' : (t('clarify.card.submit') || 'Submit')}
                </button>
              </div>
            </div>
          )}
          {!showFreeform && !isSubmitting && opts.length > 0 && (
            <div style={{ marginTop: 6, fontSize: 10, color: 'var(--fg-4)', fontFamily: 'var(--font-mono, monospace)' }}>
              {opts.map((_, i) => i + 1).join(' / ')} — keyboard shortcut · ↑/↓ navigate
            </div>
          )}
        </div>

        {/* Right: preview panel */}
        <div style={{
          flexGrow: 1,
          borderLeft: '1px solid var(--border)',
          paddingLeft: 12,
          minHeight: 120,
          display: 'flex',
          flexDirection: 'column',
        }}>
          <div style={{ fontSize: 10, color: 'var(--fg-4)', marginBottom: 6, fontFamily: 'var(--font-mono, monospace)' }}>
            {t('clarify.card.preview_hint') || 'Hover options to preview'}
          </div>
          {focusedPreviewHtml
            ? (
              <div
                className="markdown-body"
                style={{ fontSize: 12, color: 'var(--fg)', lineHeight: 1.6, overflow: 'auto' }}
                dangerouslySetInnerHTML={{ __html: focusedPreviewHtml }}
              />
            )
            : (
              <em style={{ fontSize: 12, color: 'var(--fg-4)' }}>
                {t('clarify.card.preview_empty') || 'No preview available'}
              </em>
            )
          }
        </div>
      </div>
    );
  }

  // ── Render: single-select, no preview (original S15 v1 layout) ─────────────
  return (
    <div>
      {!showFreeform && (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
          {opts.map((opt, idx) => (
            <SingleOptionButton
              key={idx}
              opt={opt}
              idx={idx}
              isSelected={selectedIdx === idx}
              isSubmitting={isSubmitting}
              isFocused={false}
              onPick={handleSinglePick}
              onFocus={null}
            />
          ))}
          <button
            disabled={isSubmitting}
            onClick={() => setShowFreeform(true)}
            style={{
              display: 'flex', alignItems: 'center', gap: 10,
              padding: '8px 12px',
              background: 'var(--bg-3)', border: '1px dashed var(--border)',
              borderRadius: 8,
              cursor: isSubmitting ? 'not-allowed' : 'pointer',
              opacity: isSubmitting ? 0.6 : 1,
              textAlign: 'left',
            }}
          >
            <span style={{
              minWidth: 22, height: 22, borderRadius: 6,
              background: 'var(--bg-2)', border: '1px solid var(--border)',
              display: 'flex', alignItems: 'center', justifyContent: 'center',
              fontSize: 10, color: 'var(--fg-4)', flexShrink: 0,
            }}>…</span>
            <span style={{ fontSize: 13, color: 'var(--fg-3)' }}>
              {t('clarify.card.other_label') || 'Other…'}
            </span>
          </button>
        </div>
      )}

      {showFreeform && (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
          <textarea
            ref={freeformRef}
            rows={3}
            value={freeformText}
            onChange={e => setFreeformText(e.target.value)}
            placeholder={t('clarify.card.freeform_placeholder') || 'Type your answer…'}
            disabled={isSubmitting}
            className="focus-ring"
            style={{
              width: '100%', padding: '8px 12px', fontSize: 13,
              background: 'var(--bg-3)', border: '1px solid var(--border)',
              borderRadius: 8, color: 'var(--fg)', resize: 'vertical',
              minHeight: 72, outline: 'none', boxSizing: 'border-box',
            }}
          />
          <div style={{ display: 'flex', gap: 8, justifyContent: 'flex-end' }}>
            <button
              onClick={() => { setShowFreeform(false); setFreeformText(''); }}
              disabled={isSubmitting}
              style={{
                padding: '6px 12px', fontSize: 12, borderRadius: 6,
                background: 'transparent', border: '1px solid var(--border)',
                color: 'var(--fg-3)', cursor: 'pointer',
              }}
            >
              {t('cancel') || 'Cancel'}
            </button>
            <button
              onClick={handleFreeformSubmit}
              disabled={isSubmitting || !freeformText.trim()}
              style={{
                padding: '6px 14px', fontSize: 12, borderRadius: 6,
                background: 'var(--accent)', border: 'none',
                color: 'var(--accent-fg, #fff)',
                cursor: (isSubmitting || !freeformText.trim()) ? 'not-allowed' : 'pointer',
                opacity: (isSubmitting || !freeformText.trim()) ? 0.5 : 1,
                fontWeight: 500,
              }}
            >
              {isSubmitting ? '…' : (t('clarify.card.submit') || 'Submit')}
            </button>
          </div>
        </div>
      )}

      {!showFreeform && !isSubmitting && opts.length > 0 && (
        <div style={{ marginTop: 10, fontSize: 10, color: 'var(--fg-4)', fontFamily: 'var(--font-mono, monospace)' }}>
          {opts.map((_, i) => i + 1).join(' / ')} — keyboard shortcut
        </div>
      )}
    </div>
  );
}

// ── ClarifyCard ───────────────────────────────────────────────────────────────
function ClarifyCard({ clarifyId, clarify, question, options, allowFreeform, status, resolvedAnswer, onSubmit, lang }) {
  const t = ccT(lang);

  // Normalize: support both clarify object prop (v1.1) and legacy flat props (v1 compat)
  const questions = ccM(() => {
    if (clarify && Array.isArray(clarify.questions) && clarify.questions.length > 0) {
      return clarify.questions;
    }
    // Legacy: single question from flat props
    return [{
      question: question || '',
      options: options || [],
      multi_select: false,
    }];
  }, [clarify, question, options]);

  // pending | resolved | timeout | submitting
  const [cardStatus, setCardStatus] = ccS(status || 'pending');
  const [collapsed, setCollapsed] = ccS(false);
  const [resolvedText, setResolvedText] = ccS(null);

  // Multi-question state
  const [currentIdx, setCurrentIdx] = ccS(0);
  const [collectedAnswers, setCollectedAnswers] = ccS({});
  const [submitError, setSubmitError] = ccS(null);

  // Sync status prop → internal state (SSE-driven resolved)
  ccE(() => {
    if (status === 'resolved' && resolvedAnswer) {
      const ans = resolvedAnswer.answers ? Object.values(resolvedAnswer.answers)[0] : '';
      setResolvedText(String(ans || ''));
      setCardStatus('resolved');
      setCollapsed(true);
    } else if (status === 'timeout') {
      setCardStatus('timeout');
    } else if (status === 'pending') {
      setCardStatus('pending');
      setCollapsed(false);
      setResolvedText(null);
      setCurrentIdx(0);
      setCollectedAnswers({});
    }
  }, [status, resolvedAnswer]);

  // Reset multi-question progress when clarify changes
  ccE(() => {
    setCurrentIdx(0);
    setCollectedAnswers({});
  }, [clarifyId]);

  const currentQuestion = questions[currentIdx] || questions[0];
  const isLast = currentIdx === questions.length - 1;
  const total = questions.length;
  const isSubmitting = cardStatus === 'submitting';

  // Called by QuestionPanel when user picks an answer for the current question
  const handleAnswer = ccC(async (answerLabel) => {
    if (cardStatus !== 'pending') return;

    const newAnswers = { ...collectedAnswers, [currentQuestion.question]: answerLabel };
    setCollectedAnswers(newAnswers);

    if (isLast) {
      // Final question answered — submit all collected answers
      setCardStatus('submitting');
      setSubmitError(null);
      const answer = { answers: newAnswers };
      try {
        await onSubmit(answer);
        // Display first answer as summary (or comma-joined for multi)
        const firstAns = Object.values(newAnswers)[0] || '';
        setResolvedText(firstAns);
        setCardStatus('resolved');
        setCollapsed(true);
      } catch (err) {
        setCardStatus('pending');
        setSubmitError(String(err && err.message ? err.message : err));
      }
    } else {
      // Advance to next question
      setCurrentIdx(currentIdx + 1);
    }
  }, [cardStatus, collectedAnswers, currentQuestion, isLast, onSubmit, currentIdx]);

  // ── Resolved (collapsed) state ─────────────────────────────────────────────
  if (cardStatus === 'resolved') {
    const preview = (resolvedText || '').slice(0, 60) + ((resolvedText || '').length > 60 ? '…' : '');
    return (
      <div className="clarify-flyout-card clarify-flyout-card--resolved">
        <button
          className="clarify-resolved-row"
          onClick={() => setCollapsed(c => !c)}
          title={collapsed ? (t('expand') || 'Expand') : (t('collapse') || 'Collapse')}
          style={{
            display: 'flex', alignItems: 'center', gap: 8, width: '100%',
            padding: '8px 14px', background: 'var(--bg-2)',
            border: '1px solid var(--border)', borderRadius: 10,
            cursor: 'pointer', color: 'var(--fg-2)', fontSize: 12,
            textAlign: 'left',
          }}
        >
          <span style={{ color: 'var(--accent)', fontWeight: 600, flexShrink: 0 }}>✓</span>
          <span style={{ flex: 1, minWidth: 0 }}>
            {t('clarify.card.resolved_prefix') || 'Answered'}:{' '}
            {collapsed ? <span className="mono" style={{ color: 'var(--fg)' }}>{preview}</span> : null}
          </span>
          <span style={{ color: 'var(--fg-4)', fontSize: 10 }}>{collapsed ? '▾' : '▴'}</span>
        </button>
        {!collapsed && (
          <div style={{
            marginTop: 4, padding: '10px 14px',
            background: 'var(--bg-2)', border: '1px solid var(--border)',
            borderRadius: 10, fontSize: 12,
          }}>
            <div style={{ color: 'var(--fg-3)', marginBottom: 6, fontSize: 11 }}>
              {t('clarify.card.resolved_prefix') || 'Answered'}:
            </div>
            {questions.map((q, i) => (
              <div key={i} style={{ marginBottom: 8 }}>
                <div
                  className="markdown-body"
                  style={{ fontSize: 12, color: 'var(--fg)', lineHeight: 1.6 }}
                  dangerouslySetInnerHTML={{ __html: renderMarkdown(q.question) }}
                />
                <div style={{
                  marginTop: 4, padding: '4px 10px',
                  background: 'var(--bg-3)', borderRadius: 6,
                  color: 'var(--fg)', fontWeight: 500, fontSize: 12,
                }}>
                  {collectedAnswers[q.question] || resolvedText || '—'}
                </div>
              </div>
            ))}
          </div>
        )}
      </div>
    );
  }

  // ── Timeout state ──────────────────────────────────────────────────────────
  if (cardStatus === 'timeout') {
    return (
      <div style={{
        padding: '8px 14px', background: 'var(--bg-2)', border: '1px solid var(--border)',
        borderRadius: 10, fontSize: 12, color: 'var(--fg-3)',
      }}>
        {t('clarify.card.expired') || 'Clarification request expired.'}
      </div>
    );
  }

  // ── Pending / submitting state ─────────────────────────────────────────────
  return (
    <div
      className="clarify-flyout-card"
      style={{
        background: 'var(--bg-2)',
        border: '1px solid var(--border)',
        borderRadius: 12,
        padding: '14px 16px',
        boxShadow: 'var(--shadow)',
      }}
    >
      {/* Progress indicator — only shown when multiple questions */}
      {total > 1 && (
        <div style={{
          display: 'flex', alignItems: 'center', justifyContent: 'space-between',
          marginBottom: 10,
        }}>
          {/* Already-answered question summaries */}
          <div style={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
            {questions.slice(0, currentIdx).map((q, i) => (
              <div key={i} style={{ fontSize: 11, color: 'var(--fg-3)', display: 'flex', alignItems: 'center', gap: 6 }}>
                <span style={{ color: 'var(--accent)', fontWeight: 700 }}>✓</span>
                <span style={{ color: 'var(--fg-4)' }}>
                  {ccInterp(t('clarify.card.answered_summary') || '✓ {label}', { label: collectedAnswers[q.question] || '' })}
                </span>
              </div>
            ))}
          </div>
          {/* "Question N of M" badge */}
          <div style={{
            fontSize: 11, color: 'var(--fg-4)',
            fontFamily: 'var(--font-mono, monospace)',
            background: 'var(--bg-3)', border: '1px solid var(--border)',
            borderRadius: 6, padding: '2px 8px',
            flexShrink: 0,
          }}>
            {ccInterp(
              t('clarify.card.question_progress') || 'Question {current} of {total}',
              { current: currentIdx + 1, total }
            )}
          </div>
        </div>
      )}

      {/* Current question — markdown rendered */}
      <div
        className="markdown-body"
        style={{ fontSize: 13, color: 'var(--fg)', marginBottom: 12, lineHeight: 1.6 }}
        dangerouslySetInnerHTML={{ __html: renderMarkdown(currentQuestion.question) }}
      />

      {/* Error */}
      {submitError && (
        <div style={{ fontSize: 11, color: 'var(--color-rose-400,#f87171)', marginBottom: 8 }}>
          {submitError}
        </div>
      )}

      {/* Question panel (handles single/multi-select and preview layout) */}
      <QuestionPanel
        question={currentQuestion}
        isSubmitting={isSubmitting}
        isLast={isLast}
        onAnswer={handleAnswer}
        lang={lang}
        t={t}
      />
    </div>
  );
}

// Export for shell.jsx / pages_chat.jsx
window.ClarifyCard = ClarifyCard;
