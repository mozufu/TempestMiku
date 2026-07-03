import { expect, test } from '@playwright/test';

test('session API streams text, finalizes, and replay resumes by Last-Event-ID', async ({ request }) => {
  const root = await request.get('/');
  expect(root.status()).toBe(200);
  expect(root.headers()['content-type']).toContain('text/html');
  expect(await root.text()).toContain('TempestMiku Remote');

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

  const history = await request.get('/sessions?limit=5');
  expect(history.ok()).toBeTruthy();
  const historyJson = await history.json();
  expect(historyJson.sessions[0].id).toBe(session.id);
  expect(historyJson.sessions[0].title).toBe('Miku heard: please fix code hello artifact://0');

  const transcript = await request.get(`/sessions/${session.id}/messages`);
  expect(transcript.ok()).toBeTruthy();
  const transcriptJson = await transcript.json();
  expect(transcriptJson.messages).toHaveLength(2);
  expect(transcriptJson.messages[0].role).toBe('user');
  expect(transcriptJson.lastEventId).toBeGreaterThanOrEqual(3);
});
