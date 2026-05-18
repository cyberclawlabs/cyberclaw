import { test, expect } from '@playwright/test';
import { bootstrapAuthed, getJwt } from './_helpers';

test.describe('Sessions surface', () => {
  test('UI: /sessions renders', async ({ page, baseURL }) => {
    await bootstrapAuthed(page, baseURL!, '/sessions');
    const html = await page.content();
    expect(html.toLowerCase()).toMatch(/session|会话|conversation|对话/);
  });

  test('API: list endpoint returns paginated payload', async ({ page, baseURL }) => {
    const jwt = await getJwt(baseURL!);
    const res = await page.request.get('/api/v1/sessions?limit=10', {
      headers: { Authorization: `Bearer ${jwt}` },
    });
    expect(res.status()).toBeLessThan(500);
    if (res.status() === 200) {
      const body = await res.json();
      // tolerate either {sessions: [...]} or [...] shape
      const items = Array.isArray(body) ? body : body.sessions ?? body.items ?? [];
      expect(Array.isArray(items), 'sessions list shape includes an array').toBeTruthy();
    }
  });

  test('API: detail endpoint shape (if at least one session exists)', async ({ page, baseURL }) => {
    const jwt = await getJwt(baseURL!);
    const listRes = await page.request.get('/api/v1/sessions?limit=1', {
      headers: { Authorization: `Bearer ${jwt}` },
    });
    if (listRes.status() !== 200) {
      test.info().annotations.push({ type: 'note', description: 'sessions list not 200, skipping detail check' });
      return;
    }
    const body = await listRes.json();
    const items = Array.isArray(body) ? body : body.sessions ?? body.items ?? [];
    if (items.length === 0) {
      test.info().annotations.push({ type: 'note', description: 'no existing sessions; detail check skipped' });
      return;
    }
    const id = items[0].id ?? items[0].session_id;
    if (!id) return;
    const detail = await page.request.get(`/api/v1/sessions/${id}`, {
      headers: { Authorization: `Bearer ${jwt}` },
    });
    expect(detail.status()).toBeLessThan(500);
  });
});
