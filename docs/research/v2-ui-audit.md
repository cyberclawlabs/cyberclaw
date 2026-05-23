# v2 UI Audit — 2026-05-11

Auditor: ui-ux-designer agent
Baseline: v1 stack (`web/src/pages_*.jsx`, `pages_c.jsx` 158 styled patterns, `pages_chat.jsx` 40 patterns, with KPI StatCards, icon-prefixed tabs, tonal stat cards, structured headers)
Mounted v2 routes: 31 pages under `/admin/v2/*`, see `web/src/AppV2.tsx` lines 128–169
Tokens checked: `web/src/index.css` (--bg, --fg, --accent, --border)

## Verdict (one-liner)

The v2 frontend is **functionally a thin "list+filter" stub layer**. Almost every page is a near-identical `<filter bar> + <single table>` with no page header, no KPI summary, no information hierarchy, and a stale "↗ Manage on /admin" escape hatch on every page that tells users "this UI is incomplete, go elsewhere". The Karpathy DRY principle has been over-applied: ~25 pages share the same shape and feel like one page rendered 25 times. v1 still uses rich KPI cards, segmented tabs, icon-prefixed sections — v2 dropped all of it without replacement.

---

## Top 10 P0 fixes (most visible / cheapest wins)

1. **Every page has a "↗ Manage on /admin" escape link** — currently shipping a public "this product is unfinished" sign on every screen. (`AgentsPage.tsx:84-89`, `SkillsPage.tsx:75-81`, `TasksPage.tsx:55-61`, `ReviewsPage.tsx:62-68`, `AuditPage.tsx:65-71`, repeated 24 more times). **Fix:** move to a global "Open in legacy admin" item inside Topbar overflow menu, or hide behind `?legacy=1`. Do not display by default on production v2.

2. **No page headers / titles on 27 of 31 pages** — user lands on `/admin/v2/agents` and immediately sees an unlabeled search input. Compare to v1 which has `<header>` with `<h2>` + subtitle on every page. **Fix:** add `<h2 className="text-base font-medium">{label}</h2>` + `<p className="text-xs text-fg-3">{count} total · last updated …</p>` above the filter bar on every page. Only `StatusPage` (AppV2.tsx:411), `PluginsPage:26`, `SettingsPage`, `BrowserConsolePage`, `ModelsPage:58` currently have them.

3. **Zero loading skeletons; every page shows "Loading…" as a single grey table row** (`TasksPage.tsx:80`, `ExecutionsPage.tsx:109`, `ReviewsPage.tsx:88`, `AuditPage.tsx:90`, `MemoryPage.tsx:108`, etc — 20+ pages). **Fix:** add a `TableSkeleton` component that renders 8 grey shimmer rows matching column widths during initial load. Same component reused everywhere.

4. **Empty states are useless one-liners** — "No tasks found. Tasks are created when agents execute work." (`TasksPage.tsx:84`) is just text in a grey table cell. No icon, no CTA, no help link. **Fix:** create an `<EmptyState icon body cta />` component with `~py-12 text-center`, an inline SVG icon (use `Bot`/`List`/`Activity` from `components/icons.tsx`), a single-line title, a 2-line description, and one accent-tone CTA. Apply consistently.

5. **Page padding inconsistent**: `AppV2.tsx:223` sets main `px-6 py-5`, but inner `<section>` uses `space-y-3` (`TasksPage.tsx:41`), `space-y-4` (`SkillsPage.tsx:61`), `space-y-5` (`ModelsPage.tsx:57`), `space-y-3` then 4-card grid that breaks rhythm. **Fix:** standardize on `space-y-4` for all top-level page sections; cards/tables inside use `space-y-3`. Stop mixing.

6. **Color token violation — `ModelsPage.tsx:66, 87, 121` uses `border-white/10` / `bg-white/5` instead of `bg-bg-2 border-border`.** This is the *only* migrated page that doesn't theme correctly in light mode. Same problem in `LogsPage.tsx:107, 117, 135, 137, 158`. **Fix:** find-replace `border-white/10` → `border-border`, `bg-white/5` → `bg-bg-3`, `opacity-60` → `text-fg-3` across both files.

