// UI primitives — shadcn-style but inlined.
const { useState, useEffect, useRef, useMemo, createContext, useContext, useCallback } = React;

// ---- Button ----
const Button = ({ variant = 'default', size = 'md', className = '', children, ...rest }) => {
  const base = 'inline-flex items-center justify-center gap-1.5 font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed select-none whitespace-nowrap rounded-md focus-ring';
  const sizes = {
    xs: 'text-[11px] h-6 px-2',
    sm: 'text-[12px] h-7 px-2.5',
    md: 'text-[13px] h-8 px-3',
    lg: 'text-[14px] h-9 px-4',
    icon: 'h-8 w-8',
    'icon-sm': 'h-7 w-7',
  };
  const variants = {
    default: 'bg-accent hover:opacity-90 text-white',
    outline: 'border border-[var(--border-strong)] bg-transparent hover:bg-[var(--hover)] text-fg',
    ghost:   'bg-transparent hover:bg-[var(--hover)] text-fg-2',
    soft:    'bg-[var(--bg-3)] hover:bg-[var(--border)] text-fg',
    danger:  'bg-rose-600 hover:bg-rose-500 text-white',
    'danger-outline': 'border border-rose-600/40 bg-transparent hover:bg-rose-600/10 text-rose-500',
    success: 'bg-emerald-600 hover:bg-emerald-500 text-white',
    link:    'bg-transparent hover:underline text-accent px-0',
  };
  return <button className={`${base} ${sizes[size]} ${variants[variant]} ${className}`} {...rest}>{children}</button>;
};

// ---- Badges ----
const Badge = ({ tone = 'default', className = '', children }) => {
  const tones = {
    default: 'border-[var(--border-strong)] bg-[var(--bg-3)] text-fg-2',
    accent:  'border-[var(--accent)]/30 bg-accent-soft text-accent',
    emerald: 'border-emerald-600/30 bg-emerald-600/10 text-emerald-400',
    amber:   'border-amber-500/30 bg-amber-500/10 text-amber-400',
    orange:  'border-orange-500/30 bg-orange-500/10 text-orange-400',
    rose:    'border-rose-600/30 bg-rose-600/10 text-rose-400',
    slate:   'border-slate-500/30 bg-slate-500/10 text-slate-400',
    violet:  'border-violet-500/30 bg-violet-500/10 text-violet-400',
    cyan:    'border-cyan-500/30 bg-cyan-500/10 text-cyan-400',
  };
  return <span className={`b ${tones[tone]} ${className}`}>{children}</span>;
};

const RISK_TONE = { low: 'slate', medium: 'amber', high: 'orange', critical: 'rose' };
const RiskBadge = ({ level, lang }) => {
  const t = tFor(lang || 'en');
  return <Badge tone={RISK_TONE[level] || 'slate'}>{t('risk_' + level) || level}</Badge>;
};

