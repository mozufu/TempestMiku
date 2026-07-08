use std::collections::BTreeSet;

use chrono::Utc;
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tm_memory::redact_dream_text;

use crate::{
    DriveAmount, DriveAttribute, DriveEvidence, DrivePutOptions, DriveVirtualQuery,
    types::{DriveError, DriveModelExtractionRequest, Result, default_model_extraction_fields},
};

pub const FALLBACK_EXTRACTOR_VERSION: &str = "tm-drive:fallback-transducer:v1";
const MAX_ATTRIBUTES: usize = 64;
const MAX_EVIDENCE_BYTES: usize = 240;
const MAX_SUMMARY_BYTES: usize = 900;
const DEFAULT_MODEL_EXTRACTION_ROLE: &str = "document_extractor";
const MIN_MODEL_PREVIEW_BYTES: usize = 128;
const MAX_MODEL_PREVIEW_BYTES: usize = 8_000;

#[derive(Debug, Clone)]
pub struct TransducerInput<'a> {
    pub bytes: &'a [u8],
    pub filename: Option<&'a str>,
    pub options: &'a DrivePutOptions,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Transduction {
    pub mime: String,
    pub title: Option<String>,
    pub doc_kind: Option<String>,
    pub project: Option<String>,
    pub tags: Vec<String>,
    pub entities: Vec<String>,
    pub dates: Vec<String>,
    pub amounts: Vec<DriveAmount>,
    pub attributes: Vec<DriveAttribute>,
    pub summary: Option<String>,
    pub redactions: Vec<String>,
    pub text_preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_request: Option<DriveModelExtractionRequest>,
    pub content_hash: String,
    pub extractor: String,
    #[serde(default)]
    pub warnings: Vec<String>,
}

