use sha2::{Digest, Sha256};
use tm_memory::MemoryRecordStatus;
use tm_modes::{ReviewAddendumChange, ReviewAddendumSection, ReviewMetadata};
use uuid::Uuid;

use crate::{
    MemoryContext, MemoryRetrievalMode, PersonaAutoCandidate, PersonaAutoCandidateEvidence,
    PersonaAutoCandidateTrigger,
    store::{
        MAX_PERSONA_AUTO_CANDIDATE_EVIDENCE, MAX_PERSONA_AUTO_CANDIDATE_EVIDENCE_REF_BYTES,
        MAX_PERSONA_AUTO_CANDIDATE_EVIDENCE_REFS, PERSONA_AUTO_CANDIDATE_SCHEMA_VERSION,
    },
};

const MAX_P8_CANDIDATES_INSPECTED: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DetectedPersonaCandidate {
    pub(super) change: ReviewAddendumChange,
    pub(super) metadata: PersonaAutoCandidate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PreferenceSignal {
    AddressAs(String),
    AvoidAddress(String),
    TraditionalChinese,
    ConciseReplies,
    DetailedExplanations,
    LeadWithOutcome,
    FewerQuestions,
    DirectTone,
    GentleTone,
}

#[derive(Debug, Clone)]
struct CurrentSignal {
    signal: PreferenceSignal,
    correction: bool,
}

pub(super) fn detect(
    source_turn_id: Uuid,
    message: &str,
    memory: &MemoryContext,
) -> Option<DetectedPersonaCandidate> {
    if memory.retrieval.mode == MemoryRetrievalMode::LegacyLexical {
        return None;
    }
    let current = classify_current(message)?;
    let mut evidence = Vec::new();
    for trace in memory
        .retrieval
        .candidates
        .iter()
        .take(MAX_P8_CANDIDATES_INSPECTED)
    {
        if !trace.included
            || trace.status != MemoryRecordStatus::Active
            || trace.evidence.is_empty()
        {
            continue;
        }
        let Some(item) = memory
            .hybrid_recall
            .iter()
            .find(|item| item.id == trace.id && item.source_uri == trace.source_uri)
        else {
            continue;
        };
        if !signal_matches_evidence(&current.signal, &item.text) {
            continue;
        }
        evidence.push(PersonaAutoCandidateEvidence {
            record_id: trace.id,
            kind: trace.kind,
            source_uri: trace.source_uri.clone(),
            evidence: trace
                .evidence
                .iter()
                .take(MAX_PERSONA_AUTO_CANDIDATE_EVIDENCE_REFS)
                .map(|reference| bounded_redacted(reference))
                .filter(|reference| !reference.is_empty())
                .collect(),
        });
        if evidence.len() == MAX_PERSONA_AUTO_CANDIDATE_EVIDENCE {
            break;
        }
    }

    let trigger = if current.correction {
        if evidence.is_empty() {
            return None;
        }
        PersonaAutoCandidateTrigger::PersonaMismatch
    } else {
        if evidence.len() < 2 {
            return None;
        }
        PersonaAutoCandidateTrigger::RepeatedPreference
    };
    let change = current.signal.change();
    let dedupe_key = persona_candidate_dedupe_key(&current.signal);
    Some(DetectedPersonaCandidate {
        change,
        metadata: PersonaAutoCandidate {
            schema_version: PERSONA_AUTO_CANDIDATE_SCHEMA_VERSION,
            trigger,
            dedupe_key,
            source_turn_id,
            evidence,
        },
    })
}

fn classify_current(message: &str) -> Option<CurrentSignal> {
    if let Some(name) = extract_address(
        message,
        &["don't call me ", "do not call me ", "不要叫我", "別叫我"],
    ) {
        return Some(CurrentSignal {
            signal: PreferenceSignal::AvoidAddress(name),
            correction: true,
        });
    }
    if let Some(name) = extract_address(
        message,
        &[
            "please call me ",
            "call me ",
            "address me as ",
            "請叫我",
            "叫我",
        ],
    ) {
        return Some(CurrentSignal {
            signal: PreferenceSignal::AddressAs(name),
            correction: contains_any(
                &normalize(message),
                &["again", "already told", "都說了", "不是說"],
            ),
        });
    }

    let normalized = normalize(message);
    for (signal, positive, corrections) in [
        (
            PreferenceSignal::TraditionalChinese,
            &[
                "請用繁體中文",
                "用繁體中文",
                "回覆用繁體",
                "use traditional chinese",
                "reply in traditional chinese",
            ][..],
            &["不要用簡體", "別用簡體", "don t use simplified chinese"][..],
        ),
        (
            PreferenceSignal::ConciseReplies,
            &[
                "請簡短",
                "簡短一點",
                "精簡一點",
                "keep it concise",
                "be more concise",
                "shorter replies",
                "less verbose",
            ][..],
            &[
                "不要那麼囉嗦",
                "別那麼囉嗦",
                "太囉嗦",
                "too verbose",
                "stop rambling",
            ][..],
        ),
        (
            PreferenceSignal::DetailedExplanations,
            &[
                "請詳細一點",
                "多一點細節",
                "explain in more detail",
                "more detailed explanations",
            ][..],
            &["不要太簡略", "too terse", "not enough detail"][..],
        ),
        (
            PreferenceSignal::LeadWithOutcome,
            &[
                "先講結果",
                "先說結論",
                "lead with the outcome",
                "start with the result",
            ][..],
            &["不要先講過程", "stop burying the result"][..],
        ),
        (
            PreferenceSignal::FewerQuestions,
            &["少問一點問題", "fewer questions", "ask fewer questions"][..],
            &[
                "不要一直問問題",
                "別一直問問題",
                "stop asking so many questions",
            ][..],
        ),
        (
            PreferenceSignal::DirectTone,
            &["直接一點", "be more direct", "use a direct tone"][..],
            &["不要拐彎抹角", "stop sugarcoating"][..],
        ),
        (
            PreferenceSignal::GentleTone,
            &[
                "溫柔一點",
                "語氣柔和一點",
                "be gentler",
                "use a gentler tone",
            ][..],
            &["語氣太兇", "too harsh", "don t be so harsh"][..],
        ),
    ] {
        if contains_any(&normalized, corrections) {
            return Some(CurrentSignal {
                signal,
                correction: true,
            });
        }
        if contains_any(&normalized, positive) {
            return Some(CurrentSignal {
                signal,
                correction: false,
            });
        }
    }
    None
}

fn signal_matches_evidence(signal: &PreferenceSignal, evidence: &str) -> bool {
    let normalized = normalize(evidence);
    match signal {
        PreferenceSignal::AddressAs(name) | PreferenceSignal::AvoidAddress(name) => {
            normalized.contains(&normalize(name))
                && contains_any(
                    &normalized,
                    &["call", "called", "address", "稱呼", "叫我", "不要叫"],
                )
        }
        PreferenceSignal::TraditionalChinese => contains_any(
            &normalized,
            &[
                "繁體中文",
                "traditional chinese",
                "不要用簡體",
                "simplified chinese",
            ],
        ),
        PreferenceSignal::ConciseReplies => contains_any(
            &normalized,
            &[
                "簡短",
                "精簡",
                "囉嗦",
                "concise",
                "shorter replies",
                "verbose",
            ],
        ),
        PreferenceSignal::DetailedExplanations => contains_any(
            &normalized,
            &[
                "詳細",
                "細節",
                "more detail",
                "detailed explanation",
                "too terse",
            ],
        ),
        PreferenceSignal::LeadWithOutcome => contains_any(
            &normalized,
            &[
                "先講結果",
                "先說結論",
                "lead with the outcome",
                "start with the result",
            ],
        ),
        PreferenceSignal::FewerQuestions => contains_any(
            &normalized,
            &[
                "少問",
                "不要一直問",
                "fewer questions",
                "asking so many questions",
            ],
        ),
        PreferenceSignal::DirectTone => contains_any(
            &normalized,
            &[
                "直接一點",
                "拐彎抹角",
                "more direct",
                "direct tone",
                "sugarcoating",
            ],
        ),
        PreferenceSignal::GentleTone => contains_any(
            &normalized,
            &[
                "溫柔",
                "柔和",
                "語氣太兇",
                "gentler",
                "gentle tone",
                "too harsh",
            ],
        ),
    }
}

impl PreferenceSignal {
    fn key(&self) -> String {
        match self {
            Self::AddressAs(name) => format!("address_as:{}", normalize(name)),
            Self::AvoidAddress(name) => format!("avoid_address:{}", normalize(name)),
            Self::TraditionalChinese => "traditional_chinese".to_string(),
            Self::ConciseReplies => "concise_replies".to_string(),
            Self::DetailedExplanations => "detailed_explanations".to_string(),
            Self::LeadWithOutcome => "lead_with_outcome".to_string(),
            Self::FewerQuestions => "fewer_questions".to_string(),
            Self::DirectTone => "direct_tone".to_string(),
            Self::GentleTone => "gentle_tone".to_string(),
        }
    }

    fn change(&self) -> ReviewAddendumChange {
        let (section, label, summary) = match self {
            Self::AddressAs(name) => (
                ReviewAddendumSection::AddressGuidance,
                "Preferred form of address",
                format!("Address the owner as {name} when using a name makes the reply clearer."),
            ),
            Self::AvoidAddress(name) => (
                ReviewAddendumSection::AddressGuidance,
                "Avoided form of address",
                format!("Do not address the owner as {name}."),
            ),
            Self::TraditionalChinese => (
                ReviewAddendumSection::InteractionPreference,
                "Traditional Chinese replies",
                "Use Traditional Chinese for routine conversation unless the owner requests another language."
                    .to_string(),
            ),
            Self::ConciseReplies => (
                ReviewAddendumSection::InteractionPreference,
                "Concise routine replies",
                "Keep routine replies concise while preserving necessary evidence and safety details."
                    .to_string(),
            ),
            Self::DetailedExplanations => (
                ReviewAddendumSection::InteractionPreference,
                "Detailed explanations",
                "Include enough implementation detail to make technical explanations self-contained."
                    .to_string(),
            ),
            Self::LeadWithOutcome => (
                ReviewAddendumSection::InteractionPreference,
                "Outcome-first replies",
                "Lead with the verified outcome before implementation details.".to_string(),
            ),
            Self::FewerQuestions => (
                ReviewAddendumSection::InteractionPreference,
                "Bounded clarification",
                "Ask only clarification questions that materially change the safe result."
                    .to_string(),
            ),
            Self::DirectTone => (
                ReviewAddendumSection::ToneGuidance,
                "Direct tone",
                "Use a direct tone without dropping warmth or safety boundaries.".to_string(),
            ),
            Self::GentleTone => (
                ReviewAddendumSection::ToneGuidance,
                "Gentler tone",
                "Use a gentler tone while remaining honest and concrete.".to_string(),
            ),
        };
        ReviewAddendumChange {
            section,
            before: None,
            after: ReviewMetadata {
                label: label.to_string(),
                summary,
            },
        }
    }
}

fn persona_candidate_dedupe_key(signal: &PreferenceSignal) -> String {
    let digest = Sha256::digest(format!("persona:miku:{}", signal.key()).as_bytes());
    format!("persona-auto:v1:sha256:{digest:x}")
}

fn extract_address(message: &str, markers: &[&str]) -> Option<String> {
    let searchable = message.to_ascii_lowercase();
    for marker in markers {
        let Some(index) = searchable.find(marker) else {
            continue;
        };
        let start = index + marker.len();
        let suffix = message.get(start..)?;
        let candidate = suffix
            .split([
                '\n', '\r', '.', ',', '!', '?', '。', '，', '！', '？', ':', '：',
            ])
            .next()
            .unwrap_or_default()
            .trim();
        let candidate = [" from now on", " instead", " please", "就好", "即可"]
            .into_iter()
            .fold(candidate, |value, suffix| {
                value.strip_suffix(suffix).unwrap_or(value).trim()
            });
        if candidate.is_empty()
            || candidate.len() > 40
            || candidate.split_whitespace().count() > 3
            || candidate
                .chars()
                .any(|ch| !(ch.is_alphanumeric() || ch == ' ' || ch == '-' || ch == '_'))
            || contains_any(
                &normalize(candidate),
                &[
                    "soul",
                    "prompt",
                    "config",
                    "capability",
                    "system",
                    "指令",
                    "設定",
                ],
            )
        {
            continue;
        }
        return Some(candidate.to_string());
    }
    None
}

fn normalize(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || ('\u{4e00}'..='\u{9fff}').contains(&ch) {
                ch
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

fn bounded_redacted(value: &str) -> String {
    let redacted = tm_memory::redact_dream_text(value).text;
    if redacted.len() <= MAX_PERSONA_AUTO_CANDIDATE_EVIDENCE_REF_BYTES {
        return redacted;
    }
    let mut end = MAX_PERSONA_AUTO_CANDIDATE_EVIDENCE_REF_BYTES;
    while !redacted.is_char_boundary(end) {
        end -= 1;
    }
    redacted[..end].to_string()
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::json;
    use tm_memory::{
        EpisodicMemoryRecord, HybridMemoryCandidate, MemoryRecordEvidence, MemoryRecordResource,
        StoredMemoryRecord,
    };

    use super::*;
    use crate::{
        AutoEvolutionReviewDisposition, InMemoryStore, NewApprovalRequest,
        NewAutoEvolutionReviewBundle, NewEvolutionReviewProposal, NewSession, Store,
    };
    use tm_modes::{ModeId, ReviewApplyContract, ReviewProposalStatus, ReviewProposalTarget};

    fn context(texts: &[&str]) -> MemoryContext {
        let now = Utc::now();
        let candidates = texts
            .iter()
            .enumerate()
            .map(|(index, text)| {
                let record =
                    StoredMemoryRecord::new(MemoryRecordResource::Episodic(EpisodicMemoryRecord {
                        schema_version: tm_memory::MEMORY_RECORD_SCHEMA_VERSION,
                        id: Uuid::new_v4(),
                        owner_subject: "brian".to_string(),
                        memory_scope: "global".to_string(),
                        text: (*text).to_string(),
                        evidence: vec![MemoryRecordEvidence::resource(
                            format!("memory://fixtures/{index}"),
                            "approved owner evidence",
                        )],
                        confidence: 0.95,
                        importance: 0.8,
                        observed_at: now,
                        effective_from: now,
                        effective_to: None,
                        status: MemoryRecordStatus::Active,
                        links: Default::default(),
                        created_at: now,
                    }))
                    .unwrap();
                HybridMemoryCandidate {
                    record,
                    lexical_rank: Some(index as u32 + 1),
                    lexical_score: Some(0.9 - index as f32 * 0.1),
                    dense_rank: None,
                    dense_score: None,
                    embedding_version: None,
                    rrf_score: 0.03 - index as f32 * 0.001,
                }
            })
            .collect();
        MemoryContext::from_hybrid_candidates_with_summaries(
            "brian",
            "global",
            Vec::new(),
            candidates,
            1_600,
            Some("fixture-v1".to_string()),
        )
    }

    #[test]
    fn repeated_preference_requires_two_distinct_bounded_p8_records() {
        let turn_id = Uuid::new_v4();
        assert!(
            detect(
                turn_id,
                "請簡短一點",
                &context(&["Brian prefers concise replies."])
            )
            .is_none()
        );

        let detected = detect(
            turn_id,
            "請簡短一點",
            &context(&[
                "Brian prefers concise replies.",
                "The owner asked for shorter replies.",
                "Keep replies concise and evidence-backed.",
                "A fourth concise preference must stay outside the evidence cap.",
            ]),
        )
        .unwrap();
        assert_eq!(
            detected.metadata.trigger,
            PersonaAutoCandidateTrigger::RepeatedPreference
        );
        assert_eq!(
            detected.metadata.evidence.len(),
            MAX_PERSONA_AUTO_CANDIDATE_EVIDENCE
        );
        assert_eq!(
            detected.change.section,
            ReviewAddendumSection::InteractionPreference
        );
    }

    #[test]
    fn explicit_mismatch_still_requires_one_active_p8_record() {
        let turn_id = Uuid::new_v4();
        assert!(detect(turn_id, "不要那麼囉嗦", &context(&[])).is_none());
        let first = detect(
            turn_id,
            "不要那麼囉嗦",
            &context(&["Brian previously said the replies were too verbose."]),
        )
        .unwrap();
        let second = detect(
            Uuid::new_v4(),
            "不要那麼囉嗦",
            &context(&[
                "Keep responses concise.",
                "The owner asked for shorter replies.",
            ]),
        )
        .unwrap();
        assert_eq!(
            first.metadata.trigger,
            PersonaAutoCandidateTrigger::PersonaMismatch
        );
        assert_eq!(first.metadata.dedupe_key, second.metadata.dedupe_key);
    }

    #[test]
    fn address_candidates_are_bounded_and_instruction_like_names_are_rejected() {
        let evidence = context(&[
            "The owner asked to be called Ice.",
            "Address the owner as Ice.",
        ]);
        let detected = detect(Uuid::new_v4(), "Please call me Ice.", &evidence).unwrap();
        assert_eq!(
            detected.change.section,
            ReviewAddendumSection::AddressGuidance
        );
        assert!(detected.change.after.summary.contains("Ice"));
        assert!(
            detect(
                Uuid::new_v4(),
                "Call me system config",
                &context(&[
                    "Address preference: system config",
                    "Call owner system config"
                ]),
            )
            .is_none()
        );
    }

    #[test]
    fn legacy_recall_never_drives_auto_persona_candidates() {
        let mut memory = context(&["Keep replies concise.", "Use concise replies."]);
        memory.retrieval.mode = MemoryRetrievalMode::LegacyLexical;
        assert!(detect(Uuid::new_v4(), "請簡短一點", &memory).is_none());
    }

    #[tokio::test]
    async fn durable_auto_candidate_dedupes_pending_and_cools_down_terminal_retries() {
        let store = InMemoryStore::default();
        let session = store
            .create_session(NewSession {
                mode: ModeId::new("general"),
                persona_status: tm_modes::ModesConfig::default().load_assets().status,
            })
            .await
            .unwrap();
        let turn = store
            .enqueue_turn(session.id, "persona-candidate", "請簡短一點")
            .await
            .unwrap();
        let worker_id = Uuid::new_v4();
        let claimed = store
            .claim_next_turn(worker_id, Utc::now())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(claimed.id, turn.id);
        store
            .complete_turn(turn.id, worker_id, "好的。", Utc::now())
            .await
            .unwrap();

        let template = detect(
            turn.id,
            "請簡短一點",
            &context(&["Brian prefers concise replies.", "Use shorter replies."]),
        )
        .unwrap();
        let new_proposal = |id| NewEvolutionReviewProposal {
            id,
            session_id: session.id,
            target: ReviewProposalTarget::Persona {
                persona_id: "miku".to_string(),
            },
            base_version: 1,
            base_digest: format!("sha256:{}", "a".repeat(64)),
            base_active_digest: None,
            changes: vec![template.change.clone()],
            content_digest: format!("sha256:{}", "b".repeat(64)),
            apply_contract: ReviewApplyContract::VersionedPersonaAddendum,
            auto_candidate: Some(template.metadata.clone()),
        };

        let first = store
            .create_auto_evolution_review_proposal(
                new_proposal(Uuid::new_v4()),
                Utc::now() - chrono::Duration::days(7),
            )
            .await
            .unwrap();
        assert_eq!(first.disposition, AutoEvolutionReviewDisposition::Created);
        let duplicate = store
            .create_auto_evolution_review_proposal(
                new_proposal(Uuid::new_v4()),
                Utc::now() - chrono::Duration::days(7),
            )
            .await
            .unwrap();
        assert_eq!(
            duplicate.disposition,
            AutoEvolutionReviewDisposition::Duplicate
        );

        store
            .update_evolution_review_proposal_status(
                first.proposal.id,
                ReviewProposalStatus::Denied,
            )
            .await
            .unwrap();
        let cooldown = store
            .create_auto_evolution_review_proposal(
                new_proposal(Uuid::new_v4()),
                Utc::now() - chrono::Duration::days(7),
            )
            .await
            .unwrap();
        assert_eq!(
            cooldown.disposition,
            AutoEvolutionReviewDisposition::Cooldown
        );
        let after_cooldown = store
            .create_auto_evolution_review_proposal(
                new_proposal(Uuid::new_v4()),
                Utc::now() + chrono::Duration::seconds(1),
            )
            .await
            .unwrap();
        assert_eq!(
            after_cooldown.disposition,
            AutoEvolutionReviewDisposition::Created
        );
    }

    #[tokio::test]
    async fn in_memory_auto_candidate_bundle_rolls_back_fault_and_commits_once() {
        let store = InMemoryStore::default();
        let session = store
            .create_session(NewSession {
                mode: ModeId::new("general"),
                persona_status: tm_modes::ModesConfig::default().load_assets().status,
            })
            .await
            .unwrap();
        let turn = store
            .enqueue_turn(session.id, "persona-bundle", "請簡短一點")
            .await
            .unwrap();
        let worker_id = Uuid::new_v4();
        store
            .claim_next_turn(worker_id, Utc::now())
            .await
            .unwrap()
            .unwrap();
        store
            .complete_turn(turn.id, worker_id, "好的。", Utc::now())
            .await
            .unwrap();
        let detected = detect(
            turn.id,
            "請簡短一點",
            &context(&["Brian prefers concise replies.", "Use shorter replies."]),
        )
        .unwrap();
        let proposal_id = Uuid::new_v4();
        let approval_id = Uuid::new_v4();
        let now = Utc::now();
        let proposal = NewEvolutionReviewProposal {
            id: proposal_id,
            session_id: session.id,
            target: ReviewProposalTarget::Persona {
                persona_id: "miku".to_string(),
            },
            base_version: 1,
            base_digest: format!("sha256:{}", "a".repeat(64)),
            base_active_digest: None,
            changes: vec![detected.change],
            content_digest: format!("sha256:{}", "b".repeat(64)),
            apply_contract: ReviewApplyContract::VersionedPersonaAddendum,
            auto_candidate: Some(detected.metadata),
        };
        let bundle = NewAutoEvolutionReviewBundle {
            proposal,
            approval: NewApprovalRequest {
                id: approval_id,
                session_id: session.id,
                turn_id: Some(turn.id),
                requester_id: Uuid::new_v4(),
                origin: "evolution-review".to_string(),
                action: "review persona addendum miku".to_string(),
                scope_json: json!({"proposalId": proposal_id}),
                options_json: json!([]),
                effect_type: "evolution_review".to_string(),
                effect_payload_json: json!({"proposalId": proposal_id}),
                resumable: true,
                created_at: now,
                expires_at: now + chrono::Duration::minutes(1),
            },
            proposal_payload_json: json!({
                "kind": "evolution_review",
                "proposalId": proposal_id,
                "status": "pending",
            }),
            approval_payload_json: json!({
                "approvalId": approval_id,
                "backend": "evolution-review",
            }),
            cooldown_since: now - chrono::Duration::days(7),
        };

        store.fail_next_auto_evolution_bundle_before_commit();
        assert!(
            store
                .create_auto_evolution_review_bundle(bundle.clone())
                .await
                .is_err()
        );
        assert!(
            store
                .evolution_review_proposals_for_session(session.id)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .approval_request(session.id, approval_id)
                .await
                .is_err()
        );
        assert!(
            store
                .events_after(session.id, None)
                .await
                .unwrap()
                .is_empty()
        );

        let created = store
            .create_auto_evolution_review_bundle(bundle.clone())
            .await
            .unwrap();
        assert_eq!(created.disposition, AutoEvolutionReviewDisposition::Created);
        assert_eq!(created.events.len(), 2);
        assert_eq!(
            created.approval.unwrap().request_event_seq,
            Some(created.events[1].seq)
        );
        let duplicate = store
            .create_auto_evolution_review_bundle(bundle)
            .await
            .unwrap();
        assert_eq!(
            duplicate.disposition,
            AutoEvolutionReviewDisposition::Duplicate
        );
        assert!(duplicate.approval.is_none());
        assert!(duplicate.events.is_empty());
        let events = store.events_after(session.id, None).await.unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == "write_proposal")
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == "approval")
                .count(),
            1
        );
        assert!(events.iter().all(|event| event.turn_id == Some(turn.id)));
    }
}
