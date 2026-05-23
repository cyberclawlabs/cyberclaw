# Security Advisory — v1.0.0 — JWT in WebSocket / SSE URL Query String

- **ID**: CC-2026-05-16-01
- **Severity**: P0 (architectural) / Medium (effective, in private-deployment context)
- **Status**: Disclosed. Targeted fix in v1.0.1.
- **Reported by**: typescript-reviewer agent during 2026-05-16 top-tier UI/code audit
- **Cross-referenced by**: designer agent (independent finding)
- **Affected versions**: v0.x — v1.0.0
- **Affected paths**:
  - `GET /api/v1/pty/ws?token=<JWT>`
  - `GET /api/v1/audit/logs/stream?token=<JWT>`
- **Frontend call sites**:
  - `web/src/pages/ChatPage.tsx:421` — `ptyWsUrl`
  - `web/src/lib/api.ts:125` — `streamAuditLogs`

## Problem

The CyberClaw admin WebSocket (PTY) and Server-Sent Events (audit log stream)
endpoints accept the JWT bearer token as a `?token=` URL query parameter.
URL query strings have a wider exposure surface than HTTP headers:

1. **Server access logs** — many reverse proxies (nginx, traefik, ALB) log the
   full request URL including query string by default.
2. **Browser history / DevTools network panel** — the full URL with token is
   visible to anyone with desktop access.
3. **Referer header** — when a page on the same origin makes outbound requests
   to third parties (CDN, analytics, telemetry), the browser may send the
   current page URL as Referer.
4. **`location.href` / `window.location.search`** — any JavaScript running on
   the page (including XSS payloads, browser extensions, injected scripts)
   can read the token.
5. **Browser autocomplete / form history** — depending on browser configuration,
   the URL may be cached.

## Effective Impact (in CyberClaw context)

CyberClaw v1.0 is positioned primarily as a **private-deployment** controlled
agent platform:

| Exposure vector | Realistic risk |
|---|---|
| Server access logs | Medium — local logs are typically firewalled; rotated regularly. JWT TTL is 24h so window is bounded. |
| Browser history | Low — admin workstation single-user |
| Referer header | Low — the admin app doesn't make outbound third-party requests on those pages |
| `location.href` JS read | Medium — relies on the `dangerouslySetInnerHTML` + DOMPurify chain being intact. Currently sanitised. |
| Browser autocomplete | Low — most modern browsers don't autocomplete `ws://` / SSE URLs |

**Compensating controls already in place**:
- `dangerouslySetInnerHTML` is consistently sanitized via DOMPurify + marked
  with `gfm: true` — XSS via assistant message content is mitigated.
- JWT bearer has a short-ish TTL (24h) — token theft has bounded value.
- All PTY/SSE access requires admin role, which is itself behind login.
- Audit chain logs the auth event itself; any unauthorized stream connection
  leaves an SHA-256-chained record.

**Net assessment**: real but bounded for private deployment. Material for
public-facing or multi-tenant cloud SaaS deployment. **Does not block v1.0.0
private-deployment GA**; **must be fixed before any public-internet exposure
or SaaS positioning**.

## Reproduction

```bash
# JWT bearer is visible in the URL
curl -i "http://127.0.0.1:38090/api/v1/audit/logs/stream?token=eyJ0eXAi…"
# → access log on server emits:
#   GET /api/v1/audit/logs/stream?token=eyJ0eXAi… 200 -
```

A privileged log reader (root, log aggregator, attacker who breached the
log server) can replay any logged token until expiry.

## Fix Plan (v1.0.1)

Introduce a one-shot, single-use, short-lived **WebSocket ticket** that the
client exchanges for the JWT just before connecting. The JWT itself never
appears in the URL.

### Backend changes

1. **New module** `apps/cyberclaw-server/src/ws_ticket.rs`:
   ```rust
   pub struct TicketStore {
       inner: Arc<Mutex<HashMap<String, (Claims, Instant)>>>,
       ttl: Duration,  // default 60s
   }
   impl TicketStore {
       pub fn mint(&self, claims: Claims) -> String { /* uuid v4 */ }
       pub fn consume(&self, ticket: &str) -> Option<Claims> { /* single-use pop */ }
       pub fn evict_expired(&self) { /* called periodically */ }
   }
   ```

2. **Inject into `AppState`** (`apps/cyberclaw-server/src/state.rs`):
   ```rust
   pub ws_ticket_store: Arc<TicketStore>,
   ```

