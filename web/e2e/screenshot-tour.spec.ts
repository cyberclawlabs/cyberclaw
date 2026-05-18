// Screenshot every sidebar route — feeds the orchestrator's visual review.
// Output: web/playwright-screenshots/<path-slug>.png
//
// Runs against the same auth flow as page-sweep / golden-paths.

import { test, request, Page } from "@playwright/test";

const ADMIN_USER = process.env.ADMIN_USER || "qa-admin";
const JWT_KEY = "cyberclaw.admin.jwt";
const OP_KEY = "cyberclaw.admin.op";
const APP_BASE = "/admin/v2";

const PAGES = [
  "/", "/chat", "/sessions", "/profiles", "/clarifications",
  "/agents", "/skills", "/memory", "/learning",
  "/tasks", "/reviews", "/cron", "/models", "/moa",
  "/capabilities", "/plugins", "/channels", "/im-platforms",
  "/overview", "/docs", "/uploads", "/audit", "/trace", "/security",
  "/logs", "/nodes", "/settings", "/me", "/cluster", "/admin-ops",
];

function slug(p: string) {
  return p === "/" ? "root" : p.replace(/^\//, "").replace(/\//g, "-");
}

async function getAuth(baseURL: string) {
  const ctx = await request.newContext({ baseURL });
  const res = await ctx.post("/admin/login", { data: { user_id: ADMIN_USER } });
  const body = await res.json();
  return { jwt: body.jwt as string, user: body.user };
}

async function bootstrap(page: Page, baseURL: string, path: string) {
  const auth = await getAuth(baseURL);
  await page.addInitScript(
    ({ jwtKey, jwtVal, opKey, opVal }: { jwtKey: string; jwtVal: string; opKey: string; opVal: string }) => {
      try {
        window.sessionStorage.setItem(jwtKey, jwtVal);
        window.sessionStorage.setItem(opKey, opVal);
        // Force lang to zh-CN to validate Chinese translations actually
        // render (otherwise Playwright defaults to en and screenshots
        // show the EN copy regardless of what zh fixes landed).
        window.localStorage.setItem("cyberclaw.admin.lang", "zh-CN");
      } catch (_) { /* noop */ }
    },
    { jwtKey: JWT_KEY, jwtVal: auth.jwt, opKey: OP_KEY, opVal: JSON.stringify(auth.user) },
  );
  await page.goto(`${APP_BASE}${path}`);
  await page.waitForLoadState("networkidle", { timeout: 15_000 });
  await page.waitForTimeout(400); // settle
}

test.describe("Screenshot tour", () => {
  for (const p of PAGES) {
    test(`shoot ${p}`, async ({ page, baseURL }) => {
      // Set a consistent desktop viewport so layouts are comparable.
      await page.setViewportSize({ width: 1440, height: 900 });
      await bootstrap(page, baseURL!, p);
      await page.screenshot({
        path: `playwright-screenshots/${slug(p)}.png`,
        fullPage: false, // viewport-sized — captures what the user actually sees first
      });
    });
  }
});
