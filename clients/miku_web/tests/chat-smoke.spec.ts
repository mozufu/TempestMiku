import { expect, test, type APIRequestContext } from '@playwright/test';

test('session API streams text, finalizes, and replay resumes by Last-Event-ID', async ({ request }) => {
  const root = await request.get('/');
  expect(root.status()).toBe(200);
  expect(root.headers()['content-type']).toContain('text/html');
  expect(await root.text()).toContain('<title>Tempest Miku</title>');

  const pairingCode = await request.post('/auth/pairing-codes');
  expect(pairingCode.ok()).toBeTruthy();
  const pairing = await pairingCode.json();
  const paired = await request.post('/auth/pair', {
    data: {
      code: pairing.code,
      deviceName: 'Playwright smoke',
      platform: 'playwright',
    },
  });
  expect(paired.ok()).toBeTruthy();
  const token = (await paired.json()).token as string;
  expect(token).toMatch(/^tmk_dev_/);
  const auth = { authorization: `Bearer ${token}` };

  const created = await request.post('/sessions', { headers: auth });
  expect(created.ok()).toBeTruthy();
  const session = await created.json();
  expect(session.label).toBe('General');

  const prompt = 'hello web smoke';
  const message = await request.post(`/sessions/${session.id}/messages`, {
    headers: auth,
    data: { clientMessageId: `web-smoke-${Date.now()}`, content: prompt },
  });
  expect(message.ok()).toBeTruthy();
  expect(message.status()).toBe(202);
  const accepted = await message.json();
  await waitForTurn(request, session.id, accepted.turnId, auth);
  const ended = await request.post(`/sessions/${session.id}/end`, {
    headers: auth,
    data: {},
  });
  expect(ended.ok()).toBeTruthy();

  const replay = await request.get(`/sessions/${session.id}/events`, {
    headers: { ...auth, 'Last-Event-ID': '1' },
    timeout: 5_000,
  });
  expect(replay.ok()).toBeTruthy();
  const body = await replay.text();
  expect(body).toContain('id: 2');
  expect(body).not.toMatch(/event: (?!session_event)/);
  const streamed = parseSessionEvents(body);
  expect(streamed.map((event) => event.envelope.type)).toEqual(
    expect.arrayContaining(['text', 'final', 'session_end', 'dream_queued']),
  );
  expect(streamed.every((event) => Number.isSafeInteger(event.id) && event.id > 0)).toBeTruthy();
  expect(
    streamed.every(
      (event) => event.envelope.turnId === null || typeof event.envelope.turnId === 'string',
    ),
  ).toBeTruthy();
  expect(
    streamed.every(
      (event) =>
        Object.keys(event.envelope).sort().join(',') === 'createdAt,payload,turnId,type',
    ),
  ).toBeTruthy();
  expect(body).toContain(`Miku heard: ${prompt}`);

  const replayByQuery = await request.get(`/sessions/${session.id}/events?lastEventId=1`, {
    headers: auth,
    timeout: 5_000,
  });
  expect(replayByQuery.ok()).toBeTruthy();
  expect(parseSessionEvents(await replayByQuery.text()).some(
    (event) => event.envelope.type === 'text',
  )).toBeTruthy();

  const history = await request.get('/sessions?limit=5', { headers: auth });
  expect(history.ok()).toBeTruthy();
  const historyJson = await history.json();
  expect(historyJson.sessions[0].id).toBe(session.id);
  expect(historyJson.sessions[0].title).toContain(`Miku heard: ${prompt}`);

  const transcript = await request.get(`/sessions/${session.id}/messages`, { headers: auth });
  expect(transcript.ok()).toBeTruthy();
  const transcriptJson = await transcript.json();
  expect(transcriptJson.messages).toHaveLength(2);
  expect(transcriptJson.messages[0].role).toBe('user');
  expect(transcriptJson.lastEventId).toBeGreaterThanOrEqual(3);
});

function parseSessionEvents(body: string) {
  return body
    .split(/\r?\n\r?\n/)
    .map((frame) => frame.trim())
    .filter((frame) => frame.includes('event: session_event'))
    .map((frame) => {
      const lines = frame.split(/\r?\n/);
      const id = Number(lines.find((line) => line.startsWith('id: '))?.slice(4));
      const data = lines
        .filter((line) => line.startsWith('data: '))
        .map((line) => line.slice(6))
        .join('\n');
      return { id, envelope: JSON.parse(data) as Record<string, unknown> };
    });
}

async function waitForTurn(
  request: APIRequestContext,
  sessionId: string,
  turnId: string,
  headers: Record<string, string>,
) {
  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline) {
    const response = await request.get(`/sessions/${sessionId}/turns/${turnId}`, { headers });
    expect(response.ok()).toBeTruthy();
    const turn = await response.json();
    if (turn.status === 'completed') return;
    if (turn.status === 'failed') throw new Error(`turn failed: ${turn.error ?? 'unknown error'}`);
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(`turn ${turnId} did not complete`);
}
