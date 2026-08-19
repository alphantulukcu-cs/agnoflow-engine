//! v2.2 simülasyon durumu — editörün simulate endpoint'leri için.
//! Engine saf olduğundan store gerekmez: SimState ⇄ Wfes dönüşümü ve
//! TransitionCommit'in in-memory uygulanması yeterlidir.
//! Not: simülasyonda claim akışı atlanır — apply öncesi state aktöre atanır.

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use wfe_core::types::{
    actor::Actor,
    dynctx::DynCtx,
    wfah::{Wfah, WfahEntry},
    wfd_v22::{JoinRule, WftTarget},
    wfe::WfeStatus,
};
use wfe_core::v22::ports::{
    BranchState, BranchStatus, CommitOutcome, NewWfe, StagedCall, TransitionCommit, Wfes,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimState {
    pub wfe_id: Uuid,
    pub orgtnt_id: Uuid,
    pub dynctx: serde_json::Value,
    pub wfah: Vec<WfahEntry>,
    pub current_node: Option<String>,
    pub status: WfeStatus,
    pub end_response: Option<serde_json::Value>,
    /// WFE'nin bittiği terminal id'si (2026-08-17) — `current_node`'un aynadaki
    /// karşılığı. `Wfes::end_terminal`in sim tarafındaki kaynağıdır; terminal
    /// `listable[]`ı (`can_view` (g)) buna bakar, yani simülasyon ile gerçek akış
    /// aynı görünürlük cevabını verebilsin diye izlenir. `#[serde(default)]` — bu
    /// alandan önce üretilmiş sim_state blob'ları onsuz da parse edilir.
    #[serde(default)]
    pub end_terminal: Option<String>,
    /// WOR-31 T4: paralel mod kol durumları (JSON alan adı `node`, bkz.
    /// `BranchState`); paralel modda değilken boş. `#[serde(default)]` — eski
    /// (fork öncesi üretilmiş) sim_state blob'ları bu alan olmadan da parse edilir.
    #[serde(default)]
    pub branches: Vec<BranchState>,
    /// WOR-31 T4: fork'ta persist edilen join hedefi; `Some` = paralel mod.
    #[serde(default)]
    pub join_target: Option<WftTarget>,
    /// WOR-72: fork'ta persist edilen quorum eşiği; `None` = AND-join (tüm kollar),
    /// `Some(k)` = k varış yeterli. `#[serde(default)]` — WOR-72 öncesi üretilmiş
    /// sim_state blob'ları bu alan olmadan parse edilir (AND olarak okunur).
    #[serde(default)]
    pub join_threshold: Option<u32>,
    /// WOR-73: fork'ta persist edilen ZEN join koşulu (`join_mode: expr`); `None` =
    /// eşik/AND kuralı. Eşikle birlikte DOLU OLMAZ (bkz. `join_rule`).
    #[serde(default)]
    pub join_when: Option<String>,
    /// WFC: bu adımda yapılacak iş akışı çağrıları.
    ///
    /// Simülasyonda GERÇEK bir WFE yaratılmaz — çağrılan akışı koşturmak simülasyonun
    /// kapsamı değildir (kendi aktörleri, kendi SLA'sı, kendi org çözümü olurdu).
    /// Onun yerine çağrı burada "bekliyor" olarak durur ve kullanıcı sonucu ELLE girer
    /// (`/simulate/call-return`). Editör bu listeyi görüp "burada şu akış çağrılacak"
    /// diyebilir; `wait` modunda akış bu çağrı çözülene kadar ilerlemez.
    ///
    /// `#[serde(default)]` — WFC öncesi üretilmiş sim_state blob'ları bu alan olmadan
    /// da parse edilir.
    #[serde(default)]
    pub pending_calls: Vec<SimCall>,
    /// 2026-08-19: simülasyonda "yüklenmiş" sayılan KATALOG belgeleri
    /// (`wfd.attachments` grup/slot). Gerçek akışta bu bilginin kaynağı depodur
    /// (`AttachmentStore::exists`); simülasyonda depo YOKTUR — baytlar hiç
    /// taşınmaz, yalnız metadata (ad/tip/boyut) tutulur ve **kapı** (bir aksiyonu
    /// kapayan zorunlu belgeler) bu listeye bakar. Böylece "belge yüklenmeden
    /// onaylanamaz" kuralı editörde de denenebilir.
    /// `#[serde(default)]` — bu alandan önce üretilmiş blob'lar onsuz parse edilir.
    #[serde(default)]
    pub attachments: Vec<SimAttachment>,
    /// 2026-08-19: simülasyonda eklenen NOTLAR (ad-hoc dosyalarıyla). Not motorun
    /// ne `$ctx`'ine ne `$wfah`'ına girer (K1) — akışın gidişatını DEĞİŞTİRMEZ;
    /// burada durmasının sebebi limitlerin (gövde uzunluğu, dosya sayısı/boyutu,
    /// WFE kotası, yasak MIME) senaryoda da denenebilmesi.
    #[serde(default)]
    pub notes: Vec<SimNote>,
}

/// Simülasyonda yüklenmiş sayılan katalog belgesi — baytlar YOK, yalnız kapının ve
/// format/boyut kuralının ihtiyaç duyduğu metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimAttachment {
    /// Katalog grup key'i (`wfd.attachments` anahtarı).
    pub group: String,
    /// Grup içindeki slot id'si (`AttachmentItem.id`).
    pub item: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default)]
    pub size_bytes: i64,
    /// Yüklemenin yapıldığı node — "hangi adımda teslim edildi" sorusu için (gerçek
    /// akışta `wf.wfe_attachment` metadata'sının karşılığı). Kapı bunu SORMAZ:
    /// dosya bir kez yüklendiyse sonraki adımlarda da yüklüdür (depo semantiği).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
}

