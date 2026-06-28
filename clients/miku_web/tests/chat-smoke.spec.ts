import { expect, test } from '@playwright/test';

test('chat streams text, finalizes, and replay resumes by Last-Event-ID', async ({ page, request }) => {
  await page.goto('/');
  await page.getByPlaceholder('Message Miku').fill('hello');
  await page.getByRole('button', { name: 'Send' }).click();

  await expect(page.locator('#stream')).toContainText('Miku heard: hello');
  await expect(page.locator('#stream')).toHaveAttribute('data-final', 'Miku heard: hello');

  const sessionId = await page.evaluate(() => window.localStorage.getItem('tm-session-id'));
  expect(sessionId).toBeTruthy();

  const replay = await request.get(`/sessions/${sessionId}/events`, {
    headers: { 'Last-Event-ID': '1' },
    timeout: 5_000,
  });
  expect(replay.ok()).toBeTruthy();
  const body = await replay.text();
  expect(body).toContain('id: 2');
  expect(body).toContain('event: text');
  expect(body).toContain('event: final');
});
