//! Server-Sent-Events parsing: a byte stream of `data:` lines -> [`StreamEvent`]s.

use async_stream::try_stream;
use futures::stream::{BoxStream, Stream, StreamExt};

use tm_core::{Error, Result, StreamEvent};

use crate::wire;

#[derive(Debug, Clone, Copy)]
pub(crate) struct SseLimits {
    pub(crate) max_line_bytes: usize,
    pub(crate) max_stream_bytes: usize,
}

/// Adapt a byte stream (e.g. `reqwest::Response::bytes_stream()`) into a stream of
/// [`StreamEvent`]s. Lines are buffered until a newline so multi-byte UTF-8 split across
/// network chunks is never mis-decoded. Malformed completion payloads fail the stream rather than
/// silently dropping text or tool-call fragments.
pub fn events<S, B>(stream: S, limits: SseLimits) -> BoxStream<'static, Result<StreamEvent>>
where
    S: Stream<Item = std::result::Result<B, reqwest::Error>> + Send + 'static,
    B: AsRef<[u8]> + Send + 'static,
{
    Box::pin(try_stream! {
        futures::pin_mut!(stream);
        let mut buf: Vec<u8> = Vec::new();
        let mut done = false;
        let mut stream_bytes = 0usize;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk
                .map_err(|error| Error::Llm(crate::redact_transport_error(error)))?;
            stream_bytes = stream_bytes
                .checked_add(chunk.as_ref().len())
                .ok_or_else(|| Error::Llm("completion stream size overflow".to_string()))?;
            if stream_bytes > limits.max_stream_bytes {
                Err(Error::Llm(format!(
                    "completion stream exceeded {} bytes",
                    limits.max_stream_bytes
                )))?;
            }
            buf.extend_from_slice(chunk.as_ref());

            while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                if pos > limits.max_line_bytes {
                    Err(Error::Llm(format!(
                        "SSE line exceeded {} bytes",
                        limits.max_line_bytes
                    )))?;
                }
                let line_bytes: Vec<u8> = buf.drain(..=pos).collect();
                let line = std::str::from_utf8(&line_bytes)
                    .map_err(|e| Error::Llm(format!("invalid utf-8 in stream: {e}")))?
                    .trim_end();

                if let Some(payload) = data_payload(line) {
                    if payload == "[DONE]" {
                        done = true;
                        break;
                    }
                    for ev in parse_chunk(payload)? {
                        yield ev;
                    }
                }
            }
            if buf.len() > limits.max_line_bytes {
                Err(Error::Llm(format!(
                    "SSE line exceeded {} bytes",
                    limits.max_line_bytes
                )))?;
            }
            if done {
                break;
            }
        }
        if !done && !buf.is_empty() {
            if buf.len() > limits.max_line_bytes {
                Err(Error::Llm(format!(
                    "SSE line exceeded {} bytes",
                    limits.max_line_bytes
                )))?;
            }
            let line = std::str::from_utf8(&buf)
                .map_err(|e| Error::Llm(format!("invalid utf-8 in stream: {e}")))?
                .trim_end();
            if let Some(payload) = data_payload(line) {
                if payload == "[DONE]" {
                    done = true;
                } else {
                    for ev in parse_chunk(payload)? {
                        yield ev;
                    }
                }
            }
        }
        if !done {
            Err(Error::Llm(
                "completion stream ended before the [DONE] marker".to_string(),
            ))?;
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
fn parse_chunk(payload: &str) -> Result<Vec<StreamEvent>> {
    let chunk = serde_json::from_str::<wire::Chunk>(payload)
        .map_err(|err| Error::Llm(format!("invalid SSE data JSON: {err}")))?;

    let mut out = Vec::new();
    if let Some(choice) = chunk.choices.into_iter().next() {
        if let Some(content) = choice.delta.content
            && !content.is_empty()
        {
            out.push(StreamEvent::Text(content));
        }
        // Reasoning tokens are separate from the visible answer; prefer `reasoning`, fall
        // back to `reasoning_content` (DeepSeek R1 and some bridges). Merge both if a
        // provider emits them together.
        if let Some(r) = choice.delta.reasoning
            && !r.is_empty()
        {
            out.push(StreamEvent::Reasoning(r));
        }
        if let Some(r) = choice.delta.reasoning_content
            && !r.is_empty()
        {
            out.push(StreamEvent::Reasoning(r));
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
    Ok(out)
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
        let mut evs = events(
            byte_stream,
            SseLimits {
                max_line_bytes: 1024 * 1024,
                max_stream_bytes: 4 * 1024 * 1024,
            },
        );
        let mut out = Vec::new();
        while let Some(ev) = evs.next().await {
            out.push(ev.expect("event"));
        }
        out
    }

    async fn collect_results(
        chunks: Vec<&'static str>,
        limits: SseLimits,
    ) -> Vec<Result<StreamEvent>> {
        let byte_stream = stream::iter(
            chunks
                .into_iter()
                .map(|c| Ok::<_, reqwest::Error>(c.as_bytes().to_vec())),
        );
        events(byte_stream, limits).collect().await
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

    #[tokio::test]
    async fn parses_reasoning_then_text() {
        // A reasoning model streams private chain-of-thought via `reasoning`, then the
        // visible answer via `content`; some providers use `reasoning_content` instead.
        let evs = collect(vec![
            "data: {\"choices\":[{\"delta\":{\"reasoning\":\"think \"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"hard\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"answer\"}}]}\n\n",
            "data: [DONE]\n\n",
        ])
        .await;
        assert_eq!(
            evs,
            vec![
                StreamEvent::Reasoning("think ".into()),
                StreamEvent::Reasoning("hard".into()),
                StreamEvent::Text("answer".into()),
            ]
        );
    }

    #[tokio::test]
    async fn accepts_done_without_a_trailing_newline() {
        let evs = collect(vec![
            "data: {\"choices\":[{\"delta\":{\"content\":\"tail\"}}]}\n",
            "data: [DONE]",
        ])
        .await;
        assert_eq!(evs, vec![StreamEvent::Text("tail".into())]);
    }

    #[tokio::test]
    async fn rejects_truncated_stream_without_done() {
        let results = collect_results(
            vec!["data: {\"choices\":[{\"delta\":{\"content\":\"tail\"}}]}"],
            SseLimits {
                max_line_bytes: 1024,
                max_stream_bytes: 4096,
            },
        )
        .await;
        assert!(matches!(
            results.first(),
            Some(Ok(StreamEvent::Text(text))) if text == "tail"
        ));
        assert!(
            matches!(results.last(), Some(Err(Error::Llm(message))) if message.contains("[DONE]"))
        );
    }

    #[tokio::test]
    async fn rejects_malformed_or_oversized_data() {
        let limits = SseLimits {
            max_line_bytes: 32,
            max_stream_bytes: 128,
        };
        let malformed = collect_results(vec!["data: not-json\n"], limits).await;
        assert!(matches!(malformed.as_slice(), [Err(Error::Llm(_))]));

        let oversized =
            collect_results(vec!["data: 123456789012345678901234567890123\n"], limits).await;
        assert!(matches!(oversized.as_slice(), [Err(Error::Llm(_))]));

        let total = collect_results(
            vec![
                "data: {\"choices\":[]}\n",
                "data: {\"choices\":[]}\n",
                "data: {\"choices\":[]}\n",
            ],
            SseLimits {
                max_line_bytes: 64,
                max_stream_bytes: 40,
            },
        )
        .await;
        assert!(total.iter().any(Result::is_err));
    }
}