/// Simülasyonda eklenen not — gerçek akıştaki `wf.wfe_note` satırının (+ dosyaları)
/// karşılığı. Draft/publish yaşam döngüsü YOKTUR: simülasyonda not eklemek tek
/// adımdır (draft → aksiyonla yayın zinciri DB'ye özgüdür).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimNote {
    pub body: String,
    #[serde(default)]
    pub audience: crate::note_rules::Audience,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<crate::note_rules::NoteFileSpec>,
    /// Notun yazıldığı adım (o anki `current_node` / verilen kol).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    /// Yazar — senaryoda adım aktörü, interaktif simülasyonda seçili aktör.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<Actor>,
}

/// Simülasyonda bekleyen bir WFC çağrısı — `wf.wfe_call`'ın kullanıcıya görünen özeti.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimCall {
    /// Root `calls` katalog key'i.
    pub call_key: String,
    /// wait | detached | terminal
    pub mode: String,
    /// 'node' (alt akış) | 'terminal' (ardıl akış)
    pub site_kind: String,
    /// Çağrı node'unun slug'ı ya da terminal id'si.
    pub site_key: String,
    /// WFC-IN — çağıranın ctx'inden ÇÖZÜLMÜŞ girdi. Editör bunu göstererek
    /// "çağrılana şu değerler gidecek" diyebilir.
    pub input: serde_json::Value,
    /// `wait` dışındaki modlarda çağrı beklenmez: satır bilgi amaçlıdır ve
    /// akışı bloklamaz (`detached` hemen devam eder, `terminal`'de akış bitmiştir).
    pub awaited: bool,
}

impl SimCall {
    fn from_staged(c: &StagedCall) -> Self {
        let awaited = c.mode == wfe_core::types::wfd_v22::CallMode::Wait;
        Self {
            call_key: c.call_key.clone(),
            mode: c.mode.as_str().into(),
            site_kind: c.site.kind().into(),
            site_key: c.site.key().into(),
            input: c.input.clone(),
            awaited,
        }
    }
}

impl SimState {
    /// `start`'ın wft'i Parallel OLAMAZ (validator + engine reddeder — WOR-31
    /// §Kısıtlar), bu yüzden `branches`/`join_target` her zaman boş başlar.
    pub fn from_new_wfe(new: &NewWfe) -> Self {
        let (status, current_node, end_response) = outcome_parts(&new.outcome);
        Self {
            wfe_id: new.wfe_id,
            orgtnt_id: new.orgtnt_id,
            dynctx: new.initial_dynctx.clone(),
            wfah: new.wfah_entries.clone(),
            current_node,
            status,
            end_response,
            end_terminal: new.end_terminal.clone(),
            branches: vec![],
            join_target: None,
            join_threshold: None,
            join_when: None,
            pending_calls: new.staged_calls.iter().map(SimCall::from_staged).collect(),
            attachments: vec![],
            notes: vec![],
        }
    }