3. **New endpoint** `POST /api/v1/auth/ws-ticket`:
   - Goes through standard JWT middleware (extracts `Claims` from `Authorization: Bearer ...`)
   - Calls `ticket_store.mint(claims)` and returns:
     ```json
     { "ticket": "<uuid>", "expires_at": "2026-05-16T10:30:00Z" }
     ```

4. **Modify `pty.rs::pty_ws_handler`** and `audit.rs::stream_logs`:
   - Accept new `?ticket=<uuid>` param alongside legacy `?token=<jwt>`.
   - If `ticket` present: call `ws_ticket_store.consume(ticket)` (which pops).
   - If `token` present (legacy): log `warn!("legacy ?token= used; please migrate to ?ticket=")`, then accept for backward compat in v1.0.x; remove in v1.1.

5. **Background task** spawned at startup: every 60s call `evict_expired()`.

### Frontend changes

1. **New helper** in `web/src/lib/api.ts`:
   ```ts
   export async function mintWsTicket(): Promise<string> {
     const resp = await apiFetch<{ ticket: string }>(
       "/api/v1/auth/ws-ticket", { method: "POST" });
     return resp.ticket;
   }
   ```

2. **Refactor `streamAuditLogs`** to be async: fetch ticket, then return EventSource.
   ```ts
   export async function streamAuditLogs(q: LogsQuery = {}): Promise<EventSource> {
     const ticket = await mintWsTicket();
     const params = /* ... existing filters ... */;
     params.set("ticket", ticket);
     return new EventSource(`/api/v1/audit/logs/stream?${params}`);
   }
   ```
   Update caller `LogsPage.tsx` to `await` it inside an effect.

3. **Refactor PTY URL generation** in `ChatPage.tsx`:
   ```ts
   const [ptyWsUrl, setPtyWsUrl] = useState<string | null>(null);
   useEffect(() => {
     if (view !== "terminal") return;
     mintWsTicket().then((ticket) => {
       const scheme = location.protocol === "https:" ? "wss" : "ws";
       setPtyWsUrl(`${scheme}://${location.host}/api/v1/pty/ws?ticket=${ticket}`);
     });
   }, [view]);
   ```

### Test plan

- Unit: `TicketStore::mint` returns 36-char uuid; `consume` returns Some once then None; `consume` returns None for expired; `evict_expired` removes only past-TTL.
- Integration: full handshake — JWT → `POST /auth/ws-ticket` → 200 + ticket → `GET /pty/ws?ticket=...` → WebSocket upgrade succeeds, second call with same ticket → 401.
- E2E (Playwright): ChatPage terminal tab still connects; LogsPage live tail still streams.
- Security regression: `?token=<jwt>` still works (warning logged) for one minor version; ticket attempted twice → second 401.

### Migration

- v1.0.1: ship the ticket endpoint + accept both legacy `?token=` and new `?ticket=`. Issue deprecation warning in server log when `?token=` is used. Frontend switches to `?ticket=` immediately.
- v1.1.0: remove `?token=` query support entirely. Bearer JWT only valid for issuing a ticket via the auth endpoint.
- Release note: document the change; if any third-party tooling was using `?token=` directly (e.g. a curl-based ops script), it must switch to the two-step flow.

## v1.1 backlog entry

Added to `docs/implementation/reports/v1.1-backlog.md` as item **#33**:

| # | Item | Severity | Effort |
|---|---|---|---|
| 33 | JWT-in-URL → ticket-exchange backend | **SEC P0** | ~250 lines (backend module + 2 endpoint updates + frontend async refactor + tests) |

## Workaround until v1.0.1

For users who must ship v1.0 to a non-private network *now* and cannot wait
for v1.0.1:

1. Front the CyberClaw server with a reverse proxy that **strips query string
   from access logs**:
   ```
   # nginx
   log_format clean '$remote_addr - $remote_user [$time_local] '
                    '"$request_method $uri" $status $body_bytes_sent';
   access_log /var/log/nginx/cyberclaw_clean.log clean;
   ```
2. Disable Referer header propagation on the admin page via meta tag:
   ```html
   <meta name="referrer" content="no-referrer">
   ```
3. Rotate the JWT signing secret and force re-login if a leak is suspected.

These are mitigations only — they do not remove the architectural problem.
The proper fix is v1.0.1.
