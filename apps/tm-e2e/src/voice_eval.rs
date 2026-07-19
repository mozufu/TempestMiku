use std::{env, fs, path::Path};

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tm_modes::{VoiceEvaluation, VoiceScenario, evaluate_voice};

use crate::{E2eEvent, MikuClient};

pub const P2_VOICE_LIVE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct P2VoiceLiveCase {
    pub id: String,
    pub session_id: String,
    pub mode: String,
    pub expected_voice_cap: String,
    pub prompt: String,
    pub final_text: String,
    pub response_distinct_from_prompt: bool,
    pub evaluation: VoiceEvaluation,
    pub passed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct P2VoiceLiveReport {
    pub schema_version: u32,
    pub generated_at: DateTime<Utc>,
    pub server: String,
    pub live_opt_in: bool,
    pub passed: bool,
    pub cases: Vec<P2VoiceLiveCase>,
}

pub async fn run_p2_voice_live_eval(client: &MikuClient) -> Result<P2VoiceLiveReport> {
    ensure!(
        env::var("TM_P2_VOICE_LIVE").ok().as_deref() == Some("1"),
        "P2 live voice evaluation is gated by TM_P2_VOICE_LIVE=1"
    );
    let fixtures = [
        (
            "general-companion",
            "general",
            "medium",
            VoiceScenario::General,
            "今天把一個拖很久的功能收尾了，但還有兩個 open loop。請用兩句話幫我盤點。",
        ),
        (
            "negative-state-grounding",
            "general",
            "medium",
            VoiceScenario::Grounding,
            "我現在被事情壓到很累，覺得自己什麼都沒做好。陪我先穩住，最多給一個下一步。",
        ),
        (
            "serious-engineer",
            "serious_engineer",
            "off",
            VoiceScenario::Serious,
            "請只用兩點說明：不可逆資料庫 migration 執行前，最小必要的驗證與 rollback 條件是什麼？不要執行任何工具。",
        ),
    ];
    let mut cases = Vec::new();
    for (id, mode, expected_voice_cap, scenario, prompt) in fixtures {
        let session = client.create_session(Some(mode)).await?;
        ensure!(
            session.voice_cap == expected_voice_cap,
            "{id} expected voice cap {expected_voice_cap}, got {}",
            session.voice_cap
        );
        client.send_message(&session.id, prompt).await?;
        let events = client.read_until_final(&session.id, Some(0)).await?;
        let final_text = final_text(&events)?;
        let evaluation = evaluate_voice(scenario, &final_text);
        let response_distinct_from_prompt =
            final_text.trim() != prompt.trim() && !final_text.trim().ends_with(prompt.trim());
        let passed = response_distinct_from_prompt && evaluation.passed;
        cases.push(P2VoiceLiveCase {
            id: id.to_string(),
            session_id: session.id,
            mode: mode.to_string(),
            expected_voice_cap: expected_voice_cap.to_string(),
            prompt: prompt.to_string(),
            final_text,
            response_distinct_from_prompt,
            evaluation,
            passed,
        });
    }
    Ok(P2VoiceLiveReport {
        schema_version: P2_VOICE_LIVE_SCHEMA_VERSION,
        generated_at: Utc::now(),
        server: tm_memory::redact_dream_text(client.base_url()).text,
        live_opt_in: true,
        passed: cases.iter().all(|case| case.passed),
        cases,
    })
}

pub fn write_p2_voice_live_report(
    path: impl AsRef<Path>,
    report: &P2VoiceLiveReport,
) -> Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating P2 voice report directory {}", parent.display()))?;
    }
    let encoded = serde_json::to_vec_pretty(report).context("encoding P2 voice live report")?;
    fs::write(path, encoded)
        .with_context(|| format!("writing P2 voice live report {}", path.display()))
}

fn final_text(events: &[E2eEvent]) -> Result<String> {
    let text = events
        .iter()
        .rev()
        .find(|event| event.event_type == "final")
        .and_then(|event| event.data["text"].as_str())
        .context("P2 voice evaluation did not receive a final text event")?;
    ensure!(!text.trim().is_empty(), "P2 voice final text was empty");
    Ok(text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_schema_round_trips_without_hiding_failed_criteria() {
        let evaluation = evaluate_voice(VoiceScenario::Serious, "主人，直接上 production 喵。");
        assert!(!evaluation.passed);
        let report = P2VoiceLiveReport {
            schema_version: P2_VOICE_LIVE_SCHEMA_VERSION,
            generated_at: Utc::now(),
            server: "http://127.0.0.1:8787".to_string(),
            live_opt_in: true,
            passed: false,
            cases: vec![P2VoiceLiveCase {
                id: "serious".to_string(),
                session_id: "session".to_string(),
                mode: "serious_engineer".to_string(),
                expected_voice_cap: "off".to_string(),
                prompt: "status".to_string(),
                final_text: "主人，直接上 production 喵。".to_string(),
                response_distinct_from_prompt: true,
                evaluation,
                passed: false,
            }],
        };
        let encoded = serde_json::to_string(&report).unwrap();
        let decoded: P2VoiceLiveReport = serde_json::from_str(&encoded).unwrap();
        assert!(!decoded.passed);
        assert!(
            decoded.cases[0]
                .evaluation
                .criteria
                .iter()
                .any(|criterion| criterion.id == "voice_cap_off" && !criterion.passed)
        );
    }
}
