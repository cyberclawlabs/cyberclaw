// pages_kanban.jsx — Kanban Dispatcher (R-07)
//
// 4-column drag-drop board: todo / in_progress / blocked / done.
// Cards show title + body excerpt + priority badge + claimed_by.
// Click card -> Sheet edit. Header has "+ New Task".
// Drag uses native HTML5 drag events — no dnd-kit.
//
// Backend contract (admin REST):
//   GET    /api/v1/admin/kanban/tasks → { tasks: [...] }
//   POST   /api/v1/admin/kanban/tasks ← { title, body?, priority?, due_at? } → { id }
//   PUT    /api/v1/admin/kanban/tasks/:id ← { ... } → { ok }
//   DELETE /api/v1/admin/kanban/tasks/:id

const { useEffect: kU, useState: kS, useMemo: kM, useRef: kR } = React;
const {
  Card: KCard, CardHeader: KCardHeader, Button: KButton, Input: KInput,
  Textarea: KTextarea, Sheet: KSheet, Dialog: KDialog, Badge: KBadge,
  Select: KSelect, EmptyState: KEmpty, SkeletonRows: KSkeletonRows,
  ErrorBanner: KErr, useToast: kUseToast, I: KI,
} = window.cc.ui || window;

const KANBAN_COLUMNS = [
  { key: 'todo',        accent: 'slate'   },
  { key: 'in_progress', accent: 'accent'  },
  { key: 'blocked',     accent: 'rose'    },
  { key: 'done',        accent: 'emerald' },
];

const PRIO_TONE = {
  P0: 'rose',
  P1: 'orange',
  P2: 'amber',
  P3: 'slate',
};

// Server returns priority as integer (e.g. `5`), not "P3" string. Tolerate
// both shapes so the card renders no matter what the API returns.
function normalizePriority(p) {
  if (p == null || p === '') return 'P3';
  if (typeof p === 'number') return `P${p}`;
  return String(p).toUpperCase();
}

const KanbanCard = ({ task, t, onOpen, onDragStart, onDragEnd, dragging }) => {
  const prioLabel = normalizePriority(task.priority);
  const tone = PRIO_TONE[prioLabel] || 'slate';
  const due = task.due_at ? new Date(task.due_at) : null;
  const overdue = due && due.getTime() < Date.now() && task.status !== 'done';
  return (
    <button
      draggable
      onDragStart={(e) => onDragStart(e, task)}
      onDragEnd={onDragEnd}
      onClick={() => onOpen(task)}
      className={`group w-full text-left bg-bg-2 border border-border rounded-md p-3 transition-all hover:border-[var(--border-strong)] hover:elev-1 ${dragging ? 'opacity-40 scale-[0.98]' : ''}`}
      style={{ cursor: dragging ? 'grabbing' : 'grab' }}
    >
      <div className="flex items-start justify-between gap-2">
        <div className="text-[12.5px] font-medium text-fg leading-snug truncate flex-1">
          {task.title || t('kanban.untitled')}
        </div>
        {task.priority != null && task.priority !== '' && (
          <KBadge tone={tone}>{prioLabel}</KBadge>
        )}
      </div>
      {task.body && (
        <div className="text-[11px] text-fg-3 mt-1.5 line-clamp-2 leading-relaxed">
          {task.body.length > 110 ? task.body.slice(0, 110) + '…' : task.body}
        </div>
      )}
      <div className="mt-2 flex items-center gap-2 text-[10px] mono text-fg-4">
        {task.claimed_by && (
          <span className="inline-flex items-center gap-1 text-fg-3">
            <KI.User size={10} />
            <span className="truncate max-w-[80px]">{task.claimed_by}</span>
          </span>
        )}
        {due && (
          <span className={`inline-flex items-center gap-1 ${overdue ? 'text-rose-400' : ''}`}>
            <KI.Clock size={10} />
            {due.toISOString().slice(0, 10)}
          </span>
        )}
        <span className="ml-auto truncate">{(task.id || '').slice(0, 8)}</span>
      </div>
    </button>
  );
};

