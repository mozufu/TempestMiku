import { defineConfig } from '@playwright/test';

const port = process.env.TM_WEB_SMOKE_PORT ?? '8787';
const baseURL = `http://127.0.0.1:${port}`;

export default defineConfig({
  testDir: './tests',
  timeout: 30_000,
  use: {
    baseURL,
  },
  webServer: {
    command: `nix shell nixpkgs#postgresql_16 --command bash scripts/start-smoke-server.sh ${port}`,
    url: `${baseURL}/health`,
    reuseExistingServer: !process.env.CI,
    timeout: 300_000,
  },
});
