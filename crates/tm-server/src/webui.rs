use axum::{Router, response::Html, routing::get};

pub fn routes<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new().route("/", get(index))
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

const INDEX_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>TempestMiku</title>
  <style>
    body { font-family: system-ui, sans-serif; max-width: 48rem; margin: 2rem auto; padding: 0 1rem; }
    #badge { display: inline-block; padding: .25rem .5rem; border: 1px solid #888; border-radius: 999px; }
    #stream { white-space: pre-wrap; border: 1px solid #ddd; min-height: 8rem; padding: 1rem; }
  </style>
</head>
<body>
  <h1>TempestMiku</h1>
  <p id="badge">Personal Assistant</p>
  <form id="chat">
    <input id="message" autocomplete="off" placeholder="Message Miku" />
    <button>Send</button>
  </form>
  <pre id="stream"></pre>
  <script>
    let sessionId = window.localStorage.getItem('tm-session-id');
    let events;
    const stream = document.getElementById('stream');
    function openEvents(id) {
      if (events) return;
      events = new EventSource(`/sessions/${id}/events`);
      events.addEventListener('text', ev => { stream.textContent += JSON.parse(ev.data).delta; });
      events.addEventListener('final', ev => { stream.dataset.final = JSON.parse(ev.data).text; });
      events.addEventListener('mode', ev => {
        const payload = JSON.parse(ev.data);
        document.getElementById('badge').textContent = payload.label || 'Personal Assistant';
      });
    }
    async function ensureSession() {
      if (sessionId) { openEvents(sessionId); return sessionId; }
      const res = await fetch('/sessions', { method: 'POST' });
      const json = await res.json();
      sessionId = json.id;
      window.localStorage.setItem('tm-session-id', sessionId);
      openEvents(sessionId);
      return sessionId;
    }
    document.getElementById('chat').addEventListener('submit', async ev => {
      ev.preventDefault();
      const id = await ensureSession();
      const input = document.getElementById('message');
      await fetch(`/sessions/${id}/messages`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ content: input.value })
      });
      input.value = '';
    });
  </script>
</body>
</html>"#;
