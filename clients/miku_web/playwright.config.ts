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
    command: `OPENAI_API_KEY= OPENAI_BASE_URL= TM_OMP_ACP_ENABLED=0 TM_SERVER_ROLE=all TM_SERVER_ADDR=127.0.0.1:${port} cargo run -p tm-server`,
    url: `${baseURL}/health`,
    reuseExistingServer: !process.env.CI,
    timeout: 300_000,
  },
});
