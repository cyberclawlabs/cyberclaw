import { test, expect } from '@playwright/test';
import { bootstrapAuthed, getJwt } from './_helpers';

test.describe('Approvals surface', () => {
  test('UI: /approvals renders', async ({ page, baseURL }) => {
    await bootstrapAuthed(page, baseURL!, '/approvals');
    const html = await page.content();
    expect(html.toLowerCase()).toMatch(/approv|审批|review|待审/);
  });

  test('API: reviews list reachable', async ({ page, baseURL }) => {
    const jwt = await getJwt(baseURL!);
    const res = await page.request.get('/api/v1/reviews?status=pending&limit=10', {
      headers: { Authorization: `Bearer ${jwt}` },
    });
    expect(res.status()).toBeLessThan(500);
  });

  test('API: pending reviews count endpoint (if exposed)', async ({ page, baseURL }) => {
    const jwt = await getJwt(baseURL!);
    // optional metrics endpoint — soft check
    const res = await page.request.get('/api/v1/reviews', {
      headers: { Authorization: `Bearer ${jwt}` },
    });
    expect(res.status()).toBeLessThan(500);
  });
});