    /// Simülasyon Wfes'i — `assigned_to` verilen aktöre bağlanır (claim bypass).
    /// WOR-31 T4: paralel modda `/simulate` altında ayrı bir `/claim` endpoint'i
    /// YOK (routes/simulate.rs yalnızca start/apply/possible-actions taşır) —
    /// bypass paralel modda da aynı ruhla genişletilir: verilen aktör TÜM aktif
    /// kolların `claimed_by`'ı sayılır (o kolun c_a'sına uygun olup olmadığına
    /// bakılmaksızın — tek-node bypass'ın zaten yaptığı basitleştirmenin
    /// birebir kol-seviyesi karşılığı). Simülasyonda SLA alanları izlenmez:
    /// deadline/claimed_at her zaman NULL.
    pub fn to_wfes(&self, assigned_to: Option<Uuid>) -> Wfes {
        let created_at = self
            .wfah
            .first()
            .map(|e| e.applied_at)
            .unwrap_or_else(chrono::Utc::now);
        let branches = match assigned_to {
            Some(uid) => self
                .branches
                .iter()
                .cloned()
                .map(|mut b| {
                    if b.status == BranchStatus::Active {
                        b.claimed_by = Some(uid);
                    }
                    b
                })
                .collect(),
            None => self.branches.clone(),
        };
        Wfes {
            wfe_id: self.wfe_id,
            orgtnt_id: self.orgtnt_id,
            environment_id: None,
            // Simülasyonun store'u yok → görünürlük projeksiyonu da yok; çapa
            // eski davranışa (soruyu soran aktörün birimi) düşer.
            origin_orgu_id: None,
            wfd_id: Uuid::nil(),
            wfd_version: 0,
            dynctx: DynCtx(self.dynctx.clone()),
            wfah: Wfah(self.wfah.clone()),
            status: self.status.clone(),
            current_node: self.current_node.clone(),
            end_terminal: self.end_terminal.clone(),
            assigned_to,
            end_response: self.end_response.clone(),
            deadline: None,
            claimed_at: None,
            created_at,
            branches,
            join_target: self.join_target.clone(),
            // WOR-72/WOR-73: iki alan → tek çözülmüş kural (adapter'daki okuma ile
            // aynı sıra: eşik önce).
            join_rule: match (self.join_threshold, self.join_when.clone()) {
                (Some(k), _) => JoinRule::Quorum(k.max(1)),
                (None, Some(expr)) => JoinRule::Expr(expr),
                (None, None) => JoinRule::All,
            },
        }
    }

    pub fn apply_commit(&mut self, commit: &TransitionCommit) {
        self.dynctx = commit.new_dynctx.clone();
        self.wfah.extend(commit.wfah_entries.iter().cloned());
        self.apply_branch_outcome(&commit.outcome);
        let (status, current_node, end_response) = outcome_parts(&commit.outcome);
        self.status = status;
        self.current_node = current_node;
        if end_response.is_some() {
            self.end_response = end_response;
        }
        // `end_terminal` yalnız DOLU geldiğinde yazılır — `end_response` ile aynı
        // gerekçe: ara bir commit onu boşaltmamalı, terminal'e varış tek yönlüdür.
        if commit.end_terminal.is_some() {
            self.end_terminal = commit.end_terminal.clone();
        }
        // WFC: yeni çağrılar eklenir. Öncekiler KORUNUR — `wait` modunda akış zaten
        // çağrı çözülmeden ilerleyemez; `detached`/`terminal` satırları ise geçmişin
        // parçasıdır ve editörde "şu adımda şu akış başlatıldı" olarak kalır.
        self.pending_calls
            .extend(commit.staged_calls.iter().map(SimCall::from_staged));
    }

