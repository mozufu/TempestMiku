import { expect, Page, test } from '@playwright/test';
import fs from 'node:fs/promises';
import path from 'node:path';

const runDir = process.env.TM_E2E_RUN_DIR;
const baseURL = process.env.TM_E2E_BASE_URL;
const evidenceTest = runDir && baseURL ? test : test.skip;
const uiDir = path.join(runDir ?? '.', 'ui');
const resultPath = path.join(uiDir, 'ui-result.json');
const consolePath = path.join(uiDir, 'console.ndjson');
const networkPath = path.join(uiDir, 'network.ndjson');

evidenceTest('real Flutter UI remote-control flow records user-visible evidence', async ({ page }) => {
  await fs.mkdir(uiDir, { recursive: true });
  const consoleRecords: unknown[] = [];
  const networkRecords: unknown[] = [];
  let sessionId: string | null = null;
  let screenshotPath: string | null = null;

  page.on('console', (message) => {
    consoleRecords.push({
      timestamp: new Date().toISOString(),
      type: message.type(),
      text: message.text(),
    });
  });
  page.on('pageerror', (error) => {
    consoleRecords.push({
      timestamp: new Date().toISOString(),
      type: 'pageerror',
      text: error.stack ?? error.message,
    });
  });
  page.on('request', (request) => {
    if (!isSessionApi(request.url())) return;
    networkRecords.push({
      timestamp: new Date().toISOString(),
      kind: 'request',
      method: request.method(),
      url: request.url(),
      postData: redactText(request.postData() ?? ''),
    });
  });
  page.on('response', async (response) => {
    if (!isSessionApi(response.url())) return;
    networkRecords.push({
      timestamp: new Date().toISOString(),
      kind: 'response',
      status: response.status(),
      url: response.url(),
    });
    if (response.url().endsWith('/sessions') && response.request().method() === 'POST') {
      try {
        const json = await response.json();
        sessionId = json.id ?? sessionId;
      } catch {
        // Non-fatal: the UI also persists the session id in localStorage.
      }
    }
  });

  try {
    await page.goto('/', { waitUntil: 'domcontentloaded' });
    await enableFlutterAccessibility(page);
    sessionId = await waitForSessionId(page);
    await waitForFonts(page);
    await page.screenshot({ path: path.join(uiDir, 'ui-loaded.png'), fullPage: true });
    await setHandoffMode(page, sessionId);

    await sendPrompt(
      page,
      'handoff actor approval for recording evidence: spawn the child and return artifact://0',
    );

    const approval = await waitForPendingApproval(page, sessionId);
    await waitForApprovalPaint(page);
    await page.screenshot({ path: path.join(uiDir, 'ui-approval-visible.png'), fullPage: true });
    const approvalResponse = await page.request.post(
      `/sessions/${sessionId}/approvals/${approval.approvalId}`,
      { data: { decision: 'approve' } },
    );
    expect(approvalResponse.ok()).toBeTruthy();

    await waitForAssistantFinal(page, sessionId, 'artifact://0');
    await page.screenshot({ path: path.join(uiDir, 'ui-final-visible.png'), fullPage: true });

    const artifactPreview = await page.request.get(
      `/sessions/${sessionId}/resources/preview?uri=${encodeURIComponent('artifact://0')}`,
    );
    expect(artifactPreview.ok()).toBeTruthy();
    const artifactJson = await artifactPreview.json();
    expect(JSON.stringify(artifactJson)).toContain('child smoke artifact');

    const promote = await page.request.post(`/sessions/${sessionId}/promote`, {
      data: {
        summary: 'UI evidence promoted the actor artifact.',
        openLoops: ['keep the UI recording evidence covered'],
        decisions: ['use tm-e2e record suite as the UI evidence gate'],
        resources: ['artifact://0'],
      },
    });
    expect(promote.ok()).toBeTruthy();
    const project = await page.request.get(`/sessions/${sessionId}/project`);
    expect(project.ok()).toBeTruthy();
    expect(JSON.stringify(await project.json())).toContain('project://tempestmiku');

    await page.reload({ waitUntil: 'domcontentloaded' });
    await waitForEventResume(networkRecords, sessionId);

    screenshotPath = path.join(uiDir, 'ui-remote-control-final.png');
    await page.screenshot({ path: screenshotPath, fullPage: true });
    await writeNdjson(consolePath, consoleRecords);
    await writeNdjson(networkPath, networkRecords);
    await fs.writeFile(
      resultPath,
      `${JSON.stringify(
        {
          ok: true,
          sessionId,
          screenshotPath,
          consolePath,
          networkPath,
        },
        null,
        2,
      )}\n`,
    );
  } catch (error) {
    screenshotPath = path.join(uiDir, 'ui-remote-control-failure.png');
    await page.screenshot({ path: screenshotPath, fullPage: true }).catch(() => {});
    await writeNdjson(consolePath, consoleRecords).catch(() => {});
    await writeNdjson(networkPath, networkRecords).catch(() => {});
    await fs
      .writeFile(
        resultPath,
        `${JSON.stringify(
          {
            ok: false,
            sessionId,
            screenshotPath,
            consolePath,
            networkPath,
            error: error instanceof Error ? error.stack ?? error.message : String(error),
          },
          null,
          2,
        )}\n`,
      )
      .catch(() => {});
    throw error;
  }
});

