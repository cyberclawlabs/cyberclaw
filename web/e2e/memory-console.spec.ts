import { test, expect } from '@playwright/test';
import { bootstrapAuthed, getJwt } from './_helpers';

test.describe('Memory console surface', () => {
  test('UI: /memory renders', async ({ page, baseURL }) => {
    await bootstrapAuthed(page, baseURL!, '/memory');
    const html = await page.content();
    expect(html.toLowerCase()).toMatch(/memory|记忆|episod|semantic|procedural/);
  });

  test('API: memory list endpoint reachable', async ({ page, baseURL }) => {
    const jwt = await getJwt(baseURL!);
    const res = await page.request.get('/api/v1/memory?limit=5', {
      headers: { Authorization: `Bearer ${jwt}` },
    });
    expect(res.status()).toBeLessThan(500);
  });

  test('API: memory search endpoint (light contract)', async ({ page, baseURL }) => {
    const jwt = await getJwt(baseURL!);
    // some deployments use POST + JSON body, others use GET + query
    const res = await page.request.get('/api/v1/memory/search?q=test&limit=3', {
      headers: { Authorization: `Bearer ${jwt}` },
    });
    // 404 acceptable if search endpoint is POST-only; mainly catching 500s
    expect(res.status()).toBeLessThan(500);
  });
});
