pub(crate) fn last_artifact_uri_in_text(text: &str) -> Option<String> {
    let marker = "artifact://";
    let mut cursor = 0;
    let mut found = None;
    while let Some(offset) = text[cursor..].find(marker) {
        let start = cursor + offset;
        let id_start = start + marker.len();
        let id_len = text[id_start..]
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
            .map(char::len_utf8)
            .sum::<usize>();
        if id_len > 0 {
            found = Some(text[start..id_start + id_len].to_string());
            cursor = id_start + id_len;
        } else {
            cursor = id_start;
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::last_artifact_uri_in_text;

    #[test]
    pub(crate) fn last_artifact_uri_in_text_uses_child_transcript_order() {
        let text = "[cell_result] first artifact://0\n[text] later\n[cell_result] artifact://12,";
        assert_eq!(
            last_artifact_uri_in_text(text).as_deref(),
            Some("artifact://12")
        );
    }
}
