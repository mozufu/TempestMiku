import { defineConfig } from '@playwright/test';
import path from 'node:path';

const runDir = process.env.TM_E2E_RUN_DIR ?? path.resolve('../../target/tm-e2e/runs/manual-ui');
const baseURL = process.env.TM_E2E_BASE_URL ?? 'http://127.0.0.1:8787';

export default defineConfig({
  testDir: './tests',
  testMatch: /evidence-.*\.spec\.ts/,
  timeout: 180_000,
  workers: 1,
  reporter: [
    ['list'],
    ['json', { outputFile: path.join(runDir, 'ui', 'playwright-report.json') }],
    ['html', { outputFolder: path.join(runDir, 'ui', 'playwright-html'), open: 'never' }],
  ],
  outputDir: path.join(runDir, 'ui', 'playwright-output'),
  use: {
    baseURL,
    viewport: { width: 390, height: 844 },
    headless: process.env.TM_E2E_HEADED === '1' ? false : true,
    screenshot: 'on',
    trace: 'on',
    video: 'on',
  },
});
