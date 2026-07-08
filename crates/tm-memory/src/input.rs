use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DreamInputBudget {
    pub max_chunks: usize,
    pub max_chunk_chars: usize,
    pub max_message_chars: usize,
}

impl Default for DreamInputBudget {
    fn default() -> Self {
        Self {
            max_chunks: 4,
            max_chunk_chars: 4_000,
            max_message_chars: 1_200,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DreamInputMessage {
    pub seq: i64,
    pub role: String,
    pub content: String,
    pub had_redactions: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DreamInputChunk {
    pub index: usize,
    pub messages: Vec<DreamInputMessage>,
    pub estimated_chars: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BudgetedDreamInput {
    pub chunks: Vec<DreamInputChunk>,
    pub total_messages: usize,
    pub included_messages: usize,
    pub omitted_messages: usize,
    pub truncated_messages: usize,
    pub total_chars: usize,
    pub included_chars: usize,
    pub truncated: bool,
}

impl DreamInputBudget {
    pub fn apply(&self, messages: Vec<DreamInputMessage>) -> BudgetedDreamInput {
        let max_chunks = self.max_chunks.max(1);
        let max_chunk_chars = self.max_chunk_chars.max(64);
        let max_message_chars = self.max_message_chars.max(16).min(max_chunk_chars);
        let total_messages = messages.len();
        let total_chars = messages
            .iter()
            .map(|message| message.content.chars().count())
            .sum();
        let mut chunks = Vec::<DreamInputChunk>::new();
        let mut truncated_messages = 0;
        let mut omitted_messages = 0;

        for message in messages {
            let prepared = if message.content.chars().count() > max_message_chars {
                truncated_messages += 1;
                DreamInputMessage {
                    content: truncate_chars(&message.content, max_message_chars),
                    ..message
                }
            } else {
                message
            };
            let estimated = message_cost(&prepared);
            let needs_new_chunk = chunks.last().is_none_or(|chunk| {
                !chunk.messages.is_empty() && chunk.estimated_chars + estimated > max_chunk_chars
            });
            if needs_new_chunk {
                if chunks.len() >= max_chunks {
                    omitted_messages += 1;
                    continue;
                }
                chunks.push(DreamInputChunk {
                    index: chunks.len(),
                    messages: Vec::new(),
                    estimated_chars: 0,
                });
            }
            if chunks.is_empty() {
                chunks.push(DreamInputChunk {
                    index: 0,
                    messages: Vec::new(),
                    estimated_chars: 0,
                });
            }
            let chunk = chunks.last_mut().expect("chunk exists");
            chunk.estimated_chars += estimated;
            chunk.messages.push(prepared);
        }

        let included_messages = chunks.iter().map(|chunk| chunk.messages.len()).sum();
        let included_chars = chunks
            .iter()
            .flat_map(|chunk| chunk.messages.iter())
            .map(|message| message.content.chars().count())
            .sum();
        BudgetedDreamInput {
            chunks,
            total_messages,
            included_messages,
            omitted_messages,
            truncated_messages,
            total_chars,
            included_chars,
            truncated: omitted_messages > 0 || truncated_messages > 0,
        }
    }
}

impl BudgetedDreamInput {
    pub fn messages(&self) -> impl Iterator<Item = &DreamInputMessage> {
        self.chunks.iter().flat_map(|chunk| chunk.messages.iter())
    }
}

fn message_cost(message: &DreamInputMessage) -> usize {
    message.role.chars().count() + message.content.chars().count() + 16
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let keep = max_chars.saturating_sub(3);
    let mut out = value.chars().take(keep).collect::<String>();
    out.push_str("...");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(seq: i64, content: &str) -> DreamInputMessage {
        DreamInputMessage {
            seq,
            role: "user".to_string(),
            content: content.to_string(),
            had_redactions: false,
        }
    }

    #[test]
    fn budgets_dream_input_into_bounded_chunks() {
        let budget = DreamInputBudget {
            max_chunks: 2,
            max_chunk_chars: 80,
            max_message_chars: 32,
        };
        let result = budget.apply(vec![
            input(1, "short first"),
            input(2, "this message is intentionally long enough to be trimmed"),
            input(3, "third chunk candidate"),
            input(4, "fourth chunk candidate"),
            input(5, "fifth chunk candidate"),
        ]);

        assert_eq!(result.total_messages, 5);
        assert!(result.included_messages < result.total_messages);
        assert_eq!(result.chunks.len(), 2);
        assert_eq!(result.truncated_messages, 1);
        assert!(result.omitted_messages > 0);
        assert!(result.truncated);
        assert!(
            result
                .messages()
                .any(|message| message.content.ends_with("..."))
        );
        assert!(
            result
                .chunks
                .iter()
                .all(|chunk| chunk.estimated_chars <= 80)
        );
    }

    #[test]
    fn keeps_at_least_one_chunk_with_tiny_config_values() {
        let result = DreamInputBudget {
            max_chunks: 0,
            max_chunk_chars: 0,
            max_message_chars: 0,
        }
        .apply(vec![input(1, "hello world")]);

        assert_eq!(result.chunks.len(), 1);
        assert_eq!(result.included_messages, 1);
        assert_eq!(result.omitted_messages, 0);
    }
}