    /// WFC: bu adımda çözülmesi BEKLENEN çağrı (yalnız `mode: wait`).
    ///
    /// `Some` ise akış bu node'da duruyor ve ilerlemesi için çağrı sonucunun elle
    /// girilmesi gerekir — editör "sonucu gir" formunu bu bilgiyle açar.
    pub fn awaited_call(&self) -> Option<&SimCall> {
        let node = self.current_node.as_deref()?;
        self.pending_calls
            .iter()
            .find(|c| c.awaited && c.site_kind == "node" && c.site_key == node)
    }

    /// Çağrı çözüldükten sonra satırı bekleyen listeden düşürür (dönüş bir kez işlenir).
    pub fn clear_awaited_call(&mut self, site_key: &str) {
        self.pending_calls
            .retain(|c| !(c.awaited && c.site_kind == "node" && c.site_key == site_key));
    }

    /// WOR-31 T4: `branches`/`join_target` yan etkileri — DB adapter'ının commit
    /// mantığının (bkz. `wfe_adapter.rs`) in-memory karşılığı. Sim TEK-THREAD'li
    /// çalıştığından eşzamanlı varış yarışı yoktur: adapter'ın `FOR UPDATE` +
    /// kol-CAS + aktif-kol sayımı doğrulaması burada YOKTUR — engine'in
    /// BranchArrived/JoinComplete görüşü sorgusuz doğru kabul edilir (T4
    /// sözleşmesi: "JoinComplete verify trivially true", Conflict imkansız).
    fn apply_branch_outcome(&mut self, outcome: &CommitOutcome) {
        match outcome {
            CommitOutcome::ForkTo {
                branches,
                join,
                join_rule,
            } => {
                self.join_target = Some(join.clone());
                let (threshold, when) = match join_rule {
                    JoinRule::All => (None, None),
                    JoinRule::Quorum(k) => (Some(*k), None),
                    JoinRule::Expr(e) => (None, Some(e.clone())),
                };
                self.join_threshold = threshold;
                self.join_when = when;
                let now = chrono::Utc::now();
                self.branches = branches
                    .iter()
                    .map(|n| BranchState {
                        branch_node: n.clone(),
                        // WOR-73: kol kimliği = giriş node'u, bir daha değişmez.
                        entry_node: n.clone(),
                        status: BranchStatus::Active,
                        claimed_by: None,
                        claimed_at: None,
                        entered_at: now,
                    })
                    .collect();
            }
            CommitOutcome::BranchMoveTo { from_node, node } => {
                if let Some(b) = self.active_branch_mut(from_node) {
                    b.branch_node = node.clone();
                    b.claimed_by = None;
                    b.claimed_at = None;
                    b.entered_at = chrono::Utc::now();
                }
            }
            CommitOutcome::BranchArrived { from_node, .. } => {
                if let Some(b) = self.active_branch_mut(from_node) {
                    b.status = BranchStatus::Arrived;
                    b.claimed_by = None;
                    b.claimed_at = None;
                }
            }
            CommitOutcome::JoinComplete { from_node, .. } => {
                if let Some(b) = self.active_branch_mut(from_node) {
                    b.status = BranchStatus::Arrived;
                }
                // `_join` — dokümante istisna: engine DEĞİL, DB'de adapter
                // (`wfe_adapter.rs`) aynı tx'te ekler; sim'de adapter yok, o
                // yüzden burada birebir taklit edilir (seq = son staged + 1).
                let seq = self.wfah.last().map(|e| e.seq + 1).unwrap_or(1);
                self.wfah.push(WfahEntry {
                    seq,
                    action: "_join".into(),
                    actor: Actor {
                        orgu_id: Uuid::nil(),
                        user_id: Uuid::nil(),
                        role: "system".into(),
                    },
                    input: None,
                    applied_at: chrono::Utc::now(),
                });
                // Join doldu — paralel mod biter; kollar (DB'nin aksine, audit
                // amacıyla) sim'de basitçe temizlenir. WOR-72: quorum modunda
                // geride kalan aktif kollar iptal edilmiş sayılır (marker'lar
                // engine'de staged edildi) — sim'de ayrı işaretleme gerekmez.
                self.join_target = None;
                self.join_threshold = None;
                self.join_when = None;
                self.branches.clear();
            }
            CommitOutcome::Terminal { .. }
            | CommitOutcome::Failed { .. }
            | CommitOutcome::Terminated { .. } => {
                // WFE tümden bitti — paralel modda aktif kollar iptal edilmiş
                // sayılır (`_branch_cancelled` marker'ları engine tarafından
                // zaten wfah_entries'e staged edildi); sim'de satırlar tutulmaz.
                self.join_target = None;
                self.join_threshold = None;
                self.join_when = None;
                self.branches.clear();
            }
            // WOR-56: node hedefli collapse — paralel mod biter (kardeşler iptal,
            // marker'lar engine'de staged), WFE `node`'a geçer (current_node
            // outcome_parts'ta set edilir); kol satırları sim'de temizlenir.
            CommitOutcome::CollapseTo { .. } => {
                self.join_target = None;
                self.join_threshold = None;
                self.join_when = None;
                self.branches.clear();
            }
            CommitOutcome::MoveTo { .. } => {}
        }
    }

