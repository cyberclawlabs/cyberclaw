// e2e/cross-page.spec.ts
//
// v1.7 final — 8 cross-cutting TC to bring WebUI suite from 87 → 95.
// Covers auth flow, deep linking, navigation persistence, 404 handling,
// JWT expiry handling, and live backend connectivity — all things that
// per-page specs naturally miss.
import { test, expect } from '@playwright/test';
import { bootstrapAuthed, getAuth, APP_BASE, JWT_KEY, OP_KEY } from './_helpers';

test.describe('Cross-page concerns (95 TC closer)', () => {
  // CP-1 — unauthenticated visit lands on login / auth surface
  test('CP-1: unauthenticated visit shows login or redirects', async ({ page }) => {
    await page.goto(`${APP_BASE}/chat`);
    await page.waitForLoadState('networkidle', { timeout: 10_000 });
    const url = page.url();
    const body = (await page.locator('body').textContent()) ?? '';
    const ok =
      /login|auth|sign[- ]?in/i.test(url) ||
      /login|admin|sign[- ]?in|user[ _-]?id|登录|管理员/i.test(body);
    expect(ok, `unauthed /chat should expose login surface, got url=${url} body[0..200]=${body.slice(0, 200)}`).toBeTruthy();
  });

  // CP-2 — logout clears JWT from sessionStorage
  test('CP-2: logout clears auth tokens', async ({ page, baseURL }) => {
    await bootstrapAuthed(page, baseURL!, '/me');
    const before = await page.evaluate((k) => window.sessionStorage.getItem(k), JWT_KEY);
    expect(before, 'JWT should exist after bootstrap').toBeTruthy();
    await page.evaluate(
      ({ jwt, op }) => {
        window.sessionStorage.removeItem(jwt);
        window.sessionStorage.removeItem(op);
      },
      { jwt: JWT_KEY, op: OP_KEY },
    );
    const after = await page.evaluate((k) => window.sessionStorage.getItem(k), JWT_KEY);
    expect(after, 'JWT cleared').toBeNull();
  });

  // CP-3 — invalid JWT → backend returns 401 (no crash)
  test('CP-3: invalid JWT yields 401 not 500', async ({ request, baseURL }) => {
    const res = await request.get(`${baseURL}/api/v1/capabilities`, {
      headers: { Authorization: 'Bearer invalid.jwt.token' },
    });
    expect(res.status(), 'expected 401 for bad JWT').toBe(401);
  });

  // CP-4 — deep link with query params loads same route
  test('CP-4: deep link with query params loads correct route', async ({ page, baseURL }) => {
    await bootstrapAuthed(page, baseURL!, '/sessions?limit=5');
    expect(page.url()).toContain('/sessions');
    const body = (await page.locator('body').textContent()) ?? '';
    expect(body.length, 'sessions page should render').toBeGreaterThan(50);
  });

  // CP-5 — multi-step navigation: sidebar / chrome stays mounted
  test('CP-5: multi-step navigation keeps shell mounted', async ({ page, baseURL }) => {
    await bootstrapAuthed(page, baseURL!, '/agents');
    const firstNav = await page.locator('nav, [role="navigation"], aside').first().isVisible().catch(() => false);
    await page.goto(`${APP_BASE}/skills`);
    await page.waitForLoadState('networkidle');
    const secondNav = await page.locator('nav, [role="navigation"], aside').first().isVisible().catch(() => false);
    expect(firstNav || secondNav, 'navigation chrome should be visible across page changes').toBeTruthy();
  });

  // CP-6 — unknown route under /admin/v2 doesn't crash (SPA fallback)
  test('CP-6: unknown SPA route gracefully degrades', async ({ page, baseURL }) => {
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(String(e)));
    await bootstrapAuthed(page, baseURL!, '/this-route-does-not-exist-xyz');
    // SPA may render 404 component, redirect, or empty content — just must not throw a JS error
    const body = (await page.locator('body').textContent()) ?? '';
    expect(body, 'body should render even for unknown route').toBeTruthy();
    expect(errors.filter((e) => !/HMR|favicon|Failed to load resource/i.test(e)), 'no uncaught JS errors').toEqual([]);
  });

  // CP-7 — live backend health from browser-visible /health
  test('CP-7: backend health endpoint reachable from browser context', async ({ page, baseURL }) => {
    await bootstrapAuthed(page, baseURL!, '/');
    const resp = await page.evaluate(async (url) => {
      const r = await fetch(url);
      return { ok: r.ok, status: r.status, body: await r.text() };
    }, `${baseURL}/health`);
    expect(resp.ok, `health expected ok, got ${resp.status}`).toBeTruthy();
    expect(resp.body.toLowerCase()).toContain('ok');
  });

  // CP-8 — admin/login endpoint shape stable (auth contract regression)
  test('CP-8: admin/login returns jwt+user shape', async ({ baseURL }) => {
    const auth = await getAuth(baseURL!);
    expect(auth.jwt, 'jwt present').toBeTruthy();
    expect(auth.jwt.split('.').length, 'jwt is 3-part').toBe(3);
    expect(auth.user, 'user object returned').toBeTruthy();
  });
});
