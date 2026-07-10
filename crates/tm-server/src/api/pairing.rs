use std::net::SocketAddr;

use axum::{
    extract::{ConnectInfo, State},
    http::{HeaderMap, header},
    response::Html,
};
use qrcode::{QrCode, render::svg};

use crate::{AppState, AuthConfig, ChatRunner, MemoryProvider, Result, ServerError, Store};

pub(crate) async fn page<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    headers: HeaderMap,
    connect: Option<ConnectInfo<SocketAddr>>,
) -> Result<Html<String>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let peer = connect.map(|value| value.0);
    let base_url = public_base_url(&headers, peer, &state);
    let issued = super::auth_devices::issue_pairing_code(&state, &headers, peer, &base_url).await?;
    let qr = QrCode::new(issued.pairing_link.as_bytes())
        .map_err(|err| ServerError::InvalidRequest(format!("could not build pairing QR: {err}")))?
        .render::<svg::Color<'_>>()
        .min_dimensions(256, 256)
        .build();
    Ok(Html(pairing_page(
        &base_url,
        &issued.code,
        &issued.expires_at.to_rfc3339(),
        &qr,
    )))
}

pub(super) fn public_base_url<S, M, C>(
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
    state: &AppState<S, M, C>,
) -> String {
    let trust_forwarded = match state.auth.config() {
        AuthConfig::Forwarded(config) => peer.map(|peer| config.trusts(peer.ip())).unwrap_or(false),
        _ => false,
    };
    derive_public_base_url(
        headers,
        std::env::var("TM_PUBLIC_BASE_URL").ok().as_deref(),
        trust_forwarded,
    )
}

pub(crate) fn derive_public_base_url(
    headers: &HeaderMap,
    env_override: Option<&str>,
    trust_forwarded: bool,
) -> String {
    if let Some(value) = env_override
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return trim_base_url(value);
    }

    let scheme = trust_forwarded
        .then(|| forwarded_proto(headers))
        .flatten()
        .unwrap_or("http");
    let host = trust_forwarded
        .then(|| forwarded_host(headers))
        .flatten()
        .or_else(|| header_first(headers, header::HOST.as_str()))
        .unwrap_or("127.0.0.1:8787");
    trim_base_url(&format!("{scheme}://{host}"))
}

fn pairing_page(base_url: &str, code: &str, expires_at: &str, qr: &str) -> String {
    let base_url_html = escape_html(base_url);
    let code_html = escape_html(code);
    let expires_html = escape_html(expires_at);
    let code_json = serde_json::to_string(code).expect("pairing code serializes");

    format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta name="referrer" content="no-referrer">
  <title>TempestMiku secure pairing</title>
  <style>
    :root {{ color-scheme: dark; --bg:#0f1418; --panel:#161d22; --text:#ecf2f4; --muted:#9fb0b7; --border:#2b3a41; --accent:#5fd0c5; }}
    * {{ box-sizing:border-box; }}
    body {{ margin:0; min-height:100vh; font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif; background:var(--bg); color:var(--text); display:grid; place-items:center; padding:24px; }}
    main {{ width:min(900px,100%); display:grid; gap:20px; }}
    h1 {{ margin:0; font-size:clamp(2rem,6vw,4rem); line-height:1; }}
    .lede,.meta {{ color:var(--muted); }}
    .grid {{ display:grid; grid-template-columns:minmax(240px,320px) 1fr; gap:20px; }}
    .card {{ border:1px solid var(--border); border-radius:12px; background:var(--panel); padding:20px; display:grid; gap:15px; }}
    .qr svg {{ width:100%; height:auto; padding:10px; background:white; border-radius:8px; }}
    code {{ overflow-wrap:anywhere; color:var(--accent); }}
    button,a {{ min-height:44px; border-radius:8px; border:1px solid var(--border); padding:11px 14px; color:var(--text); background:#202a30; text-decoration:none; font:inherit; cursor:pointer; text-align:center; }}
    button.primary,a.primary {{ background:var(--accent); color:#061012; border-color:var(--accent); }}
    .actions {{ display:grid; gap:10px; }}
    @media (max-width:700px) {{ .grid {{ grid-template-columns:1fr; }} }}
  </style>
</head>
<body>
  <main>
    <header>
      <h1>Secure pairing</h1>
      <p class="lede">This one-time code expires in five minutes and can authorize exactly one device.</p>
    </header>
    <section class="grid">
      <div class="card qr">
        {qr}
      </div>
      <div class="card">
        <div><div class="meta">Server</div><code>{base_url_html}</code></div>
        <div><div class="meta">One-time code</div><code>{code_html}</code></div>
        <div><div class="meta">Expires</div><code>{expires_html}</code></div>
        <div class="actions">
          <button id="pair-web" class="primary" type="button">Pair this web browser</button>
        </div>
        <p id="status" class="meta">Only continue on a server you recognize.</p>
      </div>
    </section>
  </main>
  <script>
    document.getElementById('pair-web').addEventListener('click', async () => {{
      const status = document.getElementById('status');
      status.textContent = 'Pairing…';
      const response = await fetch('/auth/pair', {{
        method: 'POST',
        credentials: 'same-origin',
        headers: {{'content-type':'application/json'}},
        body: JSON.stringify({{code:{code_json}, deviceName:'Web browser', platform:'web'}})
      }});
      if (!response.ok) {{ status.textContent = 'Pairing failed or the code expired. Refresh this page.'; return; }}
      location.assign('/');
    }});
  </script>
</body>
</html>"#
    )
}

pub(crate) fn pairing_link(base_url: &str, code: &str) -> String {
    format!(
        "tempestmiku://pair?v=1&server={}&code={}",
        percent_encode_query(trim_base_url(base_url).as_bytes()),
        percent_encode_query(code.as_bytes())
    )
}

fn forwarded_proto(headers: &HeaderMap) -> Option<&str> {
    header_first(headers, "x-forwarded-proto")
        .or_else(|| forwarded_pair(headers, "proto"))
        .map(|value| if value == "https" { "https" } else { "http" })
}

fn forwarded_host(headers: &HeaderMap) -> Option<&str> {
    header_first(headers, "x-forwarded-host").or_else(|| forwarded_pair(headers, "host"))
}

fn forwarded_pair<'a>(headers: &'a HeaderMap, key: &str) -> Option<&'a str> {
    let value = header_first(headers, "forwarded")?;
    for part in value.split(';') {
        let Some((name, raw_value)) = part.trim().split_once('=') else {
            continue;
        };
        if name.eq_ignore_ascii_case(key) {
            return Some(raw_value.trim_matches('"'));
        }
    }
    None
}

fn header_first<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)?
        .to_str()
        .ok()?
        .split(',')
        .map(str::trim)
        .find(|value| !value.is_empty())
}

fn trim_base_url(value: &str) -> String {
    let trimmed = value.trim();
    let without_fragment = trimmed.split('#').next().unwrap_or(trimmed);
    let without_query = without_fragment
        .split('?')
        .next()
        .unwrap_or(without_fragment);
    without_query.trim_end_matches('/').to_string()
}

fn escape_html(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn percent_encode_query(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(bytes.len());
    for &byte in bytes {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char)
            }
            _ => {
                encoded.push('%');
                encoded.push(HEX[(byte >> 4) as usize] as char);
                encoded.push(HEX[(byte & 0x0f) as usize] as char);
            }
        }
    }
    encoded
}