    /// Şu an adım BEKLENEN node'lar: tekil modda `current_node`, paralel modda aktif
    /// kolların node'ları. Belge yükleme izni (`step::attach`) ve not çapası bunu sorar.
    pub fn active_nodes(&self) -> Vec<String> {
        if let Some(n) = &self.current_node {
            return vec![n.clone()];
        }
        self.branches
            .iter()
            .filter(|b| b.status == BranchStatus::Active)
            .map(|b| b.branch_node.clone())
            .collect()
    }

    /// Bu slot simülasyonda yüklü mü? Kapının (`wfe_core::v22::attachments::
    /// missing_required`) "yüklenmiş" tanımı — gerçek akıştaki `AttachmentStore::exists`
    /// karşılığı.
    pub fn has_attachment(&self, group: &str, item: &str) -> bool {
        self.attachments
            .iter()
            .any(|a| a.group == group && a.item == item)
    }

    /// Not dosyalarının şimdiye kadarki toplamı — WFE kotası (`MAX_WFE_QUOTA_BYTES`)
    /// bunun üzerine sorulur.
    pub fn note_files_total_bytes(&self) -> i64 {
        self.notes
            .iter()
            .flat_map(|n| n.files.iter())
            .map(|f| f.size_bytes)
            .sum()
    }

    fn active_branch_mut(&mut self, node: &str) -> Option<&mut BranchState> {
        self.branches
            .iter_mut()
            .find(|b| b.status == BranchStatus::Active && b.branch_node == node)
    }
}

fn outcome_parts(
    outcome: &CommitOutcome,
) -> (WfeStatus, Option<String>, Option<serde_json::Value>) {
    match outcome {
        CommitOutcome::MoveTo { node } => (WfeStatus::Active, Some(node.clone()), None),
        CommitOutcome::Terminal { end_response } => {
            (WfeStatus::Terminal, None, Some(end_response.clone()))
        }
        CommitOutcome::Failed { end_response } => {
            (WfeStatus::Error, None, Some(end_response.clone()))
        }
        CommitOutcome::Terminated { end_response } => {
            (WfeStatus::Terminated, None, Some(end_response.clone()))
        }
        // WOR-31: paralel outcome'lar aktiftir, wfe-seviyesi current_node
        // taşımaz; kol durumunun sim'e işlenmesi T4'te (SimState.branches).
        CommitOutcome::ForkTo { .. }
        | CommitOutcome::BranchMoveTo { .. }
        | CommitOutcome::BranchArrived { .. } => (WfeStatus::Active, None, None),
        CommitOutcome::JoinComplete { next, .. } => outcome_parts(next),
        // WOR-56: node hedefli collapse — paralel mod biter, WFE tekil modda `node`'a.
        CommitOutcome::CollapseTo { node, .. } => (WfeStatus::Active, Some(node.clone()), None),
    }
}

/// Bir adımın motor tarafı — `routes/simulate.rs` ve `scenario::run` ORTAK kullanır.
/// Ayrı yazılsalardı simülasyonda geçen bir senaryo koşucuda kalabilirdi.
pub mod step {
    use super::SimState;
    use wfe_core::v22::attachments as attach_rules;
    use serde_json::Value;
    use uuid::Uuid;
    use wfe_core::types::actor::Actor;
    use wfe_core::types::wfd_v22::Wfd;
    use wfe_core::types::wfe::WfeStatus;
    use wfe_core::v22::pipeline::Engine;
    use wfe_core::EngineError;

