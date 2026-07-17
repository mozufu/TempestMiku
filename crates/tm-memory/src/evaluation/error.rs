#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RecallEvaluationError {
    #[error("invalid recall evaluation manifest: {0}")]
    InvalidManifest(String),
    #[error("unsupported recall evaluation schema version {0}")]
    UnsupportedSchemaVersion(u16),
    #[error("unsupported recall evaluator version {0}")]
    UnsupportedEvaluatorVersion(String),
    #[error("recall observations do not exactly match the fixture cases")]
    ObservationSetMismatch,
    #[error("case {case_id} has {actual} latency samples; expected {expected}")]
    InvalidLatencySampleCount {
        case_id: String,
        expected: usize,
        actual: usize,
    },
    #[error("case {0} returned a record outside the fixture manifest")]
    UnknownObservedRecord(String),
    #[error("case {0} returned duplicate or inconsistent candidate ids")]
    InvalidObservedCandidates(String),
}
