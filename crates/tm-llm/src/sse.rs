//! Server-Sent-Events parsing: a byte stream of `data:` lines -> [`StreamEvent`]s.

use async_stream::try_stream;
use futures::stream::{BoxStream, Stream, StreamExt};

use tm_core::{Error, Result, StreamEvent};

use crate::wire;

/// Adapt a byte stream (e.g. `reqwest::Response::bytes_stream()`) into a stream of
/// [`StreamEvent`]s. Lines are buffered until a newline so multi-byte UTF-8 split across
/// network chunks is never mis-decoded. Unparseable `data:` payloads are skipped rather than
/// killing the stream — OpenAI-compatible servers vary.
pub fn events<S, B>(stream: S) -> BoxStream<'static, Result<StreamEvent>>
where
    S: Stream<Item = std::result::Result<B, reqwest::Error>> + Send + 'static,
    B: AsRef<[u8]> + Send + 'static,
{
    Box::pin(try_stream! {
        futures::pin_mut!(stream);
        let mut buf: Vec<u8> = Vec::new();
        let mut done = false;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| Error::Llm(e.to_string()))?;
            buf.extend_from_slice(chunk.as_ref());

            while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                let line_bytes: Vec<u8> = buf.drain(..=pos).collect();
                let line = std::str::from_utf8(&line_bytes)
                    .map_err(|e| Error::Llm(format!("invalid utf-8 in stream: {e}")))?
                    .trim_end();

                if let Some(payload) = data_payload(line) {
                    if payload == "[DONE]" {
                        done = true;
                        break;
                    }
                    for ev in parse_chunk(payload) {
                        yield ev;
                    }
                }
            }
            if done {
                break;
            }
        }
    })
}

/// Extract the payload of an SSE `data:` line (stripping one optional leading space).
/// Returns `None` for comments, `event:`/`id:` lines, and blanks.
fn data_payload(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("data:")?;
    Some(rest.strip_prefix(' ').unwrap_or(rest))
}

/// Map one chunk JSON payload to zero or more events. A payload that doesn't deserialize as a
/// chunk yields nothing (keep-alives, provider quirks).
fn parse_chunk(payload: &str) -> Vec<StreamEvent> {
    let Ok(chunk) = serde_json::from_str::<wire::Chunk>(payload) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    if let Some(choice) = chunk.choices.into_iter().next() {
        if let Some(content) = choice.delta.content
            && !content.is_empty()
        {
            out.push(StreamEvent::Text(content));
        }
        for tc in choice.delta.tool_calls {
            let (name, arguments) = match tc.function {
                Some(f) => (f.name, f.arguments),
                None => (None, None),
            };
            out.push(StreamEvent::ToolCall {
                index: tc.index,
                id: tc.id,
                name,
                arguments,
            });
        }
        if let Some(reason) = choice.finish_reason {
            out.push(StreamEvent::Finish {
                reason: Some(reason),
            });
        }
    }
    if let Some(usage) = chunk.usage {
        out.push(StreamEvent::Usage(usage));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;

    async fn collect(chunks: Vec<&'static str>) -> Vec<StreamEvent> {
        let byte_stream = stream::iter(
            chunks
                .into_iter()
                .map(|c| Ok::<_, reqwest::Error>(c.as_bytes().to_vec())),
        );
        let mut evs = events(byte_stream);
        let mut out = Vec::new();
        while let Some(ev) = evs.next().await {
            out.push(ev.expect("event"));
        }
        out
    }

    #[tokio::test]
    async fn parses_text_then_done() {
        let evs = collect(vec![
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n",
            "data: [DONE]\n\n",
        ])
        .await;
        assert_eq!(
            evs,
            vec![
                StreamEvent::Text("Hel".into()),
                StreamEvent::Text("lo".into()),
            ]
        );
    }

    #[tokio::test]
    async fn handles_chunk_split_across_byte_boundaries() {
        // The same JSON event, delivered in three arbitrary byte slices.
        let evs = collect(vec![
            "data: {\"choices\":[{\"de",
            "lta\":{\"content\":\"hi\"}}]}\n",
            "data: [DONE]\n",
        ])
        .await;
        assert_eq!(evs, vec![StreamEvent::Text("hi".into())]);
    }

    #[tokio::test]
    async fn parses_tool_call_fragments() {
        let evs = collect(vec![
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"function\":{\"name\":\"execute\",\"arguments\":\"{\\\"co\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"de\\\":1}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: [DONE]\n\n",
        ])
        .await;
        assert_eq!(
            evs,
            vec![
                StreamEvent::ToolCall {
                    index: 0,
                    id: Some("c1".into()),
                    name: Some("execute".into()),
                    arguments: Some("{\"co".into()),
                },
                StreamEvent::ToolCall {
                    index: 0,
                    id: None,
                    name: None,
                    arguments: Some("de\":1}".into()),
                },
                StreamEvent::Finish {
                    reason: Some("tool_calls".into()),
                },
            ]
        );
    }
}