const STATUS_MAP = {
  active:   { tone: 'emerald', dot: 'bg-emerald-500', pulse: true, label: 'active' },
  idle:     { tone: 'slate',   dot: 'bg-slate-400', label: 'idle' },
  paused:   { tone: 'amber',   dot: 'bg-amber-400', label: 'paused' },
  online:   { tone: 'emerald', dot: 'bg-emerald-500', pulse: true, label: 'online' },
  offline:  { tone: 'rose',    dot: 'bg-rose-500', label: 'offline' },
  draining: { tone: 'amber',   dot: 'bg-amber-400', label: 'draining' },
  leader:   { tone: 'accent',  dot: 'bg-indigo-400', pulse: true, label: 'leader' },
  follower: { tone: 'slate',   dot: 'bg-slate-400', label: 'follower' },
  running:  { tone: 'accent',  dot: 'bg-indigo-400', pulse: true, label: 'running' },
  done:     { tone: 'emerald', dot: 'bg-emerald-500', label: 'done' },
  failed:   { tone: 'rose',    dot: 'bg-rose-500', label: 'failed' },
  cancelled:{ tone: 'slate',   dot: 'bg-slate-400', label: 'cancelled' },
  pending:  { tone: 'amber',   dot: 'bg-amber-400', label: 'pending' },
  pending_review: { tone: 'amber', dot: 'bg-amber-400', pulse: true, label: 'pending_review' },
  connected:{ tone: 'emerald', dot: 'bg-emerald-500', pulse: true, label: 'connected' },
  disabled: { tone: 'slate',   dot: 'bg-slate-500', label: 'disabled' },
  approved: { tone: 'emerald', dot: 'bg-emerald-500', label: 'approved' },
  rejected: { tone: 'rose',    dot: 'bg-rose-500', label: 'rejected' },
  healthy:  { tone: 'emerald', dot: 'bg-emerald-500', pulse: true, label: 'healthy' },
  degraded: { tone: 'amber',   dot: 'bg-amber-400', pulse: true, label: 'degraded' },
  incident: { tone: 'rose',    dot: 'bg-rose-500', pulse: true, label: 'incident' },
  halt:     { tone: 'rose',    dot: 'bg-rose-500', label: 'halt' },
  ok:       { tone: 'emerald', dot: 'bg-emerald-500', label: 'ok' },
};
const StatusBadge = ({ status }) => {
  const s = STATUS_MAP[status] || STATUS_MAP.idle;
  return (
    <Badge tone={s.tone}>
      <span className={`inline-block w-1.5 h-1.5 rounded-full ${s.dot} ${s.pulse ? 'pulse-dot' : ''}`} />
      {s.label}
    </Badge>
  );
};

// ---- Input ----
const Input = ({ className = '', ...rest }) => (
  <input
    className={`h-8 w-full rounded-md border border-[var(--border-strong)] bg-[var(--bg-2)] px-2.5 text-[13px] text-fg placeholder:text-fg-4 focus-ring ${className}`}
    {...rest}
  />
);
const Textarea = ({ className = '', ...rest }) => (
  <textarea
    className={`w-full rounded-md border border-[var(--border-strong)] bg-[var(--bg-2)] p-2.5 text-[13px] mono text-fg placeholder:text-fg-4 focus-ring ${className}`}
    {...rest}
  />
);

// ---- Switch ----
const Switch = ({ checked, onChange, size = 'md' }) => {
  const h = size === 'sm' ? 16 : 18;
  const w = size === 'sm' ? 28 : 32;
  const knob = size === 'sm' ? 12 : 14;
  return (
    <button
      type="button"
      onClick={() => onChange(!checked)}
      className={`relative inline-flex rounded-full transition-colors ${checked ? 'bg-accent' : 'bg-[var(--bg-3)] border border-[var(--border-strong)]'}`}
      style={{ height: h, width: w }}
    >
      <span
        className="bg-white rounded-full transition-transform shadow-sm"
        style={{
          height: knob,
          width: knob,
          transform: `translate(${checked ? w - knob - 2 : 2}px, ${(h - knob) / 2}px)`,
        }}
      />
    </button>
  );
};

// ---- Tabs ----
const Tabs = ({ value, onChange, items, className = '' }) => (
  <div className={`inline-flex items-center gap-0.5 p-0.5 rounded-md border border-[var(--border)] bg-[var(--bg-2)] ${className}`}>
    {items.map((it) => (
      <button
        key={it.value}
        onClick={() => onChange(it.value)}
        className={`px-2.5 h-7 text-[12px] rounded-[5px] transition-colors ${value === it.value ? 'bg-[var(--bg-3)] text-fg border border-[var(--border)]' : 'text-fg-3 hover:text-fg'}`}
      >
        {it.label}{it.count != null && <span className="ml-1.5 text-fg-4 mono">{it.count}</span>}
      </button>
    ))}
  </div>
);

// ---- Card ----
const Card = ({ className = '', children, ...rest }) => (
  <div className={`bg-bg-2 border border-border rounded-lg elev-1 ${className}`} {...rest}>{children}</div>
);
const CardHeader = ({ title, subtitle, right, className = '' }) => (
  <div className={`flex items-start justify-between px-4 pt-3 pb-2 ${className}`}>
    <div>
      <div className="text-[13px] font-medium text-fg">{title}</div>
      {subtitle && <div className="text-[11px] text-fg-3 mt-0.5">{subtitle}</div>}
    </div>
    {right}
  </div>
);

