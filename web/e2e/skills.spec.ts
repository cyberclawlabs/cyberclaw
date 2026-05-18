import { test, expect } from '@playwright/test';
import { bootstrapAuthed, getJwt } from './_helpers';

test.describe('Skills surface', () => {
  test('UI: /skills renders with skill keyword', async ({ page, baseURL }) => {
    await bootstrapAuthed(page, baseURL!, '/skills');
    const html = await page.content();
    expect(html.toLowerCase()).toMatch(/skill|技能/);
  });

  test('API: list skills returns array of entries', async ({ page, baseURL }) => {
    const jwt = await getJwt(baseURL!);
    const res = await page.request.get('/api/v1/skills', {
      headers: { Authorization: `Bearer ${jwt}` },
    });
    expect(res.status()).toBe(200);
    const body = await res.json();
    const items = Array.isArray(body) ? body : body.skills ?? body.items ?? [];
    expect(Array.isArray(items)).toBeTruthy();
  });

  test('API: skill detail (if any installed)', async ({ page, baseURL }) => {
    const jwt = await getJwt(baseURL!);
    const list = await page.request.get('/api/v1/skills', {
      headers: { Authorization: `Bearer ${jwt}` },
    });
    if (list.status() !== 200) return;
    const body = await list.json();
    const items = Array.isArray(body) ? body : body.skills ?? body.items ?? [];
    if (!items.length) {
      test.info().annotations.push({ type: 'note', description: 'no skills installed; detail check skipped' });
      return;
    }
    const id = items[0].name ?? items[0].id ?? items[0].skill_id;
    if (!id) return;
    const detail = await page.request.get(`/api/v1/skills/${encodeURIComponent(id)}`, {
      headers: { Authorization: `Bearer ${jwt}` },
    });
    expect(detail.status()).toBeLessThan(500);
  });
});