    /// WFE'nin bittiğini söyleyen tek yer — iki durum da "artık adım atılamaz".
    pub fn is_terminal(state: &SimState) -> bool {
        matches!(state.status, WfeStatus::Terminal | WfeStatus::Terminated)
    }

    /// `POST /wfe/simulate/start` gövdesi.
    pub async fn start(
        engine: &Engine<'_>,
        wfd: &Wfd,
        actor: &Actor,
        orgtnt_id: Uuid,
        action: Option<&str>,
        input: &Value,
    ) -> Result<SimState, EngineError> {
        let new = engine
            .start(wfd, actor, orgtnt_id, action, input, Uuid::new_v4(), None)
            .await?;
        Ok(SimState::from_new_wfe(&new))
    }

    /// `apply`'ın iki ayrık başarısızlığı. Belge kapısı `EngineError` DEĞİLDİR:
    /// motor dosyaya hiç değmez, kapı portal/edge katmanının kuralıdır (gerçek akışta
    /// `routes/wfe.rs` `422 attachment.missing` verir) — bu ayrım route'un aynı statüyü
    /// döndürebilmesi ve senaryo koşucusunun eksik belgeyi ADIYLA yazabilmesi için.
    #[derive(Debug)]
    pub enum ApplyError {
        /// Bu aksiyonu KAPAYAN zorunlu belgeler eksik (`"grup/slot"` listesi).
        MissingAttachments(Vec<String>),
        /// Aktör bu adım için claim-uygun değil (node'un `c_a`'sı eşleşmiyor).
        /// Gerçek akışta claim reddedilirdi (403) — simülasyon claim'i YAZMAZ ama
        /// UYGUNLUĞU atlamaz, yoksa sim'de herkes her şeyi yapabilir görünürdü.
        NotEligible,
        Engine(EngineError),
    }