pub trait Transducer: Send + Sync {
    fn extract(&self, input: TransducerInput<'_>) -> Result<Transduction>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FallbackTransducer;

impl Transducer for FallbackTransducer {
    fn extract(&self, input: TransducerInput<'_>) -> Result<Transduction> {
        transduce_document(input)
    }
}

pub fn transduce_document(input: TransducerInput<'_>) -> Result<Transduction> {
    let content_hash = hex::encode(Sha256::digest(input.bytes));
    let text = String::from_utf8(input.bytes.to_vec()).ok();
    let mut mime = input
        .options
        .mime
        .clone()
        .or_else(|| input.filename.and_then(mime_from_filename))
        .unwrap_or_else(|| "application/octet-stream".to_string());
    if mime == "application/octet-stream" && text.is_some() {
        mime = "text/plain".to_string();
    }
    let filename_title = input.filename.and_then(title_from_filename);
    let now = Utc::now().date_naive().to_string();
    let mut warnings = Vec::new();
    let mut attributes = Vec::new();
    attr(
        &mut attributes,
        "mime",
        &mime,
        1.0,
        None,
        FALLBACK_EXTRACTOR_VERSION,
    );
    attr(
        &mut attributes,
        "content_hash",
        &content_hash,
        1.0,
        None,
        FALLBACK_EXTRACTOR_VERSION,
    );
    attr(
        &mut attributes,
        "created_date",
        &now,
        0.45,
        None,
        FALLBACK_EXTRACTOR_VERSION,
    );

    let redacted = text.as_deref().map(redact_dream_text);
    let redaction_kinds = redacted
        .as_ref()
        .map(|report| {
            report
                .redactions
                .iter()
                .map(|item| item.kind.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let text = redacted.as_ref().map(|report| report.text.as_str());
    if input.bytes.len() > 0 && text.is_none() {
        warnings.push("binary_or_non_utf8_content_used_fallback_metadata".to_string());
    }

    let title = input
        .options
        .title
        .clone()
        .or_else(|| text.and_then(extract_markdown_title))
        .or(filename_title)
        .or_else(|| Some("untitled".to_string()));
    if let Some(title) = title.as_deref() {
        attr(
            &mut attributes,
            "title",
            title,
            0.82,
            text.and_then(|text| evidence_for(text, title)),
            FALLBACK_EXTRACTOR_VERSION,
        );
    }

    let doc_kind = input
        .options
        .doc_kind
        .clone()
        .or_else(|| text.map(classify_doc_kind))
        .or_else(|| {
            if mime.starts_with("image/") {
                Some("image".to_string())
            } else if mime == "application/json" {
                Some("data".to_string())
            } else if mime.starts_with("text/") {
                Some("note".to_string())
            } else {
                None
            }
        });
    if let Some(kind) = doc_kind.as_deref() {
        attr(
            &mut attributes,
            "doc_kind",
            kind,
            if input.options.doc_kind.is_some() {
                1.0
            } else {
                0.68
            },
            text.and_then(|text| evidence_for(text, kind)),
            FALLBACK_EXTRACTOR_VERSION,
        );
    }

    let project = input.options.project.clone();
    if let Some(project) = project.as_deref() {
        attr(
            &mut attributes,
            "project",
            project,
            1.0,
            None,
            FALLBACK_EXTRACTOR_VERSION,
        );
    }

    let mut tags = BTreeSet::new();
    for tag in &input.options.tags {
        clean_tag(tag).map(|tag| tags.insert(tag));
    }
    if let Some(kind) = doc_kind.as_deref() {
        tags.insert(kind.to_string());
    }
    if let Some(text) = text {
        for tag in extract_hashtags(text) {
            tags.insert(tag);
        }
    }
    let tags = tags.into_iter().take(20).collect::<Vec<_>>();
    for tag in &tags {
        attr(
            &mut attributes,
            "tag",
            tag,
            0.7,
            text.and_then(|text| evidence_for(text, tag)),
            FALLBACK_EXTRACTOR_VERSION,
        );
    }

    let entities = text.map(extract_entities).unwrap_or_default();
    for entity in &entities {
        attr(
            &mut attributes,
            "entity",
            entity,
            0.54,
            text.and_then(|text| evidence_for(text, entity)),
            FALLBACK_EXTRACTOR_VERSION,
        );
    }

    let dates = text.map(extract_dates).unwrap_or_default();
    for date in &dates {
        attr(
            &mut attributes,
            "date",
            date,
            0.64,
            text.and_then(|text| evidence_for(text, date)),
            FALLBACK_EXTRACTOR_VERSION,
        );
    }

    let amounts = text.map(extract_amounts).unwrap_or_default();
    for amount in &amounts {
        attr(
            &mut attributes,
            "amount",
            &amount.raw,
            0.62,
            text.and_then(|text| evidence_for(text, &amount.raw)),
            FALLBACK_EXTRACTOR_VERSION,
        );
    }

    let summary = text.and_then(summary_from_text);
    if let Some(summary) = summary.as_deref() {
        attr(
            &mut attributes,
            "summary",
            summary,
            0.55,
            Some(DriveEvidence {
                snippet: cap(summary, MAX_EVIDENCE_BYTES),
                selector: Some("1-3".to_string()),
            }),
            FALLBACK_EXTRACTOR_VERSION,
        );
    }
    attributes.truncate(MAX_ATTRIBUTES);
    attach_attribute_provenance(&mut attributes, input.options, &content_hash);
    let model_request = model_extraction_request(&input, &mime, &content_hash, text, &mut warnings);

    Ok(Transduction {
        mime,
        title,
        doc_kind,
        project,
        tags,
        entities,
        dates,
        amounts,
        attributes,
        summary,
        redactions: redaction_kinds,
        text_preview: text.map(|text| cap(text, MAX_SUMMARY_BYTES)),
        model_request,
        content_hash,
        extractor: FALLBACK_EXTRACTOR_VERSION.to_string(),
        warnings,
    })
}

fn mime_from_filename(filename: &str) -> Option<String> {
    let ext = filename.rsplit('.').next()?.to_ascii_lowercase();
    let mime = match ext.as_str() {
        "txt" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "json" => "application/json",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "pdf" => "application/pdf",
        _ => return None,
    };
    Some(mime.to_string())
}

fn title_from_filename(filename: &str) -> Option<String> {
    let leaf = filename.rsplit('/').next().unwrap_or(filename);
    let stem = leaf.rsplit_once('.').map_or(leaf, |(stem, _)| stem);
    let title = stem
        .replace(['_', '-'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (!title.is_empty()).then_some(title)
}

fn extract_markdown_title(text: &str) -> Option<String> {
    for line in text.lines().take(12) {
        let line = line.trim();
        if let Some(title) = line.strip_prefix("# ") {
            let title = title.trim();
            if !title.is_empty() {
                return Some(title.to_string());
            }
        }
        if !line.is_empty() && line.len() <= 100 {
            return Some(line.trim_matches(['#', '*', '`']).trim().to_string());
        }
    }
    None
}

fn classify_doc_kind(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    if lower.contains("invoice")
        || lower.contains("amount due")
        || lower.contains("bill to")
        || lower.contains("receipt")
    {
        if lower.contains("receipt") && !lower.contains("invoice") {
            "receipt"
        } else {
            "invoice"
        }
        .to_string()
    } else if lower.contains("abstract") && (lower.contains("references") || lower.contains("doi"))
    {
        "paper".to_string()
    } else if lower.contains("adr") || lower.contains("architecture decision") {
        "decision".to_string()
    } else if lower.contains("roadmap") || lower.contains("milestone") {
        "project_doc".to_string()
    } else {
        "note".to_string()
    }
}

fn extract_hashtags(text: &str) -> Vec<String> {
    let re = Regex::new(r"(?m)(?:^|\s)#([A-Za-z][A-Za-z0-9_-]{1,48})\b").unwrap();
    uniq(
        re.captures_iter(text)
            .map(|cap| cap[1].to_ascii_lowercase()),
    )
}

fn extract_entities(text: &str) -> Vec<String> {
    let re = Regex::new(r"\b([A-Z][a-z]+(?:\s+[A-Z][a-z]+){0,3})\b").unwrap();
    uniq(re.captures_iter(text).filter_map(|cap| {
        let value = cap[1].trim();
        if matches!(value, "The" | "This" | "That" | "From" | "Date") {
            None
        } else {
            Some(value.to_string())
        }
    }))
    .into_iter()
    .take(12)
    .collect()
}

fn extract_dates(text: &str) -> Vec<String> {
    let iso =
        Regex::new(r"\b(20\d{2}|19\d{2})[-/](0?[1-9]|1[0-2])[-/](0?[1-9]|[12]\d|3[01])\b").unwrap();
    let us = Regex::new(r"\b(0?[1-9]|1[0-2])/(0?[1-9]|[12]\d|3[01])/(20\d{2}|19\d{2})\b").unwrap();
    let mut dates = Vec::new();
    dates.extend(iso.find_iter(text).map(|m| m.as_str().replace('/', "-")));
    dates.extend(us.captures_iter(text).map(|cap| {
        format!(
            "{}-{:02}-{:02}",
            &cap[3],
            cap[1].parse::<u32>().unwrap_or(1),
            cap[2].parse::<u32>().unwrap_or(1)
        )
    }));
    uniq(dates).into_iter().take(12).collect()
}

fn extract_amounts(text: &str) -> Vec<DriveAmount> {
    let re = Regex::new(
        r"(?i)(USD|EUR|JPY|TWD|\$|\x{20AC}|\x{00A5}|NT\$)?\s*([0-9][0-9,]*(?:\.[0-9]{2})?)\b",
    )
    .unwrap();
    let mut out = Vec::new();
    for cap in re.captures_iter(text) {
        let raw = cap.get(0).unwrap().as_str().trim().to_string();
        let number = cap[2].replace(',', "").parse::<f64>().ok();
        let currency = cap.get(1).map(|m| match m.as_str() {
            "$" => "USD".to_string(),
            "\u{20ac}" => "EUR".to_string(),
            "\u{00a5}" => "JPY".to_string(),
            other => other.to_ascii_uppercase(),
        });
        if currency.is_some() || raw.contains('.') {
            out.push(DriveAmount {
                raw,
                value: number,
                currency,
            });
        }
        if out.len() >= 12 {
            break;
        }
    }
    out
}

fn summary_from_text(text: &str) -> Option<String> {
    let summary = text
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            (!line.is_empty()).then_some(line)
        })
        .take(3)
        .collect::<Vec<_>>()
        .join("\n");
    (!summary.is_empty()).then(|| cap(&summary, MAX_SUMMARY_BYTES))
}

fn attr(
    attrs: &mut Vec<DriveAttribute>,
    key: &str,
    value: &str,
    confidence: f32,
    evidence: Option<DriveEvidence>,
    extractor: &str,
) {
    if value.trim().is_empty() {
        return;
    }
    attrs.push(DriveAttribute {
        key: key.to_string(),
        value: cap(value.trim(), MAX_EVIDENCE_BYTES),
        confidence,
        evidence,
        extractor: extractor.to_string(),
        source_uri: None,
        session_id: None,
        event_seq: None,
        content_hash: None,
    });
}

fn attach_attribute_provenance(
    attrs: &mut [DriveAttribute],
    options: &DrivePutOptions,
    content_hash: &str,
) {
    for attr in attrs {
        attr.source_uri = options.source_uri.clone();
        attr.session_id = options.session_id.clone();
        attr.event_seq = options.event_seq;
        attr.content_hash = Some(content_hash.to_string());
    }
}

fn model_extraction_request(
    input: &TransducerInput<'_>,
    mime: &str,
    content_hash: &str,
    text: Option<&str>,
    warnings: &mut Vec<String>,
) -> Option<DriveModelExtractionRequest> {
    let options = &input.options.model_extraction;
    if !options.enabled {
        return None;
    }
    let Some(text) = text else {
        warnings.push("model_extraction_requested_without_text_preview".to_string());
        return None;
    };
    let max_preview_bytes = options
        .max_preview_bytes
        .clamp(MIN_MODEL_PREVIEW_BYTES, MAX_MODEL_PREVIEW_BYTES);
    Some(DriveModelExtractionRequest {
        role: options
            .role
            .as_deref()
            .map(str::trim)
            .filter(|role| !role.is_empty())
            .unwrap_or(DEFAULT_MODEL_EXTRACTION_ROLE)
            .to_string(),
        fields: cleaned_model_fields(&options.fields),
        mime: mime.to_string(),
        filename: input.filename.map(str::to_string),
        content_hash: content_hash.to_string(),
        text_preview: cap(text, max_preview_bytes),
    })
}

fn cleaned_model_fields(fields: &[String]) -> Vec<String> {
    let fields = uniq(
        fields
            .iter()
            .map(|field| field.trim().to_ascii_lowercase())
            .filter(|field| !field.is_empty()),
    );
    if fields.is_empty() {
        default_model_extraction_fields()
    } else {
        fields
    }
}

fn evidence_for(text: &str, needle: &str) -> Option<DriveEvidence> {
    if needle.trim().is_empty() {
        return None;
    }
    for (index, line) in text.lines().enumerate() {
        if line
            .to_ascii_lowercase()
            .contains(&needle.to_ascii_lowercase())
        {
            return Some(DriveEvidence {
                snippet: cap(line.trim(), MAX_EVIDENCE_BYTES),
                selector: Some(format!("{}-{}", index + 1, index + 1)),
            });
        }
    }
    None
}

fn clean_tag(tag: &str) -> Option<String> {
    let out = tag
        .trim()
        .trim_start_matches('#')
        .to_ascii_lowercase()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    (!out.is_empty()).then_some(out)
}

fn cap(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut idx = max;
    while idx > 0 && !text.is_char_boundary(idx) {
        idx -= 1;
    }
    format!("{}...", &text[..idx])
}

fn uniq(items: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for item in items {
        let item = item.trim().to_string();
        if item.is_empty() || !seen.insert(item.to_ascii_lowercase()) {
            continue;
        }
        out.push(item);
    }
    out
}

#[allow(dead_code)]
fn _virtual_query_marker(_: &DriveVirtualQuery) -> Result<()> {
    Err(DriveError::InvalidArgs(
        "not a transducer query".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DriveModelExtractionOptions, DrivePutOptions};

    #[test]
    fn classifies_invoice_and_redacts_before_summary() {
        let opts = DrivePutOptions {
            project: Some("Taxes".to_string()),
            tags: vec!["finance".to_string()],
            source_uri: Some("artifact://7".to_string()),
            session_id: Some("session-1".to_string()),
            event_seq: Some(42),
            ..DrivePutOptions::default()
        };
        let input = b"# Invoice 42\nAuthorization: Bearer sk-secretsecret123\nAmount due: $42.00\nDate: 2026-07-08";
        let tx = transduce_document(TransducerInput {
            bytes: input,
            filename: Some("invoice-42.md"),
            options: &opts,
        })
        .unwrap();

        assert_eq!(tx.mime, "text/markdown");
        assert_eq!(tx.doc_kind.as_deref(), Some("invoice"));
        assert!(tx.tags.contains(&"finance".to_string()));
        assert!(tx.summary.unwrap().contains("[REDACTED_SECRET]"));
        assert!(tx.model_request.is_none());
        assert!(tx.dates.contains(&"2026-07-08".to_string()));
        assert_eq!(tx.amounts[0].currency.as_deref(), Some("USD"));
        assert!(tx.attributes.iter().all(|attr| {
            attr.extractor == FALLBACK_EXTRACTOR_VERSION
                && attr.source_uri.as_deref() == Some("artifact://7")
                && attr.session_id.as_deref() == Some("session-1")
                && attr.event_seq == Some(42)
                && attr.content_hash.as_deref() == Some(tx.content_hash.as_str())
        }));
    }

    #[test]
    fn binary_content_keeps_fallback_metadata() {
        let opts = DrivePutOptions::default();
        let tx = transduce_document(TransducerInput {
            bytes: &[0, 159, 146, 150],
            filename: Some("photo.png"),
            options: &opts,
        })
        .unwrap();

        assert_eq!(tx.mime, "image/png");
        assert_eq!(tx.doc_kind.as_deref(), Some("image"));
        assert!(tx.text_preview.is_none());
        assert!(!tx.warnings.is_empty());
    }

    #[test]
    fn model_extraction_hook_is_opt_in_redacted_and_bounded() {
        let opts = DrivePutOptions {
            model_extraction: DriveModelExtractionOptions {
                enabled: true,
                role: Some("document_classifier".to_string()),
                fields: vec![
                    "summary".to_string(),
                    "entities".to_string(),
                    "summary".to_string(),
                ],
                max_preview_bytes: 128,
            },
            ..DrivePutOptions::default()
        };
        let input = format!(
            "# Intake\nAuthorization: Bearer sk-secretsecret123\n{}\nTail marker",
            "classification context ".repeat(20)
        );
        let tx = transduce_document(TransducerInput {
            bytes: input.as_bytes(),
            filename: Some("intake.md"),
            options: &opts,
        })
        .unwrap();

        let request = tx.model_request.expect("enabled model request");
        assert_eq!(request.role, "document_classifier");
        assert_eq!(request.fields, vec!["summary", "entities"]);
        assert_eq!(request.mime, "text/markdown");
        assert_eq!(request.filename.as_deref(), Some("intake.md"));
        assert_eq!(request.content_hash, tx.content_hash);
        assert!(request.text_preview.contains("[REDACTED_SECRET]"));
        assert!(!request.text_preview.contains("sk-secretsecret123"));
        assert!(request.text_preview.len() <= 131);
    }
}