7. **Filter bar label "search / trust / status" is barely visible** — current `text-fg-4` + `text-[10px]` (effectively #5a5a63 12px-ish) on dark fails WCAG AA contrast. (`AgentsPage.tsx:50, 60, 72`, every page). **Fix:** bump label to `text-fg-3` (#8a8a93) and add `text-[11px] font-medium uppercase tracking-wide`. Matches the StatCard label pattern from `AppV2.tsx:440`.

8. **Filter inputs/selects all `h-7` (28px) with `text-[13px]`** — too small, fail 44px tap target, padding cramped. (`AgentsPage.tsx:54`, `TasksPage.tsx:50`, `ChatPage.tsx:479`). **Fix:** standardize to `h-8 px-3 text-[13px]` (32px), allow `h-9` on form-heavy pages like Settings.

9. **ChatPage height calc is fragile and clips on short windows** — `h-[calc(100vh-200px)] min-h-[480px]` (`ChatPage.tsx:340`) hardcodes a 200px header allowance that breaks if Topbar or filter bar height shifts. **Fix:** use `h-full` after restructuring `<main>` in `AppV2.tsx:222` to be a flex column without scroll on the body; only the chat content area scrolls.

10. **No "active filter" pill / count / clear-all** — when a user has set search="foo" + trust="high" + status="active" on AgentsPage, there's no visible "3 filters active · clear all". User has to manually reset each `<select>`. **Fix:** add a `<FilterChips>` row below the filter bar showing active filters with × to remove individually, plus a "Clear all" link when ≥2 active.

---

## Cross-cutting issues (apply to most pages)

### CC-1 Page header pattern missing
**Severity P0**
**Evidence:** Compare `AgentsPage.tsx` (jumps straight to filter bar at line 47) vs v1 `pages_c.jsx:170-173` which has KPI StatCard row.
**Affected:** 27/31 pages — all except StatusPage, BrowserConsolePage, PluginsPage, SettingsPage.
**Fix:** Standard header component:
```
<PageHeader
  title="Agents"
  subtitle="62 configured · 4 active · 0 quarantined"
  actions={<Button>+ New agent</Button>}
/>
```

### CC-2 No KPI summary on data-rich pages
**Severity P0**
**Evidence:** v1 has `StatCard` arrays with tone-coded numbers — `pages_c.jsx:1035-1038` (Audit Runtime KPIs), `pages_c.jsx:170-173` (Agent decisions/accuracy/auto/patterns).
**Affected:** TasksPage, ExecutionsPage, ReviewsPage, AuditPage, NodesPage, MemoryPage, CapabilitiesPage, ChannelsPage, ImPlatformsPage, ClusterPage, CapabilityMonitorPage, LearningPage, CuratorPage, CronPage, ToolsPage, HandoffsPage, ClarificationsPage, ChatPage.
**Fix:** Add 4-card KPI strip above each table. Example for Executions: `total · running · pending_review · failed` with `bg-bg-2 border-border p-3 rounded-lg`, label in `text-[10px] mono uppercase text-fg-3`, value in `text-2xl font-semibold mono`.

### CC-3 Table rows uniformly cramped — `px-3 py-2` everywhere
**Severity P1**
**Evidence:** `AgentsPage.tsx:122`, `TasksPage.tsx:90`, `ExecutionsPage.tsx:119`, `AuditPage.tsx:102`, all use `px-3 py-2`. With `text-xs` (12px) line-height ~16px, that gives row height ~32px. Difficult to scan and click.
**Affected:** All 20+ table pages.
**Fix:** Bump to `px-4 py-2.5` (16/10) for rows, `px-4 py-3` for `<thead>`. Or introduce density toggle (compact/comfortable) with localStorage persistence.

### CC-4 Hover state too subtle
**Severity P1**
**Evidence:** `hover:bg-hover` is `--hover: #18181b` on dark — only 8% lighter than `--bg: #0a0a0b`. Difficult to perceive on small rows. (`AgentsPage.tsx:122`)
**Fix:** Change `--hover` to `#1f1f23` (15% lighter) or add a 1px left border accent on hover: `hover:border-l-2 hover:border-l-accent` for table rows.

### CC-5 Status / Risk / Trust badge styles inconsistent across pages
**Severity P1**
**Evidence:** 
- `TasksPage.tsx:8` uses `bg-amber-500/15 text-amber-300`
- `ReviewsPage.tsx:8` uses `bg-amber-500/15 text-amber-300` (consistent)
- `LogsPage.tsx:9` uses `text-blue-400` (no bg, different pattern!)
- `AgentsPage.tsx:15-19` uses `bg-emerald-500/15` but pending=missing in TRUST_TONE table
- `CapabilityMonitorPage.tsx:20` redefines `VERDICT_TONE` with same emerald
- 8 pages define `STATUS_TONE` locally, each slightly different keys
**Fix:** Create `web/src/lib/badges.ts` exporting `BadgeTone` enum (success/warning/danger/info/neutral/critical) and `<Badge tone="success">running</Badge>` component. Centralize all status→tone mapping.

### CC-6 Inconsistent date/timestamp formatting
**Severity P1**
**Evidence:**
- `TasksPage.tsx:18` uses `toLocaleString()` (locale-aware, varies)
- `SkillsPage.tsx:22` uses `toLocaleDateString()` (date only)
- `KanbanPage.tsx:18` uses `toLocaleDateString()`
- `NodesPage.tsx:115` uses `.replace("T", " ").slice(0, 19)` (raw ISO truncation)
- `LogsPage.tsx:160` uses `toLocaleString()`
- `CronPage.tsx:21` has relative-time helper `relTime()` — only page that does
**Fix:** Centralize: `lib/format.ts` with `fmtAbsolute(iso) | fmtRelative(iso) | fmtDuration(secs)`. Use absolute for timestamp columns, relative for "last_run" / "started" hints in tooltips.

### CC-7 Mono font used everywhere — including non-code data
**Severity P1**
**Evidence:** ID column (`mono text-fg-4`) is correct usage. But many pages use `mono` for *names* (`ToolsPage.tsx:122 t.name`, `ImPlatformsPage.tsx:78 p.name`, `AdminOpsPage.tsx:64 s.name`, `MoaPage.tsx:100 p.model`), descriptions stay sans. Visually mixed.
**Fix:** Reserve mono for: IDs (`exec_id`, `agent_id`, `id`), numerals (counts, ports), code snippets, dates. Names and descriptions stay in Inter. Audit & fix per-page.

### CC-8 Action buttons in tables visually unclear
**Severity P1**
**Evidence:** `CronPage.tsx:336-359` has 3 inline `pause/run/del` buttons. Each is `px-1.5 py-0.5 text-[10px]` — that's 6px×2px padding, 10px text. The del button uses `border-rose-500/30 text-rose-400` but the other two have no semantic differentiation. No icons.
**Fix:** Use icon-only buttons (24×24, `Pause`/`Play`/`Trash2` from icons.tsx) with `title=` for screen readers. Wrap in `<ActionMenu>` triggered by `⋯` for >2 actions.

### CC-9 Error state styled differently than empty/loading
**Severity P1**
**Evidence:** Errors use `text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded` inline above the table (`AgentsPage.tsx:92`). Doesn't communicate "retry available" or what failed. No icon.
**Fix:** `<ErrorBanner icon={AlertTriangle} title="Failed to load agents" detail={err} retry={onRetry} dismiss />`. Place above table consistently.

### CC-10 No pagination, no "showing N of M"
**Severity P1**
**Evidence:** Every page calls `fetchTasks(status, 50)` / `fetchMemory(tab, 50)` / `fetchExecutions(…, 50)` — hardcoded 50. No way to see >50, no "load more". `TasksPage.tsx:33`, `ExecutionsPage.tsx:49`, `MemoryPage.tsx:44`, `ReviewsPage.tsx:40`, `AuditPage.tsx:39`.
**Fix:** Add `<Pagination total={total} pageSize={50} page={page} onChange/>` at table footer. Backend should return `{items, total, has_more}`.

### CC-11 Sidebar `v2` badge on every nav item is noise
**Severity P1**
**Evidence:** `Sidebar.tsx:138-142` puts a tiny `v2` pill in every NavLink because all 28 items have `migrated: true`. When everything is highlighted, nothing is.
**Fix:** Remove the `v2` badge once cutover is complete. Until then, only show on items that are *partially* migrated (e.g., "stub" vs "full"). Add a single `v2 (beta)` badge in the sidebar header (`Sidebar.tsx:95`) and stop spamming every item.

### CC-12 Sidebar group headings nearly invisible
**Severity P2**
**Evidence:** `Sidebar.tsx:109` uses `text-[10px] mono uppercase tracking-[0.14em] text-fg-4` — that's 10px #5a5a63 on #111113. Contrast ratio ~3:1, fails WCAG AA for text.
**Fix:** Bump to `text-[11px] text-fg-3` and add 4px more bottom-margin between groups. Or use a thin `border-t border-border/40` divider before each group label.

### CC-13 Topbar lacks contextual info
**Severity P2**
**Evidence:** `Topbar.tsx:39-83` — left side is only the collapse arrow, then a giant `flex-1` empty gap, then theme/lang/avatar/logout. v1 has search bar, breadcrumbs, mode toggle.
**Fix:** Add breadcrumb `Knowledge / Agents` left of the spacer (driven by sidebar group structure). Add Cmd+K command palette trigger button.

### CC-14 Focus states inherit browser default
**Severity P2**
**Evidence:** `index.css:76` defines `.focus-ring` but it's only applied to inputs (`AgentsPage.tsx:54`, etc.). Buttons, NavLinks, table rows lack explicit focus rings — keyboard navigation invisible.
**Fix:** Add `.focus-ring` (or equivalent) to all interactive surfaces via Tailwind `focus-visible:` variant. NavLink in Sidebar particularly. Apply: `focus-visible:ring-2 focus-visible:ring-accent-ring focus-visible:outline-none`.

### CC-15 Animations only on page mount, not on data
**Severity P2**
**Evidence:** `AppV2.tsx:223` uses `anim-fade-in` (`index.css:141`) on page swap. But tables populating, badges flipping, errors appearing all have no transition.
**Fix:** Apply `anim-slide-up` (already defined `index.css:142`) to table rows on insertion via CSS animation-delay nth-child. Smooth tone changes via `transition-colors`.

---

## Per-page findings

### Status / Dashboard (`/admin/v2/`) — `AppV2.tsx:411-435` (inline `StatusPage`)
**P0** Header text says **"v2 stack 已迁移页：Status (本页) · Models · Logs"** (line 418) which is **factually wrong** — sidebar shows all 28 items as migrated. Update to actual scope or remove.
**P0** Only 4 trivial StatCards (operator/role/stack/theme persisted) — none represent system state. v1 dashboard has runtime KPIs (active executions, pending reviews, etc.). Add real metrics: total agents, active executions, pending reviews, system uptime, recent errors.
**P1** "Quick links" section (line 426) is a bullet list with only 3 links — Status (the current page), Models, Logs. Useless. Replace with "Recent activity" feed (latest 5 audit entries) and "Needs attention" (count of pending reviews, failed executions in last hour).
**P2** No greeting/time-aware welcome ("Good afternoon, qa-admin"). Cheap win, sets professional tone.

### Chat (`/admin/v2/chat`) — `ChatPage.tsx`
**P0** Layout uses hardcoded `h-[calc(100vh-200px)] min-h-[480px]` (line 340). Breaks on short windows and short topbar. Use flex layout instead.
**P0** Conversation sidebar fixed `w-[220px]` (line 401) — too narrow for long titles, no way to resize. Should be `w-[260px] resize-x`.
**P0** Delete button uses `window.confirm()` (line 221) — looks like 1998. Use a custom confirm dialog component with the chat title in monospace and red destructive button.
**P1** Composer textarea is `rows={2}` (line 559), can't grow. Should auto-grow up to 8 rows. v1 chat composer does this.
**P1** Token ring at line 563 has tooltip but no visible legend — user doesn't know if 80% means context window or something else. Add small "% of context" label below.
**P1** Models hardcoded in `MODELS` array (line 30-35) — diverges from ModelsPage list. Should fetch from `/api/v1/models`.
**P1** Empty state in `EmptyState` (line 491) calls `t("auth.login.submit", lang).replace(...)` — that's i18n abuse; means the start-chatting button label is hacked from the login button label. Define proper `chat.start_chatting` key.
**P1** Stop button uses `bg-rose-500/15 text-rose-300` (line 567) but rest of error UI uses `bg-rose-500/10 text-rose-400`. Inconsistent.
**P2** Markdown copy-code button (line 105-115) is created by direct DOM manipulation in a `useEffect`. Fragile. Better: use a markdown renderer plugin or React-walk the AST.
**P2** Slash command hints disappear after 4-5s (line 240) — too short to read for slow users.
**P2** "thinking…" text (line 151) is in italic but `splitThink` reasoning blocks are also `<details>` collapsibles — overlap of indicators.

### Agents (`/admin/v2/agents`) — `AgentsPage.tsx`
**P0** No page header, no count summary. Cold open into filter bar.
**P0** No "+ New Agent" button — primary action missing. Link to /admin acknowledges this. Either add inline create or surface the link prominently.
**P1** Description column `truncate max-w-xs` (line 125) — long descriptions cut off without expand. Add tooltip or click-to-expand.
**P1** Filter bar has 3 fields visible side by side with no visual grouping. Use a card or border to contain them.
**P1** Tools/skills/runs columns mono-right-aligned but with `—` placeholder mixed with numbers (line 140-142). Looks misaligned. Pad numbers, or use a chart sparkline column.
**P2** TRUST_TONE missing "high"/"medium"/"low" alternatives like "trusted"/"unknown" — backend may send other values; falls back to `bg-white/10` with no border.

### Skills (`/admin/v2/skills`) — `SkillsPage.tsx`
**P0** Grid cards are tiny (`p-3` line 104) and reveal almost nothing — name, source badge, 2-line description, ID footer. v1 skill cards show: icon, name, category, version, install date, run count, source, danger flag. Restore at least version + run count.
**P0** No "Install from Hub" button — v1 has it prominently. Page header should include this CTA.
**P1** Category headers are `text-sm font-medium` with a tiny count (line 99-101) — visually weak. Add `<hr>` separator under each category title.
**P1** SOURCE_TONE table (line 25-29) only has builtin/hub/local — what about marketplace/custom? Default fallback `bg-white/10` shows mystery sources without warning.
**P2** Hover on card adds `border-border-strong` (line 104) but no shadow, no transform — feels static.

### Memory (`/admin/v2/memory`) — `MemoryPage.tsx`
**P0** L0/L1/L2 tabs are pills `bg-bg-3 border-border-strong` (line 64-67) — no visual indication of currently active vs hover. The selected tab and unselected tab look almost identical (only `text-fg` vs `text-fg-3`).
**P0** No memory-level explanations: user doesn't know what L0/L1/L2 means at first glance. Add description below tabs: "L0 = working memory (transient session-scoped)…"
**P1** Preview column truncated; no "view full content" modal. Memory records are typically the whole point — show them.
**P1** Agent filter is free-text only — should be a dropdown populated from records actually present, otherwise typo = empty result.

### Tasks (`/admin/v2/tasks`) — `TasksPage.tsx`
**P0** Title column max-width truncates aggressively (line 93 `max-w-xs`) — long task titles unreadable.
**P0** No way to view task detail / output. Tasks are the meat of the system — clicking should open a detail drawer.
**P0** No row action (cancel running, retry failed, view logs). Rows are inert.
**P1** Created/updated columns side by side — usually only one matters. Default to "updated" and put "created" in tooltip.
**P1** Status filter only 5 values, no "blocked"/"queued" — likely incomplete vs backend.

### Executions (`/admin/v2/executions`) — `ExecutionsPage.tsx`
**P0** Same as Tasks — no detail view, no action column. exec_id (line 120 `truncate max-w-[120px]`) cuts off useful info.
**P0** Capability column shows raw `capability` string — should be `connector::capability` formatted.
**P0** Duration column shows `2m3s` for done, `—` for running — should show live elapsed time on running rows.
**P1** Risk badge missing for many rows because backend may not always populate `ex.risk` (line 129). Empty risk = `—` looks identical to "Low" badge.
**P1** No filter for time range. With 50-row limit and no pagination, this is critical.

### Reviews (`/admin/v2/reviews`) — `ReviewsPage.tsx`
**P0** No "Approve / Reject" action buttons inline. Pending reviews are useless from this view — user must go to /admin to act. **This is the central governance UX, and v2 makes it read-only.**
**P0** Decision column shows raw text (line 113) — should be tone-coded badge.
**P1** Summary truncated at `max-w-xs` (line 114) — most informative column hidden.
**P2** Risk badge keys use lowercase `risk.toLowerCase()` (line 103) but `RISK_TONE` keys also lowercase — should normalize earlier, possibly at API boundary.

### Audit (`/admin/v2/audit`) — `AuditPage.tsx`
**P0** Same data as LogsPage but with `kind=mutation` filter — confusing for users that there are two log pages.
**P0** No security-relevant KPIs ("Denied last 24h", "Pending approval", "Failed mutations") despite being the security page. v1 has them (`pages_c.jsx:1035-1038`).
**P1** Target column truncated `max-w-[160px]` (line 111) — security target paths often long and truncation hides critical info.
**P1** Result column shows `success`/reason in `text-[10px] mono` (line 112) — too small for important data.

### Logs (`/admin/v2/logs`) — `LogsPage.tsx`
**P0** Uses `border-white/10` / `bg-white/5` instead of theme tokens (lines 107, 117, 135, 137, 158). Breaks light mode badly.
**P0** "live tail (SSE)" checkbox is a tiny pre-form element (line 122) — checkbox label opacity-60. Should be a proper toggle pill with on/off state.
**P1** No row click → expand to show full JSON payload. Audit entries often have details column truncated to nothing.
**P1** Filter inputs all `w-44` (line 208) — wide for short filter, hides table width.
**P1** Refresh button label re-uses `t("common.retry")` (line 119) — "Retry" wrong word for "Refresh".

### Capabilities (`/admin/v2/capabilities`) — `CapabilitiesPage.tsx`
**P0** Only 4 columns, no `effects` or `connector` link. Capability is platform's core abstraction — table is too thin.
**P1** Source filter is a select but values come from data (line 30) — no "all" placeholder differentiation; first load shows everything filtered.
**P1** Risk column only 4 values, missing "Unknown" tone for nulls.

### Tools (`/admin/v2/tools`) — `ToolsPage.tsx`
**P0** Tool name in mono font (line 122) — tool names like "BrowseUrl" or "SearchWeb" should be sans-serif.
**P0** No enable/disable toggle inline, link to /admin for promote/demote.
**P1** Effects column joins array with `, ` (line 131) — if 5+ effects, row wraps badly.

### Plugins (`/admin/v2/plugins`) — `PluginsPage.tsx`
**P1** This is one of the better pages — has page header (line 27-31), empty state with help text (line 56-67). Use this pattern elsewhere.
**P1** Icon column accepts `p.icon ?? "—"` (line 72) — text emoji renders inconsistently across OSes. Restrict to known emoji set or replace with SVG icon component.
**P2** `tab_path` column `truncate` (line 75) — usually short paths but no width control.

### Channels (`/admin/v2/channels`) — `ChannelsPage.tsx`
**P0** Empty filter bar (just an "↗ Manage" link, line 33-41). Wasteful row. Put kind filter or add-button here.
**P1** Webhook URL column long, truncated. No copy-to-clipboard button.
**P2** Secret column shows `✓` or `—` (line 78) — could show "configured" / "missing" + a "rotate" mini-action.

### IM Platforms (`/admin/v2/im-platforms`) — `ImPlatformsPage.tsx`
**P0** Empty filter bar (line 42-50). Add at least kind-filter (slack/discord/etc).
**P1** last_error column in rose-400 text (line 94) — should be in a banner above table if any platform has error, not buried in column.
**P2** No "test send" button per row — basic IM platform validation missing.

### Multimodal (`/admin/v2/multimodal`) — `MultimodalPage.tsx`
**P0** **Static hardcoded list of 4 capabilities** (line 24-29). No interactive UI. Just shows endpoint URLs. Worse than missing — it's a fake page.
**P1** "active" badge for all 4 with no actual status check — pure decoration. Replace with live ping.

### MoA / Mixture-of-Agents (`/admin/v2/moa`) — `MoaPage.tsx`
**P0** Aggregator/Proposers split into two cards with redundant headers. Combine.
**P1** Weight column right-aligned numeric (line 108) but no bar / sparkline. Hard to compare proposer weights at a glance.
**P2** No "test prompt" harness — entire point of MoA is to compare, can't from this view.

### Kanban (`/admin/v2/kanban`) — `KanbanPage.tsx`
**P0** **Read-only kanban with no drag-drop** is anti-kanban. Either implement DnD or remove the page in favor of TasksPage with status grouping.
**P1** Column heights `min-h-[320px]` (line 47) but no max-height; long columns become wall-of-cards. Add column scrolling or virtualization.
**P1** Task card shows `task.id.slice(0, 8)` (line 32) — partial IDs collide visually; user can't search by them in TasksPage.

### Workbench (`/admin/v2/workbench`) — `WorkbenchPage.tsx`
**P0** Page says "Workbench full UI on /admin" in a yellow banner (line 37-50) and shows an agent list table. This is **not a workbench** — it's an agent list. Either build the real thing (prompt-test, capability-invoke) or redirect.
**P1** Banner uses `text-fg-3` for the body — not enough visual hierarchy with the agent list below.

### Cron (`/admin/v2/cron`) — `CronPage.tsx`
**P0** Inline 3-button actions `pause/run/del` (lines 336-359) — tiny 10px text, no icons, no separation between safe (run) and destructive (del). Use icon buttons.
**P0** New job dialog has 5 fields stacked with no grouping (lines 97-150). Schedule field next to action_type but they're related. Group "what runs" vs "when it runs".
**P0** Schedule field is free-text 6-field cron (line 113) — no live validation, no human-readable preview ("Every 5 minutes"). Use a cron-builder widget or cron-parser preview.
**P1** Modal uses `fixed inset-0 z-50 bg-black/60` (line 88) — no fade-in, no Escape-key dismiss handled.
**P1** Status column shows last_status but no last error message inline.
**P2** Cancel/Create buttons in modal footer (line 153-167) — Create uses `bg-accent text-white` (white forced, breaks if accent is light). Use `text-accent-fg`.

### Models (`/admin/v2/models`) — `ModelsPage.tsx`
**P0** Hardcoded MODELS list (line 9-15) — out of sync with backend. Should fetch from `/api/v1/models`.
**P0** Uses `border-white/10 / bg-white/5` instead of theme tokens — light-mode broken.
**P0** Stats grid at bottom uses `text-2xl font-semibold mono` for value but `text-[10px] opacity-50` for label — label too faint.
**P1** Status column shows only "available" — no actual health probe per provider.
**P1** No model add/remove/test action.

### Nodes (`/admin/v2/nodes`) — `NodesPage.tsx`
**P0** Heartbeat column raw ISO truncation `replace("T", " ").slice(0,19)` (line 115) — should show "5s ago" with absolute in tooltip.
**P0** No node detail view; node_id `mono truncate max-w-[200px]` (line 106) cuts off IDs.
**P1** Role/status filter both `select` — could be segmented buttons for fewer-option enums.

### Cluster (`/admin/v2/cluster`) — `ClusterPage.tsx`
**P0** Coordinator/election-term/active-loops cards (lines 86-98) use generic `bg-bg-2 border-border` — no special treatment for the leader/coordinator card despite it being the most important.
**P0** Endpoint comment says "/api/v1/cluster returns 404" — page may show error to all users currently. Should detect 404 and show "Cluster mode disabled" empty state, not raw HTTP error.
**P1** brains list duplicates info from NodesPage. Confusing two-page split.

### Admin Ops (`/admin/v2/admin-ops`) — `AdminOpsPage.tsx`
**P0** Page is one tiny MCP servers table + an amber banner saying "go to /admin". Effectively a placeholder. Either fill in or remove from sidebar.
**P1** MCP Servers section header (line 39) puts the endpoint URL as a subtitle in `text-[10px] mono text-fg-4` — debug detail leaking into UI.

### Settings (`/admin/v2/settings`) — `SettingsPage.tsx`
**P0** Just 4-5 about info cards + a "go to /admin" CTA. No actual settings to set. Mislabeled — should be "About" or "System info".
**P0** No theme/language toggle here (it's only in Topbar) — most users look in settings first.
**P1** node_id only shown conditionally (line 63) — should show "single-node mode" if absent.

### Audit (separate from logs) — already covered above; same shape.

### Capability Monitor (`/admin/v2/capability-monitor`) — `CapabilityMonitorPage.tsx`
**P0** Endpoint `/api/v1/capability-monitor` returns 404 (comment line 2) — page perpetually shows raw HTTP 404 error to user. Detect and show clean empty state.
**P1** Reason column truncated `max-w-xs` — verdict reason is the most informative column.

### Clarifications / Handoffs / Learning / Curator / Cluster — **shared boilerplate**
**P0** All 5 pages are structurally identical: 2 filters → table → footer link. No differentiating UX despite differing data semantics. Each page should highlight what's actionable:
- **Clarifications**: pending = needs answer → primary action "Answer" button
- **Handoffs**: pending = needs accept → "Accept/Reject" inline
- **Learning**: active = streaming → progress bar or live transcript
- **Curator**: blocked = security event → audit timeline view
- **Cluster**: degraded → recovery actions
**P1** Page comments admit "Endpoint /api/v1/X returns 404 — rendered with empty-state fallback" — user sees raw HTTP 404 errors.

### Browser Console (`/admin/v2/browser-console`) — `BrowserConsolePage.tsx`
**P1** This is well-done — clear amber banner explains *why* the feature is disabled (lines 11-27), gives alternatives (lines 33-62). Use as template for "feature intentionally not migrated" pages.

---

## Component-level recommendations (build once, reuse)

### Components missing from `web/src/components/`
1. **`PageHeader.tsx`** — title + subtitle + actions slot. Use on all data pages.
2. **`StatCard.tsx`** — KPI summary tile. Extract from `AppV2.tsx:437-444`, add tone prop.
3. **`Badge.tsx`** — central status/risk/tone badge with enum-driven mapping.
4. **`Table.tsx`** + **`TableSkeleton.tsx`** + **`TableEmpty.tsx`** — extract from the 20 duplicated implementations.
5. **`FilterBar.tsx`** + **`FilterChips.tsx`** — current ad-hoc `<div className="flex flex-wrap…">` everywhere should be a real component.
6. **`Pagination.tsx`** — currently absent everywhere.
7. **`Modal.tsx`** — currently inline `fixed inset-0` in CronPage; need Esc-handler, focus-trap, scroll-lock.
8. **`ConfirmDialog.tsx`** — replaces `window.confirm()` in ChatPage and CronPage.
9. **`Toast.tsx`** — currently no transient feedback when actions succeed.
10. **`ErrorBanner.tsx`** — currently inline rose styling everywhere.
11. **`EmptyState.tsx`** — currently text-only in `<td>`.
12. **`Drawer.tsx`** — for table-row detail views.

### Tokens to add to `index.css`
- `--success` / `--warning` / `--danger` / `--info` / `--neutral` semantic colors (currently each page redefines emerald/amber/rose).
- `--badge-bg` / `--badge-fg` paired tokens.
- `--row-hover` separate from `--hover` (so we can make table hovers more visible without changing nav hovers).
- `--shadow-1` / `--shadow-2` for elevated cards (currently all flat).

---

## Suggested fix prioritization (1-week sprint sizing)

**Day 1 — Foundation (P0 blockers)**
- Remove "↗ Manage on /admin" everywhere → single Topbar entry
- Add `PageHeader` component, apply to all 27 unheaded pages
- Add `StatCard` + 4-card KPI strip to Tasks/Executions/Reviews/Audit/Memory
- Fix `border-white/10` → `border-border` in ModelsPage + LogsPage

**Day 2 — Reusable atoms**
- `Badge` component + centralized tone enum
- `Table` + `TableSkeleton` + `TableEmpty`
- `EmptyState` component, apply everywhere
- `ErrorBanner` component, apply everywhere

**Day 3 — Interaction patterns**
- `Modal` + `ConfirmDialog` (replace `window.confirm`)
- `Drawer` for row-detail
- `Toast` for action feedback
- `Pagination` component

**Day 4 — Page-specific P0s**
- Chat: flex layout + auto-grow composer + delete confirm dialog
- Cron: cron-builder widget + grouped form fields
- Reviews: inline Approve/Reject buttons
- Tasks/Executions: row-click drawer with detail

**Day 5 — Polish**
- Density bump (`px-4 py-2.5`)
- Hover state stronger
- Focus-visible rings on all interactives
- Filter labels contrast fix
- v2 badge cleanup in Sidebar

---

## Counts

- **Total findings:** 95
- **P0 (most visible / blocks usage):** 41
- **P1 (significant polish gap):** 38
- **P2 (nice-to-have):** 16
- **Pages audited:** 31 (all migrated routes)
- **Cross-cutting issues:** 15
- **Components to build:** 12

## Files referenced

- `/Users/max/project/cyberclaw/web/src/AppV2.tsx` (lines 110-179 route table, 411-444 inline StatusPage)
- `/Users/max/project/cyberclaw/web/src/index.css` (lines 5-38 tokens, 76 focus-ring, 80-95 table layout)
- `/Users/max/project/cyberclaw/web/src/components/Sidebar.tsx` (lines 15-72 nav groups, 109-145 NavLink rendering)
- `/Users/max/project/cyberclaw/web/src/components/Topbar.tsx` (lines 39-84)
- `/Users/max/project/cyberclaw/web/src/pages/*.tsx` — see per-page sections above
- v1 baseline: `/Users/max/project/cyberclaw/web/src/pages_c.jsx:170-173,1035-1038,1150-1152` (KPI patterns to port)
