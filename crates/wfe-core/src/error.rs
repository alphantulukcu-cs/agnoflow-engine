use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("permission denied: actor is not in candidate set for action '{0}'")]
    PermissionDenied(String),
    #[error("transition not found for action '{0}' in current state")]
    TransitionNotFound(String),
    #[error("wfe is terminal — no further actions accepted")]
    WfeTerminal,
    #[error("wfe is unassigned — claim required before acting")]
    NotClaimed,
    #[error("actor is not the assignment owner of this wfe")]
    NotOwner,
    #[error("actor is not authorized to view this wfe")]
    Unauthorized,
    #[error("WFD.NoConditionMatched: no wft condition matched and no default given")]
    NoConditionMatched,
    #[error("invalid action input: {0}")]
    InvalidInput(String),
    #[error("start rule not matched — actor not eligible to initiate this workflow")]
    StartNotEligible,
    #[error("zen evaluation error: {0}")]
    ZenEvaluation(String),
    #[error("invalid expression: {0}")]
    InvalidExpression(String),
    #[error("org port error: {0}")]
    OrgPort(String),
    #[error("wfd port error: {0}")]
    WfdPort(String),
    #[error("wfe port error: {0}")]
    WfePort(String),
    #[error("invalid wfd: {0}")]
    InvalidWfd(String),
    #[error("unsupported wfd_version: {0} (desteklenen: 2.2)")]
    UnsupportedWfdVersion(String),
    #[error("effect value error: {0}")]
    EffectValue(String),
    #[error("autoexec error: {0}")]
    Autoexec(String),
}
