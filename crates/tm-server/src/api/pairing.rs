use axum::{
    http::{HeaderMap, header},
    response::{Html, IntoResponse},
};
use qrcode::{QrCode, render::svg};

use crate::{Result, ServerError};

pub(crate) async fn page(headers: HeaderMap) -> Html<String> {
    let base_url = public_base_url(&headers);
    Html(pairing_page(&base_url))
}

pub(crate) async fn qr_svg(headers: HeaderMap) -> Result<impl IntoResponse> {
    let base_url = public_base_url(&headers);
    let link = pairing_link(&base_url);
    let code = QrCode::new(link.as_bytes())
        .map_err(|err| ServerError::InvalidRequest(format!("could not build pairing QR: {err}")))?;
    let image = code
        .render::<svg::Color<'_>>()
        .min_dimensions(256, 256)
        .build();
    Ok((
        [
            (header::CONTENT_TYPE, "image/svg+xml; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        image,
    ))
}

fn public_base_url(headers: &HeaderMap) -> String {
    derive_public_base_url(headers, std::env::var("TM_PUBLIC_BASE_URL").ok().as_deref())
}

pub(crate) fn derive_public_base_url(headers: &HeaderMap, env_override: Option<&str>) -> String {
    if let Some(value) = env_override
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return trim_base_url(value);
    }

    let scheme = forwarded_proto(headers).unwrap_or("http");
    let host = forwarded_host(headers)
        .or_else(|| header_first(headers, header::HOST.as_str()))
        .unwrap_or("127.0.0.1:8787");
    trim_base_url(&format!("{scheme}://{host}"))
}

fn pairing_page(base_url: &str) -> String {
    let link = pairing_link(base_url);
    let base_url_html = escape_html(base_url);
    let link_html = escape_html(&link);
    let warning = loopback_warning(base_url)
        .map(|message| {
            format!(
                r#"<section class="warning"><strong>Loopback target</strong><p>{}</p></section>"#,
                escape_html(message)
            )
        })
        .unwrap_or_default();

    format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>TempestMiku Pairing</title>
  <style>
    :root {{
      color-scheme: light dark;
      --bg: #0f1418;
      --panel: #161d22;
      --text: #ecf2f4;
      --muted: #9fb0b7;
      --border: #2b3a41;
      --accent: #5fd0c5;
      --warn: #f6c35b;
    }}
    * {{ box-sizing: border-box; }}
    body {{
      margin: 0;
      min-height: 100vh;
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
      background: var(--bg);
      color: var(--text);
      display: grid;
      place-items: center;
      padding: 24px;
    }}
    main {{
      width: min(920px, 100%);
      display: grid;
      gap: 22px;
    }}
    header {{
      display: flex;
      justify-content: space-between;
      gap: 18px;
      align-items: end;
      border-bottom: 1px solid var(--border);
      padding-bottom: 18px;
    }}
    h1 {{
      font-size: clamp(2rem, 6vw, 4.4rem);
      line-height: 0.95;
      margin: 0;
      letter-spacing: 0;
    }}
    .health {{
      border: 1px solid var(--border);
      border-radius: 8px;
      padding: 10px 12px;
      color: var(--muted);
      white-space: nowrap;
    }}
    .health b {{ color: var(--accent); }}
    .grid {{
      display: grid;
      grid-template-columns: minmax(220px, 320px) 1fr;
      gap: 22px;
      align-items: start;
    }}
    .qr, .details, .warning {{
      border: 1px solid var(--border);
      border-radius: 8px;
      background: var(--panel);
    }}
    .qr {{ padding: 18px; display: grid; gap: 14px; justify-items: center; }}
    .qr img {{
      width: min(256px, 100%);
      height: auto;
      background: white;
      border-radius: 8px;
      padding: 10px;
    }}
    .details {{ padding: 20px; display: grid; gap: 16px; }}
    label {{ color: var(--muted); font-size: 0.86rem; }}
    input {{
      width: 100%;
      margin-top: 6px;
      padding: 12px;
      border-radius: 8px;
      border: 1px solid var(--border);
      background: #0b1013;
      color: var(--text);
      font: inherit;
    }}
    .actions {{ display: flex; flex-wrap: wrap; gap: 10px; }}
    a, button {{
      min-height: 42px;
      border-radius: 8px;
      border: 1px solid var(--border);
      padding: 10px 14px;
      color: var(--text);
      background: #202a30;
      text-decoration: none;
      font: inherit;
      cursor: pointer;
    }}
    a.primary {{ background: var(--accent); color: #061012; border-color: var(--accent); }}
    .manual, .link {{ color: var(--muted); overflow-wrap: anywhere; }}
    .warning {{ border-color: color-mix(in srgb, var(--warn) 70%, var(--border)); padding: 14px 16px; }}
    .warning strong {{ color: var(--warn); }}
    .warning p {{ margin: 6px 0 0; color: var(--muted); }}
    @media (max-width: 720px) {{
      header {{ align-items: start; flex-direction: column; }}
      .grid {{ grid-template-columns: 1fr; }}
      .health {{ white-space: normal; }}
    }}
  </style>
</head>
<body>
  <main>
    <header>
      <h1>TempestMiku Pairing</h1>
      <div class="health">tm-server health: <b>ok</b></div>
    </header>
    {warning}
    <section class="grid">
      <div class="qr">
        <img src="/pair/qr.svg" alt="Android pairing QR code">
        <div class="manual">Scan from Android, or open the pairing link below.</div>
      </div>
      <div class="details">
        <label>Server URL
          <input id="server-url" readonly value="{base_url_html}">
        </label>
        <div class="actions">
          <a class="primary" href="{link_html}">Open Android App</a>
          <button type="button" onclick="navigator.clipboard && navigator.clipboard.writeText(document.getElementById('server-url').value)">Copy URL</button>
          <a href="/">Open Web App</a>
        </div>
        <div class="manual">Manual fallback: Android app -> More -> Server target -> paste the Server URL.</div>
        <div class="link">{link_html}</div>
      </div>
    </section>
  </main>
</body>
</html>"#
    )
}

pub(crate) fn pairing_link(base_url: &str) -> String {
    format!(
        "tempestmiku://pair?server={}",
        percent_encode_query(trim_base_url(base_url).as_bytes())
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

fn loopback_warning(base_url: &str) -> Option<&'static str> {
    let lower = base_url.to_ascii_lowercase();
    let host = lower
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(lower.as_str())
        .split('/')
        .next()
        .unwrap_or("")
        .split('@')
        .next_back()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");
    matches!(host, "127.0.0.1" | "localhost" | "10.0.2.2").then_some(
        "This server URL is only local to the current machine or emulator; use TM_PUBLIC_BASE_URL with a LAN or tailnet host for a physical Android device.",
    )
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
