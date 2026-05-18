import { test, expect, request, Page } from '@playwright/test';

const ADMIN_USER = process.env.ADMIN_USER || 'qa-admin';
const JWT_KEY = 'cyberclaw.admin.jwt';
const OP_KEY = 'cyberclaw.admin.op';
const APP_BASE = '/admin/v2';

const PAGES = [
  '/', '/chat', '/sessions', '/profiles', '/clarifications',
  '/agents', '/skills', '/memory', '/learning',
  '/tasks', '/reviews', '/cron', '/models', '/moa',
  '/capabilities', '/plugins', '/channels', '/im-platforms',
  '/overview', '/docs', '/uploads', '/audit', '/trace', '/security',
  '/logs', '/nodes', '/settings', '/me', '/cluster', '/admin-ops',
];

const NOISE_PATTERNS = [
  /Failed to load resource/i,
  /favicon/i,
  /net::ERR_/i,
  /\[HMR\]/i,
];

async function getAuth(baseURL: string) {
  const ctx = await request.newContext({ baseURL });
  const res = await ctx.post('/admin/login', { data: { user_id: ADMIN_USER } });
  expect(res.ok(), 'admin login should succeed').toBeTruthy();
  const body = await res.json();
  expect(body.jwt).toBeTruthy();
  return { jwt: body.jwt as string, user: body.user };
}

async function bootstrap(page: Page, baseURL: string, path: string) {
  const auth = await getAuth(baseURL);
  await page.addInitScript(
    ({ jwtKey, jwtVal, opKey, opVal }: { jwtKey: string; jwtVal: string; opKey: string; opVal: string }) => {
      try {
        window.sessionStorage.setItem(jwtKey, jwtVal);
        window.sessionStorage.setItem(opKey, opVal);
      } catch (_) { /* noop */ }
    },
    { jwtKey: JWT_KEY, jwtVal: auth.jwt, opKey: OP_KEY, opVal: JSON.stringify(auth.user) },
  );
  await page.goto(`${APP_BASE}${path}`);
  await page.waitForLoadState('networkidle', { timeout: 15_000 });
}

test.describe('CyberClaw WebUI page sweep — every sidebar route loads cleanly', () => {
  for (const p of PAGES) {
    test(`page-sweep ${p}`, async ({ page, baseURL }) => {
      const errors: string[] = [];
      page.on('pageerror', (e) => errors.push(`pageerror: ${String(e)}`));
      page.on('console', (m) => {
        if (m.type() === 'error') errors.push(`console: ${m.text()}`);
      });

      await bootstrap(page, baseURL!, p);
      // Light settle — give async data fetches time to resolve.
      await page.waitForTimeout(300);

      const body = (await page.locator('body').textContent()) ?? '';
      expect(body.length, `${p} body should render >50 chars`).toBeGreaterThan(50);

      const real = errors.filter((e) => !NOISE_PATTERNS.some((rx) => rx.test(e)));
      expect(real, `${p} unexpected console errors: ${real.join(' | ')}`).toEqual([]);
    });
  }
});
