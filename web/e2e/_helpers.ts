import { request, Page } from '@playwright/test';

export const ADMIN_USER = process.env.ADMIN_USER || 'qa-admin';
export const JWT_KEY = 'cyberclaw.admin.jwt';
export const OP_KEY = 'cyberclaw.admin.op';
export const APP_BASE = '/admin/v2';

export interface AuthBundle {
  jwt: string;
  user: unknown;
}

export async function getAuth(baseURL: string): Promise<AuthBundle> {
  const ctx = await request.newContext({ baseURL });
  const res = await ctx.post('/admin/login', { data: { user_id: ADMIN_USER } });
  if (!res.ok()) throw new Error(`admin login failed: ${res.status()}`);
  const body = await res.json();
  return { jwt: body.jwt as string, user: body.user };
}

export async function getJwt(baseURL: string): Promise<string> {
  return (await getAuth(baseURL)).jwt;
}

export async function bootstrapAuthed(page: Page, baseURL: string, path: string) {
  const auth = await getAuth(baseURL);
  await page.addInitScript(
    ({ jwtKey, jwtVal, opKey, opVal }: { jwtKey: string; jwtVal: string; opKey: string; opVal: string }) => {
      try {
        window.sessionStorage.setItem(jwtKey, jwtVal);
        window.sessionStorage.setItem(opKey, opVal);
      } catch (_) {
        /* noop */
      }
    },
    { jwtKey: JWT_KEY, jwtVal: auth.jwt, opKey: OP_KEY, opVal: JSON.stringify(auth.user) },
  );
  await page.goto(`${APP_BASE}${path}`);
  await page.waitForLoadState('networkidle');
}
