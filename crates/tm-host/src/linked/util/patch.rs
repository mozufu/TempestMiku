use std::collections::BTreeMap;

use crate::{HostError, Result};

use super::super::tools::PatchHunk;

pub(in crate::linked) fn apply_line_hunks(old: &str, hunks: &[PatchHunk]) -> Result<String> {
    let newline = dominant_line_ending(old);
    let had_trailing_newline = old.ends_with(newline);
    let body = if had_trailing_newline {
        &old[..old.len() - newline.len()]
    } else {
        old
    };
    let lines: Vec<String> = if body.is_empty() {
        Vec::new()
    } else {
        body.split(newline).map(str::to_string).collect()
    };
    #[derive(Clone)]
    struct Replacement {
        start: usize,
        end: usize,
        lines: Vec<String>,
    }
    let mut replacements: Vec<Replacement> = Vec::new();
    let mut inserts: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for hunk in hunks {
        match hunk {
            PatchHunk::Replace {
                start_line,
                end_line,
                expected_lines,
                lines: new_lines,
            } => {
                validate_range(*start_line, *end_line, lines.len())?;
                validate_expected_range(*start_line, *end_line, expected_lines, &lines)?;
                replacements.push(Replacement {
                    start: start_line - 1,
                    end: *end_line,
                    lines: new_lines.clone(),
                });
            }
            PatchHunk::Delete {
                start_line,
                end_line,
                expected_lines,
            } => {
                validate_range(*start_line, *end_line, lines.len())?;
                validate_expected_range(*start_line, *end_line, expected_lines, &lines)?;
                replacements.push(Replacement {
                    start: start_line - 1,
                    end: *end_line,
                    lines: Vec::new(),
                });
            }
            PatchHunk::InsertBefore {
                line,
                expected_line,
                lines: new_lines,
            } => {
                validate_line(*line, lines.len())?;
                validate_expected_line(*line, expected_line, &lines)?;
                inserts
                    .entry(line - 1)
                    .or_default()
                    .extend(new_lines.clone());
            }
            PatchHunk::InsertAfter {
                line,
                expected_line,
                lines: new_lines,
            } => {
                validate_line(*line, lines.len())?;
                validate_expected_line(*line, expected_line, &lines)?;
                inserts.entry(*line).or_default().extend(new_lines.clone());
            }
            PatchHunk::Prepend { lines: new_lines } => {
                inserts.entry(0).or_default().extend(new_lines.clone());
            }
            PatchHunk::Append { lines: new_lines } => {
                inserts
                    .entry(lines.len())
                    .or_default()
                    .extend(new_lines.clone());
            }
        }
    }
    replacements.sort_by_key(|replacement| replacement.start);
    for pair in replacements.windows(2) {
        if pair[0].end > pair[1].start {
            return Err(HostError::InvalidArgs(
                "overlapping replace/delete hunks".to_string(),
            ));
        }
    }
    for replacement in &replacements {
        if inserts
            .keys()
            .any(|position| replacement.start < *position && *position < replacement.end)
        {
            return Err(HostError::InvalidArgs(
                "insert hunk overlaps a replaced/deleted line range".to_string(),
            ));
        }
    }
    let mut out = Vec::new();
    let mut idx = 0;
    let mut replacement_idx = 0;
    while idx <= lines.len() {
        if let Some(new_lines) = inserts.get(&idx) {
            out.extend(new_lines.clone());
        }
        if idx == lines.len() {
            break;
        }
        if replacement_idx < replacements.len() && replacements[replacement_idx].start == idx {
            out.extend(replacements[replacement_idx].lines.clone());
            idx = replacements[replacement_idx].end;
            replacement_idx += 1;
            continue;
        }
        out.push(lines[idx].clone());
        idx += 1;
    }
    let mut new = out.join(newline);
    if had_trailing_newline {
        new.push_str(newline);
    }
    Ok(new)
}