// ---- Dialog (modal) ----
const Dialog = ({ open, onClose, title, subtitle, children, width = 520, footer }) => {
  useEffect(() => {
    if (!open) return;
    const onKey = (e) => e.key === 'Escape' && onClose();
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [open, onClose]);
  if (!open) return null;
  return (
    <div className="fixed inset-0 z-50">
      <div className="absolute inset-0 bg-black/60 anim-fade-in" onClick={onClose} />
      <div
        className="absolute top-1/2 left-1/2 bg-bg-2 border border-border rounded-lg shadow-2xl anim-dialog-in"
        style={{ width, maxWidth: 'calc(100vw - 32px)', transform: 'translate(-50%,-50%)' }}
      >
        <div className="px-5 pt-4 pb-3 border-b border-border flex items-start justify-between">
          <div>
            <div className="text-[14px] font-semibold text-fg">{title}</div>
            {subtitle && <div className="text-[12px] text-fg-3 mt-0.5">{subtitle}</div>}
          </div>
          <button onClick={onClose} className="text-fg-3 hover:text-fg p-1 -mr-1 -mt-1 rounded">
            <I.Close size={16} />
          </button>
        </div>
        <div className="p-5">{children}</div>
        {footer && <div className="px-5 pb-4 pt-1 flex justify-end gap-2">{footer}</div>}
      </div>
    </div>
  );
};

// ---- Sheet (side panel, right) ----
const Sheet = ({ open, onClose, title, subtitle, children, width = 620, right }) => {
  useEffect(() => {
    if (!open) return;
    const onKey = (e) => e.key === 'Escape' && onClose();
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [open, onClose]);
  if (!open) return null;
  return (
    <div className="fixed inset-0 z-40">
      <div className="absolute inset-0 bg-black/50 anim-fade-in" onClick={onClose} />
      <div
        className="absolute right-0 top-0 bottom-0 bg-bg-2 border-l border-border shadow-2xl anim-sheet-in flex flex-col"
        style={{ width, maxWidth: '96vw' }}
      >
        <div className="px-5 pt-4 pb-3 border-b border-border flex items-start justify-between">
          <div className="min-w-0">
            <div className="text-[14px] font-semibold text-fg truncate">{title}</div>
            {subtitle && <div className="text-[12px] text-fg-3 mono mt-0.5 truncate">{subtitle}</div>}
          </div>
          <div className="flex items-center gap-2 shrink-0 ml-3">
            {right}
            <button onClick={onClose} className="text-fg-3 hover:text-fg p-1 rounded">
              <I.Close size={16} />
            </button>
          </div>
        </div>
        <div className="flex-1 overflow-auto">{children}</div>
      </div>
    </div>
  );
};

// ---- Toast ----
const ToastCtx = createContext(null);
const ToastProvider = ({ children }) => {
  const [toasts, setToasts] = useState([]);
  const push = useCallback((t) => {
    const id = Math.random().toString(36).slice(2);
    setToasts((ts) => [...ts, { id, ...t }]);
    setTimeout(() => setToasts((ts) => ts.filter((x) => x.id !== id)), t.duration || 3000);
  }, []);
  return (
    <ToastCtx.Provider value={{ toast: push }}>
      {children}
      <div className="fixed bottom-4 right-4 z-[60] flex flex-col gap-2 max-w-sm">
        {toasts.map((t) => (
          <div key={t.id} className="anim-slide-up bg-bg-2 border border-border-strong rounded-lg shadow-lg px-3.5 py-2.5 flex items-start gap-3 min-w-[260px]">
            <div className={`mt-0.5 ${t.tone === 'error' ? 'text-rose-500' : t.tone === 'success' ? 'text-emerald-500' : 'text-accent'}`}>
              {t.tone === 'error' ? <I.AlertTriangle size={14} /> : t.tone === 'success' ? <I.CheckCircle size={14} /> : <I.Bell size={14} />}
            </div>
            <div className="flex-1 min-w-0">
              <div className="text-[13px] font-medium text-fg">{t.title}</div>
              {t.desc && <div className="text-[12px] text-fg-3 mt-0.5">{t.desc}</div>}
            </div>
            {t.action && <button className="text-[12px] text-accent hover:underline mt-0.5" onClick={t.action.onClick}>{t.action.label}</button>}
          </div>
        ))}
      </div>
    </ToastCtx.Provider>
  );
};
const useToast = () => useContext(ToastCtx);

// ---- JSON viewer ----
const JsonNode = ({ k, v, depth = 0, last = true }) => {
  const [open, setOpen] = useState(depth < 2);
  const isObj = v !== null && typeof v === 'object';
  const isArr = Array.isArray(v);
  const [copied, setCopied] = useState(false);
  const onCopy = (e) => {
    e.stopPropagation();
    try { navigator.clipboard.writeText(JSON.stringify(v, null, 2)); } catch {}
    setCopied(true); setTimeout(() => setCopied(false), 800);
  };
  if (!isObj) {
    const color = typeof v === 'string' ? 'text-emerald-400' : typeof v === 'number' ? 'text-amber-400' : typeof v === 'boolean' ? 'text-violet-400' : 'text-fg-3';
    return (
      <div className="pl-4 mono text-[12px]">
        {k != null && <span className="text-accent">{JSON.stringify(k)}</span>}
        {k != null && <span className="text-fg-4">: </span>}
        <span className={color}>{JSON.stringify(v)}</span>
        {!last && <span className="text-fg-4">,</span>}
      </div>
    );
  }
  const entries = isArr ? v.map((x, i) => [i, x]) : Object.entries(v);
  const brackets = isArr ? ['[', ']'] : ['{', '}'];
  return (
    <div className="mono text-[12px]">
      <div className="flex items-center gap-1 cursor-pointer group" onClick={() => setOpen(!open)}>
        <I.ChevronRight size={10} className={`text-fg-4 transition-transform ${open ? 'rotate-90' : ''}`} />
        {k != null && <><span className="text-accent">{JSON.stringify(k)}</span><span className="text-fg-4">: </span></>}
        <span className="text-fg-3">{brackets[0]}</span>
        {!open && <span className="text-fg-4">{isArr ? `${v.length} items` : `${entries.length} keys`}</span>}
        {!open && <span className="text-fg-3">{brackets[1]}</span>}
        <button
          onClick={onCopy}
          className="ml-auto opacity-0 group-hover:opacity-100 text-fg-4 hover:text-fg p-0.5 rounded"
          title={copied ? 'Copied' : 'Copy'}
        >
          {copied ? <I.Check size={11} /> : <I.Copy size={11} />}
        </button>
      </div>
      {open && (
        <div className="border-l border-border ml-[4px]">
          {entries.map(([kk, vv], i) => (
            <JsonNode key={kk} k={kk} v={vv} depth={depth + 1} last={i === entries.length - 1} />
          ))}
          <div className="pl-4 text-fg-3">{brackets[1]}{!last && <span className="text-fg-4">,</span>}</div>
        </div>
      )}
    </div>
  );
};
const JsonViewer = ({ value }) => (
  <div className="bg-bg-3 border border-border rounded-md p-3 overflow-auto">
    <JsonNode v={value} depth={0} />
  </div>
);

// ---- Empty state ----
const EmptyState = ({ icon: IconCmp = I.Circle, title, subtitle, action }) => (
  <div className="flex flex-col items-center justify-center py-14 px-6 text-center">
    <div className="h-10 w-10 rounded-full bg-[var(--bg-3)] border border-border flex items-center justify-center text-fg-3 mb-3">
      <IconCmp size={18} />
    </div>
    <div className="text-[13px] font-medium text-fg">{title}</div>
    {subtitle && <div className="text-[12px] text-fg-3 mt-1 max-w-[260px]">{subtitle}</div>}
    {action && <div className="mt-4">{action}</div>}
  </div>
);

// ---- Skeleton ----
const Skeleton = ({ w = '100%', h = 14, className = '' }) => (
  <div className={`skeleton ${className}`} style={{ width: w, height: h }} />
);

// ---- Skeleton rows (table-friendly placeholder) ----
const SkeletonRows = ({ rows = 6, cols = 4 }) => (
  <div className="divide-y divide-[var(--border)]">
    {Array.from({ length: rows }).map((_, r) => (
      <div key={r} className="px-4 py-3 flex items-center gap-3">
        {Array.from({ length: cols }).map((_, c) => (
          <Skeleton key={c} w={c === 0 ? 120 : c === cols - 1 ? 80 : '18%'} h={12} />
        ))}
      </div>
    ))}
  </div>
);

// ---- Error banner (soft, non-blocking) ----
const ErrorBanner = ({ message, onRetry }) => (
  <div className="border border-amber-500/30 bg-amber-500/5 text-amber-300 rounded-md px-3 py-2 text-[12px] flex items-center gap-2">
    <I.AlertTriangle size={13} />
    <span className="flex-1 mono truncate">{message}</span>
    {onRetry && (
      <button onClick={onRetry} className="text-accent hover:underline text-[12px]">
        retry
      </button>
    )}
  </div>
);

// ---- KBar / misc ----
const Kbd = ({ children }) => <span className="kbd">{children}</span>;

// ---- Trace tree ----
const SPAN_COLOR = {
  agent: 'bg-indigo-500/80',
  llm: 'bg-violet-500/80',
  skill: 'bg-cyan-500/80',
  tool: 'bg-emerald-500/80',
  capability: 'bg-amber-500/80',
  policy: 'bg-rose-500/80',
  wait: 'bg-slate-500/60',
};
const SpanRow = ({ span, depth, totalMs }) => {
  const [open, setOpen] = useState(true);
  const hasKids = span.children && span.children.length;
  const left = (span.start / totalMs) * 100;
  const w = Math.max(0.4, (span.dur / totalMs) * 100);
  return (
    <div>
      <div className="grid grid-cols-[minmax(260px,1fr)_1fr_80px] gap-3 items-center border-b border-border/60 hover:bg-[var(--hover)] row-pad px-3 text-[12px]">
        <div className="flex items-center gap-1 min-w-0" style={{ paddingLeft: depth * 14 }}>
          {hasKids ? (
            <button onClick={() => setOpen(!open)} className="text-fg-4 hover:text-fg shrink-0">
              <I.ChevronRight size={11} className={`transition-transform ${open ? 'rotate-90' : ''}`} />
            </button>
          ) : <span className="w-[11px] shrink-0" />}
          <span className={`inline-block w-1.5 h-1.5 rounded-sm shrink-0 ${SPAN_COLOR[span.kind] || 'bg-slate-400'}`} />
          <span className="mono text-[11.5px] text-fg truncate">{span.name}</span>
          <span className="mono text-[10px] text-fg-4 ml-1.5 uppercase tracking-wider shrink-0">{span.kind}</span>
          {span.status !== 'ok' && <StatusBadge status={span.status} />}
        </div>
        <div className="relative h-[14px]">
          <div className="absolute inset-y-0 left-0 right-0 border-l border-r border-border/40" />
          <div
            className={`trace-bar ${SPAN_COLOR[span.kind] || 'bg-slate-400'}`}
            style={{ position: 'absolute', left: left + '%', width: w + '%' }}
            title={`${span.start}ms → ${span.start + span.dur}ms (${span.dur}ms)`}
          />
        </div>
        <div className="mono text-[11px] text-fg-3 text-right">{span.dur} ms</div>
      </div>
      {hasKids && open && span.children.map((c) => <SpanRow key={c.id} span={c} depth={depth + 1} totalMs={totalMs} />)}
    </div>
  );
};
const TraceTimeline = ({ spans, totalMs }) => (
  <div>
    <div className="grid grid-cols-[minmax(260px,1fr)_1fr_80px] gap-3 px-3 py-2 text-[10px] uppercase tracking-wider text-fg-4 mono border-b border-border bg-[var(--bg-3)]">
      <div>span</div>
      <div className="flex items-center justify-between">
        <span>0 ms</span><span>{Math.round(totalMs / 2)} ms</span><span>{totalMs} ms</span>
      </div>
      <div className="text-right">duration</div>
    </div>
    {spans.map((s) => <SpanRow key={s.id} span={s} depth={0} totalMs={totalMs} />)}
  </div>
);

// ---- Simple select ----
// Overlap fix (UX 5.1): widen right padding so long option labels never
// crash into the decorative chevron, carry a non-default stacking context
// on the wrapper so the native dropdown list stays above sibling toolbar
// elements, and force `overflow: visible` so parent flex/card clipping
// does not truncate the open list.
const Select = ({ value, onChange, options, className = '', placeholder }) => (
  <div className={`relative z-10 ${className}`} style={{ overflow: 'visible' }}>
    <select
      value={value || ''}
      onChange={(e) => onChange(e.target.value)}
      style={{ lineHeight: 1.4 }}
      className="h-8 w-full appearance-none rounded-md border border-[var(--border-strong)] bg-[var(--bg-2)] pl-2.5 pr-8 text-[12px] text-fg focus-ring"
    >
      {placeholder && <option value="">{placeholder}</option>}
      {options.map((o) => <option key={o.value} value={o.value}>{o.label}</option>)}
    </select>
    <I.ChevronDown size={12} className="absolute right-2 top-1/2 -translate-y-1/2 text-fg-4 pointer-events-none" />
  </div>
);

// ---- Small counter card ----
const StatCard = ({ label, value, trend, trendTone = 'emerald', icon: IconCmp, sparkline }) => (
  <Card className="p-4">
    <div className="flex items-start justify-between">
      <div className="text-[11px] uppercase tracking-wider text-fg-3">{label}</div>
      {IconCmp && <div className="text-fg-4"><IconCmp size={14} /></div>}
    </div>
    <div className="mt-2 flex items-baseline gap-2">
      <div className="text-[28px] font-semibold tracking-tight text-fg tabular-nums">{value}</div>
      {trend && <div className={`text-[11px] mono ${trendTone === 'emerald' ? 'text-emerald-400' : trendTone === 'rose' ? 'text-rose-400' : 'text-fg-3'}`}>{trend}</div>}
    </div>
    {sparkline && <div className="mt-3">{sparkline}</div>}
  </Card>
);

// Sparkline (deterministic from seed)
const Sparkline = ({ values, tone = 'accent', height = 28 }) => {
  const max = Math.max(...values);
  const min = Math.min(...values);
  const range = Math.max(1, max - min);
  const w = 120;
  const step = w / (values.length - 1);
  const pts = values.map((v, i) => [i * step, height - ((v - min) / range) * height]);
  const d = 'M ' + pts.map(p => p.map(n => n.toFixed(1)).join(' ')).join(' L ');
  const color = tone === 'accent' ? 'var(--accent)' : tone === 'emerald' ? '#10b981' : tone === 'rose' ? '#f43f5e' : '#a1a1aa';
  return (
    <svg width={w} height={height} viewBox={`0 0 ${w} ${height}`} className="block">
      <path d={d} fill="none" stroke={color} strokeWidth="1.4" />
      <path d={d + ` L ${w} ${height} L 0 ${height} Z`} fill={color} opacity="0.08" />
    </svg>
  );
};

Object.assign(window, {
  Button, Badge, RiskBadge, StatusBadge, Input, Textarea, Switch, Tabs,
  Card, CardHeader, Dialog, Sheet, ToastProvider, useToast,
  JsonViewer, EmptyState, Skeleton, SkeletonRows, ErrorBanner, Kbd, TraceTimeline, Select,
  StatCard, Sparkline, STATUS_MAP,
});