const KanbanColumn = ({ col, tasks, t, onOpen, onDrop, onDragOver, onDragLeave, hot, dragging, onDragStart, onDragEnd }) => {
  const tone = col.accent;
  const dot = {
    slate:   'bg-slate-400',
    accent:  'bg-indigo-400',
    rose:    'bg-rose-500',
    emerald: 'bg-emerald-500',
  }[tone] || 'bg-slate-400';

  return (
    <div
      onDragOver={(e) => { e.preventDefault(); onDragOver(col.key); }}
      onDragLeave={() => onDragLeave(col.key)}
      onDrop={(e) => { e.preventDefault(); onDrop(col.key); }}
      className={`flex flex-col min-w-0 rounded-lg border transition-colors ${hot ? 'border-[var(--accent-ring)] bg-accent-soft/40' : 'border-border bg-bg-2/40'}`}
      style={{ minHeight: 480 }}
    >
      <div className="flex items-center justify-between px-3 pt-3 pb-2 shrink-0">
        <div className="flex items-center gap-2">
          <span className={`w-2 h-2 rounded-full ${dot}`} />
          <div className="text-[12px] font-medium text-fg uppercase tracking-wider">
            {t('kanban.col.' + col.key)}
          </div>
          <span className="text-[10px] mono text-fg-4 tabular-nums">{tasks.length}</span>
        </div>
      </div>
      <div className="flex-1 px-2 pb-2 space-y-2 overflow-y-auto">
        {tasks.length === 0 ? (
          <div className="border border-dashed border-border/60 rounded-md py-6 text-center text-[11px] text-fg-4">
            {t('kanban.column_empty')}
          </div>
        ) : (
          tasks.map((task) => (
            <KanbanCard
              key={task.id}
              task={task}
              t={t}
              onOpen={onOpen}
              onDragStart={onDragStart}
              onDragEnd={onDragEnd}
              dragging={dragging === task.id}
            />
          ))
        )}
      </div>
    </div>
  );
};

const KanbanEditor = ({ open, task, isNew, onClose, onSave, onDelete, t }) => {
  const [draft, setDraft] = kS(task || {});
  const [confirmDelete, setConfirmDelete] = kS(false);
  kU(() => { setDraft(task || {}); setConfirmDelete(false); }, [task]);
  if (!open) return null;
  const save = () => {
    if (!draft.title || !draft.title.trim()) return;
    onSave(draft);
  };
  return (
    <>
      <KSheet
        open={open}
        onClose={onClose}
        title={isNew ? t('kanban.editor.new_title') : draft.title || t('kanban.untitled')}
        subtitle={isNew ? '' : draft.id || ''}
        right={!isNew && (
          <KButton size="sm" variant="danger-outline" onClick={() => setConfirmDelete(true)}>
            <KI.Trash size={11} />
          </KButton>
        )}
      >
        <div className="p-5 space-y-4">
          <div>
            <div className="text-[10px] uppercase tracking-wider text-fg-3 mb-1.5">{t('kanban.field.title')}</div>
            <KInput
              value={draft.title || ''}
              onChange={(e) => setDraft({ ...draft, title: e.target.value })}
              placeholder={t('kanban.field.title_placeholder')}
            />
          </div>
          <div>
            <div className="text-[10px] uppercase tracking-wider text-fg-3 mb-1.5">{t('kanban.field.body')}</div>
            <KTextarea
              rows={6}
              value={draft.body || ''}
              onChange={(e) => setDraft({ ...draft, body: e.target.value })}
              placeholder={t('kanban.field.body_placeholder')}
            />
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div>
              <div className="text-[10px] uppercase tracking-wider text-fg-3 mb-1.5">{t('kanban.field.priority')}</div>
              <KSelect
                value={draft.priority || 'P2'}
                onChange={(v) => setDraft({ ...draft, priority: v })}
                options={[
                  { value: 'P0', label: 'P0 — Critical' },
                  { value: 'P1', label: 'P1 — High' },
                  { value: 'P2', label: 'P2 — Medium' },
                  { value: 'P3', label: 'P3 — Low' },
                ]}
              />
            </div>
            <div>
              <div className="text-[10px] uppercase tracking-wider text-fg-3 mb-1.5">{t('kanban.field.status')}</div>
              <KSelect
                value={draft.status || 'todo'}
                onChange={(v) => setDraft({ ...draft, status: v })}
                options={KANBAN_COLUMNS.map((c) => ({ value: c.key, label: t('kanban.col.' + c.key) }))}
              />
            </div>
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div>
              <div className="text-[10px] uppercase tracking-wider text-fg-3 mb-1.5">{t('kanban.field.due_at')}</div>
              <KInput
                type="date"
                value={draft.due_at ? draft.due_at.slice(0, 10) : ''}
                onChange={(e) => setDraft({ ...draft, due_at: e.target.value ? e.target.value + 'T00:00:00Z' : null })}
              />
            </div>
            <div>
              <div className="text-[10px] uppercase tracking-wider text-fg-3 mb-1.5">{t('kanban.field.claimed_by')}</div>
              <KInput
                value={draft.claimed_by || ''}
                onChange={(e) => setDraft({ ...draft, claimed_by: e.target.value })}
                placeholder="op_id or agent_id"
              />
            </div>
          </div>
          <div className="flex justify-end gap-2 pt-2 border-t border-border/60">
            <KButton variant="ghost" onClick={onClose}>{t('kanban.editor.cancel')}</KButton>
            <KButton onClick={save} disabled={!draft.title || !draft.title.trim()}>
              <KI.Check size={12} /> {isNew ? t('kanban.editor.create') : t('kanban.editor.save')}
            </KButton>
          </div>
        </div>
      </KSheet>
      <KDialog
        open={confirmDelete}
        onClose={() => setConfirmDelete(false)}
        title={t('kanban.delete.title')}
        subtitle={t('kanban.delete.subtitle')}
        footer={<>
          <KButton variant="ghost" onClick={() => setConfirmDelete(false)}>{t('kanban.editor.cancel')}</KButton>
          <KButton variant="danger" onClick={() => { onDelete(draft); setConfirmDelete(false); }}>
            <KI.Trash size={12} /> {t('kanban.delete.confirm')}
          </KButton>
        </>}
      >
        <p className="text-[13px] text-fg-2">
          {t('kanban.delete.body', { title: draft.title || draft.id || '' })}
        </p>
      </KDialog>
    </>
  );
};

