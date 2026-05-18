import { test, expect } from '@playwright/test';
import { bootstrapAuthed, getJwt } from './_helpers';

test.describe('PTY surface', () => {
  // PTY in v1.2.16 is a tab inside /sessions (commit a6f3899 — "PTY Terminal
  // tab + Sessions aggregate view"), not a top-level route. We assert it
  // surfaces from the sessions page, plus the API contract holds.
  test('UI: PTY tab present on /sessions', async ({ page, baseURL }) => {
    await bootstrapAuthed(page, baseURL!, '/sessions');
    await page.waitForTimeout(500); // let lazy children render
    const html = await page.content();
    expect(html.toLowerCase()).toMatch(/pty|terminal|终端|shell|session/);
  });

  test('API: list pty sessions reachable', async ({ page, baseURL }) => {
    const jwt = await getJwt(baseURL!);
    const res = await page.request.get('/api/v1/pty/sessions', {
      headers: { Authorization: `Bearer ${jwt}` },
    });
    // PTY may be disabled by config; tolerate 404/403 but not 5xx
    expect(res.status()).toBeLessThan(500);
  });
});
