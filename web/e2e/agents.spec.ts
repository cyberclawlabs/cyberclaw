import { test, expect } from '@playwright/test';
import { bootstrapAuthed, getJwt } from './_helpers';

test.describe('Agents surface', () => {
  test('UI: /agents renders', async ({ page, baseURL }) => {
    await bootstrapAuthed(page, baseURL!, '/agents');
    const html = await page.content();
    expect(html.toLowerCase()).toMatch(/agent|代理|智能体/);
  });

  test('API: list agents', async ({ page, baseURL }) => {
    const jwt = await getJwt(baseURL!);
    const res = await page.request.get('/api/v1/agents', {
      headers: { Authorization: `Bearer ${jwt}` },
    });
    expect(res.status()).toBe(200);
    const body = await res.json();
    const list = Array.isArray(body) ? body : body.agents ?? body.items ?? [];
    expect(Array.isArray(list)).toBeTruthy();
  });

  test('API: agent detail (if any exists)', async ({ page, baseURL }) => {
    const jwt = await getJwt(baseURL!);
    const list = await page.request.get('/api/v1/agents', {
      headers: { Authorization: `Bearer ${jwt}` },
    });
    if (list.status() !== 200) return;
    const body = await list.json();
    const items = Array.isArray(body) ? body : body.agents ?? body.items ?? [];
    if (!items.length) {
      test.info().annotations.push({ type: 'note', description: 'no agents configured; detail check skipped' });
      return;
    }
    const first = items[0];
    const id = first.name ?? first.id ?? first.agent_id;
    if (!id) return;
    const detail = await page.request.get(`/api/v1/agents/${id}`, {
      headers: { Authorization: `Bearer ${jwt}` },
    });
    expect(detail.status()).toBeLessThan(500);
  });
});
