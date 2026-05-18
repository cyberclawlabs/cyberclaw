import { test, expect } from '@playwright/test';
import { bootstrapAuthed, getJwt } from './_helpers';

test.describe('Audit surface', () => {
  test('UI: /audit renders', async ({ page, baseURL }) => {
    await bootstrapAuthed(page, baseURL!, '/audit');
    const html = await page.content();
    expect(html.toLowerCase()).toMatch(/audit|审计|chain|trail/);
  });

  test('API: verify endpoint reports chain intact', async ({ page, baseURL }) => {
    const jwt = await getJwt(baseURL!);
    const res = await page.request.get('/api/v1/audit/verify', {
      headers: { Authorization: `Bearer ${jwt}` },
    });
    expect(res.status()).toBe(200);
    const body = await res.json();
    expect(body.corrupted_at, 'audit chain corrupted_at should be null').toBeNull();
  });

  test('API: list with limit param does not error', async ({ page, baseURL }) => {
    const jwt = await getJwt(baseURL!);
    const res = await page.request.get('/api/v1/audit?limit=5', {
      headers: { Authorization: `Bearer ${jwt}` },
    });
    expect(res.status()).toBeLessThan(500);
  });
});