fn validate_expected_range(
    start_line: usize,
    end_line: usize,
    expected_lines: &[String],
    actual_lines: &[String],
) -> Result<()> {
    let range_len = end_line - start_line + 1;
    if expected_lines.len() != range_len {
        return Err(HostError::InvalidArgs(format!(
            "fs.patch expectedLines has {} lines but range {start_line}-{end_line} has {range_len}",
            expected_lines.len()
        )));
    }
    if actual_lines[start_line - 1..end_line] != expected_lines[..] {
        return Err(HostError::InvalidArgs(format!(
            "fs.patch context mismatch at lines {start_line}-{end_line}; re-read the file and retry"
        )));
    }
    Ok(())
}

fn validate_expected_line(line: usize, expected_line: &str, actual_lines: &[String]) -> Result<()> {
    if actual_lines[line - 1] != expected_line {
        return Err(HostError::InvalidArgs(format!(
            "fs.patch context mismatch at line {line}; re-read the file and retry"
        )));
    }
    Ok(())
}

fn dominant_line_ending(text: &str) -> &'static str {
    let newline_count = text.bytes().filter(|byte| *byte == b'\n').count();
    let crlf_count = text
        .as_bytes()
        .windows(2)
        .filter(|pair| *pair == b"\r\n")
        .count();
    if newline_count > 0 && newline_count == crlf_count {
        "\r\n"
    } else {
        "\n"
    }
}

pub(in crate::linked) fn validate_range(start: usize, end: usize, len: usize) -> Result<()> {
    if start == 0 || end < start || end > len {
        return Err(HostError::InvalidArgs(format!(
            "invalid line range {start}-{end} for {len} lines"
        )));
    }
    Ok(())
}

pub(in crate::linked) fn validate_line(line: usize, len: usize) -> Result<()> {
    if line == 0 || line > len {
        return Err(HostError::InvalidArgs(format!(
            "invalid line {line} for {len} lines"
        )));
    }
    Ok(())
}

pub(in crate::linked) fn simple_diff(old: &str, new: &str, path: &str) -> String {
    if old == new {
        return String::new();
    }
    const CONTEXT_LINES: usize = 3;
    let old_lines = old.lines().collect::<Vec<_>>();
    let new_lines = new.lines().collect::<Vec<_>>();
    let prefix = old_lines
        .iter()
        .zip(&new_lines)
        .take_while(|(old, new)| old == new)
        .count();
    let suffix = old_lines[prefix..]
        .iter()
        .rev()
        .zip(new_lines[prefix..].iter().rev())
        .take_while(|(old, new)| old == new)
        .count();
    let old_change_end = old_lines.len() - suffix;
    let new_change_end = new_lines.len() - suffix;
    let old_hunk_start = prefix.saturating_sub(CONTEXT_LINES);
    let new_hunk_start = prefix.saturating_sub(CONTEXT_LINES);
    let old_hunk_end = (old_change_end + CONTEXT_LINES).min(old_lines.len());
    let new_hunk_end = (new_change_end + CONTEXT_LINES).min(new_lines.len());

    let mut diff = format!(
        "--- {path}\n+++ {path}\n@@ -{},{} +{},{} @@\n",
        old_hunk_start + 1,
        old_hunk_end.saturating_sub(old_hunk_start),
        new_hunk_start + 1,
        new_hunk_end.saturating_sub(new_hunk_start)
    );
    for line in &old_lines[old_hunk_start..prefix] {
        diff.push(' ');
        diff.push_str(line);
        diff.push('\n');
    }
    for line in &old_lines[prefix..old_change_end] {
        diff.push('-');
        diff.push_str(line);
        diff.push('\n');
    }
    for line in &new_lines[prefix..new_change_end] {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    for line in &new_lines[new_change_end..new_hunk_end] {
        diff.push(' ');
        diff.push_str(line);
        diff.push('\n');
    }
    if old_lines == new_lines {
        diff.push_str("\\ No newline at end of file changed\n");
    }
    diff
}