const KanbanPage = ({ lang }) => {
  const t = tFor(lang || 'en');
  const toast = kUseToast();
  const [tasks, setTasks] = kS([]);
  const [loading, setLoading] = kS(true);
  const [error, setError] = kS(null);
  const [editing, setEditing] = kS(null);
  const [editingNew, setEditingNew] = kS(false);
  const [dragId, setDragId] = kS(null);
  const [hotCol, setHotCol] = kS(null);

  const reload = async () => {
    setLoading(true);
    setError(null);
    try {
      const r = await window.cc.api.kanban.listTasks();
      const list = Array.isArray(r) ? r : Array.isArray(r && r.tasks) ? r.tasks : [];
      setTasks(list);
    } catch (e) {
      setError(e && e.message ? e.message : String(e));
      setTasks([]);
    } finally {
      setLoading(false);
    }
  };
  kU(() => { reload(); }, []);

  const grouped = kM(() => {
    const g = { todo: [], in_progress: [], blocked: [], done: [] };
    tasks.forEach((t) => {
      const s = (t.status || 'todo').toLowerCase();
      if (g[s]) g[s].push(t);
      else g.todo.push(t);
    });
    return g;
  }, [tasks]);

  const onCardOpen = (task) => { setEditingNew(false); setEditing(task); };
  const onNew = () => {
    setEditingNew(true);
    setEditing({ title: '', body: '', priority: 'P2', status: 'todo' });
  };

  const onDragStart = (e, task) => {
    setDragId(task.id);
    try { e.dataTransfer.effectAllowed = 'move'; } catch {}
    try { e.dataTransfer.setData('text/plain', task.id); } catch {}
  };
  const onDragEnd = () => { setDragId(null); setHotCol(null); };
  const onDragOver = (key) => { setHotCol(key); };
  const onDragLeave = (key) => { if (hotCol === key) setHotCol(null); };
  const onDrop = async (key) => {
    setHotCol(null);
    if (!dragId) return;
    const target = tasks.find((x) => x.id === dragId);
    if (!target || target.status === key) { setDragId(null); return; }
    // Optimistic update
    const prev = tasks;
    setTasks(tasks.map((x) => x.id === dragId ? { ...x, status: key } : x));
    setDragId(null);
    try {
      await window.cc.api.kanban.updateTask(target.id, { status: key });
      toast && toast.toast({ title: t('kanban.toast.moved'), desc: t('kanban.col.' + key), tone: 'success' });
    } catch (e) {
      setTasks(prev);
      toast && toast.toast({ title: t('kanban.toast.move_failed'), desc: String(e), tone: 'error' });
    }
  };

  const onSave = async (draft) => {
    try {
      if (editingNew) {
        const resp = await window.cc.api.kanban.createTask({
          title: draft.title,
          body: draft.body,
          priority: draft.priority,
          due_at: draft.due_at,
        });
        toast && toast.toast({ title: t('kanban.toast.created'), tone: 'success' });
        setEditing(null);
        setEditingNew(false);
        await reload();
        // If status was changed to non-default, also patch
        if (draft.status && draft.status !== 'todo' && resp && resp.id) {
          try { await window.cc.api.kanban.updateTask(resp.id, { status: draft.status }); reload(); } catch {}
        }
      } else {
        await window.cc.api.kanban.updateTask(draft.id, {
          title: draft.title,
          body: draft.body,
          priority: draft.priority,
          status: draft.status,
          due_at: draft.due_at,
          claimed_by: draft.claimed_by,
        });
        toast && toast.toast({ title: t('kanban.toast.saved'), tone: 'success' });
        setEditing(null);
        reload();
      }
    } catch (e) {
      toast && toast.toast({ title: t('kanban.toast.save_failed'), desc: String(e), tone: 'error' });
    }
  };

  const onDelete = async (draft) => {
    try {
      await window.cc.api.kanban.deleteTask(draft.id);
      toast && toast.toast({ title: t('kanban.toast.deleted'), tone: 'success' });
      setEditing(null);
      reload();
    } catch (e) {
      toast && toast.toast({ title: t('kanban.toast.delete_failed'), desc: String(e), tone: 'error' });
    }
  };

  return (
    <div className="space-y-4">
      <KCard className="overflow-hidden">
        <KCardHeader
          title={t('kanban.board.title')}
          subtitle={<span className="mono">{t('kanban.board.subtitle')}</span>}
          right={
            <div className="flex items-center gap-2">
              <KButton size="sm" variant="ghost" onClick={reload}>
                <KI.Refresh size={12} /> {t('kanban.action.reload')}
              </KButton>
              <KButton size="sm" onClick={onNew}>
                <KI.Plus size={12} /> {t('kanban.action.new')}
              </KButton>
            </div>
          }
        />
        {error && (
          <div className="px-4 pb-3">
            <KErr message={error} onRetry={reload} />
          </div>
        )}
      </KCard>

      {loading ? (
        <KCard><KSkeletonRows rows={6} cols={4} /></KCard>
      ) : tasks.length === 0 && !error ? (
        <KCard className="py-2">
          <KEmpty
            icon={KI.List}
            title={t('kanban.empty.title')}
            subtitle={t('kanban.empty.subtitle')}
            action={<KButton onClick={onNew}><KI.Plus size={12} /> {t('kanban.action.new')}</KButton>}
          />
        </KCard>
      ) : (
        <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-4 gap-3">
          {KANBAN_COLUMNS.map((col) => (
            <KanbanColumn
              key={col.key}
              col={col}
              tasks={grouped[col.key] || []}
              t={t}
              hot={hotCol === col.key}
              dragging={dragId}
              onOpen={onCardOpen}
              onDragOver={onDragOver}
              onDragLeave={onDragLeave}
              onDrop={onDrop}
              onDragStart={onDragStart}
              onDragEnd={onDragEnd}
            />
          ))}
        </div>
      )}

      <KanbanEditor
        open={!!editing}
        task={editing}
        isNew={editingNew}
        onClose={() => { setEditing(null); setEditingNew(false); }}
        onSave={onSave}
        onDelete={onDelete}
        t={t}
      />
    </div>
  );
};

Object.assign(window, { KanbanPage });
