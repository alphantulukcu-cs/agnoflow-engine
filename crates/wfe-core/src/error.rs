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
    /// WOR-65: WFE revizyon token'ı (`Wfes::rev()` — son WFAH seq'i) eskimiş,
    /// stale write reddedildi. İki yoldan üretilir:
    ///   1. **Açık** — istemci `expected_rev` gönderdi, yüklenen durumun revizyonu
    ///      farklı. Precondition ihlali; `WfeExecutor` retry ETMEZ (reload aynı
    ///      uyuşmazlığı üretir), doğrudan 409.
    ///   2. **Örtük** — commit sırasında `wf.wfah`/`wf.wfe_dynctx`'in
    ///      `UNIQUE (wfe_id, seq)` kısıtı ihlal edildi: engine'in seq'i eskimiş
    ///      bir load'dan hesaplanmış, araya başka bir commit girmiş. `expected_rev`
    ///      GÖNDERMEYEN istemciler için de lost-update koruması; bu yol retry
    ///      edilebilir (aşağıdaki `is_retryable`).
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
    ///
    /// WOR-65 notu — `StaleRevision` `true`'dur ama bu YALNIZ örtük (seq çakışması)
    /// yolu içindir: reload taze seq verir, aksiyon meşru biçimde uygulanabilir.
    /// İstemci `expected_rev` GÖNDERDİYSE retry pratikte tek turda biter: döngü
    /// reload eder, `expected_rev` artık taze duruma uymaz ve döngünün başındaki
    /// açık kontrol `StaleRevision` ile ERKEN döner. Yani If-Match semantiği
    /// (durum değiştiyse uygulama) retry döngüsüne rağmen korunur.
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
    #[error("reassign target is not eligible for the current node's candidate actor")]
    TargetNotEligible,
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
    /// WFC: çağrılan WFD bulunamadı / pinlenen versiyon yayınlanmamış.
    #[error("WFD.CallNotFound: {0}")]
    CallNotFound(String),
    /// WFC: çağrılanı başlatan aktör, çağrılanın start node c_a'sıyla eşleşmiyor.
    /// Statik doğrulanamaz (org resolve runtime'dır) — bu yüzden runtime hatasıdır.
    #[error("WFD.CallUnauthorized: {0}")]
    CallUnauthorized(String),
    #[error("WFD.CallFailed: {0}")]
    CallFailed(String),
    #[error("WFD.CallTimeout: {0}")]
    CallTimeout(String),
}
