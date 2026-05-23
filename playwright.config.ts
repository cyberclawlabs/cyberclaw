import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './tests/e2e',
  // Real-LLM E2E (CYBERCLAW_E2E_REAL_LLM=1) needs ≥120s per case for MiniMax 12-iteration loops.
  // Non-LLM cases finish in ≤5s so the longer ceiling is harmless when LLM gate is off.
  timeout: process.env.CYBERCLAW_E2E_REAL_LLM === '1' ? 240000 : 30000,
  retries: 0,
  use: {
    baseURL: process.env.CYBERCLAW_TEST_BASE_URL || 'http://127.0.0.1:38090',
    headless: true,
    trace: 'on-first-retry',
    screenshot: 'only-on-failure',
  },
  webServer: process.env.CYBERCLAW_AUTO_START === '1'
    ? {
        command: 'cargo run -p cyberclaw-server',
        url: 'http://127.0.0.1:38090/health',
        timeout: 60000,
        reuseExistingServer: !process.env.CI,
        env: {
          ENVIRONMENT: 'development',
          USE_TLS: 'false',
          CYBERCLAW_ADDR: '127.0.0.1:38090',
          CYBERCLAW_CLUSTER_SHARED_TOKEN:
            'test_cluster_token_32chars_min_placeholder',
          // Sync with QA_JWT_SECRET in tests/e2e/helpers/auth.ts so
          // pre-signed QA tokens validate against the auto-started server.
          JWT_SECRET: 'change-this-to-a-real-secret-at-least-32-chars',
          // LLM client is constructed unconditionally at startup (main.rs
          // create_llm_client). E2E tests don't exercise LLM paths — provide
          // a dummy so server boots cleanly.
          LLM_PROVIDER: 'openai',
          LLM_API_KEY: 'sk-e2e-placeholder-not-used-in-tests',
          LLM_BASE_URL: 'http://127.0.0.1:1',
          // Lift rate limit so E2E suite (which fires many calls in
          // succession from one IP) doesn't hit 429. Defaults are 1 r/s +
          // burst 60 — fine for prod, too tight for back-to-back tests.
          // Sprint 29-34 added 6 new admin spec files (~70 backend cases);
          // alphabetically-last spec was hitting 429 at 100/500. Bump to
          // 1000/5000 so the worst case (entire suite serially against one
          // IP) still has headroom.
          RATE_LIMIT_PER_SECOND: '1000',
          RATE_LIMIT_BURST_SIZE: '5000',
        },
      }
    : undefined,
  projects: [
    {
      name: 'chromium',
      use: { browserName: 'chromium' },
    },
  ],
});