    impl std::fmt::Display for ApplyError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::MissingAttachments(missing) => {
                    write!(f, "Eksik zorunlu belgeler: {}", missing.join(", "))
                }
                Self::NotEligible => write!(
                    f,
                    "aktör bu adım için yetkili değil (c_a eşleşmiyor) — gerçek akışta claim reddedilirdi"
                ),
                Self::Engine(e) => write!(f, "{e}"),
            }
        }
    }

    /// Sim claim-eşdeğeri uygunluk — gerçek `WfeExecutor::can_claim` ile AYNI kural:
    /// zaten sahipse uygun; değilse `Engine::can_claim` (matcher §7.1, delegation dahil).
    /// Sahiplik ATANMAMIŞ wfes üzerinde denetlenir (sim'in geçici pre-claim'i yetkiyi
    /// gölgelemesin diye).
    ///
    /// 2026-08-19'da route'tan (`routes/simulate.rs::sim_eligible`) BURAYA taşındı:
    /// yalnız orada yaşadığı için **senaryo koşucusu bu kapıyı hiç sormuyordu** —
    /// yetkisiz aktörle yazılmış bir senaryo yeşil geçiyor, aynı adım portalda 403
    /// alıyordu. Adım mantığının ortak olması (`sim::step`) tam bu yüzden var.
    pub async fn eligible(
        engine: &Engine<'_>,
        wfd: &Wfd,
        state: &SimState,
        actor: &Actor,
        node: Option<&str>,
    ) -> Result<bool, EngineError> {
        let wfes = state.to_wfes(None);
        let owned = match node {
            Some(n) => wfes
                .branches
                .iter()
                .any(|b| b.branch_node == n && b.claimed_by == Some(actor.user_id)),
            None => wfes.assigned_to == Some(actor.user_id),
        };
        if owned {
            return Ok(true);
        }
        Ok(matches!(
            engine.can_claim(wfd, &wfes, actor, node).await?,
            wfe_core::v22::pipeline::ClaimCheck::Ok
        ))
    }

    /// Belge kapısı — gerçek akıştaki `apply_action`/`submit_action` kapısının sim
    /// karşılığı: kapı **aksiyon bazlıdır** ve node = verilen kol ?? `current_node`.
    /// Kural `wfe_core::v22::attachments`'tan gelir (gerçek akışla TEK kaynak);
    /// buradaki tek fark "yüklenmiş" tanımıdır: depo değil `SimState.attachments`.
    pub fn missing_gate_attachments(
        wfd: &Wfd,
        state: &SimState,
        action: &str,
        node: Option<&str>,
    ) -> Vec<String> {
        let Some(node_key) = node
            .map(str::to_string)
            .or_else(|| state.current_node.clone())
        else {
            return vec![]; // paralel modda kol verilmemişse motor zaten reddeder
        };
        let slots = attach_rules::gate_slots(wfd, &node_key, Some(action));
        attach_rules::missing_required(&slots, |g, i| state.has_attachment(g, i))
    }

    /// `POST /wfe/simulate/apply` gövdesi — claim YAZILMAZ ama uygunluk
    /// çağıranın sorumluluğundadır (route `sim_eligible` ile denetler).
    ///
    /// `target`: GLB hedef seçimi — gerçek akıştaki `ApplyBody.target`ın karşılığı.
    #[allow(clippy::too_many_arguments)]
    pub async fn apply(
        engine: &Engine<'_>,
        wfd: &Wfd,
        state: &mut SimState,
        actor: &Actor,
        action: &str,
        input: &Value,
        node: Option<&str>,
        target: Option<&str>,
    ) -> Result<(), ApplyError> {
        // Sıra gerçek akışla aynı: önce YETKİ (403 karşılığı), sonra BELGE kapısı
        // (422 karşılığı), sonra motor. Yetki kapısı burada olmasaydı senaryo koşucusu
        // onu hiç sormazdı (route'ta duruyordu) ve yetkisiz senaryo yeşil geçerdi.
        if !eligible(engine, wfd, state, actor, node)
            .await
            .map_err(ApplyError::Engine)?
        {
            return Err(ApplyError::NotEligible);
        }
        let missing = missing_gate_attachments(wfd, state, action, node);
        if !missing.is_empty() {
            return Err(ApplyError::MissingAttachments(missing));
        }
        let wfes = state.to_wfes(Some(actor.user_id));
        let commit = engine
            .apply(wfd, &wfes, actor, action, input, node, target)
            .await
            .map_err(ApplyError::Engine)?;
        state.apply_commit(&commit);
        Ok(())
    }

    // ── Belge yükleme (katalog slotları) ────────────────────────────────────

    /// `attach`ın reddi. Gerçek akıştaki karşılıkları: bilinmeyen slot →
    /// `422 attachment.rejected` / `unknown_slot`; format/boyut → `415`/`413`.
    #[derive(Debug)]
    pub enum AttachError {
        /// Grup katalogda (`wfd.attachments`) yok.
        UnknownGroup(String),
        /// Grup var, slot yok.
        UnknownItem { group: String, item: String },
        /// Slot var ama ŞU AN aktif olan hiçbir node'da toplanmıyor — gerçek akışta
        /// `upload_catalog` de yalnız aktörün erişebildiği node'ların slotlarını açar.
        NotCollectedHere { group: String, item: String },
        /// Format/boyut kuralı (`AttachmentItem.formats`) reddetti.
        Rejected(attach_rules::UploadReject),
    }

    impl std::fmt::Display for AttachError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::UnknownGroup(g) => write!(f, "\"{g}\" belge grubu katalogda yok"),
                Self::UnknownItem { group, item } => {
                    write!(f, "\"{group}\" grubunda \"{item}\" dosya slotu yok")
                }
                Self::NotCollectedHere { group, item } => write!(
                    f,
                    "\"{group}/{item}\" şu an aktif olan adımda toplanmıyor"
                ),
                Self::Rejected(r) => write!(f, "{r}"),
            }
        }
    }

    /// Bir katalog slotuna dosya "yükler" — baytlar YOK, yalnız metadata. Aynı slota
    /// tekrar yükleme ÜZERİNE YAZAR (gerçek akışta yeni sürüm açılır; kapı açısından
    /// ikisi aynıdır: slot yüklüdür).
    pub fn attach(
        wfd: &Wfd,
        state: &mut SimState,
        group: &str,
        item: &str,
        filename: Option<&str>,
        content_type: Option<&str>,
        size_bytes: i64,
    ) -> Result<(), AttachError> {
        if !wfd.attachments.contains_key(group) {
            return Err(AttachError::UnknownGroup(group.into()));
        }
        let def = attach_rules::find_item(wfd, group, item).ok_or_else(|| {
            AttachError::UnknownItem {
                group: group.into(),
                item: item.into(),
            }
        })?;
        let active = state.active_nodes();
        let collected_here = active.iter().any(|n| {
            attach_rules::gate_slots(wfd, n, None)
                .iter()
                .any(|s| s.group == group && s.item == item)
        });
        if !collected_here {
            return Err(AttachError::NotCollectedHere {
                group: group.into(),
                item: item.into(),
            });
        }
        attach_rules::check_upload(def, content_type, size_bytes.max(0) as usize)
            .map_err(AttachError::Rejected)?;
        state.attachments.retain(|a| !(a.group == group && a.item == item));
        state.attachments.push(super::SimAttachment {
            group: group.into(),
            item: item.into(),
            filename: filename.map(str::to_string),
            content_type: content_type.map(str::to_string),
            size_bytes,
            node: active.first().cloned(),
        });
        Ok(())
    }

    /// Yüklenmiş sayılan bir slotu geri alır (gerçek akıştaki tekil `DELETE`).
    /// Dönüş: satır VAR MIYDI (yoksa çağıran "zaten yok" diyebilir).
    pub fn detach(state: &mut SimState, group: &str, item: &str) -> bool {
        let before = state.attachments.len();
        state
            .attachments
            .retain(|a| !(a.group == group && a.item == item));
        state.attachments.len() != before
    }

    // ── Not ekleme (ad-hoc dosyalarla) ──────────────────────────────────────

    /// Not ekler. Limitler `crate::note_rules::check_note` — gerçek akışla TEK kaynak.
    /// Not akışın gidişatını DEĞİŞTİRMEZ (K1): `$ctx`/`$wfah`'a yazılmaz, yalnız
    /// `SimState.notes`'a düşer ve senaryo bunun limitlere uygunluğunu dener.
    pub fn add_note(
        state: &mut SimState,
        actor: Option<&Actor>,
        body: &str,
        audience: crate::note_rules::Audience,
        files: Vec<crate::note_rules::NoteFileSpec>,
    ) -> Result<(), crate::note_rules::NoteReject> {
        crate::note_rules::check_note(body, &files, state.note_files_total_bytes())?;
        let node = state.active_nodes().first().cloned();
        state.notes.push(super::SimNote {
            body: body.to_string(),
            audience,
            files,
            node,
            author: actor.cloned(),
        });
        Ok(())
    }

    /// `call_return`'ün iki ayrık başarısızlığı. `EngineError`'a katlanamaz:
    /// "bekleyen çağrı yok" route'ta **409** döner ve `EngineError`'ın hiçbir
    /// varyantı bu eşlemeyi vermiyor (`Conflict` `String` değil `ConflictKind`
    /// alıyor, `CallNotFound` 404'e düşerdi) — davranışı korumak için ayrı tip.
    #[derive(Debug)]
    pub enum CallReturnError {
        NoAwaitedCall,
        Engine(EngineError),
    }

    impl std::fmt::Display for CallReturnError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::NoAwaitedCall => {
                    write!(f, "bu adımda çözülmeyi bekleyen bir iş akışı çağrısı yok")
                }
                Self::Engine(e) => write!(f, "{e}"),
            }
        }
    }

    /// `POST /wfe/simulate/call-return` gövdesi.
    pub async fn call_return(
        engine: &Engine<'_>,
        wfd: &Wfd,
        state: &mut SimState,
        status: &str,
        result: Option<&Value>,
    ) -> Result<(), CallReturnError> {
        let awaited = state
            .awaited_call()
            .cloned()
            .ok_or(CallReturnError::NoAwaitedCall)?;
        let wfes = state.to_wfes(None);
        let commit = engine
            .fire_call_return(wfd, &wfes, status, None, result, &[], chrono::Utc::now())
            .await
            .map_err(CallReturnError::Engine)?;
        state.apply_commit(&commit);
        state.clear_awaited_call(&awaited.site_key);
        Ok(())
    }
}
