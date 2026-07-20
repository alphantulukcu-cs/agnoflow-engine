use thiserror::Error;

/// WOR-62: `EngineError::Conflict`'in makine-okunur ayrımı. HTTP 409 gövdesindeki
/// `code` alanı doğrudan `ConflictKind::code()`'tan gelir; portal "ne oldu"
/// sorusunu (collapse mı oldu, kol mu taşındı, başkası mı aldı) bu kodla yanıtlar
/// — hata METNİNİ parse etmesi GEREKMEZ.
///
/// WOR-65 (WFE revizyon token'ı + stale-write koruması) bu taksonomiyi PAYLAŞIR:
/// yeni conflict sebepleri buraya varyant olarak eklenir, `EngineError`'a yeni
/// bir üst-seviye hata tipi AÇILMAZ. `StaleRevision` o iş için şimdiden ayrılmıştır.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    /// Paralel mod bitmiş (`join_target IS NULL`): bir kardeş kol collapse /
    /// join / terminal ile WFE'yi taşımış. Kaybeden aksiyonun ÖNCÜLÜ (paralel
    /// modda bir kolda duruyorum) artık geçersiz — retry bunu düzeltemez.
    Collapsed,
    /// Kol satırı CAS'ı tutmadı: kol node'u değişmiş ya da artık `active` değil.
    BranchMoved,
    /// Kol-varış sayımı engine'in görüşüyle uyuşmadı (BranchArrived/JoinComplete).
    BranchArrival,
    /// WFE satırı yok ya da tenant uyuşmuyor — `FOR UPDATE` kilidi alınamadı.
    WfeGone,
    /// Claim CAS'ı kaybedildi: kol/WFE bu arada başkasına atanmış.
    AlreadyClaimed,
    /// WOR-65 rezervi: WFE revizyon token'ı eskimiş, stale write reddedildi.
    StaleRevision,
}

impl ConflictKind {
    /// API sözleşmesinin parçası olan STABİL kod. Değiştirilirse portal kırılır.
    pub fn code(self) -> &'static str {
        match self {
            Self::Collapsed => "conflict.collapsed",
            Self::BranchMoved => "conflict.branch_moved",
            Self::BranchArrival => "conflict.branch_arrival",
            Self::WfeGone => "conflict.wfe_gone",
            Self::AlreadyClaimed => "conflict.already_claimed",
            Self::StaleRevision => "conflict.stale_revision",
        }
    }

    /// Reload + engine'i yeniden koşmak sonucu DEĞİŞTİREBİLİR mi?
    /// (bkz. `WfeExecutor::apply` retry döngüsü). `false` olanlar kalıcı bir
    /// durum geçişini bildirir: tekrar denemek aynı cevabı verir, doğrudan 409.
    pub fn is_retryable(self) -> bool {
        match self {
            Self::BranchMoved | Self::BranchArrival | Self::StaleRevision => true,
            Self::Collapsed | Self::WfeGone | Self::AlreadyClaimed => false,
        }
    }
}

impl std::fmt::Display for ConflictKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("permission denied: actor is not in candidate set for action '{0}'")]
    PermissionDenied(String),
    #[error("transition not found for action '{0}' in current state")]
    TransitionNotFound(String),
    #[error("ambiguous action '{action}' — matches multiple active parallel branches, specify node (candidates: {candidates:?})")]
    AmbiguousAction {
        action: String,
        candidates: Vec<String>,
    },
    #[error("wfe is terminal — no further actions accepted")]
    WfeTerminal,
    #[error("wfe deadline (SLA-3) has passed — no further actions accepted pending SLA sweep")]
    WfeExpired,
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
    /// WOR-62: sebep `ConflictKind` ile taşınır — HTTP 409 gövdesindeki `code`
    /// alanı ve executor'ın retry kararı bu ayrımdan okunur.
    #[error("optimistic concurrency conflict [{0}]: state changed under commit")]
    Conflict(ConflictKind),
    #[error("invalid wfd: {0}")]
    InvalidWfd(String),
    #[error("unsupported wfd_version: {0} (desteklenen: 2.2)")]
    UnsupportedWfdVersion(String),
    #[error("effect value error: {0}")]
    EffectValue(String),
    #[error("autoexec error: {0}")]
    Autoexec(String),
}
