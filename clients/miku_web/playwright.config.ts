import { defineConfig } from '@playwright/test';

const port = process.env.TM_WEB_SMOKE_PORT ?? '8787';
const baseURL = `http://127.0.0.1:${port}`;
const serverCommand = process.env.CI
  ? `cd ../.. && OPENAI_API_KEY= OPENAI_BASE_URL= TM_OMP_ACP_ENABLED=0 TM_SERVER_ROLE=all TM_SERVER_ADDR=127.0.0.1:${port} target/debug/tm-server`
  : `nix shell nixpkgs#postgresql_16 --command bash scripts/start-smoke-server.sh ${port}`;

export default defineConfig({
  testDir: './tests',
  timeout: 30_000,
  use: {
    baseURL,
  },
  webServer: {
    command: serverCommand,
    url: `${baseURL}/health`,
    reuseExistingServer: !process.env.CI,
    timeout: 300_000,
  },
});
