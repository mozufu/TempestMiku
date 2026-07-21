use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{HostError, Result};

const MAX_STDIN_BYTES: usize = 1024 * 1024;
const MAX_STDIN_APPROVAL_PREVIEW_BYTES: usize = 256;
const MAX_PROCESS_ARTIFACT_BYTES: usize = 4 * 1024 * 1024;
pub(in crate::linked::tools) const MAX_RETAINED_PROCESS_OUTPUT_BYTES: usize =
    MAX_PROCESS_ARTIFACT_BYTES - 256;

pub(in crate::linked::tools) fn bounded_inline_output(
    stdout: &str,
    stderr: &str,
    limit: usize,
) -> (String, String) {
    let stdout_end = utf8_prefix_len(stdout, limit);
    let stdout = stdout[..stdout_end].to_string();
    let remaining = limit.saturating_sub(stdout.len());
    let stderr_end = utf8_prefix_len(stderr, remaining);
    let stderr = stderr[..stderr_end].to_string();
    (stdout, stderr)
}

fn utf8_prefix_len(value: &str, limit: usize) -> usize {
    let mut end = value.len().min(limit);
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    end
}

pub(super) fn stdin_approval_preview(stdin: Option<&[u8]>) -> Result<(Option<String>, bool)> {
    let Some(stdin) = stdin else {
        return Ok((None, false));
    };
    let stdin = std::str::from_utf8(stdin)
        .map_err(|_| HostError::HostCall("validated proc.run stdin was not UTF-8".to_string()))?;
    let redacted = tm_memory::redact_dream_text(stdin).text;
    if redacted.len() <= MAX_STDIN_APPROVAL_PREVIEW_BYTES {
        return Ok((Some(redacted), false));
    }

    let marker = format!("...[truncated:{} bytes]", redacted.len());
    let prefix_limit = MAX_STDIN_APPROVAL_PREVIEW_BYTES.saturating_sub(marker.len());
    let prefix_end = utf8_prefix_len(&redacted, prefix_limit);
    Ok((Some(format!("{}{}", &redacted[..prefix_end], marker)), true))
}

pub(super) fn parse_stdin(stdin: Option<Value>) -> Result<Option<Vec<u8>>> {
    match stdin {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(stdin)) => {
            if stdin.len() > MAX_STDIN_BYTES {
                return Err(HostError::InvalidArgs(format!(
                    "proc.run stdin must not exceed {MAX_STDIN_BYTES} UTF-8 bytes"
                )));
            }
            Ok(Some(stdin.into_bytes()))
        }
        Some(_) => Err(HostError::InvalidArgs(
            "proc.run stdin must be a UTF-8 string".to_string(),
        )),
    }
}

pub(super) async fn write_stdin<W>(stdin: Option<W>, data: Option<Vec<u8>>) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    match (stdin, data) {
        (Some(mut stdin), Some(data)) => {
            if let Err(error) = stdin.write_all(&data).await {
                return if error.kind() == std::io::ErrorKind::BrokenPipe {
                    Ok(())
                } else {
                    Err(error)
                };
            }
            match stdin.shutdown().await {
                Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
                result => result,
            }
        }
        _ => Ok(()),
    }
}

pub(in crate::linked::tools) struct BoundedOutput {
    pub(in crate::linked::tools) bytes: Vec<u8>,
    pub(in crate::linked::tools) truncated: bool,
}

pub(in crate::linked::tools) async fn read_bounded_output<R>(
    mut reader: R,
    retained: Arc<AtomicUsize>,
    limit: usize,
) -> std::io::Result<BoundedOutput>
where
    R: AsyncRead + Unpin,
{
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut chunk = [0_u8; 8 * 1024];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        let keep = reserve_output_bytes(&retained, read, limit);
        bytes.extend_from_slice(&chunk[..keep]);
        truncated |= keep < read;
    }
    Ok(BoundedOutput { bytes, truncated })
}

fn reserve_output_bytes(retained: &AtomicUsize, requested: usize, limit: usize) -> usize {
    loop {
        let current = retained.load(Ordering::Relaxed);
        let keep = requested.min(limit.saturating_sub(current));
        if keep == 0 {
            return 0;
        }
        if retained
            .compare_exchange_weak(
                current,
                current + keep,
                Ordering::Relaxed,
                Ordering::Relaxed,
            )
            .is_ok()
        {
            return keep;
        }
    }
}

pub(super) fn bounded_process_artifact(mut output: String, output_limit_reached: bool) -> String {
    let marker = if output_limit_reached || output.len() > MAX_PROCESS_ARTIFACT_BYTES {
        format!(
            "\n… proc.run retained-output limit reached at {MAX_RETAINED_PROCESS_OUTPUT_BYTES} bytes …"
        )
    } else {
        String::new()
    };
    let content_cap = MAX_PROCESS_ARTIFACT_BYTES.saturating_sub(marker.len());
    if output.len() > content_cap {
        let mut end = content_cap;
        while end > 0 && !output.is_char_boundary(end) {
            end -= 1;
        }
        output.truncate(end);
    }
    output.push_str(&marker);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_process_output_budget_never_exceeds_the_limit() {
        let retained = AtomicUsize::new(0);
        assert_eq!(reserve_output_bytes(&retained, 7, 10), 7);
        assert_eq!(reserve_output_bytes(&retained, 7, 10), 3);
        assert_eq!(reserve_output_bytes(&retained, 1, 10), 0);
        assert_eq!(retained.load(Ordering::Relaxed), 10);
    }

    #[test]
    fn process_artifact_keeps_a_bounded_truncation_marker() {
        let output = bounded_process_artifact("x".repeat(MAX_PROCESS_ARTIFACT_BYTES), true);
        assert!(output.len() <= MAX_PROCESS_ARTIFACT_BYTES);
        assert!(output.contains("retained-output limit reached"));
    }

    #[test]
    fn inline_stdout_and_stderr_share_one_budget() {
        let (stdout, stderr) = bounded_inline_output("12345678", "abcdef", 10);
        assert_eq!(stdout, "12345678");
        assert_eq!(stderr, "ab");
        assert_eq!(stdout.len() + stderr.len(), 10);

        let (stdout, stderr) = bounded_inline_output("世界", "界", 4);
        assert_eq!(stdout, "世");
        assert_eq!(stderr, "");
    }

    #[tokio::test]
    async fn early_stdin_pipe_closure_is_not_a_process_failure() {
        let (writer, reader) = tokio::io::duplex(1);
        drop(reader);
        write_stdin(Some(writer), Some(vec![b'x'; 1024]))
            .await
            .unwrap();
    }
}
