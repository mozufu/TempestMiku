use super::*;
pub fn default_root() -> PathBuf {
    Path::new(".tempestmiku").to_path_buf()
}

pub fn preview(s: &str, cap: usize) -> String {
    if s.len() <= cap {
        return s.to_string();
    }
    let end = floor_boundary(s, cap);
    format!("{}\n… ({} bytes total)", &s[..end], s.len())
}

pub(super) fn select_text_file(
    path: &Path,
    selector: Option<&str>,
    limits: ArtifactLimits,
) -> Result<(String, bool)> {
    ensure_regular_file(path, &path.display().to_string())?;
    let (start, end) = parse_selector(selector, limits)?;
    let max_page_bytes = if selector.is_some() {
        limits.max_page_bytes
    } else {
        limits.default_page_bytes
    };
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    for _ in 1..start {
        if reader.skip_until(b'\n')? == 0 {
            return Ok((String::new(), false));
        }
    }

    let mut selected = Vec::new();
    let mut truncated = false;
    for (selected_lines, _) in (start..=end).enumerate() {
        let separator = usize::from(selected_lines > 0);
        let remaining = max_page_bytes.saturating_sub(selected.len().saturating_add(separator));
        let Some((line, line_truncated)) = read_bounded_line(&mut reader, remaining)? else {
            break;
        };
        if separator == 1 {
            selected.push(b'\n');
        }
        selected.extend_from_slice(&line);
        if line_truncated {
            truncated = true;
            break;
        }
    }

    let has_more = truncated || !reader.fill_buf()?.is_empty();
    if let Err(err) = std::str::from_utf8(&selected) {
        selected.truncate(err.valid_up_to());
    }
    let selected = String::from_utf8(selected)
        .map_err(|_| ArtifactError::Integrity(path.display().to_string()))?;
    Ok((selected, has_more))
}

pub(super) fn parse_selector(
    selector: Option<&str>,
    limits: ArtifactLimits,
) -> Result<(usize, usize)> {
    let Some(selector) = selector else {
        return Ok((1, limits.default_page_lines));
    };
    let (start, end) = selector
        .split_once('-')
        .ok_or_else(|| ArtifactError::InvalidSelector(selector.to_string()))?;
    let start: usize = start
        .parse()
        .map_err(|_| ArtifactError::InvalidSelector(selector.to_string()))?;
    let end: usize = end
        .parse()
        .map_err(|_| ArtifactError::InvalidSelector(selector.to_string()))?;
    let line_count = end
        .checked_sub(start)
        .and_then(|count| count.checked_add(1))
        .ok_or_else(|| ArtifactError::InvalidSelector(selector.to_string()))?;
    if start == 0 || end < start || line_count > limits.max_page_lines {
        return Err(ArtifactError::InvalidSelector(selector.to_string()));
    }
    Ok((start, end))
}

pub(super) fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    limit: usize,
) -> io::Result<Option<(Vec<u8>, bool)>> {
    let mut line = Vec::new();
    loop {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Ok(Some((line, false)))
            };
        }
        if let Some(newline) = buf.iter().position(|byte| *byte == b'\n') {
            let content = &buf[..newline];
            let available = limit.saturating_sub(line.len());
            let copied = content.len().min(available);
            line.extend_from_slice(&content[..copied]);
            if copied < content.len() {
                reader.consume(copied);
                return Ok(Some((line, true)));
            }
            reader.consume(newline + 1);
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return Ok(Some((line, false)));
        }

        let available = limit.saturating_sub(line.len());
        let buf_len = buf.len();
        let copied = buf_len.min(available);
        line.extend_from_slice(&buf[..copied]);
        reader.consume(copied);
        if copied < buf_len || available == 0 {
            return Ok(Some((line, true)));
        }
    }
}

pub(super) fn floor_boundary(s: &str, mut idx: usize) -> usize {
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}