async function sendPrompt(page: Page, prompt: string) {
  await clickComposer(page);
  await page.keyboard.insertText(prompt);
  await page.waitForTimeout(250);
  await clickSubmit(page);
}

async function enableFlutterAccessibility(page: Page) {
  const button = page.getByRole('button', { name: 'Enable accessibility' });
  if ((await button.count()) === 0) return;
  await button.click({ force: true, timeout: 5_000 }).catch(() => {});
  await page.waitForTimeout(250);
}

async function clickComposer(page: Page) {
  const textbox = page.getByRole('textbox').first();
  if ((await textbox.count()) > 0) {
    await textbox.click({ timeout: 5_000 });
    return;
  }
  const viewport = page.viewportSize();
  if (!viewport) throw new Error('viewport is unavailable');
  await page.mouse.click(Math.floor(viewport.width * 0.38), viewport.height - 55);
}

async function clickSubmit(page: Page) {
  const submit = page.getByRole('button', { name: /submit|send|送出/i }).last();
  if ((await submit.count()) > 0 && (await submit.isVisible().catch(() => false))) {
    await submit.click({ timeout: 5_000 });
    return;
  }
  const viewport = page.viewportSize();
  if (!viewport) throw new Error('viewport is unavailable');
  await page.mouse.click(viewport.width - 38, viewport.height - 55);
}

async function waitForSessionId(page: Page) {
  await page.waitForFunction(
    () => window.localStorage.getItem('tempestmiku.sessionId'),
    null,
    { timeout: 60_000 },
  );
  const sessionId = await page.evaluate(() => window.localStorage.getItem('tempestmiku.sessionId'));
  if (!sessionId) throw new Error('session id was not persisted by the Flutter UI');
  return sessionId;
}

async function waitForFonts(page: Page) {
  await page.evaluate(() => document.fonts?.ready ?? Promise.resolve());
  await page.waitForTimeout(750);
}

async function setHandoffMode(page: Page, sessionId: string) {
  const response = await page.request.post(`/sessions/${sessionId}/mode/override`, {
    data: {
      mode: 'handoff',
      reason: 'UI evidence actor approval flow',
      source: 'tm-e2e',
    },
  });
  expect(response.ok()).toBeTruthy();
}

async function waitForApprovalPaint(page: Page) {
  await page.waitForTimeout(1_000);
}

type PendingApproval = {
  approvalId: string;
};

async function waitForPendingApproval(page: Page, sessionId: string): Promise<PendingApproval> {
  return pollJson(async () => {
    const response = await page.request.get(`/sessions/${sessionId}/messages`);
    expect(response.ok()).toBeTruthy();
    const json = await response.json();
    const event = (json.pendingEvents ?? []).find(
      (item: any) => item.type === 'approval' && item.data?.backend === 'native-deno',
    );
    if (!event?.data?.approvalId) return null;
    return { approvalId: event.data.approvalId };
  }, 60_000);
}

async function waitForAssistantFinal(page: Page, sessionId: string, needle: string) {
  return pollJson(async () => {
    const response = await page.request.get(`/sessions/${sessionId}/messages`);
    expect(response.ok()).toBeTruthy();
    const json = await response.json();
    const final = (json.messages ?? []).find(
      (item: any) => item.role === 'assistant' && `${item.content}`.includes(needle),
    );
    return final ?? null;
  }, 60_000);
}

async function waitForEventResume(networkRecords: unknown[], sessionId: string) {
  const started = Date.now();
  while (Date.now() - started < 60_000) {
    if (
      networkRecords.some((record: any) => {
        return (
          record.kind === 'request' &&
          `${record.url}`.includes(`/sessions/${sessionId}/events`) &&
          `${record.url}`.includes('lastEventId=')
        );
      })
    ) {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error('UI did not reconnect to the session event stream with lastEventId');
}

async function pollJson<T>(load: () => Promise<T | null>, timeoutMs: number): Promise<T> {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    const value = await load();
    if (value) return value;
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`timed out after ${timeoutMs}ms`);
}

function isSessionApi(url: string) {
  return url.includes('/sessions') || url.endsWith('/health') || url.endsWith('/modes');
}

function redactText(text: string) {
  if (/bearer\s+/i.test(text)) return '[REDACTED]';
  return text.replace(/("?(?:token|apiKey|api_key|secret|authorization)"?\s*:\s*)"[^"]*"/gi, '$1"[REDACTED]"');
}

async function writeNdjson(file: string, records: unknown[]) {
  const body = records.map((record) => JSON.stringify(record)).join('\n');
  await fs.writeFile(file, body.length > 0 ? `${body}\n` : '');
}
