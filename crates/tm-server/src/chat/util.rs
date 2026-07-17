use tm_artifacts::ArtifactId;

pub(crate) fn last_artifact_uri_in_text(text: &str) -> Option<String> {
    let marker = "artifact://";
    let mut cursor = 0;
    let mut found = None;
    while let Some(offset) = text[cursor..].find(marker) {
        let start = cursor + offset;
        let id_start = start + marker.len();
        let id_len = text[id_start..]
            .chars()
            .take_while(char::is_ascii_digit)
            .map(char::len_utf8)
            .sum::<usize>();
        if id_len > 0 {
            let end = id_start + id_len;
            let uri = &text[start..end];
            let has_uri_boundary = text[end..].chars().next().is_none_or(|ch| {
                ch.is_whitespace()
                    || matches!(
                        ch,
                        ',' | '.' | ';' | ':' | '!' | ')' | ']' | '}' | '"' | '\'' | '`'
                    )
            });
            if has_uri_boundary && ArtifactId::parse_uri(uri).is_ok() {
                found = Some(uri.to_string());
            }
            cursor = end;
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

    #[test]
    fn last_artifact_uri_in_text_rejects_noncanonical_or_extended_ids() {
        for text in [
            "artifact://SECRET",
            "artifact://01",
            "artifact://18446744073709551616",
            "artifact://12-secret",
            "artifact://12/secret",
            "artifact://12?secret",
            "artifact://12#secret",
        ] {
            assert_eq!(last_artifact_uri_in_text(text), None, "accepted {text:?}");
        }
    }
}
