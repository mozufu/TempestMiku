import { expect, test } from '@playwright/test';

test('session API streams text, finalizes, and replay resumes by Last-Event-ID', async ({ request }) => {
  const root = await request.get('/');
  expect(root.status()).toBe(404);

  const created = await request.post('/sessions');
  expect(created.ok()).toBeTruthy();
  const session = await created.json();
  expect(session.label).toBe('Personal Assistant');

  const message = await request.post(`/sessions/${session.id}/messages`, {
    data: { content: 'please fix code hello artifact://0' },
  });
  expect(message.ok()).toBeTruthy();

  const replay = await request.get(`/sessions/${session.id}/events`, {
    headers: { 'Last-Event-ID': '1' },
    timeout: 5_000,
  });
  expect(replay.ok()).toBeTruthy();
  const body = await replay.text();
  expect(body).toContain('id: 2');
  expect(body).toContain('event: mode');
  expect(body).toContain('serious_engineer');
  expect(body).toContain('event: text');
  expect(body).toContain('Miku heard: please fix code hello artifact://0');
  expect(body).toContain('event: final');

  const replayByQuery = await request.get(`/sessions/${session.id}/events?lastEventId=1`, {
    timeout: 5_000,
  });
  expect(replayByQuery.ok()).toBeTruthy();
  expect(await replayByQuery.text()).toContain('event: mode');
});
