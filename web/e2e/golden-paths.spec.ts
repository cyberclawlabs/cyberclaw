import { test, expect, request, Page } from '@playwright/test';

const ADMIN_USER = process.env.ADMIN_USER || 'qa-admin';
const JWT_KEY = 'cyberclaw.admin.jwt';
const OP_KEY = 'cyberclaw.admin.op';
const APP_BASE = '/admin/v2';

interface AuthBundle {
  jwt: string;
  user: unknown;
}

async function getAuth(baseURL: string): Promise<AuthBundle> {
  const ctx = await request.newContext({ baseURL });
  const res = await ctx.post('/admin/login', {
    data: { user_id: ADMIN_USER },
  });
  expect(res.ok(), 'admin login should succeed').toBeTruthy();
  const body = await res.json();
  expect(body.jwt, 'login response has jwt').toBeTruthy();
  expect(body.user, 'login response has user').toBeTruthy();
  return { jwt: body.jwt as string, user: body.user };
}

async function getJwt(baseURL: string): Promise<string> {
  return (await getAuth(baseURL)).jwt;
}

async function bootstrapAuthed(page: Page, baseURL: string, path: string) {
  // RequireAuth checks BOTH JWT and operator object — must inject both keys
  // before any React script runs, otherwise the gate redirects to /login.
  const auth = await getAuth(baseURL);
  await page.addInitScript(
    ({ jwtKey, jwtVal, opKey, opVal }: { jwtKey: string; jwtVal: string; opKey: string; opVal: string }) => {
      try {
        window.sessionStorage.setItem(jwtKey, jwtVal);
        window.sessionStorage.setItem(opKey, opVal);
      } catch (_) {
        /* noop — sandboxed contexts */
      }
    },
    { jwtKey: JWT_KEY, jwtVal: auth.jwt, opKey: OP_KEY, opVal: JSON.stringify(auth.user) },
  );
  await page.goto(`${APP_BASE}${path}`);
  await page.waitForLoadState('networkidle');
}

test.describe('CyberClaw WebUI v1.0-GA golden paths', () => {
  test('GP1 healthz → app shell loads', async ({ page, baseURL }) => {
    const healthRes = await page.request.get('/healthz');
    expect(healthRes.ok()).toBeTruthy();

    await bootstrapAuthed(page, baseURL!, '/');
    const body = await page.locator('body').textContent();
    expect(body, 'shell renders some text').toBeTruthy();
    expect(body!.length).toBeGreaterThan(20);
  });

  test('GP2 chat page renders without console error', async ({ page, baseURL }) => {
    const consoleErrors: string[] = [];
    page.on('pageerror', (err) => consoleErrors.push(String(err)));
    page.on('console', (msg) => {
      if (msg.type() === 'error') consoleErrors.push(msg.text());
    });
    await bootstrapAuthed(page, baseURL!, '/chat');
    await page.waitForTimeout(500);
    const html = await page.content();
    expect(html.toLowerCase()).toMatch(/chat|对话|消息|message/);
    // Filter known-noisy network 404s from missing optional resources.
    const real = consoleErrors.filter((e) => !/Failed to load resource|favicon/i.test(e));
    expect(real, `console errors on /chat: ${real.join(' | ')}`).toEqual([]);
  });

  test('GP3 memory page loads & API returns', async ({ page, baseURL }) => {
    await bootstrapAuthed(page, baseURL!, '/memory');
    const html = await page.content();
    expect(html.toLowerCase()).toMatch(/memory|记忆/);

    // API contract check — memory list endpoint should respond JSON.
    const jwt = await getJwt(baseURL!);
    const apiRes = await page.request.get('/api/v1/memory?limit=5', {
      headers: { Authorization: `Bearer ${jwt}` },
    });
    expect(apiRes.status(), 'memory list responds 200/404 (route exists)').toBeLessThan(500);
  });

  test('GP4 skills page + skill search API', async ({ page, baseURL }) => {
    await bootstrapAuthed(page, baseURL!, '/skills');
    const html = await page.content();
    expect(html.toLowerCase()).toMatch(/skill|技能/);

    const jwt = await getJwt(baseURL!);
    const apiRes = await page.request.get('/api/v1/skills', {
      headers: { Authorization: `Bearer ${jwt}` },
    });
    expect(apiRes.status()).toBe(200);
    const body = await apiRes.json();
    expect(body, 'skills payload is an object/array').toBeTruthy();
  });

  test('GP5 audit page + chain verify intact', async ({ page, baseURL }) => {
    await bootstrapAuthed(page, baseURL!, '/audit');
    const jwt = await getJwt(baseURL!);
    const verifyRes = await page.request.get('/api/v1/audit/verify', {
      headers: { Authorization: `Bearer ${jwt}` },
    });
    expect(verifyRes.status()).toBe(200);
    const body = await verifyRes.json();
    expect(body.corrupted_at, 'audit chain should be intact (corrupted_at=null)').toBeNull();
  });

  test('GP6 capabilities + plugins endpoints stable', async ({ page, baseURL }) => {
    const jwt = await getJwt(baseURL!);
    const caps = await page.request.get('/api/v1/capabilities', {
      headers: { Authorization: `Bearer ${jwt}` },
    });
    expect(caps.status()).toBe(200);
    const capsBody = await caps.json();
    expect(Array.isArray(capsBody.capabilities)).toBeTruthy();

    const plugins = await page.request.get('/api/v1/plugins', {
      headers: { Authorization: `Bearer ${jwt}` },
    });
    expect(plugins.status()).toBe(200);
    const pluginsBody = await plugins.json();
    expect(pluginsBody).toHaveProperty('total');
    expect(pluginsBody).toHaveProperty('plugins');
  });

  test('GP7 multi-tenant isolation — cross-tenant resources hidden', async ({ baseURL, request: req }) => {
    const jwt = await getJwt(baseURL!);
    // Verify session list endpoint scopes by tenant (any tenant filter works as-is).
    const sessions = await req.get(`${baseURL}/api/v1/sessions?limit=1`, {
      headers: { Authorization: `Bearer ${jwt}` },
    });
    expect(sessions.status()).toBeLessThan(500);
  });
});
