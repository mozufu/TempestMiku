import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './tests',
  timeout: 30_000,
  use: {
    baseURL: 'http://127.0.0.1:8787',
  },
  webServer: {
    command: 'cargo run -p tm-server',
    url: 'http://127.0.0.1:8787/health',
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});
