//! §7 Transition Runtime Pipeline (M8 — atomik, staged diff'ler tek commit'te).
//!
//! ```text
//! 1. WFE assigned mi? Actor owner mı? Değilse ACT reddedilir.
//! 2. `actions.<x>.extra_c_a` varsa: owner bu EK kurala da match etmeli (§3).
//!    (v2.3/`Ç6`: eskiden `transitions[].c_a` idi; ad `extra_c_a` oldu, anlamı aynı —
//!    node havuzunu DARALTAN ek kısıt.)
//! 3. current_node ∈ transition.from? Değilse aday değildir.
//! 4. Adaylar array sırasıyla; when'i true olan İLK transition seçilir.
//! 5. Action input validate edilir.
//! 6. transition.wfes_effects STAGED.
//! 7. trigger[] sırayla: when → execute (timeout) → fail'de retry → catch.
//! 8. transition.wft staged DynCtx üzerinden evaluate edilir.
//! 9. COMMIT (atomik) — store'a TransitionCommit olarak devredilir.
//! ```
//!
//! Linear: WOR-36 (current_node + ilk-match), WOR-39 (WFT formları),
//! WOR-41/45 (trigger + retry/catch), WOR-42 (terminal), WOR-43 (atomik commit),
//! WOR-46 (timeout), WOR-47 (escalation).

use crate::error::EngineError;
use crate::ports::OrgPort;
use crate::types::actor::{Actor, CandidateActor as ResolvedCandidate};
use crate::types::wfah::{Wfah, WfahEntry};
use std::collections::{BTreeSet, VecDeque};
use crate::types::wfd_v22::{
    ActionDef, AutoexecDef, CaGrantRule, CallMode, CandidateActor, CuItem, EscalationStep,
    GlobalAction, JoinRule, SendBackTarget, StartAs, StartRule, TriggerInvocation, WfAdminRule,
    Wfd, Wft, WftTarget,
};
use crate::types::wfe::WfeStatus;
use crate::v22::duration::parse_iso8601_duration;
use crate::v22::effects::{apply_effects, get_path, resolve_value, EffectEnv};
use crate::v22::env::RunEnv;
use crate::v22::eval::{evaluate_bool, CallOutcome, EvalEnv, JoinEnv};
use crate::v22::grants::{matches_grant_rules, require_global_action, wf_admin_global_actions};
use crate::v22::ownership::{
    seconds_between, wait_base, ClaimAuthority, ClaimReleased, ClaimTaken, OwnershipBranch,
};
use crate::v22::matcher::{authorize, authorize_anchored, AuthDecision, MatchEnv};
use crate::v22::ports::{
    AutoexecRunner, BranchState, BranchStatus, CallSite, ClaimRecheck, CollapseCause,
    CommitOutcome, ExecEnv, ExecFailure, NewWfe, StagedCall, TransitionCommit, Wfes,
};
use crate::v22::resolver::{resolve_c_orgu, resolve_cu_ident};
use crate::v22::valid;
use crate::v22::valid::ValidRules;
use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};
use uuid::Uuid;

pub struct Engine<'a> {
    pub org: &'a dyn OrgPort,
    pub exec: &'a dyn AutoexecRunner,
    /// Koşumun ortam konfigürasyonu (`$env`). Çağıran, WFE'nin `environment_id`'sine göre
    /// çözüp verir; `Default` boş ortamdır ($env kullanmayan WFD'ler için).
    pub env: RunEnv,
}

/// Claim uygunluk sonucu — portal'a neden bilgisi taşır.
#[derive(Debug, PartialEq)]
pub enum ClaimCheck {
    Ok,
    Terminal,
    /// SLA-3 deadline geçmiş ama sweeper henüz `terminated`'a taşımadı (2026-07-16 fix).
    Expired,
    AlreadyClaimed,
    NotEligible,
    /// WFC: WFE bir ÇAĞRI NODE'unda bekliyor. Buradan insan aksiyonu alınamaz
    /// (`call_node_has_action`), dolayısıyla claim de anlamsızdır: iş kimseye
    /// atanamaz, çağrılan bitince akış kendi ilerler. Node'un `c_a`'sı yalnız
    /// GÖRÜNÜRLÜK verir (bkz. runtime-semantics §10b).
    CallInProgress,
}

/// Node'un ilk ateşlenmemiş escalation adımı için giriş/vade bilgisi.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EscalationForecast {
    pub step_idx: usize,
    pub entered_at: DateTime<Utc>,
    pub deadline: DateTime<Utc>,
    pub overdue: bool,
}

/// T‑A5: WF Admin atlamasının sonucu. `entry` store tarafından WFAH'a eklenir
/// (`reassign`'ın deseni: core kaydı üretir, I/O çağırana ait).
#[derive(Debug, Clone)]
pub struct EscalationSkip {
    pub step_idx: usize,
    pub node: String,
    /// Yazılan WFAH aksiyon adı — istemciye neye dokunulduğunu söylemek için.
    pub marker: String,
    pub entry: WfahEntry,
}

/// `fire_claim_timeout` sonucu — `wft` verilmişse node taşıması
/// (`TransitionCommit` — normal `commit()` yolu), verilmemişse yalnızca
/// claimed_by/claimed_at temizliği (`release_claim` yolu, node DEĞİŞMEZ).
#[derive(Debug, Clone)]
pub enum ClaimTimeoutOutcome {
    Move(TransitionCommit),
    Release(ClaimRelease),
}

/// `ClaimTimeoutOutcome::Release` gövdesi — node/status DEĞİŞMEZ, yalnız
/// assignment temizlenir. 2026-07-28: SLA-1 `wfes_effects` taşıyorsa yeni DynCtx
/// de bu yolla persist edilir (`new_dynctx = None` → effects yok, ctx'e dokunulmaz).
#[derive(Debug, Clone)]
pub struct ClaimRelease {
    pub wfah_entry: WfahEntry,
    pub new_dynctx: Option<Value>,
}

/// `Engine::possible_actions` öğesi: uygulanabilir bir aksiyon + (geri gönderme ise)
/// o aksiyonun seçilebilir hedefleri.
///
/// Çekirdek burada ANAHTAR taşır, gösterim ÜRETMEZ: `SendBackChoice::label` belgedeki
/// HAM metindir (yoksa `None`), nihai etiketi tek bir yer (`v22::display`) çözer ve
/// dış görünüme (`Ref`) adapter katmanında çevirir.
#[derive(Debug, Clone, PartialEq)]
pub struct ActionChoice {
    pub action: String,
    /// `Wft::SendBack` transition'ında seçilebilir hedefler (belgedeki SIRAYLA);
    /// düz aksiyonda `None` — "hedef seçimi yok" ile "hedef listesi boş" ayrımı
    /// korunsun diye `Option`, boş `Vec` değil.
    pub targets: Option<Vec<SendBackChoice>>,
}

/// `ActionChoice::targets` öğesi: hedef node anahtarı + belgedeki ham etiket.
#[derive(Debug, Clone, PartialEq)]
pub struct SendBackChoice {
    pub node: String,
    /// `SendBackTarget::label` AYNEN — çözüm (`node_label`'a düşme) adapter'ın işi.
    pub label: Option<String>,
}

/// `Terminal` ve `Terminated` her ikisi de "aktif değil" sınıfıdır: yeni
/// aksiyon/claim/escalation kabul etmezler (2026-07-16 SLA sözleşmesi).
fn is_terminal_class(status: &WfeStatus) -> bool {
    matches!(status, WfeStatus::Terminal | WfeStatus::Terminated)
}

/// Varılan site bir TERMİNAL ise id'si (2026-08-17) — `wf.wfe.end_terminal` kolonuna
/// ve terminal `listable[]` projeksiyonuna giden değer.
///
/// Kaynak neden `CommitOutcome` değil `CallSite`: `CommitOutcome::Terminal` yalnız
/// `end_response` taşır ve taşıdığını genişletmek bu enum'un `PartialEq`'ine dayanan
/// onlarca testi, hedefle ilgisi olmayan bir alan yüzünden değiştirmek demekti.
/// `CallSite` "nereye varıldı" sorusunun ZATEN var olan cevabıdır (WFC outbox'ı ardıl
/// çağrıyı onunla buluyor) — ikinci bir kayıt tutmak yerine aynı cevap okunur.
///
/// `Some` dönmesi başarılı `Terminal` demektir: `Failed`/`Terminated` yollarında bir
/// site hiç üretilmez, `MoveTo`/fork/kol yollarında ise site NODE'dur.
fn end_terminal_of(landed: Option<&CallSite>) -> Option<String> {
    match landed {
        Some(CallSite::Terminal(id)) => Some(id.clone()),
        _ => None,
    }
}

/// **K-2 — bu WFE'nin GERÇEKTEN uğradığı node kümesi.** Geri gönderme menüsünün
/// çalışma anı süzgeci; saf, store'suz, sim ile gerçek akış için AYNI.
///
/// Belgedeki `wft.targets` STATİKTİR: tasarımcı "buraya geri gönderilebilir" der, ama
/// o node'a BU örnekte uğranmış olması gerekmez (koşullu dal seçilmedi, adım atlandı).
/// Uğranmamış bir node'a "geri" göndermek geri gönderme DEĞİL ileri atlamadır: akış hiç
/// görmediği bir adıma düşer, o adımın beklediği ctx alanları hiç yazılmamıştır ve
/// oradaki `when` koşulları boş geçmişle değerlendirilir.
///
/// Küme ÜÇ kaynağın birleşimidir:
///
/// 1. `wfes.visited_nodes` — WFAH akış izinden (`wf.wfah.from_node`/`to_node`) gelen
///    gövde. Escalation/claim_timeout ile taşınan node'lar DAHİLDİR: WFE orada bekledi.
/// 2. **Start node'u** — WFAH'ın İLK kaydının aksiyonunu taşıyan `start[]` kurallarının
///    `from`u. Start satırında `from_node` NULL'dır (K7: "öncesi yok") ve `to_node` ilk
///    havuzdur, yani start node'u (1)'e HİÇ girmez — oysa "başa gönder" tam olarak
///    oraya gönderir. Aynı aksiyonu iki start kuralı paylaşıyorsa İKİSİ de kümeye
///    girer: hangisinin ateşlendiği WFAH'ta yazılı değildir ve ikisi de meşru bir
///    "hazırlayan havuzu"dur — fazla daraltmak "başa gönder"i sessizce yok ederdi.
/// 3. Şu anki duruş — `current_node` + iptal OLMAYAN kol node'ları. (1) bunu zaten
///    içerir; ikinci kaynak olarak durması `visited_nodes` doldurulmamış bir çağıranda
///    menünün BOŞ kalmasını değil, en azından bulunulan yeri döndürmesini sağlar.
///
/// SINIR: iptal olmuş paralel KARDEŞ kolun node'ları (1)'de kalır — tasarım zamanı
/// kuralı (editör SB-P/SB-R) o hedefleri zaten yasaklar; bu kesişim EK bir daraltmadır,
/// onun yerine geçmez.
pub fn visited_nodes<'w>(wfd: &'w Wfd, wfes: &'w Wfes) -> BTreeSet<&'w str> {
    let mut out: BTreeSet<&str> = wfes.visited_nodes.iter().map(String::as_str).collect();
    if let Some(first) = wfes.wfah.entries().first() {
        // v2.3: "başlatan node" = start aksiyonunun `from`u (`Ç7+Ç8`).
        for rule in &wfd.start {
            if rule.action == first.action {
                if let Some(a) = crate::types::wfd_v22::start_action(wfd, rule) {
                    out.insert(a.from.as_str());
                }
            }
        }
    }
    if let Some(node) = wfes.current_node.as_deref() {
        out.insert(node);
    }
    for b in &wfes.branches {
        if b.status != BranchStatus::Cancelled {
            out.insert(b.branch_node.as_str());
        }
    }
    out
}

/// R02: **SLA-2'nin ve sahiplik hesabının TEK node giriş tanımı** — "bu iş bu adıma
/// ne zaman geldi".
///
/// Cevap: WFAH'ın **`to_node != null` olan SON satırının** `applied_at`'i, yani
/// akışı gerçekten bir node'a TAŞIYAN son satır. Marker satırları (`escalate:`,
/// `trigger:`, `claim_released:`, `claim_taken:`, `_branch_*`,
/// `_collapse`, `_join`, `call:`) `to_node` taşımaz (Ç2) ve tabanı kaydırmaz.
///
/// Soru bir ad öneki listesiyle SORULMAZ: eski hâl `!action.starts_with("escalate:")`
/// filtresiydi ve escalation DIŞINDAKİ her marker türü tabanı ileri atıyordu; yeni bir
/// marker türü eklendiğinde sessizce kayan bir tasarımdı (R02, reddedilen Seçenek B).
/// **Koda hiçbir marker adı girmez.**
///
/// `to_node != null` satır YOKSA `None` — migration öncesi satırlar için **yedek yol
/// KURULMAZ** (R02/S2; NULL'ların ne olacağı R01'in işi).
///
/// R04'ün `$prev` tanımıyla (son AKSİYON satırı) **AYNI ŞEY DEĞİL**: `_fork` bir aksiyon
/// satırıdır ama tek bir node'a taşımadığı için `to_node` taşımaz — `$prev` onu görür,
/// bu taban görmez. İki sinyal bilinçli olarak ayrı.
///
/// KOL MODU BURADA YOK (R02/S3): paralel modda kol girişi `BranchState.entered_at`
/// gerçek DB kolonundan okunur ve bu türetimle **BİRLEŞTİRİLMEZ**. Çağıran hangi modda
/// olduğunu bilir.
pub fn node_entered_at(wfah: &Wfah) -> Option<DateTime<Utc>> {
    wfah.entries()
        .iter()
        .rfind(|e| e.to_node.is_some())
        .map(|e| e.applied_at)
}

/// Bir geri gönderme menüsünün BU örnekte gerçekten seçilebilir hedefleri: belgedeki
/// sıra KORUNUR (tasarımcının yazdığı sıra ekranda anlam taşır), yalnız uğranmamış
/// olanlar düşer.
fn offered_targets<'w>(
    targets: &'w [SendBackTarget],
    visited: &BTreeSet<&str>,
) -> Vec<&'w SendBackTarget> {
    targets
        .iter()
        .filter(|t| visited.contains(t.node.as_str()))
        .collect()
}

/// Geri gönderme hedef seçimini transition'ın wft'sine UYGULAR.
///
/// `Wft::SendBack` çalıştırılabilir bir hedef DEĞİLDİR — bir MENÜDÜR. Seçim
/// yapıldıktan sonra kalan yol normal `Wft::Node` yoludur (MoveTo), yani hedef
/// seçimi runtime'a yeni bir geçiş türü sokmaz: yalnız hangi node'a gidileceğini
/// belirler. Bu yüzden burada `Cow` ile TEK bir noktada çözülür ve `resolve_wft`
/// menüden habersiz kalır.
///
/// Simetri bilinçlidir: menü varsa seçim ZORUNLU, menü yoksa seçim YASAK. İkincisi
/// sessizce yok sayılsaydı istemcinin yanlış transition'ı hedeflediği gizlenirdi.
fn select_wft<'w>(
    wft: &'w Wft,
    target: Option<&str>,
    visited: &BTreeSet<&str>,
) -> Result<std::borrow::Cow<'w, Wft>, EngineError> {
    match wft {
        Wft::SendBack { targets } => {
            let chosen = target.ok_or(EngineError::TargetRequired)?;
            // K-2: menüde OLMAK yetmez, o node'a UĞRANMIŞ olmak da gerekir. İki kapı
            // AYNI kümeye bakar (`offered_targets`) — `possible_actions`ın sunmadığı bir
            // hedefi apply kabul ederse istemci menüyü atlayıp ileri atlayabilirdi.
            // Ayrı hata kodu YOK: istemci için "bu hedef bu işte geçerli değil" tek
            // durumdur ve `action.target_invalid` zaten onu söylüyor.
            if !offered_targets(targets, visited)
                .iter()
                .any(|t| t.node == chosen)
            {
                return Err(EngineError::TargetInvalid(chosen.to_string()));
            }
            Ok(std::borrow::Cow::Owned(Wft::Node {
                node: chosen.to_string(),
            }))
        }
        other => {
            if target.is_some() {
                return Err(EngineError::TargetUnexpected);
            }
            Ok(std::borrow::Cow::Borrowed(other))
        }
    }
}

/// **Kapı B** — bir commit'in `$ctx`'e YAZDIĞI değerlerin tip denetimi (2026-08-19).
///
/// Motor bilir kişidir: bağlama yazılan değer context şemasına uymuyorsa geçiş
/// UYGULANMAZ. Kapı A (`validate_action_input`) yalnız İSTEK girdisini görür; buradaki
/// kaynaklar farklıdır ve hepsi `wfes_effects` üzerinden geçer:
///   · autoexec sonucu (`$exec.result.*`) — dış sistem ne döndürdüyse,
///   · WFC dönüşü (`$call.result.*`) — çağrılan akışın `wfe_end_response`'u,
///   · `$env` değeri (ortam konfigürasyonu),
///   · sistem/sabit yazımlar (bunlar tasarım zamanında da denetli: `effect_type_mismatch`).
///
/// YALNIZ DEĞİŞEN kök alanlar denetlenir (`ctx_types::validate_written`): enforcement'tan
/// önce bozulmuş eski veri bu geçişi durdurmaz — o ayrı bir kapının işidir.
///
/// Ölçüm (2026-08-19, `ctx_type_report`, test DB): 25 WFE / 0 ihlal → kapı doğrudan
/// REDDEDEREK açıldı (önce `warn` fazı gerekmedi).
fn guard_written_ctx(wfd: &Wfd, before: &Value, after: &Value) -> Result<(), EngineError> {
    let violations = crate::v22::ctx_types::validate_written(&wfd.context, before, after);
    if violations.is_empty() {
        Ok(())
    } else {
        Err(EngineError::CtxTypeMismatch(violations))
    }
}

impl<'a> Engine<'a> {
    // ---------------------------------------------------------------- start

    /// Yeni WFE başlatır. `wfe_id` çağıran tarafından üretilir ve effects
    /// GERÇEK id ile çözülür (WOR-6).
    ///
    /// `action`: M16 sonrası start aksiyonları gerçek ad taşır; verilirse yalnız o
    /// action adını taşıyan start kuralları aday olur (spec runtime resolution —
    /// "actor, start[].action ile adlandırılmış aksiyonu çağırır"). `None` = tüm
    /// start kuralları aday (tek start aksiyonlu WFD'ler ve eski istemciler).
    /// `deadline`: SLA-3 — başlatan kullanıcının opsiyonel ISO 8601 duration'ı
    /// (start anından itibaren). `wfd.timeout` tanımlıysa `deadline ≤ timeout`
    /// olmalı (aksi InvalidInput); resolved mutlak deadline `NewWfe.deadline`'a
    /// yazılır (2026-07-16 SLA sözleşmesi).
    #[allow(clippy::too_many_arguments)]
    pub async fn start(
        &self,
        wfd: &Wfd,
        actor: &Actor,
        orgtnt_id: Uuid,
        action: Option<&str>,
        input: &Value,
        wfe_id: Uuid,
        deadline: Option<&str>,
    ) -> Result<NewWfe, EngineError> {
        let empty_ctx = json!({});
        let empty_wfah = Wfah::empty();

        // §7.5 simetrisi: start input'u transition input'ları gibi doğrulanır —
        // `action.input.required` mevcut olmalı, bildirilmemiş yol reddedilir; başlangıç
        // ctx'i YALNIZ bildirilen yollardan + effects'ten tohumlanır (serbest-form
        // context enjeksiyonu kapalı). Normalizasyon SEÇİMDEN ÖNCE yapılır çünkü
        // `validate_action_input` artık seçim döngüsünün İÇİNDE koşuyor.
        let input_norm = match input {
            Value::Object(_) => input.clone(),
            Value::Null => json!({}),
            _ => return Err(EngineError::InvalidInput("start input obje olmalı".into())),
        };
        let input = &input_norm;

        // Aktörün başlatabildiği ilk kural (istenirse action adına daraltılmış).
        //
        // v2.3 (`Ç7+Ç8` + `E11`) — **SEÇİM SIRASI BAĞLAYICI: yetki → input → when.**
        // Bugüne kadar `Engine::start` `when`e HİÇ BAKMIYORDU ve
        // `validate_action_input` seçimden SONRA koşuyordu. Yeni sıranın gerekçesi:
        //   1. yetki  — `nodes[action.from].c_a` (+ varsa `extra_c_a`, AND'lenir)
        //   2. input  — `when`den ÖNCE, ki `$action.input.*` DOĞRULANMAMIŞ girdi
        //               üzerinden okunmasın. Bugünkü SERT reddi aynen korunur (`?`):
        //               yetkili bir kuralın girdisi bozuksa sıradakine düşülmez.
        //   3. when   — false → SIRADAKİ start kuralına geç.
        // Hiçbiri tutmazsa `StartNotEligible` (yeni hata türü AÇILMAZ).
        let mut selected: Option<(&StartRule, &ActionDef)> = None;
        for r in &wfd.start {
            if let Some(a) = action {
                if r.action != a {
                    continue;
                }
            }
            let Some(action_def) = wfd.actions.get(&r.action) else {
                // Bozuk belge: validator `start_action` bunu yakalar. Runtime'da
                // sıradaki kurala geçilir — tek kural buysa `StartNotEligible` döner.
                continue;
            };
            // Simetrik start: initiator yetkisi start aksiyonunun `from` node'unun
            // `c_a`'sında yaşar (`Ç7+Ç8`: `start[].from` silindi, aynı gerçek iki
            // yerde tutulmaz).
            let node = wfd.nodes.get(&action_def.from).ok_or_else(|| {
                EngineError::InvalidWfd(format!(
                    "actions.{}.from bilinmeyen node: '{}'",
                    r.action, action_def.from
                ))
            })?;
            let env = MatchEnv {
                ctx: &empty_ctx,
                wfah: &empty_wfah,
                orgtnt_id,
            };
            if !authorize(&node.c_a, actor, env, self.org).await? {
                continue;
            }
            // `extra_c_a` havuzu DARALTIR (`Ç6`) — start yolunda da AND'lenir.
            if let Some(extra_rule) = &action_def.extra_c_a {
                let env = MatchEnv {
                    ctx: &empty_ctx,
                    wfah: &empty_wfah,
                    orgtnt_id,
                };
                if !authorize_anchored(extra_rule, actor, None, env, self.org).await? {
                    continue;
                }
            }
            validate_action_input(action_def, input, &wfd.context)?;
            if let Some(expr) = &action_def.when {
                let env = EvalEnv::new(&empty_ctx)
                    .with_wfah(&empty_wfah, &ValidRules::for_version(wfd))
                    .with_actor(actor)
                    .with_wfe_id(wfe_id)
                    .with_action_input(input);
                if !evaluate_bool(expr, &env)? {
                    continue;
                }
            }
            selected = Some((r, action_def));
            break;
        }
        let (rule, action_def) = selected.ok_or(EngineError::StartNotEligible)?;
        let mut staged = json!({});

        let now = Utc::now();
        let mut wfah_entries: Vec<WfahEntry> = Vec::new();
        let mut seq = 1u32;

        // SLA-3: efektif deadline çözümü — deadline verildi → now+parse(deadline)
        // (wfd.timeout tavanına tabi DEĞİL, çağıran serbestçe uzatabilir);
        // verilmedi ve wfd.timeout var → now+parse(timeout); ikisi de yok → NULL.
        // Çağıran girdisindeki parse hatası InvalidInput'tur (InvalidWfd değil —
        // kusur WFD'de değil, istekte) ve beklenen biçimi tarif etmelidir.
        let parse_caller_deadline = |d: &str| {
            parse_iso8601_duration(d).map_err(|_| {
                EngineError::InvalidInput(format!(
                    "deadline '{d}' geçersiz — ISO 8601 süre bekleniyor: PT30M (30 dakika), PT2H (2 saat), P1D (1 gün), P1DT12H (1 gün 12 saat)"
                ))
            })
        };
        let resolved_deadline: Option<DateTime<Utc>> = match (deadline, &wfd.timeout) {
            (Some(d), _) => Some(now + parse_caller_deadline(d)?),
            (None, Some(t)) => Some(now + parse_iso8601_duration(t)?),
            (None, None) => None,
        };

        if let Some(effects) = &action_def.wfes_effects {
            let env = EffectEnv {
                env: self.env.public(),
                call: None,
                actor,
                wfe_id,
                node: None,
                action_input: Some(input),
                exec_result: None,
                now,
            };
            staged = apply_effects(&staged, effects, &env)?;
        }

        // Ç2: start'ın HAREKET satırı. `from_node` YOK (öncesi yok), `to_node` outcome
        // çözüldükten sonra `stamp_movement` ile yazılır. Start'ta fork YASAK →
        // `branch_entry` daima `None` (satır bir kolda değil).
        let action_row = wfah_entries.len();
        wfah_entries.push(WfahEntry {
            seq,
            // Transition'lar gibi düz action adını yazar (rule.id DEĞİL). M16: start
            // aksiyonu gerçek adını taşır; `c_orgu` WFAH anchor'ları da aynı gerçek
            // adı referans alır (from.wfah = "<start action adı>").
            action: rule.action.clone(),
            actor: actor.clone(),
            input: Some(input.clone()),
            applied_at: now,
            from_node: None,
            to_node: None,
            branch_entry: None,
            branch_round: None,
        });
        seq += 1;

        self.run_triggers(
            &action_def.trigger,
            wfd,
            &mut staged,
            &mut wfah_entries,
            &mut seq,
            actor,
            wfe_id,
            None,
            // Ç4: start'ta paralel mod yok.
            None,
            Some(input),
            &empty_wfah,
            orgtnt_id,
        )
        .await?;

        // §7.8 — nereye gidiyoruz?
        let (outcome, final_ctx, landed) = self
            .resolve_wft(
                &action_def.wft,
                wfd,
                staged,
                &empty_wfah,
                actor,
                wfe_id,
                Some(input),
                None,
                WftMode::Start,
                // Start'ta geri gönderme menüsü YASAKTIR (`send_back_wft_placement`).
                false,
            )
            .await?;

        stamp_movement(&mut wfah_entries[action_row], &outcome, None);

        // Aksiyon işlendi: `start[].action` kaydı artık defterde (M16 — çapalar bu
        // gerçek adı referans alır). Adaylar bu defterle çözülür.
        let wfah = empty_wfah.extended(&wfah_entries);
        let resolved_c_a = self
            .candidates_at(
                &outcome,
                landed.as_ref(),
                wfd,
                &final_ctx,
                &wfah,
                // Start: WFE henüz yok → çapa başlatanın birimi (= origin_orgu_id).
                actor.orgu_id,
                orgtnt_id,
            )
            .await?;

        // WOR-70: `context.required` KALDIRILDI. Zorunluluk artık iki tasarım-zamanı
        // kuralıyla sağlanır (validator): (1) her declared input bir wfes_effects
        // tarafından tüketilmek zorunda, (2) her context alanı en az bir wfes_effects
        // tarafından yazılmak zorunda. Çalışma anında ayrı bir ctx doluluk denetimi yok.
        let staged_calls =
            self.stage_calls(wfd, landed.as_ref(), &final_ctx, actor, wfe_id, now)?;
        // Kapı B: start'ta ctx SIFIRDAN kurulur, dolayısıyla tüm alanlar "yazılmış"tır.
        guard_written_ctx(wfd, &json!({}), &final_ctx)?;

        Ok(NewWfe {
            wfe_id,
            orgtnt_id,
            wfd_id: parse_wfd_uuid(wfd)?,
            wfd_version: 0, // store katmanı gerçek versiyon satırını bilir; executor doldurur
            // Ortam kimliği de executor'ın işi: çekirdek `RunEnv`'in DEĞERLERİNİ görür,
            // hangi satırdan geldiğini değil (I/O yok).
            environment_id: None,
            initial_dynctx: final_ctx,
            wfah_entries,
            outcome,
            resolved_c_a,
            deadline: resolved_deadline,
            staged_calls,
            // Görünürlük projeksiyonu saf pipeline'da BOŞ bırakılır: org portuna ve
            // WFE'nin çapasına ihtiyaç duyar, `WfeExecutor::fill_view_grants` doldurur.
            view_c_a: Vec::new(),
            current_view_c_a: Vec::new(),
            branch_c_a: Vec::new(),
            branch_view_c_a: Vec::new(),
            end_view_c_a: Vec::new(),
            end_terminal: end_terminal_of(landed.as_ref()),
            // Görünürlük çapası: akışı BAŞLATAN aktörün birimi. Start'ın kendisi
            // zaten bu aktörle çözüm yapıyor; WFE ömrü boyunca sabit kalacak olan
            // değer burada donar.
            origin_orgu_id: actor.orgu_id,
            // Çağıran bağı store/executor katmanında doldurulur: `Engine::start` saf bir
            // hesaptır ve `wf.wfe_call` satırının id'sini bilmez.
            caller: None,
        })
    }

    // ---------------------------------------------------------------- apply

    /// `node_hint`: WOR-31 — paralel modda aksiyon birden fazla aktif kolun
    /// transition'ıyla eşleşebilir; çağıran kol node'unu vererek belirsizliği
    /// çözer. Paralel mod dışında `None` eski davranıştır; verilirse
    /// current_node ile örtüşmek zorundadır.
    ///
    /// `target`: geri gönderme (`wft: {targets}`) hedef seçimi — hedefi belge değil, aksiyonu
    /// ALAN KİŞİ seçer. `Targets` transition'ında ZORUNLU, diğerlerinde YASAK
    /// (bkz. `select_wft`). Seçim ctx'e YAZILMAZ ve `$wfah` izdüşümüne girmez:
    /// nereye gidildiği zaten geçişin kendisinde (`to_node`) görünür.
    #[allow(clippy::too_many_arguments)]
    pub async fn apply(
        &self,
        wfd: &Wfd,
        wfes: &Wfes,
        actor: &Actor,
        action: &str,
        input: &Value,
        node_hint: Option<&str>,
        target: Option<&str>,
    ) -> Result<TransitionCommit, EngineError> {
        if is_terminal_class(&wfes.status) {
            return Err(EngineError::WfeTerminal);
        }
        // SLA-3: deadline geçtiyse sweeper (60s tick) henüz `terminated`'a taşımamış
        // olsa bile aksiyon reddedilir — request-time re-check (2026-07-16 fix,
        // bkz. can_claim'deki eşdeğer kapı).
        if self.deadline_due(wfes, Utc::now()) {
            return Err(EngineError::WfeExpired);
        }
        // WOR-31: paralel mod — adaylar tek current_node yerine TÜM aktif kol
        // node'ları üzerinden aranır (kol-bazlı assignment kontrolüyle).
        if wfes.join_target.is_some() {
            return self
                .apply_parallel(wfd, wfes, actor, action, input, node_hint, target)
                .await;
        }
        let current_node = wfes
            .current_node
            .as_deref()
            .ok_or_else(|| EngineError::InvalidWfd("aktif WFE'nin current_node'u yok".into()))?;
        if let Some(hint) = node_hint {
            if hint != current_node {
                return Err(EngineError::InvalidInput(format!(
                    "node '{hint}' bu WFE'nin aktif node'u değil ('{current_node}')"
                )));
            }
        }

        // §7.1 — assignment / owner kontrolü
        match wfes.assigned_to {
            None => return Err(EngineError::NotClaimed),
            Some(owner) if owner != actor.user_id => return Err(EngineError::NotOwner),
            _ => {}
        }

        // §7.3–7.4 — aksiyon kaydı + `when` kapısı
        //
        // v2.3 (`Ç5` + `Ç10`): **İLK-MATCH SEMANTİĞİ ÖLDÜ.** Eskiden aynı
        // `(node, action)` çifti için birden çok `transitions[]` girdisi olabiliyor,
        // motor da dizi sırasında ilk `when`i tutanı seçiyordu — `Ç10` "sıra artık
        // seçim yapmaz" dedi. Artık kimlik map anahtarıdır: aday YA TEKTİR ya da yoktur.
        // `when` false dönerse aksiyon o an ALINAMAZ (ikinci bir adaya düşülmez).
        let ctx = wfes.dynctx.as_value().clone();
        let selected = wfd.actions.get(action).filter(|t| t.from == *current_node);
        let transition =
            selected.ok_or_else(|| EngineError::TransitionNotFound(action.to_string()))?;
        if let Some(expr) = &transition.when {
            let env = EvalEnv::new(&ctx)
                .with_wfah(&wfes.wfah, &ValidRules::for_version(wfd))
                .with_node(Some(current_node))
                .with_actor(actor)
                .with_wfe_id(wfes.wfe_id)
                .with_action_input(input);
            if !evaluate_bool(expr, &env)? {
                return Err(EngineError::TransitionNotFound(action.to_string()));
            }
        }

        // §7.2 — ek yetki kısıtı
        if let Some(extra_rule) = &transition.extra_c_a {
            let env = MatchEnv {
                ctx: &ctx,
                wfah: &wfes.wfah,
                orgtnt_id: wfes.orgtnt_id,
            };
            // Çapa WFE'nin kendi birimi (2026-08-13): `self` gibi bir selector
            // aksiyon kapısında da "işin ait olduğu birim" demektir. Eskiden
            // çapa SORAN KİŞİYDİ, yani `self` karşılaştırmayı kendisiyle yapıp
            // daima geçiyordu — başka şubedeki aynı roldeki kişi işi havuzunda
            // GÖRMEZ ama id'yi bilse aksiyon ALABİLİYORDU. Görünürlük ile yetki
            // artık aynı çapayı kullanır.
            if !authorize_anchored(extra_rule, actor, wfes.origin_orgu_id, env, self.org).await? {
                return Err(EngineError::PermissionDenied(action.to_string()));
            }
        }

        // §7.5 — input sözleşme denetimi (ctx'e yazım YOK — yalnız wfes_effects yazar)
        let action_def = wfd
            .actions
            .get(action)
            .ok_or_else(|| EngineError::InvalidWfd(format!("action '{action}' tanımsız")))?;
        validate_action_input(action_def, input, &wfd.context)?;
        // Geri gönderme hedef seçimi — effects STAGE EDİLMEDEN önce doğrulanır: reddedilecek
        // bir aksiyon için hiçbir hesap yapılmasın.
        let wft = select_wft(&transition.wft, target, &visited_nodes(wfd, wfes))?;
        let mut staged = ctx.clone();

        let now = Utc::now();
        let mut seq = wfes.wfah.entries().last().map(|e| e.seq + 1).unwrap_or(1);
        let mut wfah_entries: Vec<WfahEntry> = Vec::new();

        // §7.6 — transition effects STAGED
        if let Some(effects) = &transition.wfes_effects {
            let env = EffectEnv {
                env: self.env.public(),
                call: None,
                actor,
                wfe_id: wfes.wfe_id,
                node: Some(current_node),
                action_input: Some(input),
                exec_result: None,
                now,
            };
            staged = apply_effects(&staged, effects, &env)?;
        }

        // Ç2: bu geçişin HAREKET satırı — from/to outcome çözülünce yazılır
        // (`stamp_movement`). Tek-kol yol → `branch_entry: None`.
        let action_row = wfah_entries.len();
        wfah_entries.push(WfahEntry {
            seq,
            action: action.to_string(),
            actor: actor.clone(),
            input: Some(input.clone()),
            applied_at: now,
            from_node: None,
            to_node: None,
            branch_entry: None,
            branch_round: None,
        });
        seq += 1;

        // §7.7 — trigger'lar
        self.run_triggers(
            &transition.trigger,
            wfd,
            &mut staged,
            &mut wfah_entries,
            &mut seq,
            actor,
            wfes.wfe_id,
            Some(current_node),
            None,
            Some(input),
            &wfes.wfah,
            wfes.orgtnt_id,
        )
        .await?;

        // §7.8 — wft staged ctx üzerinden: nereye gidiyoruz?
        let (outcome, final_ctx, landed) = self
            .resolve_wft(
                &wft,
                wfd,
                staged,
                &wfes.wfah,
                actor,
                wfes.wfe_id,
                Some(input),
                None,
                WftMode::Single,
                // Tekil modda fork alt-grafı diye bir şey yok — test hiç sorulmaz.
                false,
            )
            .await?;

        stamp_movement(&mut wfah_entries[action_row], &outcome, Some(current_node));

        // Aksiyon işlendi — varılan yeri kim yapabilir?
        let wfah = wfes.wfah.extended(&wfah_entries);
        let resolved_c_a = self
            .candidates_at(
                &outcome,
                landed.as_ref(),
                wfd,
                &final_ctx,
                &wfah,
                // Çapa WFE'nin kendi birimi; işlemi yapan kişiyle DEĞİŞMEZ.
                wfes.origin_orgu_id.unwrap_or(actor.orgu_id),
                wfes.orgtnt_id,
            )
            .await?;

        // WOR-31: wft Parallel'e çözüldüyse `_fork` marker'ı engine tarafından staged.
        stage_parallel_markers(
            wfes,
            &Trigger {
                // Tekil mod: bu yol yalnız `_fork` yazabilir (paralel mod BİTMEZ),
                // `_fork` de tetikleyici alanı taşımaz — `kind` hiç serileşmez.
                kind: TriggerKind::System,
                branch: None,
                action: Some(action),
                actor,
            },
            &outcome,
            &mut wfah_entries,
            &mut seq,
            now,
        );

        // WFC: varılan site bir çağrı taşıyorsa outbox satırı AYNI tx'te stage edilir.
        let staged_calls =
            self.stage_calls(wfd, landed.as_ref(), &final_ctx, actor, wfes.wfe_id, now)?;
        guard_written_ctx(wfd, wfes.dynctx.as_value(), &final_ctx)?;

        let claim_recheck = self
            .stage_claim_recheck(wfd, wfes, &outcome, &wfah_entries, &final_ctx, None, now)
            .await?;
        Ok(TransitionCommit {
            claim_recheck,
            wfe_id: wfes.wfe_id,
            orgtnt_id: wfes.orgtnt_id,
            new_dynctx: final_ctx,
            wfah_entries,
            outcome,
            resolved_c_a,
            staged_calls,
            // Görünürlük projeksiyonu saf pipeline'da BOŞ bırakılır: org portuna ve
            // WFE'nin çapasına ihtiyaç duyar, `WfeExecutor::fill_view_grants` doldurur.
            view_c_a: Vec::new(),
            current_view_c_a: Vec::new(),
            branch_c_a: Vec::new(),
            branch_view_c_a: Vec::new(),
            end_view_c_a: Vec::new(),
            end_terminal: end_terminal_of(landed.as_ref()),
        })
    }

    // ------------------------------------------------------- apply (parallel)

    /// WOR-31 — paralel modda apply: aday transitions TÜM aktif kol node'ları
    /// üzerinden aranır; her kol için array sırasında ilk when-match geçerlidir.
    /// Aksiyon ≥2 farklı kolun transition'ıyla eşleşir ve `node_hint` verilmemişse
    /// `AmbiguousAction` (kol subgraph'ları ayrık olduğundan tek kol eşleşmesi
    /// kesin sahiplik verir). Assignment/owner kontrolü KOL-bazlıdır.
    #[allow(clippy::too_many_arguments)]
    async fn apply_parallel(
        &self,
        wfd: &Wfd,
        wfes: &Wfes,
        actor: &Actor,
        action: &str,
        input: &Value,
        node_hint: Option<&str>,
        target: Option<&str>,
    ) -> Result<TransitionCommit, EngineError> {
        let join = wfes
            .join_target
            .as_ref()
            .expect("apply_parallel yalnız paralel modda çağrılır");
        let active: Vec<&BranchState> = wfes
            .branches
            .iter()
            .filter(|b| b.status == BranchStatus::Active)
            .collect();
        if let Some(hint) = node_hint {
            if !active.iter().any(|b| b.branch_node == hint) {
                return Err(EngineError::InvalidInput(format!(
                    "node '{hint}' aktif bir paralel kol değil"
                )));
            }
        }

        // §7.3–7.4 kol-bazlı aday seçimi
        let ctx = wfes.dynctx.as_value().clone();
        let mut matched: Vec<(&BranchState, &ActionDef)> = Vec::new();
        for b in active.iter().copied() {
            if node_hint.is_some_and(|h| h != b.branch_node) {
                continue;
            }
            // v2.3: tek-kol yolundaki ile AYNI mantık — aday tek bir aksiyon kaydıdır,
            // ilk-match döngüsü yok (`Ç10`).
            //
            // ⚠️ Bunun `AmbiguousAction`a maliyeti var ve ÖLÇÜLDÜ: `actions.get(action)`
            // TEK kayıt döndürür ve `from` tekil string olduğundan (`K3`) filtre yalnız
            // `branch_node == t.from` olan kolu geçirir. İki aktif kol aynı node'da
            // duramayacağı için (`parallel_disjoint`) `matched` en fazla BİR eleman alır
            // → aşağıdaki `_` kolu GEÇERLİ bir belgeyle tetiklenemez. Kol savunma olarak
            // duruyor (kural tasarım zamanında, bu kod çalışma zamanında), ama artık
            // "aynı aksiyonu birden çok kol taşıyor" senaryosunun karşılığı DEĞİLDİR.
            let Some(t) = wfd.actions.get(action).filter(|t| t.from == b.branch_node) else {
                continue;
            };
            let matches = match &t.when {
                None => true,
                Some(expr) => {
                    let env = EvalEnv::new(&ctx)
                        .with_wfah(&wfes.wfah, &ValidRules::for_version(wfd))
                        .with_node(Some(&b.branch_node))
                        .with_actor(actor)
                        .with_wfe_id(wfes.wfe_id)
                        .with_action_input(input);
                    evaluate_bool(expr, &env)?
                }
            };
            if matches {
                matched.push((b, t));
            }
        }
        let (branch, transition) = match matched.len() {
            0 => return Err(EngineError::TransitionNotFound(action.to_string())),
            1 => matched[0],
            _ => {
                return Err(EngineError::AmbiguousAction {
                    action: action.to_string(),
                    candidates: matched.iter().map(|(b, _)| b.branch_node.clone()).collect(),
                })
            }
        };
        let branch_node = branch.branch_node.as_str();

        // §7.1 — assignment/owner kontrolü KOL üzerinden (paralel modda
        // wfe-seviyesi assigned_to NULL'dır).
        match branch.claimed_by {
            None => return Err(EngineError::NotClaimed),
            Some(owner) if owner != actor.user_id => return Err(EngineError::NotOwner),
            _ => {}
        }

        // §7.2 — ek yetki kısıtı
        if let Some(extra_rule) = &transition.extra_c_a {
            let env = MatchEnv {
                ctx: &ctx,
                wfah: &wfes.wfah,
                orgtnt_id: wfes.orgtnt_id,
            };
            // Çapa WFE'nin kendi birimi (2026-08-13): `self` gibi bir selector
            // aksiyon kapısında da "işin ait olduğu birim" demektir. Eskiden
            // çapa SORAN KİŞİYDİ, yani `self` karşılaştırmayı kendisiyle yapıp
            // daima geçiyordu — başka şubedeki aynı roldeki kişi işi havuzunda
            // GÖRMEZ ama id'yi bilse aksiyon ALABİLİYORDU. Görünürlük ile yetki
            // artık aynı çapayı kullanır.
            if !authorize_anchored(extra_rule, actor, wfes.origin_orgu_id, env, self.org).await? {
                return Err(EngineError::PermissionDenied(action.to_string()));
            }
        }

        // §7.5 — input sözleşme denetimi (ctx'e yazım YOK — yalnız wfes_effects yazar)
        let action_def = wfd
            .actions
            .get(action)
            .ok_or_else(|| EngineError::InvalidWfd(format!("action '{action}' tanımsız")))?;
        validate_action_input(action_def, input, &wfd.context)?;
        // Geri gönderme hedef seçimi (tek-kol yolla AYNI kural — kolda da geçerlidir).
        let wft = select_wft(&transition.wft, target, &visited_nodes(wfd, wfes))?;
        let mut staged = ctx.clone();

        let now = Utc::now();
        let mut seq = wfes.wfah.entries().last().map(|e| e.seq + 1).unwrap_or(1);
        let mut wfah_entries: Vec<WfahEntry> = Vec::new();

        // §7.6 — transition effects STAGED
        if let Some(effects) = &transition.wfes_effects {
            let env = EffectEnv {
                env: self.env.public(),
                call: None,
                actor,
                wfe_id: wfes.wfe_id,
                node: Some(branch_node),
                action_input: Some(input),
                exec_result: None,
                now,
            };
            staged = apply_effects(&staged, effects, &env)?;
        }

        // Ç2: kolun HAREKET satırı — from/to outcome çözülünce yazılır.
        // Ç4: satır KOLDA üretiliyor → kolun DEĞİŞMEZ kimliği (`entry_node`) etiketi.
        let action_row = wfah_entries.len();
        wfah_entries.push(WfahEntry {
            seq,
            action: action.to_string(),
            actor: actor.clone(),
            input: Some(input.clone()),
            applied_at: now,
            from_node: None,
            to_node: None,
            branch_entry: Some(branch.entry_node.clone()),
            branch_round: branch_round_of(&wfes.wfah, Some(branch.entry_node.as_str())),
        });
        seq += 1;

        // §7.7 — trigger'lar (node bağlamı = kol node'u)
        self.run_triggers(
            &transition.trigger,
            wfd,
            &mut staged,
            &mut wfah_entries,
            &mut seq,
            actor,
            wfes.wfe_id,
            Some(branch_node),
            Some(branch.entry_node.as_str()),
            Some(input),
            &wfes.wfah,
            wfes.orgtnt_id,
        )
        .await?;

        // §7.8 — wft, kol bağlamıyla (varış / kol hareketi / WFE-terminal ayrımı).
        // WOR-73: kol kimlikleri (giriş node'ları) + bu varış dahil varış kümesi —
        // join kuralının değerlendirileceği snapshot.
        let all_entries = all_entry_nodes(wfes);
        let arrived_entries = arrived_entries_with(wfes, branch_node);
        let (outcome, final_ctx, landed) = self
            .resolve_wft(
                &wft,
                wfd,
                staged,
                &wfes.wfah,
                actor,
                wfes.wfe_id,
                Some(input),
                None,
                WftMode::Branch {
                    join,
                    from_node: branch_node,
                    others_active: active.len() - 1,
                    rule: &wfes.join_rule,
                    all_entries: &all_entries,
                    arrived_entries: &arrived_entries,
                },
                // Ç4-EK/S4: menüden hedef seçildiyse bu bir GERİ GÖNDERMEDİR.
                // `select_wft` menüyü `Wft::Node`'a indirdiği için ölçüt
                // transition'ın YAZILI wft'sidir, çözülmüş hâli değil.
                matches!(transition.wft, Wft::SendBack { .. }),
            )
            .await?;

        stamp_movement(&mut wfah_entries[action_row], &outcome, Some(branch_node));

        // Aksiyon işlendi — varılan yeri kim yapabilir? (kol varışında aday yok)
        let wfah = wfes.wfah.extended(&wfah_entries);
        let resolved_c_a = self
            .candidates_at(
                &outcome,
                landed.as_ref(),
                wfd,
                &final_ctx,
                &wfah,
                // Çapa WFE'nin kendi birimi; işlemi yapan kişiyle DEĞİŞMEZ.
                wfes.origin_orgu_id.unwrap_or(actor.orgu_id),
                wfes.orgtnt_id,
            )
            .await?;

        // WOR-31 marker'ları: `_branch_arrived` / sibling `_branch_cancelled`
        stage_parallel_markers(
            wfes,
            &Trigger {
                kind: TriggerKind::Branch,
                branch: Some(branch_node),
                action: Some(action),
                actor,
            },
            &outcome,
            &mut wfah_entries,
            &mut seq,
            now,
        );

        // WFC: varılan site bir çağrı taşıyorsa outbox satırı AYNI tx'te stage edilir.
        let staged_calls =
            self.stage_calls(wfd, landed.as_ref(), &final_ctx, actor, wfes.wfe_id, now)?;
        guard_written_ctx(wfd, wfes.dynctx.as_value(), &final_ctx)?;

        let claim_recheck = self
            .stage_claim_recheck(
                wfd,
                wfes,
                &outcome,
                &wfah_entries,
                &final_ctx,
                Some(branch_node),
                now,
            )
            .await?;
        Ok(TransitionCommit {
            claim_recheck,
            wfe_id: wfes.wfe_id,
            orgtnt_id: wfes.orgtnt_id,
            new_dynctx: final_ctx,
            wfah_entries,
            outcome,
            resolved_c_a,
            staged_calls,
            // Görünürlük projeksiyonu saf pipeline'da BOŞ bırakılır: org portuna ve
            // WFE'nin çapasına ihtiyaç duyar, `WfeExecutor::fill_view_grants` doldurur.
            view_c_a: Vec::new(),
            current_view_c_a: Vec::new(),
            branch_c_a: Vec::new(),
            branch_view_c_a: Vec::new(),
            end_view_c_a: Vec::new(),
            end_terminal: end_terminal_of(landed.as_ref()),
        })
    }

    // ---------------------------------------------------------------- claim

    /// `branch`: WOR-31 — paralel modda claim KOL-bazlıdır (node adıyla). Kol
    /// node'u verilirse uygunluk O KOLUN node'unun c_a'sına göre + o kolun kendi
    /// claim durumuna göre değerlendirilir (wfe-seviyesi assigned_to yerine).
    /// `None` paralel-olmayan davranıştır: paralel modda `None` gelirse
    /// current_node NULL olduğundan `NotEligible` döner.
    pub async fn can_claim(
        &self,
        wfd: &Wfd,
        wfes: &Wfes,
        actor: &Actor,
        branch: Option<&str>,
    ) -> Result<ClaimCheck, EngineError> {
        if is_terminal_class(&wfes.status) {
            return Ok(ClaimCheck::Terminal);
        }
        // SLA-3: deadline geçmiş ama sweeper henüz `terminated`'a taşımadıysa
        // status hâlâ 'active' okunur — claim bu request-time kontrolle reddedilir
        // (2026-07-16 fix; sweeper 60s tick'e kadar tek başına yeterli değildi).
        if self.deadline_due(wfes, Utc::now()) {
            return Ok(ClaimCheck::Expired);
        }
        // WOR-31: paralel modda claim KOL-bazlıdır (node adıyla). Kol claim'i
        // o kolun `BranchState.claimed_by`'ından okunur; wfe-seviyesi assigned_to
        // paralel modda NULL'dır.
        let node_key = match branch {
            Some(b) => {
                let Some(bs) = active_branch(wfes, b) else {
                    return Ok(ClaimCheck::NotEligible);
                };
                if bs.claimed_by.is_some() {
                    return Ok(ClaimCheck::AlreadyClaimed);
                }
                b
            }
            None => {
                if wfes.assigned_to.is_some() {
                    return Ok(ClaimCheck::AlreadyClaimed);
                }
                let Some(node_key) = wfes.current_node.as_deref() else {
                    return Ok(ClaimCheck::NotEligible);
                };
                node_key
            }
        };
        let Some(node) = wfd.nodes.get(node_key) else {
            return Ok(ClaimCheck::NotEligible);
        };
        // WFC: çağrı node'unda claim YOK. `c_a` burada yalnız "kim görür"dür; iş
        // kimseye atanmaz çünkü alınacak bir aksiyon yoktur. Bu kapı olmadan node'un
        // c_a'sına uyan biri işi claim edip havuzdan çekiyor ama hiçbir şey
        // yapamıyordu — üstelik dönüş commit'i assignment'ı zaten sıfırlıyor.
        if node.call.is_some() {
            return Ok(ClaimCheck::CallInProgress);
        }
        // E04: havuz sorusu `node.c_a` DEĞİL, **`node.c_a ∪ açılmış grantlar`**dır.
        // Doğrudan `node.c_a`ya bakmak, escalation'la genişletilmiş havuzu GÖRMEYEN
        // sessiz bir kapı bırakırdı: grant ateşlenir, kişi havuzda görünür, claim
        // düğmesi 403 döner. Vekâlet provenansı `authorize_node_decision`ta korunur.
        if crate::v22::grants::authorize_node(wfd, wfes, node_key, actor, self.org).await? {
            Ok(ClaimCheck::Ok)
        } else {
            Ok(ClaimCheck::NotEligible)
        }
    }

    /// Madde 6: bir claim'in DOĞRUDAN mı VEKALETEN mi uygun olduğunu döner (audit
    /// marker'ı için provenance). `can_claim` uygunluğu zaten kapıladıktan sonra
    /// executor bunu çağırır; node çözümü `can_claim` ile aynıdır. Uygun değilse
    /// `Denied`.
    pub async fn claim_decision(
        &self,
        wfd: &Wfd,
        wfes: &Wfes,
        actor: &Actor,
        branch: Option<&str>,
    ) -> Result<AuthDecision, EngineError> {
        let node_key = match branch {
            Some(b) => b,
            None => match wfes.current_node.as_deref() {
                Some(nk) => nk,
                None => return Ok(AuthDecision::Denied),
            },
        };
        let Some(node) = wfd.nodes.get(node_key) else {
            return Ok(AuthDecision::Denied);
        };
        // WFC: çağrı node'unda claim yoktur (bkz. `can_claim`). Bu kapı `claim_as`
        // görünümünü de kapatır — aksi halde portal "Claim et" düğmesini gösterir,
        // kullanıcı tıklar ve `can_claim` reddeder. İki yol AYNI kuralı uygulamalı.
        if node.call.is_some() {
            return Ok(AuthDecision::Denied);
        }
        // `can_claim` ile AYNI gövde (E04): havuz = `node.c_a ∪ açılmış grantlar`.
        // İki yol ayrı gövdeye bakarsa portal düğmeyi gösterir, `claim` reddeder.
        crate::v22::grants::authorize_node_decision(wfd, wfes, node_key, actor, self.org).await
    }

    /// `E02`/S2 — **commit'in SONUNDA claim sahibinin yetkisi hâlâ geçerli mi.**
    ///
    /// `Ç9` grant'ın `when`ini "her yetki sorgusunda değerlendirilir" dedi; guard'ın
    /// girdileri `$ctx`, `$wfah`, `$node` ve açık grant kümesi de DEFTERDEN türüyor
    /// (`E04`/S2). Yani deftere satır eklemek guard sonucunu (`count($wfah, …)`)
    /// çevirebilir — ctx'e hiç dokunmayan bir marker commit'i bile. Bu yüzden soru
    /// "ctx yazan commit"te değil, **WFAH satırı stage eden HER commit'te** sorulur ve
    /// POST-APPEND defterle sorulur.
    ///
    /// ⚠️ Yetki sorusu bir AKTÖR ister (birim + rol); `Wfes` yalnız `user_id` taşır.
    /// Aktör `Ç13`ün `claim_taken:<node>` satırından okunur — sahipliği DOĞURAN olay
    /// aktörü de yazar. Satır yoksa (E12 öncesi açılmış claim) soru SORULMAZ: yanlış
    /// bir aktörle sorup claim düşürmek, sormamaktan kötüdür.
    #[allow(clippy::too_many_arguments)]
    async fn stage_claim_recheck(
        &self,
        wfd: &Wfd,
        wfes: &Wfes,
        outcome: &CommitOutcome,
        staged_entries: &[WfahEntry],
        new_ctx: &Value,
        branch: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<ClaimRecheck, EngineError> {
        // Ç13 okuma kuralı (b): hareket claim'i zaten düşürüyorsa soru sorulmaz.
        if outcome.clears_claim() {
            return Ok(ClaimRecheck::NotApplicable);
        }
        let (node_key, owner, branch_state) = match branch {
            Some(b) => match active_branch(wfes, b) {
                Some(bs) => (bs.branch_node.as_str(), bs.claimed_by, Some(bs)),
                None => return Ok(ClaimRecheck::NotApplicable),
            },
            None => match wfes.current_node.as_deref() {
                Some(n) => (n, wfes.assigned_to, None),
                None => return Ok(ClaimRecheck::NotApplicable),
            },
        };
        let Some(owner) = owner else {
            return Ok(ClaimRecheck::NotApplicable);
        };
        let Some(claimant) = claimant_actor(&wfes.wfah, node_key, owner) else {
            return Ok(ClaimRecheck::NotApplicable);
        };

        // POST-APPEND görünüm: bu commit'in satırları + yeni ctx. `Wfes`in kopyası
        // yalnız BURADA kurulur ve store'a gitmez — guard'ın göreceği dünyayı temsil
        // eder.
        let mut post = wfes.clone();
        post.wfah = wfes.wfah.extended(staged_entries);
        post.dynctx = crate::types::dynctx::DynCtx(new_ctx.clone());
        if crate::v22::grants::authorize_node(wfd, &post, node_key, &claimant, self.org).await? {
            return Ok(ClaimRecheck::Kept);
        }

        let ownership_branch = branch_state.map(|bs| OwnershipBranch {
            entry: bs.entry_node.as_str(),
            at_node: bs.branch_node.as_str(),
        });
        let claimed_at = branch_state.map_or(wfes.claimed_at, |bs| bs.claimed_at);
        let released = ClaimReleased::grant_guard_false(node_key, owner)
            .held(claimed_at.map(|c| seconds_between(c, now)))
            .in_branch(ownership_branch);
        let seq = post.wfah.entries().last().map(|e| e.seq + 1).unwrap_or(1);
        Ok(ClaimRecheck::Released {
            entry: WfahEntry {
                seq,
                action: released.marker(),
                // Bırakmayı KİMSE talep etmedi — kural işledi. Aktör sistemdir; sahiplik
                // ÖZNESİ payload'ın `owner` alanındadır (Ç13: `actor` eylemi YAPAN).
                actor: system_actor(),
                input: Some(released.input()),
                applied_at: now,
                // Ç2: sahiplik satırı hareket taşımaz.
                from_node: None,
                to_node: None,
                branch_entry: branch_state.map(|bs| bs.entry_node.clone()),
                branch_round: branch_state
                    .and_then(|bs| valid::round_of_opt(&post.wfah, Some(bs.entry_node.as_str()))),
            },
        })
    }

    /// Görünürlük projeksiyonu (2026-08-13): `wfd.listable[] ∪ wfd.wf_admin[]`
    /// kurallarının ÇÖZÜLMÜŞ aday listesi — `wf.wfe.view_c_a` kolonuna yazılır.
    ///
    /// Neden ayrı bir kolon: bu grant'lar WFE bittiğinde de geçerlidir
    /// (`current_c_a` terminal'de boşaltılır), ve `when` guard'ı UYGULANMIŞ
    /// olarak yazılır — havuzun bugünkü "when'i yok say, over-inclusive kabul"
    /// yaklaşıklığı böylece kalkar.
    ///
    /// İKİ VIEWER BAĞIMSIZLIK ŞARTI (grant'lar viewer bilinmezken yazılır):
    ///   1. `c_orgu` çapası `origin_orgu` — WFE'nin kendi birimi, viewer'ın
    ///      birimi DEĞİL. Eskiden viewer'a çapalanıyordu ve `{c_orgu:"self"}`
    ///      birim karşılaştırmasını kendisiyle yapıp her zaman true dönüyordu
    ///      (yani sessizce "tenant genelinde o rol" anlamına geliyordu).
    ///   2. `when` guard'ı AKTÖRSÜZ değerlendirilir; `$actor` referansı bu
    ///      guard'larda YASAKTIR (validator `grant_when_actor_ref` ile yayında
    ///      keser) — aksi halde guard viewer'a bağlı olur ve projeksiyona sığmaz.
    pub async fn view_grants(
        &self,
        wfd: &Wfd,
        ctx: &Value,
        wfah: &Wfah,
        current_node: Option<&str>,
        wfe_id: Uuid,
        origin_orgu: Uuid,
        orgtnt_id: Uuid,
    ) -> Result<Vec<ResolvedCandidate>, EngineError> {
        let mut out: Vec<ResolvedCandidate> = Vec::new();
        self.extend_grant_candidates(
            &mut out,
            &wfd.listable,
            ctx,
            wfah,
            &ValidRules::for_version(wfd),
            current_node,
            wfe_id,
            origin_orgu,
            orgtnt_id,
        )
        .await?;
        // Görme yetkisi `allowed_global_actions`tan BAĞIMSIZDIR: bir kurala uymak
        // WFE'yi görmeye yeter (`can_view` (e)) — liste yalnız müdahaleyi kapılar.
        self.extend_grant_candidates(
            &mut out,
            wfd.wf_admin.iter().map(WfAdminRule::grant_ref),
            ctx,
            wfah,
            &ValidRules::for_version(wfd),
            current_node,
            wfe_id,
            origin_orgu,
            orgtnt_id,
        )
        .await?;
        Ok(out)
    }

    /// Node-seviyesi görünürlük projeksiyonu (2026-08-13): `nodes.<key>.listable[]`
    /// kurallarının ÇÖZÜLMÜŞ aday listesi — `wf.wfe.current_view_c_a` (tek-kol) ve
    /// `wf.wfe_branch.view_c_a` (kol) kolonlarına yazılır.
    ///
    /// Kök `listable`/`wf_admin` ile AYNI çözücüyü (`extend_grant_candidates`)
    /// kullanır: iki yerin `when`/çapa davranışı ayrışırsa `can_view` (f) ile SQL
    /// süzgeci sessizce farklı cevap verir. Çapa da AYNIDIR (`origin_orgu`) —
    /// aynı `{c_a, when}` şeklini taşıyan iki kuralın `self`'i başka şey demesi
    /// tasarımcı için tuzak olurdu.
    ///
    /// `guard_node` = `when` guard'ının göreceği `$node`, yani `can_view`'in OKUMA
    /// anında `matches_grant_rules`e verdiği değer (`wfes.current_node`). Tek-kol
    /// yolunda varılan node'un kendisidir; paralel modda wfe-seviyesi
    /// `current_node` NULL olduğu için kol projeksiyonunda `None`'dır — kolon ile
    /// referans okuma arasındaki eşitlik bu ayrıntıya bağlıdır.
    ///
    /// Bilinmeyen node = boş liste: bu bir yetki sorusu değil cache üretimidir,
    /// eksik node'da hata atmak commit'i düşürürdü.
    #[allow(clippy::too_many_arguments)]
    pub async fn node_view_grants(
        &self,
        wfd: &Wfd,
        node_key: &str,
        ctx: &Value,
        wfah: &Wfah,
        guard_node: Option<&str>,
        wfe_id: Uuid,
        origin_orgu: Uuid,
        orgtnt_id: Uuid,
    ) -> Result<Vec<ResolvedCandidate>, EngineError> {
        let Some(node) = wfd.nodes.get(node_key) else {
            return Ok(Vec::new());
        };
        let mut out: Vec<ResolvedCandidate> = Vec::new();
        self.extend_grant_candidates(
            &mut out,
            &node.listable,
            ctx,
            wfah,
            &ValidRules::for_version(wfd),
            guard_node,
            wfe_id,
            origin_orgu,
            orgtnt_id,
        )
        .await?;
        Ok(out)
    }

    /// Terminal-seviyesi görünürlük projeksiyonu (2026-08-17): `terminals[].listable[]`
    /// kurallarının ÇÖZÜLMÜŞ hâli → `wf.wfe.end_view_c_a`.
    ///
    /// `node_view_grants`in kardeşidir ve AYNI çözücüyü paylaşır — ayrışırlarsa
    /// `can_view` (g) ile SQL süzgeci sessizce farklı cevap verir. İki fark:
    /// * Girdi node değil TERMİNALDİR (`terminals[]` bir dizi olduğundan id ile aranır).
    /// * `guard_node` YOKTUR: terminal'de `current_node` NULL'dır, dolayısıyla okuma
    ///   anında `matches_grant_rules`in `$node`'u da `None` olacaktır. Projeksiyonu
    ///   varılan terminal'in adıyla yazmak, kolon ile referans okuma arasındaki
    ///   eşitliği bozardı (`visibility_report` tam bunu ölçüyor).
    ///
    /// Bilinmeyen terminal = boş liste — `node_view_grants` ile aynı gerekçe: bu bir
    /// yetki sorusu değil cache üretimidir, eksik kayıtta hata atmak commit'i düşürürdü.
    #[allow(clippy::too_many_arguments)]
    pub async fn terminal_view_grants(
        &self,
        wfd: &Wfd,
        terminal_id: &str,
        ctx: &Value,
        wfah: &Wfah,
        wfe_id: Uuid,
        origin_orgu: Uuid,
        orgtnt_id: Uuid,
    ) -> Result<Vec<ResolvedCandidate>, EngineError> {
        let Some(terminal) = wfd.terminals.iter().find(|t| t.id == terminal_id) else {
            return Ok(Vec::new());
        };
        let mut out: Vec<ResolvedCandidate> = Vec::new();
        self.extend_grant_candidates(
            &mut out,
            &terminal.listable,
            ctx,
            wfah,
            &ValidRules::for_version(wfd),
            None,
            wfe_id,
            origin_orgu,
            orgtnt_id,
        )
        .await?;
        Ok(out)
    }

    /// `{c_a, when?}` grant kurallarını çözüp `out`a EKLER — kök
    /// `listable`/`wf_admin` (`view_grants`) ve node `listable`
    /// (`node_view_grants`) tek çözücü paylaşır.
    ///
    /// `when` guard'ı AKTÖRSÜZ değerlendirilir (`$actor` bu guard'larda validator
    /// tarafından yasaklı — `grant_when_actor_ref`): projeksiyon viewer
    /// bilinmezken yazılır.
    #[allow(clippy::too_many_arguments)]
    async fn extend_grant_candidates<'r, I>(
        &self,
        out: &mut Vec<ResolvedCandidate>,
        rules: I,
        ctx: &Value,
        wfah: &Wfah,
        // E05: `$valid`/`#.is_send_back` belgeden türer; guard ifadesi de onları
        // görebildiği için kural seti buraya kadar taşınmak ZORUNDA (bu fonksiyonun
        // elinde WFD yok).
        valid_rules: &ValidRules,
        guard_node: Option<&str>,
        wfe_id: Uuid,
        origin_orgu: Uuid,
        orgtnt_id: Uuid,
    ) -> Result<(), EngineError>
    where
        I: IntoIterator<Item = &'r CaGrantRule>,
    {
        for rule in rules {
            if let Some(expr) = &rule.when {
                let env = EvalEnv::new(ctx)
                    .with_wfah(wfah, valid_rules)
                    .with_node(guard_node)
                    .with_wfe_id(wfe_id);
                if !evaluate_bool(expr, &env)? {
                    continue;
                }
            }
            let mut extra = self
                .resolve_candidates(&rule.c_a, ctx, wfah, origin_orgu, orgtnt_id)
                .await?;
            // Aynı aday iki kuraldan da gelebilir (listable + wf_admin); kolon
            // containment ile sorgulandığı için tekrar zararsız ama gereksiz.
            extra.retain(|c| !out.contains(c));
            out.append(&mut extra);
        }
        Ok(())
    }

    // -------------------------------------------------------------- reassign

    /// Madde 7: yetkili claim devri — SAF. İki `authorize` koşar ve persist
    /// edilecek sahiplik satır(lar)ını döner (asıl yazım `WfeStore::reassign`):
    /// 1. Aktif node (paralel modda `branch` kolu) çözülür; `reassign` kuralı
    ///    yoksa devir bu node'da kapalıdır → `Unauthorized`.
    /// 2. `reassigner` node.reassign kuralına uymalı → aksi `Unauthorized`.
    /// 3. `target = Some` ise hedef node.c_a'ya uygun olmalı → aksi
    ///    `TargetNotEligible`; `target = None` (havuza bırakma) bu adımı atlar.
    ///
    /// Ç13/E12: satırlar sahiplik ailesindendir (`claim_released:` / `claim_taken:`);
    /// `reassign` ve `unclaim` WFAH aksiyon adları KALKTI. Dönen satırlar AYNI
    /// transaction'da, döndükleri SIRAYLA yazılır — ardışık `seq` bunu gerektirir.
    pub async fn reassign(
        &self,
        wfd: &Wfd,
        wfes: &Wfes,
        reassigner: &Actor,
        target: Option<&Actor>,
        branch: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<Vec<WfahEntry>, EngineError> {
        if is_terminal_class(&wfes.status) {
            return Err(EngineError::WfeTerminal);
        }
        if self.deadline_due(wfes, now) {
            return Err(EngineError::WfeExpired);
        }
        // Aktif node + o an geçerli owner (paralel modda kol-bazlı). Sahiplik
        // satırlarının iki türetilmiş süresi de buradan beslenir: `claimed_at`
        // tutma süresinin, node/kol girişi ise bekleme tabanının başlangıcıdır.
        let (node_key, from_owner, claimed_at, entered_at) = match branch {
            Some(b) => {
                let Some(bs) = active_branch(wfes, b) else {
                    return Err(EngineError::InvalidWfd(format!(
                        "reassign için aktif kol yok: '{b}'"
                    )));
                };
                // R02/S3: kol girişi gerçek kolondan okunur, defterden TÜRETİLMEZ.
                (b, bs.claimed_by, bs.claimed_at, Some(bs.entered_at))
            }
            None => {
                let Some(nk) = wfes.current_node.as_deref() else {
                    return Err(EngineError::InvalidWfd(
                        "reassign için current_node yok".into(),
                    ));
                };
                (nk, wfes.assigned_to, wfes.claimed_at, node_entered_at(&wfes.wfah))
            }
        };
        let node = wfd
            .nodes
            .get(node_key)
            .ok_or_else(|| EngineError::InvalidWfd(format!("bilinmeyen node '{node_key}'")))?;

        // 1+2. Yetki İKİ yoldan gelir (T‑A5):
        //   · node.reassign — o node'un kendi amiri (Madde 7, bugünkü davranış)
        //   · wfd.wf_admin  — akış yöneticisi; node'un kuralı OLMASA da devredebilir
        // İkisi de yoksa devir kapalıdır (403).
        let ctx = wfes.dynctx.as_value();
        let env = MatchEnv {
            ctx,
            wfah: &wfes.wfah,
            orgtnt_id: wfes.orgtnt_id,
        };
        let by_node_rule = match node.reassign.as_ref() {
            Some(rule) => {
                authorize_anchored(rule, reassigner, wfes.origin_orgu_id, env, self.org).await?
            }
            None => false,
        };
        let by_wf_admin = if by_node_rule {
            false // node kuralı yetti; wf_admin sorgusu gereksiz I/O olurdu
        } else {
            matches_grant_rules(
                wfd.wf_admin.iter().map(WfAdminRule::grant_ref),
                reassigner,
                wfes,
                &ValidRules::for_version(wfd),
                self.org,
            )
            .await?
        };
        if !by_node_rule && !by_wf_admin {
            return Err(EngineError::Unauthorized);
        }
        // A-2: `wf_admin` yolu artık kuralın VERDİĞİ global aksiyonu ister. Üç ayrı
        // aksiyon çünkü üçü ayrı işler ve hassas akışta ayrı ayrı kısıtlanır:
        //   · hedef yok                → `reclaim_to_pool` (işi havuza döndür)
        //   · hedef var, sahip yok     → `assign_from_pool` (havuzdaki işi kişiye ver)
        //   · hedef var, sahip var     → `reassign` (kişiden kişiye devir)
        // `node.reassign` yolu KAPILANMAZ: o akış tasarımcısının o node'a yazdığı
        // yetkidir, global aksiyon değildir.
        if by_wf_admin {
            let needed = match (target.is_some(), from_owner.is_some()) {
                (false, _) => GlobalAction::ReclaimToPool,
                (true, false) => GlobalAction::AssignFromPool,
                (true, true) => GlobalAction::Reassign,
            };
            require_global_action(&wfd.wf_admin, reassigner, needed, wfes, &ValidRules::for_version(wfd), self.org).await?;
        }

        // 3. Hedef (varsa) node.c_a'ya uygun olmalı.
        if let Some(t) = target {
            if !authorize_anchored(&node.c_a, t, wfes.origin_orgu_id, env, self.org).await? {
                return Err(EngineError::TargetNotEligible);
            }
        }

        // Ç4/E14: kol devrinde satır O KOLDA üretilir — kimlik + tur tek yerden.
        let (branch_entry, branch_round) = branch_label(wfes, branch);
        // E12/S5: paralel modda kol alanları ZORUNLUDUR ve YAZMA ANINDA dolar —
        // E14 kol satırlarını tur kapanınca sildiği için sonradan türetilemezler.
        let ownership_branch = branch.map(|b| OwnershipBranch {
            entry: branch_entry.as_deref().unwrap_or(b),
            at_node: b,
        });
        // Ç13: üç global aksiyonu marker ADI değil bu alan ayırır. Eski `reassign`/
        // `unclaim` adları yetkiyi ayırmıyordu; "yokluğu anlam taşır" antipattern'i
        // (yol adının OLMAMASI = node kuralı) bu alanla kapandı.
        let global_action = match (target.is_some(), from_owner.is_some()) {
            (false, _) => GlobalAction::ReclaimToPool,
            (true, false) => GlobalAction::AssignFromPool,
            (true, true) => GlobalAction::Reassign,
        };
        // E12/S2: yetkili devirde bekleme YOKTUR — iş zaten birinin üzerindeydi.
        // Havuzdan atamada gerçek bekleme yazılır (kural İKİ cümledir).
        let waited_for_seconds = if from_owner.is_some() {
            Some(0)
        } else {
            entered_at.map(|e| {
                seconds_between(
                    wait_base(&wfes.wfah, node_key, branch_entry.as_deref(), e),
                    now,
                )
            })
        };

        // Ç13/E12/S4: satır SAYISI `from_owner`ın varlığına bağlıdır.
        //   · kişiden kişiye devir → İKİ satır (önce bırakma, sonra alma), ardışık seq
        //   · havuzdan kişiye atama → TEK `claim_taken:` (kaybeden sahip YOK)
        //   · havuza bırakma        → TEK `claim_released:`
        // "Bir satır = bir sahiplik öznesi" değişmezi bunun sebebidir: tek satırlık
        // `{from, to}` okuyucuyu her yerde iki şekle hazır durmaya zorlar ve iki
        // türetilmiş süre kendi öznelerinden ayrı düşerdi.
        let mut rows: Vec<(String, Value)> = Vec::with_capacity(2);
        if let Some(owner) = from_owner {
            let released = if by_wf_admin {
                ClaimReleased::by_admin(node_key, owner, global_action)
            } else if owner == reassigner.user_id {
                ClaimReleased::by_self(node_key, owner)
            } else {
                ClaimReleased::taken_by_other(node_key, owner)
            }
            .held(claimed_at.map(|c| seconds_between(c, now)))
            .in_branch(ownership_branch);
            rows.push((released.marker(), released.input()));
        }
        if let Some(t) = target {
            // `authority`: hedefin uygunluğu (3. adım) bugün YALNIZ `node.c_a`ya
            // bakıyor — açık grant'lar claim yoluna `E04` (`authorize_node`) ile
            // girecek ve değer ORADAN gelecek. `c_a` zaten öncelikli taraftır
            // (E12/S1), yani E04 indiğinde bu satırın anlamı değişmez, yalnız
            // grant'la gelen hedefler `grant` yazmaya başlar.
            let taken = if by_wf_admin {
                ClaimTaken::admin_assigned(node_key, t.user_id, ClaimAuthority::Ca, global_action)
            } else {
                ClaimTaken::assigned(node_key, t.user_id, ClaimAuthority::Ca)
            }
            .waited(waited_for_seconds)
            .in_branch(ownership_branch);
            rows.push((taken.marker(), taken.input()));
        }

        let mut seq = wfes.wfah.entries().last().map(|e| e.seq + 1).unwrap_or(1);
        Ok(rows
            .into_iter()
            .map(|(action, input)| {
                let entry = WfahEntry {
                    seq,
                    action,
                    actor: reassigner.clone(),
                    input: Some(input),
                    applied_at: now,
                    // Ç2: sahiplik devri node DEĞİŞTİRMEZ — hareket satırı değil.
                    from_node: None,
                    to_node: None,
                    // Ç4: kol devrinde satır O KOLDA üretilir.
                    branch_entry: branch_entry.clone(),
                    branch_round,
                };
                seq += 1;
                entry
            })
            .collect())
    }

    // ------------------------------------------------------ possible actions

    /// Owner'ın şu an gerçekleştirebileceği aksiyonlar + (geri gönderme ise) seçilebilir
    /// hedefleri.
    ///
    /// Dönüş tipi düz `Vec<String>` DEĞİLDİR: hedef artık aksiyon anahtarına
    /// kodlanmadığı için, "hangi aksiyonlar mümkün" sorusunun cevabı "hangi hedefler
    /// seçilebilir" bilgisi olmadan eksik kalır — istemci hedef listesini WFD'yi
    /// okuyarak türetmek zorunda kalırdı.
    ///
    /// `branch`: WOR-31 — paralel modda kol node'u verilirse mümkün aksiyonlar
    /// O KOLUN node'una ve KOLUN claimed_by'ına göre hesaplanır (wfe-seviyesi
    /// current_node/assigned_to yerine). `None` paralel-olmayan eski davranış;
    /// paralel modda `None` ile çağrılırsa current_node NULL olduğundan boş
    /// döner — T4 (executor/route seviyesi) aktif kollar üzerinden birleşim
    /// kurmak için bu fonksiyonu her aktif kol için ayrı çağırır.
    pub async fn possible_actions(
        &self,
        wfd: &Wfd,
        wfes: &Wfes,
        actor: &Actor,
        branch: Option<&str>,
    ) -> Result<Vec<ActionChoice>, EngineError> {
        if is_terminal_class(&wfes.status) {
            return Ok(vec![]);
        }
        if self.deadline_due(wfes, Utc::now()) {
            return Ok(vec![]);
        }
        let (node_key, owner) = match branch {
            Some(b) => match active_branch(wfes, b) {
                Some(bs) => (b, bs.claimed_by),
                None => return Ok(vec![]),
            },
            None => {
                let Some(nk) = wfes.current_node.as_deref() else {
                    return Ok(vec![]);
                };
                (nk, wfes.assigned_to)
            }
        };
        if owner != Some(actor.user_id) {
            return Ok(vec![]);
        }
        let ctx = wfes.dynctx.as_value().clone();
        // K-2: geri gönderme menüsünün süzgeci — döngü başına BİR kez hesaplanır.
        let visited = visited_nodes(wfd, wfes);
        let mut actions: Vec<ActionChoice> = Vec::new();
        // v2.3: kimlik map anahtarı olduğu için "aynı aksiyon iki kez sunulmasın"
        // tekilleştirmesi (`actions.iter().any(...)`) GEREKSİZ — map bir anahtarı bir kez
        // taşır. `Ç5` bu ayıklamayı yapısal olarak yaptı.
        for (action_key, t) in &wfd.actions {
            if t.from != *node_key {
                continue;
            }
            let when_ok = match &t.when {
                None => true,
                Some(expr) => {
                    let env = EvalEnv::new(&ctx)
                        .with_wfah(&wfes.wfah, &ValidRules::for_version(wfd))
                        .with_node(Some(node_key))
                        .with_actor(actor)
                        .with_wfe_id(wfes.wfe_id);
                    // input'a bağlı guard'lar input'suz değerlendirilemez — aday sayılır
                    evaluate_bool(expr, &env).unwrap_or(true)
                }
            };
            if !when_ok {
                continue;
            }
            if let Some(extra_rule) = &t.extra_c_a {
                let env = MatchEnv {
                    ctx: &ctx,
                    wfah: &wfes.wfah,
                    orgtnt_id: wfes.orgtnt_id,
                };
                // Aksiyon kapısıyla AYNI çapa — liste ile gerçek kapı ayrı düşmesin.
                if !authorize_anchored(extra_rule, actor, wfes.origin_orgu_id, env, self.org)
                    .await?
                {
                    continue;
                }
            }
            let targets = match &t.wft {
                Wft::SendBack { targets } => {
                    // K-2: menü ÖRNEĞE göre süzülür. Hiç uğranmış hedef kalmazsa aksiyon
                    // HİÇ SUNULMAZ — boş menülü bir satır kullanıcıya "geri gönder" düğmesi
                    // gösterip her seçimde 400 döndürürdü.
                    let offered = offered_targets(targets, &visited);
                    if offered.is_empty() {
                        continue;
                    }
                    Some(
                        offered
                            .into_iter()
                            .map(|g| SendBackChoice {
                                node: g.node.clone(),
                                label: g.label.clone(),
                            })
                            .collect(),
                    )
                }
                _ => None,
            };
            actions.push(ActionChoice {
                action: action_key.clone(),
                targets,
            });
        }
        Ok(actions)
    }

    // ------------------------------------------------------------ escalation

    /// Node'un henüz ateşlenmemiş ilk escalation adımı için giriş anı + vade
    /// bilgisi — dashboard insight'ları (yaklaşan/geciken escalation) ve
    /// `due_escalation` ortak temeli. Node'a giriş anı HAREKET TAŞIYAN son WFAH
    /// kaydından türetilir (`node_entered_at` — R02); ateşlenen adımlar
    /// `escalate:<node>:<idx>` WFAH kayıtlarıyla izlenir.
    /// `branch`: WOR-31 — paralel modda dwell KOL-bazlıdır; kol node'u verilirse
    /// giriş anı `BranchState.entered_at`'tan okunur (WFAH türetimi değil).
    /// `None` paralel mod dışındaki eski davranıştır (paralel modda `None` ile
    /// çağrılırsa `current_node` NULL olduğundan `None` döner).
    pub fn next_escalation(
        &self,
        wfd: &Wfd,
        wfes: &Wfes,
        now: DateTime<Utc>,
        branch: Option<&str>,
    ) -> Result<Option<EscalationForecast>, EngineError> {
        if is_terminal_class(&wfes.status) {
            return Ok(None);
        }
        let (node_key, entered_at) = match branch {
            Some(b) => match active_branch(wfes, b) {
                Some(bs) => (b, bs.entered_at),
                None => return Ok(None),
            },
            None => {
                let Some(node_key) = wfes.current_node.as_deref() else {
                    return Ok(None);
                };
                // R02: taban HAREKET taşıyan son satırdır (`to_node != null`) ve tanım
                // `node_entered_at`te TEK yerde durur — Ç13'ün `waited_for_seconds`'ı da
                // onu çağırır. Marker satırları `to_node` taşımadığı için tabanı
                // kaydırmaz; ad öneki filtresi (eski hâl) KALKTI.
                let Some(entered_at) = node_entered_at(&wfes.wfah) else {
                    return Ok(None);
                };
                (node_key, entered_at)
            }
        };
        let Some(node) = wfd.nodes.get(node_key) else {
            return Ok(None);
        };
        for (idx, step) in node.escalation.iter().enumerate() {
            let marker = escalation_marker(node_key, idx);
            let skipped_marker = skipped_escalation_marker(node_key, idx);
            // "Bu adım kapandı" iki yoldan olur: otomatik/elle ATEŞLENDİ ya da WF Admin
            // ATLADI. İkisi de aynı defteri kullanır (T‑A5).
            //
            // R02: `applied_at >= entered_at` kapısı taban düzeltilince KENDİLİĞİNDEN
            // doğrulanır — taban artık gerçek node girişi olduğu için o node'da
            // ateşlenmiş `escalate:<node>:<idx>[:skipped]` satırları koşulu HEP sağlar.
            // Eski tabanda marker satırı tabanı kendi önüne atıp ateşlenmiş kademeyi
            // "ateşlenmemiş" gösterebiliyordu; tekrar ateşleme yolu böyle kapandı.
            let settled = wfes.wfah.entries().iter().any(|e| {
                (e.action == marker || e.action == skipped_marker) && e.applied_at >= entered_at
            });
            if settled {
                continue;
            }
            // adımlar sıralı — ilk ateşlenmemiş adım bu turun cevabıdır
            let after = parse_iso8601_duration(&step.after)?;
            let deadline = entered_at + after;
            return Ok(Some(EscalationForecast {
                step_idx: idx,
                entered_at,
                deadline,
                overdue: now >= deadline,
            }));
        }
        Ok(None)
    }

    /// T‑A5: WF Admin'in ELLE TETİKLEMESİ — yetki kapısı DAHİL.
    ///
    /// `fire_escalation_by` mekanik primitiftir (yetki sormaz); bu fonksiyon yetkiyi,
    /// terminal kontrolünü ve "hangi adım" seçimini bir arada yapar. Ayrı bırakılsa
    /// yetki denetimi çağıran katmanın hatırlamasına kalırdı — unutulduğunda yetkisiz
    /// bir aktör akışı ilerletebilirdi.
    ///
    /// Adım numarası ÇAĞIRANDAN alınmaz: sıradaki ateşlenmemiş adım uygulanır, böylece
    /// escalation adımlarının sıralı olma sözleşmesi korunur. Vade GEREKMEZ.
    ///
    /// Bekleyen adım yoksa `Ok(None)`.
    pub async fn admin_fire_escalation(
        &self,
        wfd: &Wfd,
        wfes: &Wfes,
        admin: &Actor,
        branch: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<Option<(usize, TransitionCommit)>, EngineError> {
        if is_terminal_class(&wfes.status) {
            return Err(EngineError::WfeTerminal);
        }
        // A-1 (2026-08-21): yetki artık örtük DEĞİL — `wf_admin` kuralına uymak yetmez,
        // kural `fire_escalation`ı da vermiş olmalı. Boş listeli admin yalnız GÖRÜR.
        require_global_action(
            &wfd.wf_admin,
            admin,
            GlobalAction::FireEscalation,
            wfes,
            &ValidRules::for_version(wfd),
            self.org,
        )
        .await?;
        let Some(forecast) = self.next_escalation(wfd, wfes, now, branch)? else {
            return Ok(None);
        };
        let commit = self
            .fire_escalation_by(wfd, wfes, forecast.step_idx, now, branch, admin)
            .await?;
        Ok(Some((forecast.step_idx, commit)))
    }

    /// T‑A5: WF Admin'in ATLAMASI. Marker yazar, geçişi UYGULAMAZ.
    ///
    /// Marker adı `escalate:<node>:<idx>:skipped` — `escalate:` öneki ZORUNLUDUR, ama
    /// artık ESCALATION TABANI için değil: R02'den beri taban `to_node != null`
    /// satırlardan geliyor ve marker satırları (bu dahil) `to_node` taşımıyor. Önek
    /// `parse_marker`/`WfahKind` ayrımı ve yayınlanmış `count($wfah, …)` sayımları için
    /// zorunludur — atlanan adımın `settled` sayılması da bu ada bakar.
    ///
    /// Bekleyen adım yoksa `Ok(None)` — bu bir hata değil, bir cevaptır; HTTP karşılığını
    /// çağıran katman verir.
    pub async fn skip_escalation(
        &self,
        wfd: &Wfd,
        wfes: &Wfes,
        admin: &Actor,
        branch: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<Option<EscalationSkip>, EngineError> {
        if is_terminal_class(&wfes.status) {
            return Err(EngineError::WfeTerminal);
        }
        // Sayaç yönetimi YALNIZ wf_admin yetkisidir; node.reassign bunu AÇMAZ.
        // A-1: `skip_escalation` global aksiyonu da listede yazmak zorunda.
        require_global_action(
            &wfd.wf_admin,
            admin,
            GlobalAction::SkipEscalation,
            wfes,
            &ValidRules::for_version(wfd),
            self.org,
        )
        .await?;
        let Some(forecast) = self.next_escalation(wfd, wfes, now, branch)? else {
            return Ok(None);
        };
        let node_key = match branch {
            Some(b) => b.to_string(),
            None => wfes.current_node.clone().unwrap_or_default(),
        };
        let marker = skipped_escalation_marker(&node_key, forecast.step_idx);
        let seq = wfes.wfah.entries().last().map(|e| e.seq + 1).unwrap_or(1);
        // Ç4/E14: kol sayacı atlanıyorsa satır O KOLDA.
        let (branch_entry, branch_round) = branch_label(wfes, branch);
        Ok(Some(EscalationSkip {
            step_idx: forecast.step_idx,
            node: node_key,
            entry: WfahEntry {
                seq,
                action: marker.clone(),
                actor: admin.clone(),
                input: Some(json!({"skipped": true, "after": forecast.deadline.to_rfc3339()})),
                applied_at: now,
                // Ç2: sayaç atlaması node DEĞİŞTİRMEZ — marker satırı.
                from_node: None,
                to_node: None,
                // Ç4: kol sayacı atlanıyorsa satır O KOLDA.
                branch_entry,
                branch_round,
            },
            marker,
        }))
    }

    // ------------------------------------------------- GLOBAL AKSİYONLAR (A-2)
    //
    // Motorun tanımladığı, WFD'ye yazılmayan ve YALNIZ Workflow Admin'in alabildiği
    // müdahaleler (toplantı kararı J‑2/J‑3). Üçü burada, üçü mevcut yollarda:
    //   · `assign_from_pool` / `reclaim_to_pool` / `reassign` → `Engine::reassign`
    //     (tek yol, üç kapı — devir mekaniği zaten oradaydı, ikinci bir kopya
    //     claim/CAS semantiğini iki yerden bakılır hâle getirirdi)
    //   · `fire_escalation` / `skip_escalation` → `admin_fire_escalation` / `skip_escalation`
    //   · `send_back` / `send_to_start` / `cancel` → BURADA
    //
    // Üçünün de değişmezleri AYNI: (1) terminal-class WFE reddedilir, (2) WFAH'a
    // GERÇEK admin `(ORGU, U, R)` üçlüsüyle yazılır — `system` ile DEĞİL, yoksa
    // müdahaleyi kimin yaptığı kaybolur, (3) `$ctx` DEĞİŞMEZ (global aksiyon iş verisi
    // yazmaz; commit yine yeni bir DynCtx revizyonu üretir, immutability korunur).

    /// Bu adminin BU WFE'de alabileceği global aksiyonlar — havuz/detay ekranı
    /// düğmeleri bununla süzülür (A-3, E-3).
    ///
    /// Boş küme "admin değil" DEMEZ: görme yetkisi listeden bağımsızdır.
    pub async fn admin_global_actions(
        &self,
        wfd: &Wfd,
        wfes: &Wfes,
        admin: &Actor,
    ) -> Result<BTreeSet<GlobalAction>, EngineError> {
        wf_admin_global_actions(&wfd.wf_admin, admin, wfes, &ValidRules::for_version(wfd), self.org).await
    }

    /// `send_back` — akışı, bu WFE'nin GERÇEKTEN uğradığı bir node'a geri atar.
    ///
    /// WFD içindeki geri gönderme (`Wft::SendBack`) ile AYNI süzgeci kullanır
    /// (`visited_nodes`, K-2) ama akışta tanımlı bir aksiyon GEREKTİRMEZ: hedef kümesi
    /// belgeden değil, örneğin gerçek geçmişinden çıkar. **Derinlik sınırı YOKTUR**
    /// (A-4): "kaç adım geriye" sorusunun cevabı listedir, sayı değil.
    pub async fn admin_send_back(
        &self,
        wfd: &Wfd,
        wfes: &Wfes,
        admin: &Actor,
        target_node: &str,
        now: DateTime<Utc>,
    ) -> Result<TransitionCommit, EngineError> {
        self.admin_move_to(wfd, wfes, admin, GlobalAction::SendBack, target_node, now)
            .await
    }

    /// `send_to_start` — akışı start node'una döndürür. **Yeni WFE AÇILMAZ**, aynı WFE
    /// geri sarar (WFAH ve DynCtx geçmişi korunur; sıfırdan başlatmak izi koparırdı).
    ///
    /// `target_node`: belgede birden çok start kuralı varsa hangi start node'una
    /// dönüleceği. Tek adayda `None` yeterlidir; çok adayda seçim ZORUNLUDUR — biri
    /// keyfî olarak seçilse akış, tasarımcının hiç kastetmediği bir havuza düşerdi.
    pub async fn admin_send_to_start(
        &self,
        wfd: &Wfd,
        wfes: &Wfes,
        admin: &Actor,
        target_node: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<TransitionCommit, EngineError> {
        // v2.3: START node'ları start aksiyonlarının `from`larıdır (`Ç7+Ç8`).
        let starts: BTreeSet<&str> = wfd
            .start
            .iter()
            .filter_map(|r| crate::types::wfd_v22::start_action(wfd, r))
            .map(|a| a.from.as_str())
            .collect();
        let chosen = match (target_node, starts.len()) {
            (Some(t), _) => {
                // Verilen hedef bir START node'u olmak zorunda: `send_to_start` ile
                // rastgele bir node'a taşımak, `send_back` kapısını (o da ayrı bir
                // global aksiyon) atlamanın yolu olurdu.
                if !starts.contains(t) {
                    return Err(EngineError::TargetInvalid(t.to_string()));
                }
                t
            }
            (None, 1) => starts.iter().next().copied().expect("len==1"),
            (None, 0) => {
                return Err(EngineError::InvalidWfd(
                    "belgede start kuralı yok: 'başa gönder' hedefi türetilemiyor".into(),
                ))
            }
            (None, _) => return Err(EngineError::TargetRequired),
        };
        self.admin_move_to(wfd, wfes, admin, GlobalAction::SendToStart, chosen, now)
            .await
    }

    /// `send_back` / `send_to_start` ortak gövdesi — ikisi de "WFE'yi bir node'a taşı"
    /// işidir, farkları hedefin NEREDEN geldiği ve hangi yetkiyi istediğidir.
    ///
    /// Taşıma normal `Wft::Node` yolundan (`resolve_wft`) geçer: yeni bir geçiş türü
    /// SOKULMAZ, dolayısıyla varılan node'un `trigger`ları, claim temizliği, aday
    /// cache'i ve görünürlük projeksiyonu her aksiyondaki gibi çalışır.
    async fn admin_move_to(
        &self,
        wfd: &Wfd,
        wfes: &Wfes,
        admin: &Actor,
        action: GlobalAction,
        target_node: &str,
        now: DateTime<Utc>,
    ) -> Result<TransitionCommit, EngineError> {
        if is_terminal_class(&wfes.status) {
            return Err(EngineError::WfeTerminal);
        }
        if self.deadline_due(wfes, now) {
            return Err(EngineError::WfeExpired);
        }
        require_global_action(&wfd.wf_admin, admin, action, wfes, &ValidRules::for_version(wfd), self.org).await?;
        // Ç4-EK/S5: paralel mod SINIRI KALKTI. Eski kapı ("hangi kolun geri gönderen
        // sayılacağı belli değil") artık cevaplı: acting kol SEÇİLMEZ — rastgele bir
        // kolu acting saymak yerine tetikleyici `trigger_kind: "admin"` ile AÇIKÇA
        // yazılır ve TÜM kollar iptal/superseded olur (dışlanan kol yok).
        if !wfd.nodes.contains_key(target_node) {
            return Err(EngineError::TargetInvalid(target_node.to_string()));
        }
        // K-2 ile AYNI küme: uğranmamış bir node'a "geri" göndermek geri gönderme
        // DEĞİL ileri atlamadır (o adımın beklediği ctx alanları hiç yazılmamıştır).
        // Süzgeç WFD içi geri göndermeyle paylaşılır — ayrışsalar adminin yolu,
        // tasarımcının kapattığı kapıyı açardı.
        //
        // `send_to_start` MUAFTIR ve bu bilinçlidir: hedefi zaten `wfd.start[].from`
        // ile sınırlıdır (yukarıda doğrulandı) ve start node'una TANIM GEREĞİ
        // uğranmıştır — akış oradan başladı. `visited_nodes` start node'unu WFAH'ın
        // ilk kaydının aksiyonunu start kurallarıyla eşleştirerek TÜRETİR; eşleşme
        // kaybolduğunda (start aksiyonu yeni bir WFD sürümünde yeniden adlandırıldı,
        // eski örnek eski adı taşıyor) "başa gönder" sessizce imkânsızlaşırdı.
        if action != GlobalAction::SendToStart {
            let visited = visited_nodes(wfd, wfes);
            if !visited.contains(target_node) {
                return Err(EngineError::TargetInvalid(target_node.to_string()));
            }
        }
        // Bulunduğu node'a "geri" göndermek işlemsizdir; tasarım zamanında da yasak
        // (`send_back_target_self`). Sessizce uygulamak claim'i düşüren ama hiçbir şey
        // değiştirmeyen bir kayıt üretirdi.
        if wfes.current_node.as_deref() == Some(target_node) {
            return Err(EngineError::TargetInvalid(target_node.to_string()));
        }

        let mut seq = wfes.wfah.entries().last().map(|e| e.seq + 1).unwrap_or(1);
        let marker = global_action_marker(action);
        let mut wfah_entries = vec![WfahEntry {
            seq,
            action: marker.clone(),
            // GERÇEK admin — `system` DEĞİL (görevlendirme A-2 kabul kriteri).
            actor: admin.clone(),
            input: Some(json!({
                "from": wfes.current_node,
                "to": target_node,
                "global_action": action.as_str(),
            })),
            applied_at: now,
            // Ç2: global aksiyon akışı TAŞIR — bu commit'in hareket satırı budur;
            // from/to outcome çözülünce yazılır. Yol paralel modda çalışmaz.
            from_node: None,
            to_node: None,
            branch_entry: None,
            branch_round: None,
        }];
        seq += 1;

        // Hedefin `c_a`'sı ve `listable` kriterleri `self`/`parent` ÇAPALI olabilir;
        // çözümleme WFE'nin kendi birimine çapalanır — adminin birimine DEĞİL, yoksa
        // müdahale eden kişi akışın adaylarını kaydırırdı. WFAH izi yukarıda gerçek
        // admin ile yazıldı; çapa YALNIZ çözümlemede kullanılır (escalation'daki
        // aynı ayrım).
        let anchored = system_actor_anchored(wfes);
        let wft = Wft::Node {
            node: target_node.to_string(),
        };
        let (outcome, final_ctx, landed) = self
            .resolve_wft(
                &wft,
                wfd,
                wfes.dynctx.as_value().clone(),
                &wfes.wfah,
                &anchored,
                wfes.wfe_id,
                None,
                None,
                WftMode::Single,
                // Admin yolu paralel modda `WftMode::Single` ile çözülür (adminin kolu
                // yoktur) — kol testi burada değil, aşağıdaki collapse dönüşümündedir.
                false,
            )
            .await?;
        // Ç4-EK/S4+S5: paralel modda "kolları toplayıp tek bir node'a inmek" bir
        // COLLAPSE'tır. `MoveTo` bırakılsaydı adapter paralel modu kapatmaz, kol
        // satırları ayakta kalır ve join beklemeye devam ederdi. Adminin kolu
        // olmadığı için `from_node` YOKTUR (`None` = "kol yok").
        let outcome = match (outcome, wfes.join_target.is_some()) {
            (CommitOutcome::MoveTo { node }, true) => CommitOutcome::CollapseTo {
                from_node: None,
                node,
                cause: CollapseCause::SentBack,
            },
            (other, _) => other,
        };
        stamp_movement(&mut wfah_entries[0], &outcome, wfes.current_node.as_deref());

        let wfah = wfes.wfah.extended(&wfah_entries);
        let resolved_c_a = self
            .candidates_at(
                &outcome,
                landed.as_ref(),
                wfd,
                &final_ctx,
                &wfah,
                wfes.origin_orgu_id.unwrap_or(anchored.orgu_id),
                wfes.orgtnt_id,
            )
            .await?;

        stage_parallel_markers(
            wfes,
            &Trigger {
                // Ç4-EK/S5: acting kol YOK; tetikleyici admin olarak yazılır ve
                // `trigger_actor` gerçek admindir (aşağıdaki `actor`).
                kind: TriggerKind::Admin,
                branch: None,
                action: Some(&marker),
                actor: admin,
            },
            &outcome,
            &mut wfah_entries,
            &mut seq,
            now,
        );

        let staged_calls = self.stage_calls(
            wfd,
            landed.as_ref(),
            &final_ctx,
            &anchored,
            wfes.wfe_id,
            now,
        )?;
        guard_written_ctx(wfd, wfes.dynctx.as_value(), &final_ctx)?;

        let claim_recheck = self
            .stage_claim_recheck(wfd, wfes, &outcome, &wfah_entries, &final_ctx, None, now)
            .await?;
        Ok(TransitionCommit {
            claim_recheck,
            wfe_id: wfes.wfe_id,
            orgtnt_id: wfes.orgtnt_id,
            new_dynctx: final_ctx,
            wfah_entries,
            outcome,
            resolved_c_a,
            staged_calls,
            view_c_a: Vec::new(),
            current_view_c_a: Vec::new(),
            branch_c_a: Vec::new(),
            branch_view_c_a: Vec::new(),
            end_view_c_a: Vec::new(),
            end_terminal: end_terminal_of(landed.as_ref()),
        })
    }

    /// `cancel` — WFE'yi iptal eder: terminal-class `terminated` durumuna sokar,
    /// sonrasında hiçbir aksiyon/claim/escalation kabul edilmez.
    ///
    /// **Durum neden `terminated`, yeni bir `cancelled` değil:** `WfeStatus::Terminated`
    /// 2026-07-16 SLA sözleşmesinde "hata değil, başarılı bitiş de değil — ama aktif de
    /// değil" olarak ve açıkça "ileride manuel iptal" için tanımlandı. Ayırt etme
    /// ihtiyacı `end_response.reason` ile karşılanır (`ADMIN.Cancelled`); yeni bir durum
    /// kolonun CHECK kısıtından havuz SQL'ine, görünürlük projeksiyonundan raporlara
    /// kadar her okuyucuyu genişletirdi ve "aktif değil" sınıfına üçüncü bir üye eklerdi.
    ///
    /// **Ardıl akış TETİKLENMEZ** (`staged_calls` boş): iptal başarılı bir bitiş
    /// değildir, `Terminal` değildir — WFC'nin "ardılın üç sert kuralı" aynen geçerli.
    /// Varılmış bir terminal olmadığı için `end_terminal` de NULL kalır.
    pub async fn admin_cancel(
        &self,
        wfd: &Wfd,
        wfes: &Wfes,
        admin: &Actor,
        reason: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<TransitionCommit, EngineError> {
        if is_terminal_class(&wfes.status) {
            return Err(EngineError::WfeTerminal);
        }
        // `cancel` deadline'ı aşmış WFE'de de ÇALIŞIR: SLA süpürücüsü henüz
        // `terminated`a taşımamış olabilir ve o satırı kapatmak tam olarak bu
        // aksiyonun işidir (diğer global aksiyonlar `WfeExpired` ile reddedilir —
        // onlar akışı SÜRDÜRÜR, bu bitirir).
        require_global_action(&wfd.wf_admin, admin, GlobalAction::Cancel, wfes, &ValidRules::for_version(wfd), self.org).await?;

        let mut seq = wfes.wfah.entries().last().map(|e| e.seq + 1).unwrap_or(1);
        let marker = global_action_marker(GlobalAction::Cancel);
        let mut input = json!({ "global_action": "cancel" });
        if let Some(r) = reason {
            input["reason"] = json!(r);
        }
        let mut wfah_entries = vec![WfahEntry {
            seq,
            action: marker.clone(),
            actor: admin.clone(),
            input: Some(input),
            applied_at: now,
            // Ç2: iptal akışı bir node'a TAŞIMAZ (WFE `terminated`) — hareket satırı yok.
            from_node: None,
            to_node: None,
            // Ç4: iptal WFE GENELİDİR, bir kolun içinde değil.
            branch_entry: None,
            branch_round: None,
        }];
        seq += 1;

        let outcome = CommitOutcome::Terminated {
            end_response: match reason {
                Some(r) => json!({"reason": "ADMIN.Cancelled", "note": r}),
                None => json!({"reason": "ADMIN.Cancelled"}),
            },
        };
        // Paralel modda iptal TÜM aktif kolları düşürür (deadline sonlanmasının aynısı).
        stage_parallel_markers(
            wfes,
            &Trigger {
                kind: TriggerKind::Admin,
                branch: None,
                action: Some(&marker),
                actor: admin,
            },
            &outcome,
            &mut wfah_entries,
            &mut seq,
            now,
        );

        Ok(TransitionCommit {
            // E02/S2: iptal WFE'yi terminal sınıfına alır; claim'i hareket düşürür.
            claim_recheck: ClaimRecheck::NotApplicable,
            wfe_id: wfes.wfe_id,
            orgtnt_id: wfes.orgtnt_id,
            // `$ctx` DEĞİŞMEZ — iptal iş verisi yazmaz. Commit yine yeni bir DynCtx
            // revizyonu üretir, immutability korunur.
            new_dynctx: wfes.dynctx.as_value().clone(),
            wfah_entries,
            outcome,
            resolved_c_a: vec![],
            staged_calls: vec![],
            view_c_a: Vec::new(),
            current_view_c_a: Vec::new(),
            branch_c_a: Vec::new(),
            branch_view_c_a: Vec::new(),
            end_view_c_a: Vec::new(),
            end_terminal: None,
        })
    }

    /// Süresi dolan ilk escalation adımının index'i (M6/§8).
    /// `branch`: bkz. `next_escalation`.
    pub fn due_escalation(
        &self,
        wfd: &Wfd,
        wfes: &Wfes,
        now: DateTime<Utc>,
        branch: Option<&str>,
    ) -> Result<Option<usize>, EngineError> {
        Ok(self
            .next_escalation(wfd, wfes, now, branch)?
            .filter(|f| f.overdue)
            .map(|f| f.step_idx))
    }

    /// Vadesi gelen escalation adımını uygular; assigned WFE'de de çalışır,
    /// taşımada assignment temizlenir (store commit'i her MoveTo'da temizler).
    /// SLA-2 akışı BİTİRMEZ (2026-07-28): her adım bir node'a devirdir; `terminated`
    /// yalnız SLA-3 (root `timeout`, bkz. `fire_deadline_timeout`) tarafından üretilir.
    /// `branch`: WOR-31 — paralel modda escalation KOL-bazlı ateşlenir; kol
    /// node'u verilirse adım o kolun node tanımından okunur ve wft çözümü
    /// paralel-farkında yapılır (varış / kol hareketi / WFE-terminal).
    pub async fn fire_escalation(
        &self,
        wfd: &Wfd,
        wfes: &Wfes,
        step_idx: usize,
        now: DateTime<Utc>,
        branch: Option<&str>,
    ) -> Result<TransitionCommit, EngineError> {
        self.fire_escalation_inner(wfd, wfes, step_idx, now, branch, None)
            .await
    }

    /// T‑A5: WF Admin'in ELLE tetiklemesi. Vade gelmiş olması ŞART DEĞİL — erken
    /// tetikleme bu yolun varlık sebebi. Yetki ÇAĞIRANDA denetlenir (executor).
    ///
    /// Otomatik yolla tek farkı WFAH marker'ının AKTÖRÜdür: marker adı aynı kalır
    /// (`escalate:<node>:<idx>`), çünkü yayınlanmış akışlar `count($wfah, #.action ==
    /// "escalate:...")` ile karar veriyor ve elle tetiklemeye ayrı ad vermek o sayımları
    /// bozar. `wfes_effects` bağlamındaki `$actor` ise SYSTEM kalır: effects akışın
    /// VERİ semantiğidir, elle tetikleme onu değiştirmemeli.
    pub async fn fire_escalation_by(
        &self,
        wfd: &Wfd,
        wfes: &Wfes,
        step_idx: usize,
        now: DateTime<Utc>,
        branch: Option<&str>,
        by: &Actor,
    ) -> Result<TransitionCommit, EngineError> {
        self.fire_escalation_inner(wfd, wfes, step_idx, now, branch, Some(by))
            .await
    }

    async fn fire_escalation_inner(
        &self,
        wfd: &Wfd,
        wfes: &Wfes,
        step_idx: usize,
        now: DateTime<Utc>,
        branch: Option<&str>,
        by: Option<&Actor>,
    ) -> Result<TransitionCommit, EngineError> {
        let node_key = match branch {
            Some(b) => {
                if active_branch(wfes, b).is_none() {
                    return Err(EngineError::InvalidWfd(format!(
                        "escalation için aktif kol yok: '{b}'"
                    )));
                }
                b
            }
            None => wfes.current_node.as_deref().ok_or_else(|| {
                EngineError::InvalidWfd("escalation için current_node yok".into())
            })?,
        };
        let step: &EscalationStep = wfd
            .nodes
            .get(node_key)
            .and_then(|n| n.escalation.get(step_idx))
            .ok_or_else(|| {
                EngineError::InvalidWfd(format!("escalation adımı yok: {node_key}[{step_idx}]"))
            })?;

        let system = system_actor();
        let mut staged = wfes.dynctx.as_value().clone();
        if let Some(effects) = &step.wfes_effects {
            let env = EffectEnv {
                env: self.env.public(),
                call: None,
                actor: &system,
                wfe_id: wfes.wfe_id,
                node: Some(node_key),
                action_input: None,
                exec_result: None,
                now,
            };
            staged = apply_effects(&staged, effects, &env)?;
        }

        let seq = wfes.wfah.entries().last().map(|e| e.seq + 1).unwrap_or(1);
        // WOR-63: collapse marker'larına tetikleyici olarak da yazılır.
        let trigger_action = escalation_marker(node_key, step_idx);
        // Ç4/E14: kol escalation'ında satır O KOLDA üretilir.
        let (branch_entry, branch_round) = branch_label(wfes, branch);
        // v2.3 (`Ç9` + `E02`): marker payload'u `{after, grant}` olur — `when` metni
        // deftere AYNEN yazılır (`E13` hükmü: guard'ın ne olduğu audit izinde durur).
        // `collapse` anahtarı ÖLDÜ: escalation collapse edemez, `wft` taşımıyor.
        let mut grant_payload = json!({"c_a": step.grant.c_a});
        if let Some(when) = &step.grant.when {
            grant_payload["when"] = json!(when);
        }
        let wfah_entries = vec![WfahEntry {
            seq,
            action: trigger_action.clone(),
            // Elle tetiklemede iz admini gösterir; otomatik yolda system aktörü.
            actor: by.cloned().unwrap_or_else(|| system.clone()),
            input: Some(json!({"after": step.after, "grant": grant_payload})),
            applied_at: now,
            // Ç2: escalation MARKER satırıdır — akış izi taşımaz (v2.3/C ekseni:
            // escalation node değiştirmeyecek; taşıma yolu `R02` ile düşer).
            from_node: None,
            to_node: None,
            // Ç4: kol escalation'ında satır O KOLDA üretilir.
            branch_entry,
            branch_round,
        }];

        // v2.3 (`Ç9` + `E02`) — **ESCALATION İŞ TAŞIMAZ.**
        //
        // Eski gövde `step.wft`i çözüp `resolve_wft` ile işi başka node'a TAŞIYORDU
        // (paralel modda `BranchMoveTo`/`CollapseTo`, tek-kolda `MoveTo`). Hepsi
        // silindi: kademe artık bir hedef değil bir **yetki kuralı** (`grant`) veriyor.
        //
        // Silinenler ve neden:
        //   - `resolve_wft` çağrısı → çözülecek hedef yok
        //   - `degraded` fallback'i (kol dışı collapse → düz devir) → collapse yok
        //   - `WftMode::Branch`/`Single` ayrımı → hareket yok, kol/tek-kol farkı
        //     yönlendirmeyi değiştirmiyor
        //   - `stage_parallel_markers` → kol hareketi olmadığı için kol marker'ı da yok
        //   - "kol escalation'ı için WFE paralel modda değil" hatası → escalation artık
        //     paralel moddan bağımsız; kol içindeki bir node'un havuzu da genişletilebilir
        //
        // Sonuç: `CommitOutcome::StayAt` — node/status/claim'e dokunulmaz, yalnız
        // marker + ctx + genişlemiş havuz kolonu yazılır.
        let outcome = CommitOutcome::StayAt {
            node: node_key.to_string(),
        };
        let final_ctx = staged;
        let landed: Option<CallSite> = None;

        // Hedefin `c_a`'sı ve `wfd.listable` kriterleri `self`/`parent` ÇAPALI olabilir;
        // saf sistem aktörünün nil orgu'su ile çözülemezler. WFAH marker'ı ve SLA
        // effects'i YUKARIDA saf `system` ile yazıldı — audit izi değişmez; çapa YALNIZ
        // çözümlemede kullanılır.
        let anchored = system_actor_anchored(wfes);

        // ⚠️ Yazılacak DEĞER `E04`ün `node_candidates`ından gelir: `node.c_a ∪ açılmış
        // grantlar`ın ÇÖZÜLMÜŞ aday listesi. Marker YUKARIDA deftere eklendi, bu yüzden
        // grant kümesi POST-APPEND defter üzerinden hesaplanır — yoksa yeni ateşlenen
        // kademenin grant'ı bir commit GEÇ yazılırdı (aynı sınıf hata
        // `view_grants_wfah_anchor` testinde bir kez yaşandı).
        let wfah = wfes.wfah.extended(&wfah_entries);
        let resolved_c_a = self
            .node_candidates(
                node_key,
                wfd,
                &final_ctx,
                &wfah,
                wfes.origin_orgu_id.unwrap_or(anchored.orgu_id),
                wfes.orgtnt_id,
            )
            .await?;

        let staged_calls = self.stage_calls(
            wfd,
            landed.as_ref(),
            &final_ctx,
            &anchored,
            wfes.wfe_id,
            now,
        )?;
        guard_written_ctx(wfd, wfes.dynctx.as_value(), &final_ctx)?;

        // E02/S2: `StayAt` claim'e DOKUNMAZ, dolayısıyla sahibin yetkisi bu commit'in
        // yazdığı marker'la düşmüş olabilir — kademe grant'ının guard'ı defterden
        // besleniyor. Soru tam BURADA sorulur.
        let claim_recheck = self
            .stage_claim_recheck(wfd, wfes, &outcome, &wfah_entries, &final_ctx, branch, now)
            .await?;
        Ok(TransitionCommit {
            claim_recheck,
            wfe_id: wfes.wfe_id,
            orgtnt_id: wfes.orgtnt_id,
            new_dynctx: final_ctx,
            wfah_entries,
            outcome,
            resolved_c_a,
            staged_calls,
            // Görünürlük projeksiyonu saf pipeline'da BOŞ bırakılır: org portuna ve
            // WFE'nin çapasına ihtiyaç duyar, `WfeExecutor::fill_view_grants` doldurur.
            view_c_a: Vec::new(),
            current_view_c_a: Vec::new(),
            branch_c_a: Vec::new(),
            branch_view_c_a: Vec::new(),
            end_view_c_a: Vec::new(),
            end_terminal: end_terminal_of(landed.as_ref()),
        })
    }

    // -------------------------------------------------------- SLA-3 deadline

    /// Instance deadline (SLA-3) aşıldı mı? — `wfe.deadline` kolonundan okunur
    /// (start'ta resolve edilmiş mutlak zaman); her tick'te ISO parse ETMEZ
    /// (eski `root_timeout_due`'nun yerini alır, 2026-07-16).
    pub fn deadline_due(&self, wfes: &Wfes, now: DateTime<Utc>) -> bool {
        if is_terminal_class(&wfes.status) {
            return false;
        }
        matches!(wfes.deadline, Some(d) if now >= d)
    }

    /// Engine-defined SLA sonlanması: WFE `terminated`'a alınır (§5'teki
    /// `Failed`/`error`'dan AYRI — SLA ihlali hata değildir).
    pub fn fire_deadline_timeout(&self, wfes: &Wfes, now: DateTime<Utc>) -> TransitionCommit {
        let mut seq = wfes.wfah.entries().last().map(|e| e.seq + 1).unwrap_or(1);
        let system = system_actor();
        let mut wfah_entries = vec![WfahEntry {
            seq,
            action: "timeout:deadline".into(),
            actor: system.clone(),
            input: Some(json!({"deadline": wfes.deadline})),
            applied_at: now,
            // Ç2: marker satırı — akış bir node'a gitmez, WFE `terminated`.
            from_node: None,
            to_node: None,
            // Ç4: akış süresi WFE GENELİDİR.
            branch_entry: None,
            branch_round: None,
        }];
        seq += 1;
        let outcome = CommitOutcome::Terminated {
            end_response: json!({"reason": "SLA.Deadline"}),
        };
        // WOR-31: paralel modda deadline TÜM aktif kolları iptal eder.
        stage_parallel_markers(
            wfes,
            &Trigger {
                kind: TriggerKind::System,
                branch: None,
                action: Some("timeout:deadline"),
                actor: &system,
            },
            &outcome,
            &mut wfah_entries,
            &mut seq,
            now,
        );
        TransitionCommit {
            // E02/S2: SLA-3 WFE'yi TERMINAL sınıfına alır; claim'i hareketin kendisi
            // düşürür (`clears_claim()`), sorulacak bir yetki kalmaz. Fonksiyon ayrıca
            // SENKRONdur — org portuna gitmeden verilebilecek TEK doğru cevap budur.
            claim_recheck: ClaimRecheck::NotApplicable,
            wfe_id: wfes.wfe_id,
            orgtnt_id: wfes.orgtnt_id,
            new_dynctx: wfes.dynctx.as_value().clone(),
            wfah_entries,
            outcome,
            resolved_c_a: vec![],
            // SLA-3 sonlanması `Terminated`'dır — BAŞARILI bitiş değildir, bu yüzden
            // ardıl akış TETİKLENMEZ (bkz. decisions.md → WFC, "ardılın üç sert kuralı").
            staged_calls: vec![],
            // Görünürlük projeksiyonu saf pipeline'da BOŞ bırakılır: org portuna ve
            // WFE'nin çapasına ihtiyaç duyar, `WfeExecutor::fill_view_grants` doldurur.
            view_c_a: Vec::new(),
            current_view_c_a: Vec::new(),
            branch_c_a: Vec::new(),
            branch_view_c_a: Vec::new(),
            end_view_c_a: Vec::new(),
            // SLA-3 `Terminated` — varılmış bir terminal YOK, dolayısıyla
            // terminal `listable[]`ı da yok (bkz. `Terminal.listable` yorumu).
            end_terminal: None,
        }
    }

    // ----------------------------------------------------- SLA-1 claim timeout

    /// Claim timeout (SLA-1) süresi doldu mu? — node'un `claim_timeout.after`'ı
    /// `wfes.claimed_at`'tan itibaren ölçülür; unassigned/terminal-class/
    /// claim_timeout tanımsız node'da her zaman false.
    /// `branch`: WOR-31 — paralel modda claim KOL-bazlıdır; kol node'u verilirse
    /// sayaç `BranchState.claimed_at`'tan ölçülür.
    pub fn claim_timeout_due(
        &self,
        wfd: &Wfd,
        wfes: &Wfes,
        now: DateTime<Utc>,
        branch: Option<&str>,
    ) -> Result<bool, EngineError> {
        if is_terminal_class(&wfes.status) {
            return Ok(false);
        }
        let (node_key, claimed_at) = match branch {
            Some(b) => match active_branch(wfes, b).and_then(|bs| bs.claimed_at) {
                Some(c) => (b, c),
                None => return Ok(false),
            },
            None => {
                let Some(claimed_at) = wfes.claimed_at else {
                    return Ok(false);
                };
                let Some(node_key) = wfes.current_node.as_deref() else {
                    return Ok(false);
                };
                (node_key, claimed_at)
            }
        };
        let Some(node) = wfd.nodes.get(node_key) else {
            return Ok(false);
        };
        let Some(ct) = &node.claim_timeout else {
            return Ok(false);
        };
        Ok(now >= claimed_at + parse_iso8601_duration(&ct.after)?)
    }

    /// Vadesi gelen claim timeout'u uygular. `wft` verilmişse escalation fire
    /// benzeri node taşıması (assignment zaten commit'te temizlenir);
    /// verilmemişse yalnızca claimed_by/claimed_at CAS ile temizlenir (sayaç
    /// sıfırlanır, node DEĞİŞMEZ) — `WfeStore::release_claim` ile persist edilir.
    /// 2026-07-28: `ct.wfes_effects` varsa her iki yolda da STAGED DynCtx'e uygulanır
    /// (`$actor` = system, `$node` = SLA'nın tetiklendiği node).
    /// `branch`: WOR-31 — paralel modda kol node'u verilir; Release yolu kolun
    /// claim'inin sıfırlanmasını temsil eder (persist T3'te kol-farkında),
    /// Move yolu paralel-farkında wft çözümünden geçer.
    pub async fn fire_claim_timeout(
        &self,
        wfd: &Wfd,
        wfes: &Wfes,
        now: DateTime<Utc>,
        branch: Option<&str>,
    ) -> Result<ClaimTimeoutOutcome, EngineError> {
        let node_key = match branch {
            Some(b) => {
                if active_branch(wfes, b).is_none() {
                    return Err(EngineError::InvalidWfd(format!(
                        "claim timeout için aktif kol yok: '{b}'"
                    )));
                }
                b
            }
            None => wfes.current_node.as_deref().ok_or_else(|| {
                EngineError::InvalidWfd("claim timeout için current_node yok".into())
            })?,
        };
        let node = wfd
            .nodes
            .get(node_key)
            .ok_or_else(|| EngineError::InvalidWfd(format!("bilinmeyen node '{node_key}'")))?;
        let ct = node.claim_timeout.as_ref().ok_or_else(|| {
            EngineError::InvalidWfd(format!("node '{node_key}' claim_timeout taşımıyor"))
        })?;
        let system = system_actor();
        let seq = wfes.wfah.entries().last().map(|e| e.seq + 1).unwrap_or(1);
        // Ç4/E14: kolun claim'i düşüyorsa satır O KOLDA — iki dal da aynı etiketi taşır.
        let (branch_entry, branch_round) = branch_label(wfes, branch);
        // Ç1-EK: marker adı `claim_timeout:` DEĞİL `claim_released:`. İki olay tek
        // olaydır (bir claim düşer, iş havuza döner); farkları SEBEPTİR ve sebep
        // payload'daki `reason` alanında taşınır. `claim_timeout:` adı SLA-1 dışı
        // bırakma sebeplerinde (Ç9 grant guard) YALAN söylüyordu.
        //
        // WFD tarafındaki ayar bloğunun adı (`nodes.<k>.claim_timeout`) DEĞİŞMEZ: o blok
        // bir ZAMANLAYICI tarif eder, bırakmayı değil.
        //
        // E12: satır sahiplik ailesinin ORTAK şeklini taşır — `owner` (sahipliğin
        // öznesi; `actor` sistemdir, düşüren o), `held_for_seconds` ve paralel modda
        // kol alanları. `owner` YOKSA düşecek bir sahiplik de yoktur.
        let (owner, claimed_at) = match branch {
            Some(b) => match active_branch(wfes, b) {
                Some(bs) => (bs.claimed_by, bs.claimed_at),
                None => (None, None),
            },
            None => (wfes.assigned_to, wfes.claimed_at),
        };
        let released_input = |after: &str| -> Value {
            match owner {
                Some(o) => ClaimReleased::timeout(node_key, o, after)
                    .held(claimed_at.map(|c| seconds_between(c, now)))
                    .in_branch(branch.map(|b| OwnershipBranch {
                        entry: branch_entry.as_deref().unwrap_or(b),
                        at_node: b,
                    }))
                    .input(),
                // Sahipsiz node'da SLA-1 ateşlenmez (`claim_timeout_due` `claimed_at`
                // ister); yine de düşerse `owner` UYDURULMAZ.
                None => json!({ "reason": "timeout", "after": after }),
            }
        };
        let marker = format!("claim_released:{node_key}");

        // SLA-1 effects (2026-07-28): varsa DynCtx'e uygulanır; yoksa staged ctx
        // aynen kalır ve Release yolu ctx satırı YAZMAZ (`new_dynctx: None`).
        let mut staged = wfes.dynctx.as_value().clone();
        let has_effects = ct.wfes_effects.is_some();
        if let Some(effects) = &ct.wfes_effects {
            let env = EffectEnv {
                env: self.env.public(),
                call: None,
                actor: &system,
                wfe_id: wfes.wfe_id,
                node: Some(node_key),
                action_input: None,
                exec_result: None,
                now,
            };
            staged = apply_effects(&staged, effects, &env)?;
        }

        // v2.3 (K13 + K19): **CLAIM TIMEOUT ARTIK İŞ TAŞIMAZ.** `ClaimTimeout`tan `wft` ve
        // `collapses_parallel` KALKTI; süre dolduğunda yapılan tek şey claim'i BIRAKMAK.
        //
        // Eski `Some(target)` dalı (devir + opsiyonel collapse) tamamen SİLİNDİ:
        //   - devir hedefi yok → `CommitOutcome::MoveTo` üretilmiyor
        //   - `collapses_parallel` yok → kardeş kol iptali / paralel kapanışı yok
        // Zamanlayıcı yönlendirme kararı VERMEZ; iş node'da kalır ve havuza döner.
        let wfah_entry = WfahEntry {
            seq,
            action: marker,
            actor: system,
            // Ç1-EK/E12 payload'ı: `reason` kapalı listedir (Rust enum);
            // `after` YALNIZ `reason: "timeout"` satırlarında yazılır.
            input: Some(released_input(&ct.after)),
            applied_at: now,
            // Ç2: yalnız claim düşer, node DEĞİŞMEZ — marker satırı.
            from_node: None,
            to_node: None,
            // Ç4: kolun claim'i düşüyorsa satır O KOLDA.
            branch_entry,
            branch_round,
        };
        Ok(ClaimTimeoutOutcome::Release(ClaimRelease {
            wfah_entry,
            new_dynctx: has_effects.then_some(staged),
        }))
    }

    // ------------------------------------------------------------- internals

    /// §7.7 — trigger zinciri: when → timeout'lu execute → retry → catch.
    /// Başarılı autoexec'in wfes_effects'i STAGED; catch effects STAGED.
    /// Unhandled + required → hata (hiçbir şey commit edilmez).
    #[allow(clippy::too_many_arguments)]
    async fn run_triggers(
        &self,
        triggers: &[TriggerInvocation],
        wfd: &Wfd,
        staged: &mut Value,
        wfah_entries: &mut Vec<WfahEntry>,
        seq: &mut u32,
        actor: &Actor,
        wfe_id: Uuid,
        node: Option<&str>,
        // Ç4: trigger bir KOL bağlamında koştuysa kolun kimliği (`entry_node`) —
        // satır o kolda üretilmiştir. Tek-kol/start yolunda `None`.
        branch_entry: Option<&str>,
        action_input: Option<&Value>,
        wfah: &Wfah,
        _orgtnt_id: Uuid,
    ) -> Result<(), EngineError> {
        for trig in triggers {
            // when guard — staged ctx üzerinden
            if let Some(when) = &trig.when {
                let mut env = EvalEnv::new(staged)
                    .with_wfah(wfah, &ValidRules::for_version(wfd))
                    .with_node(node)
                    .with_actor(actor)
                    .with_wfe_id(wfe_id);
                if let Some(input) = action_input {
                    env = env.with_action_input(input);
                }
                if !evaluate_bool(when, &env)? {
                    continue;
                }
            }

            let def = wfd.autoexec.get(&trig.use_).ok_or_else(|| {
                EngineError::InvalidWfd(format!("autoexec '{}' tanımsız", trig.use_))
            })?;

            let system = Actor {
                role: "system".into(),
                ..actor.clone()
            };
            match self
                .execute_with_retry(
                    def,
                    trig,
                    staged,
                    wfe_id,
                    node,
                    &system,
                    wfah,
                    action_input,
                    &ValidRules::for_version(wfd),
                )
                .await
            {
                Ok(result) => {
                    if let Some(effects) = &def.wfes_effects {
                        let env = EffectEnv {
                            env: self.env.public(),
                            call: None,
                            actor: &system,
                            wfe_id,
                            node,
                            action_input,
                            exec_result: Some(&result),
                            now: Utc::now(),
                        };
                        *staged = apply_effects(staged, effects, &env)?;
                    }
                    wfah_entries.push(WfahEntry {
                        seq: *seq,
                        action: format!("trigger:{}", trig.use_),
                        actor: system,
                        input: Some(json!({"result": result})),
                        applied_at: Utc::now(),
                        // Ç2: trigger MARKER satırıdır — hareketi aynı commit'teki
                        // aksiyon satırı taşır.
                        from_node: None,
                        to_node: None,
                        branch_entry: branch_entry.map(str::to_string),
                        branch_round: branch_round_of(wfah, branch_entry),
                    });
                    *seq += 1;
                }
                Err(failure) => {
                    // catch?
                    let caught = trig.catch.as_ref().filter(|c| {
                        c.error_equals
                            .iter()
                            .any(|e| e == "WFD.ALL" || e == &failure.error)
                    });
                    if let Some(catch) = caught {
                        let env = EffectEnv {
                            env: self.env.public(),
                            call: None,
                            actor: &system,
                            wfe_id,
                            node,
                            action_input,
                            exec_result: None,
                            now: Utc::now(),
                        };
                        *staged = apply_effects(staged, &catch.wfes_effects, &env)?;
                        wfah_entries.push(WfahEntry {
                            seq: *seq,
                            action: format!("trigger:{}", trig.use_),
                            actor: system,
                            input: Some(json!({
                                "error": failure.error,
                                "message": failure.message,
                                "handled": true,
                            })),
                            applied_at: Utc::now(),
                            from_node: None,
                            to_node: None,
                            branch_entry: branch_entry.map(str::to_string),
                            branch_round: branch_round_of(wfah, branch_entry),
                        });
                        *seq += 1;
                        continue; // handled — devam (routing YOK)
                    }
                    if trig.required {
                        return Err(EngineError::Autoexec(format!(
                            "{}: {} ({})",
                            trig.use_, failure.error, failure.message
                        )));
                    }
                    // required=false → atla, kayıt düş
                    wfah_entries.push(WfahEntry {
                        seq: *seq,
                        action: format!("trigger:{}", trig.use_),
                        actor: system,
                        input: Some(json!({
                            "error": failure.error,
                            "message": failure.message,
                            "handled": false,
                            "required": false,
                        })),
                        applied_at: Utc::now(),
                        from_node: None,
                        to_node: None,
                        branch_entry: branch_entry.map(str::to_string),
                        branch_round: branch_round_of(wfah, branch_entry),
                    });
                    *seq += 1;
                }
            }
        }
        Ok(())
    }

    /// Timeout'lu tek çalıştırma + ASL retry döngüsü.
    /// Bekleme = interval * backoff^attempt, max_delay ile kırpılır (§7.7).
    async fn execute_with_retry(
        &self,
        def: &AutoexecDef,
        trig: &TriggerInvocation,
        staged: &Value,
        wfe_id: Uuid,
        node: Option<&str>,
        system: &Actor,
        wfah: &Wfah,
        action_input: Option<&Value>,
        valid_rules: &ValidRules,
    ) -> Result<Value, ExecFailure> {
        let env = ExecEnv {
            env: self.env.clone(),
            valid_rules: valid_rules.clone(),
            wfe_id,
            ctx: staged.clone(),
            node: node.map(String::from),
            actor: system.clone(),
            // WOR-84: `calc` ifadeleri geçmişi ve ACT girdisini görür. Kapsam trigger'ın
            // `when` guard'ıyla aynı — bkz. ExecEnv::wfah.
            wfah: wfah.clone(),
            action_input: action_input.cloned(),
        };
        let mut attempts_per_retrier: Vec<u32> = vec![0; trig.retry.len()];

        loop {
            let run = self.exec.run(def, &env);
            let outcome = tokio::time::timeout(
                std::time::Duration::from_secs(def.timeout_seconds as u64),
                run,
            )
            .await;
            let failure = match outcome {
                Ok(Ok(result)) => return Ok(result),
                Ok(Err(f)) => f,
                Err(_) => ExecFailure::timeout(),
            };

            // eşleşen ilk retrier
            let matching = trig.retry.iter().enumerate().find(|(_, r)| {
                r.error_equals
                    .iter()
                    .any(|e| e == "WFD.ALL" || e == &failure.error)
            });
            let Some((idx, retrier)) = matching else {
                return Err(failure);
            };
            // ASL semantiği: max_attempts = yeniden deneme sayısı (ilk çağrı hariç)
            let attempt = attempts_per_retrier[idx];
            if attempt >= retrier.max_attempts {
                return Err(failure);
            }
            attempts_per_retrier[idx] = attempt + 1;

            let delay = retrier.interval_seconds as f64 * retrier.backoff_rate.powi(attempt as i32);
            let delay = match retrier.max_delay_seconds {
                Some(max) => delay.min(max as f64),
                None => delay,
            };
            tokio::time::sleep(std::time::Duration::from_secs_f64(delay.max(0.0))).await;
        }
    }

    // ------------------------------------------------------------ WFC-RETURN

    /// WFC-RETURN: çağrılan WFE bitti, çağıran o node'daki `call.wft`'ye göre ilerler.
    ///
    /// `fire_escalation` / `fire_claim_timeout` ile AYNI sınıftır: system aktörü, insan
    /// ACT'i olmayan bir kenar. Farkı, bağlamda `$call.*` namespace'inin bağlı olması.
    ///
    /// `outcome`: çağrılanın nasıl bittiği — "completed" | "failed" | "terminated" |
    /// "timeout". `end_response` yalnız `completed`'da doludur.
    pub async fn fire_call_return(
        &self,
        wfd: &Wfd,
        wfes: &Wfes,
        call_status: &str,
        callee_wfe_id: Option<Uuid>,
        end_response: Option<&Value>,
        // Çağrılanın kendi WFAH'ı — çağıranın geçmişine SATIR SATIR işlenir (aşağıya bkz.).
        // Simülasyonda gerçek bir çağrılan yoktur; orada boş dilim geçilir.
        callee_wfah: &[WfahEntry],
        now: DateTime<Utc>,
    ) -> Result<TransitionCommit, EngineError> {
        let node_key = wfes
            .current_node
            .as_deref()
            .ok_or_else(|| EngineError::InvalidWfd("çağrı dönüşü için current_node yok".into()))?;
        let call_ref = wfd
            .nodes
            .get(node_key)
            .and_then(|n| n.call.as_ref())
            .ok_or_else(|| {
                EngineError::InvalidWfd(format!("'{node_key}' bir çağrı node'u değil"))
            })?;

        let call = CallOutcome {
            result: end_response.cloned().unwrap_or(Value::Null),
            status: call_status.to_string(),
            wfe_id: callee_wfe_id,
        };
        let system = system_actor();

        let mut staged = wfes.dynctx.as_value().clone();
        if let Some(effects) = &call_ref.wfes_effects {
            let env = EffectEnv {
                env: self.env.public(),
                actor: &system,
                wfe_id: wfes.wfe_id,
                node: Some(node_key),
                // WFC-RETURN'ü system tetikler: aksiyon girdisi ve autoexec sonucu
                // YOKTUR (validator `call_effect_namespace`). Görünen tek yeni
                // namespace `$call.*`.
                action_input: None,
                exec_result: None,
                call: Some(&call),
                now,
            };
            staged = apply_effects(&staged, effects, &env)?;
        }

        let mut seq = wfes.wfah.entries().last().map(|e| e.seq + 1).unwrap_or(1);
        let marker = format!("call:{}", call_ref.use_);
        let mut wfah_entries: Vec<WfahEntry> = Vec::new();

        // --- Çağrılanın geçmişi çağıranın geçmişine işlenir ---
        //
        // Neden burada ve neden kapanış marker'ından ÖNCE: çağıran, çağrı node'una
        // girdiği andan dönüşe kadar WFAH'ına HİÇBİR kayıt yazamaz — çağrı node'undan
        // insan aksiyonu alınamaz (`call_node_has_action`) ve escalation/claim_timeout/
        // reassign o node'da yasaktır (`call_node_forbidden_field`). Dolayısıyla
        // çağrılanın tüm satırları, zaman olarak tam bu aralığa düşer. Onları burada
        // sırayla eklemek `seq` artışını `applied_at` artışıyla UYUMLU tutar; sona
        // eklemek ise kapanış marker'ından sonra daha ESKİ zaman damgaları üretir,
        // yani tarihsel akışı bozardı.
        //
        // Korunanlar: özgün `actor` (işi kim yaptı — denetimin asıl değeri) ve özgün
        // `applied_at`. Değişen tek şey `action` adı: `call:<anahtar>/<aksiyon>` olarak
        // ad-alanına alınır. Ham adla eklemek, çağıranın `$wfah` ifadelerinde
        // (`some($wfah, #.action == '...')`) KAZARA eşleşmelere yol açardı — alt akışın
        // aksiyonu çağıranınkiyle aynı ada sahip olabilir.
        let inlined = callee_wfah.len().min(MAX_INLINED_CALL_ENTRIES);
        for entry in &callee_wfah[..inlined] {
            wfah_entries.push(WfahEntry {
                seq,
                action: format!("{marker}/{}", entry.action),
                actor: entry.actor.clone(),
                input: Some(json!({
                    "callee_wfe_id": callee_wfe_id,
                    "callee_seq": entry.seq,
                    "action": entry.action,
                    "input": entry.input,
                })),
                applied_at: entry.applied_at,
                // Ç2: ÇAĞRILANIN akış izi çağıranın node'larına ait değildir —
                // ad-alanına alınmış bir kopyadır, hareket taşımaz.
                from_node: None,
                to_node: None,
                // Ç4: çağrı node'u paralel modda olamaz (validator).
                branch_entry: None,
                branch_round: None,
            });
            seq += 1;
        }
        // Kırpma SESSİZ olmaz: kaç satırın atlandığı ve tam geçmişin hangi WFE'de
        // olduğu kayda geçer. Sınır, çağıranın WFAH'ının (her `load`'da tümüyle
        // okunur) uzun alt akışlarla şişmesini engeller.
        if callee_wfah.len() > inlined {
            wfah_entries.push(WfahEntry {
                seq,
                action: format!("{marker}/…"),
                actor: system.clone(),
                input: Some(json!({
                    "callee_wfe_id": callee_wfe_id,
                    "omitted": callee_wfah.len() - inlined,
                    "reason": "call_history_truncated",
                })),
                applied_at: callee_wfah[inlined.saturating_sub(1)].applied_at,
                from_node: None,
                to_node: None,
                branch_entry: None,
                branch_round: None,
            });
            seq += 1;
        }

        // Kapanış marker'ı — dönüşün İŞLENDİĞİ an.
        wfah_entries.push(WfahEntry {
            seq,
            action: marker.clone(),
            actor: system.clone(),
            input: Some(json!({
                "status": call_status,
                "callee_wfe_id": callee_wfe_id,
            })),
            applied_at: now,
            // Ç2: dönüş MARKER satırıdır.
            from_node: None,
            to_node: None,
            branch_entry: None,
            branch_round: None,
        });
        seq += 1;

        let wft = call_ref.wft.as_ref().ok_or_else(|| {
            EngineError::InvalidWfd(format!("'{node_key}' çağrı node'u wft içermeli"))
        })?;

        // WFC node'u paralel modda olamaz (validator: kol giriş node'u olarak çağrı
        // node'u Faz 2 kapsamı dışı) — tekil mod yeterlidir.
        // Hedef node'un `c_orgu`'su `self`-çapalı olabilir; saf sistem aktörünün nil
        // orgu'su ile çözülemez (bkz. `system_actor_anchored`). WFAH marker'ı ve
        // çağrı effects'i YUKARIDA saf `system` ile yazıldı — audit izi değişmez.
        let anchored = system_actor_anchored(wfes);
        let (outcome, final_ctx, landed) = self
            .resolve_wft(
                wft,
                wfd,
                staged,
                &wfes.wfah,
                &anchored,
                wfes.wfe_id,
                None,
                Some(&call),
                WftMode::Single,
                // WFC dönüşü ileri bir harekettir.
                false,
            )
            .await?;

        // Çağrı dönüş marker'ı işlendi — varılan yeri kim yapabilir?
        let wfah = wfes.wfah.extended(&wfah_entries);
        let resolved_c_a = self
            .candidates_at(
                &outcome,
                landed.as_ref(),
                wfd,
                &final_ctx,
                &wfah,
                // Çapa WFE'nin kendi birimi; işlemi yapan kişiyle DEĞİŞMEZ.
                wfes.origin_orgu_id.unwrap_or(anchored.orgu_id),
                wfes.orgtnt_id,
            )
            .await?;

        stage_parallel_markers(
            wfes,
            &Trigger {
                kind: TriggerKind::System,
                branch: None,
                action: Some(&marker),
                actor: &system,
            },
            &outcome,
            &mut wfah_entries,
            &mut seq,
            now,
        );

        let staged_calls = self.stage_calls(
            wfd,
            landed.as_ref(),
            &final_ctx,
            &anchored,
            wfes.wfe_id,
            now,
        )?;
        // Kapı B — WFC dönüşü: çağrılan akışın `wfe_end_response`'u `$call.result.*` ile
        // ctx'e yazılır. Dış bir akışın döndürdüğü şekil bizim şemamıza uymak zorundadır.
        guard_written_ctx(wfd, wfes.dynctx.as_value(), &final_ctx)?;

        // E02/S2: WFC dönüşünde kol bağlamı YOK — çağrı node'unda claim de yoktur
        // (`can_claim` `CallInProgress` der). Soru yine TEK gövdeden geçer.
        let claim_recheck = self
            .stage_claim_recheck(wfd, wfes, &outcome, &wfah_entries, &final_ctx, None, now)
            .await?;
        Ok(TransitionCommit {
            claim_recheck,
            wfe_id: wfes.wfe_id,
            orgtnt_id: wfes.orgtnt_id,
            new_dynctx: final_ctx,
            wfah_entries,
            outcome,
            resolved_c_a,
            staged_calls,
            // Görünürlük projeksiyonu saf pipeline'da BOŞ bırakılır: org portuna ve
            // WFE'nin çapasına ihtiyaç duyar, `WfeExecutor::fill_view_grants` doldurur.
            view_c_a: Vec::new(),
            current_view_c_a: Vec::new(),
            branch_c_a: Vec::new(),
            branch_view_c_a: Vec::new(),
            end_view_c_a: Vec::new(),
            end_terminal: end_terminal_of(landed.as_ref()),
        })
    }

    // ---------------------------------------------------------------- WFC outbox

    /// Varılan siteye bakıp bu commit ile aynı tx'te kuyruğa alınacak WFC çağrılarını
    /// üretir (§WFC). Boş vektör = burada çağrı yok.
    ///
    /// Neden commit'in İÇİNDE değil de outbox: çağrılan WFE'yi burada yaratmak,
    /// çağıranın atomik transaction'ını başka bir WFE'nin tüm start pipeline'ına
    /// bağlardı. Niyet aynı tx'te kalıcı olur, gerçek start ayrı tx'te koşar.
    fn stage_calls(
        &self,
        wfd: &Wfd,
        landed: Option<&CallSite>,
        ctx: &Value,
        actor: &Actor,
        wfe_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<Vec<StagedCall>, EngineError> {
        let Some(site) = landed else {
            return Ok(Vec::new());
        };
        // Site → referans. Node yerleşimi `wait`/`detached`, terminal yerleşimi
        // `terminal` taşır (validator `call_mode_placement` bunu garanti eder; runtime
        // yine de kontrol eder ki bozuk bir WFD sessizce yanlış davranmasın).
        let call_ref = match site {
            CallSite::Node(key) => wfd.nodes.get(key).and_then(|n| n.call.as_ref()),
            CallSite::Terminal(id) => wfd
                .terminals
                .iter()
                .find(|t| t.id == *id)
                .and_then(|t| t.call.as_ref()),
        };
        let Some(call_ref) = call_ref else {
            return Ok(Vec::new());
        };
        let placement_ok = match site {
            CallSite::Node(_) => call_ref.mode.is_node_site(),
            CallSite::Terminal(_) => !call_ref.mode.is_node_site(),
        };
        if !placement_ok {
            return Err(EngineError::InvalidWfd(format!(
                "WFC modu '{}' bu yerleşimde geçerli değil ({} '{}')",
                call_ref.mode.as_str(),
                site.kind(),
                site.key()
            )));
        }

        let def = wfd.calls.get(&call_ref.use_).ok_or_else(|| {
            EngineError::CallNotFound(format!("'{}' calls katalogunda yok", call_ref.use_))
        })?;

        // WFC-IN çözümü: `$ctx.*` / `$actor` / `$timestamp` / `$wfe_id` / literal.
        // `action_input`/`exec_result`/`call` BİLİNÇLİ olarak `None` — bu namespace'ler
        // çağrı girdisinde yasaktır (validator `call_input_namespace`); burada `None`
        // olması onları sessizce `null` yapar, yani bozuk bir WFD veri uydurmaz.
        let env = EffectEnv {
            env: self.env.public(),
            actor,
            wfe_id,
            node: match site {
                CallSite::Node(key) => Some(key.as_str()),
                CallSite::Terminal(_) => None,
            },
            action_input: None,
            exec_result: None,
            call: None,
            now,
        };
        let mut input = Map::new();
        for (key, raw) in &def.input {
            input.insert(key.clone(), resolve_value(raw, ctx, &env)?);
        }

        // `wait` süre sınırı mutlak zamana çevrilir — her tick'te ISO parse etmemek
        // için (SLA-3 deadline'ıyla aynı gerekçe).
        //
        // YALNIZ `wait`: `detached` çağrılanın sonucunu hiç beklemez, `terminal`'de ise
        // dönüş yoktur — ikisinde de bir süre sınırı uygulanacak bekleme YOK. (Validator
        // bunu `call_node_forbidden_field` ile zaten reddediyor; runtime da uydurmuyor.)
        let deadline = match (&call_ref.timeout, call_ref.mode) {
            (Some(iso), CallMode::Wait) => Some(now + parse_iso8601_duration(iso)?),
            _ => None,
        };

        Ok(vec![StagedCall {
            call_key: call_ref.use_.clone(),
            mode: call_ref.mode,
            site: site.clone(),
            input: Value::Object(input),
            deadline,
            start_as: call_ref.start_as.unwrap_or(StartAs::Actor),
            max_next: call_ref.max_next,
        }])
    }

    /// §7.8 — WFT çözümü. Terminal'de terminal.wfes_effects uygulanır ve
    /// wfe_end_response $-string'leri FINAL staged ctx ile çözülür (M9/WOR-42).
    /// `mode`: WOR-31 — Parallel hedefin ve paralel kol bağlamının sınıflaması
    /// (bkz. `WftMode`).
    ///
    /// Üçüncü dönüş değeri "nereye varıldı" (`CallSite`): `CommitOutcome::Terminal`
    /// terminal id'sini TAŞIMAZ, ama WFC outbox'ı ardıl çağrıyı bulmak için ona ihtiyaç
    /// duyar — bu yüzden ayrıca döner.
    ///
    /// YALNIZ "nereye" sorusunu cevaplar. "Varılan yeri kim yapabilir" AYRI bir adımdır
    /// (`candidates_at`) ve aksiyon deftere işlendikten SONRA sorulur — bkz. oradaki
    /// yorum. Bu yüzden buradaki `wfah` daima aksiyon ÖNCESİ geçmiştir: koşullar
    /// ($prev, `count($wfah, ...)`) uygulanmakta olan aksiyonu saymamalıdır.
    #[allow(clippy::too_many_arguments)]
    async fn resolve_wft(
        &self,
        wft: &Wft,
        wfd: &Wfd,
        staged: Value,
        wfah: &Wfah,
        actor: &Actor,
        wfe_id: Uuid,
        action_input: Option<&Value>,
        // WFC-RETURN bağlamı — `wft.conditions` ve terminal `wfe_end_response` içinde
        // `$call.*` görünür olsun. Diğer yollarda `None`.
        call: Option<&CallOutcome>,
        mode: WftMode<'_>,
        // Ç4-EK/S4: bu çözüm bir GERİ GÖNDERME mi (`Wft::SendBack` menüsünden
        // seçilmiş hedef ya da admin `send_back`/`send_to_start`). `select_wft`
        // menüyü `Wft::Node`'a indirdiği için buradan görülemez; kol bağlamında
        // fork alt-grafı testini YALNIZ bu yol tetikler — escalation / claim
        // timeout / WFC dönüşü davranış değiştirmez.
        send_back: bool,
    ) -> Result<(CommitOutcome, Value, Option<CallSite>), EngineError> {
        // WOR-56: collapse — yalnız kol bağlamında. Kardeşleri düşürüp WFE'yi
        // hedefe götürür. Terminal hedef = mevcut Terminal yolu (paralel modda
        // stage_parallel_markers zaten kardeşleri iptal eder). Node hedef =
        // yeni CollapseTo (paralel mod biter, current_node = node).
        if let Wft::Collapse { collapse } = wft {
            let from_node = match mode {
                WftMode::Branch { from_node, .. } => from_node.to_string(),
                _ => {
                    return Err(EngineError::InvalidWfd(
                        "collapse wft yalnızca paralel dal içinde geçerli (WOR-56)".into(),
                    ))
                }
            };
            return match collapse {
                WftTarget::Terminal { terminal } => {
                    let (end_response, final_ctx) = self.terminal_outcome(
                        terminal,
                        wfd,
                        staged,
                        actor,
                        wfe_id,
                        action_input,
                        call,
                    )?;
                    Ok((
                        CommitOutcome::Terminal { end_response },
                        final_ctx,
                        Some(CallSite::Terminal(terminal.clone())),
                    ))
                }
                WftTarget::Node { node } => Ok((
                    CommitOutcome::CollapseTo {
                        from_node: Some(from_node),
                        node: node.clone(),
                        cause: CollapseCause::Collapse,
                    },
                    staged,
                    Some(CallSite::Node(node.clone())),
                )),
            };
        }
        let target = match wft {
            Wft::Node { node } => Target::Node(node.clone()),
            Wft::Terminal { terminal } => Target::Terminal(terminal.clone()),
            // Geri gönderme menüsü buraya HİÇ ulaşmamalı: `select_wft` seçimi apply'ın
            // başında `Wft::Node`'a indirger. Ulaşıyorsa hedef seçimi olmayan bir
            // yerde (start / escalation / çağrı dönüşü) menü yazılmış demektir —
            // validator `send_back_wft_placement` ile bunu yayından önce keser.
            Wft::SendBack { .. } => {
                return Err(EngineError::InvalidWfd(
                    "geri gönderme menüsü (`wft: {targets}`) yalnız transitions[].wft içinde kullanılabilir".into(),
                ))
            }
            // WOR-31: fork — yalnız tekil modda geçerli. Start'ta ve paralel
            // modda (nested) validator zaten reddeder; runtime yine de korunur.
            Wft::Parallel { parallel } => {
                return match mode {
                    WftMode::Start => Err(EngineError::InvalidWfd(
                        "start wft'i parallel olamaz (WOR-31)".into(),
                    )),
                    WftMode::Branch { .. } => Err(EngineError::InvalidWfd(
                        "nested parallel çalıştırılamaz (WOR-31)".into(),
                    )),
                    WftMode::Single => Ok((
                        CommitOutcome::ForkTo {
                            branches: parallel.branches.clone(),
                            join: parallel.join.clone(),
                            // WOR-72/WOR-73: mod + eşik/ifade burada TEK çözülmüş
                            // kurala indirgenir; runtime bundan sonra yalnız onu taşır.
                            join_rule: parallel.join_rule(),
                        },
                        staged,
                        // Fork BİRDEN FAZLA node'a girer; WFC outbox tek site
                        // taşır. Bir kol giriş node'unun çağrı node'u olması
                        // Faz 2 kapsamı dışıdır (validator ileride yasaklar).
                        None,
                    )),
                };
            }
            Wft::Conditional {
                conditions,
                default,
            } => {
                let mut chosen = None;
                for cond in conditions {
                    let mut env = EvalEnv::new(&staged)
                        .with_wfah(wfah, &ValidRules::for_version(wfd))
                        .with_actor(actor)
                        .with_wfe_id(wfe_id);
                    if let Some(input) = action_input {
                        env = env.with_action_input(input);
                    }
                    if let Some(c) = call {
                        env = env.with_call(c.clone());
                    }
                    if evaluate_bool(&cond.when, &env)? {
                        chosen = Some(match (&cond.node, &cond.terminal) {
                            (Some(n), None) => Target::Node(n.clone()),
                            (None, Some(t)) => Target::Terminal(t.clone()),
                            _ => {
                                return Err(EngineError::InvalidWfd(
                                    "wft condition tam olarak bir hedef içermeli".into(),
                                ))
                            }
                        });
                        break;
                    }
                }
                match (chosen, default) {
                    (Some(t), _) => t,
                    (None, Some(WftTarget::Node { node })) => Target::Node(node.clone()),
                    (None, Some(WftTarget::Terminal { terminal })) => {
                        Target::Terminal(terminal.clone())
                    }
                    (None, None) => return Err(EngineError::NoConditionMatched),
                }
            }
            // WOR-56: yukarıda erken return ile ele alındı.
            Wft::Collapse { .. } => unreachable!("collapse resolve_wft başında işlenir"),
        };

        // WOR-31 kol bağlamı: hedef join'e EŞİTSE varış (kol arrived, join node
        // işgal edilmez); normal node ise kol hareketi; join'den FARKLI bir
        // terminal ise aşağıdaki normal terminal yoluna düşer — TÜM WFE orada
        // biter (sibling `_branch_cancelled` marker'ları çağıranda staged edilir).
        if let WftMode::Branch {
            join,
            from_node,
            others_active,
            rule,
            all_entries,
            arrived_entries,
        } = mode
        {
            let arrived = match (&target, join) {
                (Target::Node(n), WftTarget::Node { node }) => n == node,
                (Target::Terminal(t), WftTarget::Terminal { terminal }) => t == terminal,
                _ => false,
            };
            if arrived {
                // WOR-72/WOR-73: "join doldu mu" ölçütü kurala göre AYRI:
                // - All (AND): kardeş aktif kol kalmamalı (WOR-31 davranışı).
                // - Quorum(k): bu varışla birlikte varış sayısı eşiğe ulaşmalı.
                // - Expr(e): ZEN koşulu bu varış dahil kol kümesiyle `true` olmalı
                //   ("(finans VE hukuk) YA DA gm" gibi sayıyla ifade edilemeyen kural).
                //
                // `quorum_collapse` = "geride İPTAL EDİLECEK aktif kol var".
                // Join'i dolduran varışta zaten varmış kardeşler kuralın ÜYESİdir —
                // onaylarının geçersizleşmesi diye bir şey yok, `superseded`
                // işaretlenmezler (bkz. `stage_parallel_markers`).
                let completes = match rule {
                    JoinRule::All => others_active == 0,
                    JoinRule::Quorum(k) => arrived_entries.len() as u32 >= *k,
                    JoinRule::Expr(expr) => {
                        let mut env = EvalEnv::new(&staged)
                            .with_wfah(wfah, &ValidRules::for_version(wfd))
                            .with_node(Some(from_node))
                            .with_actor(actor)
                            .with_wfe_id(wfe_id)
                            .with_join(JoinEnv {
                                all: all_entries.to_vec(),
                                arrived: arrived_entries.to_vec(),
                            });
                        if let Some(input) = action_input {
                            env = env.with_action_input(input);
                        }
                        if let Some(c) = call {
                            env = env.with_call(c.clone());
                        }
                        evaluate_bool(expr, &env)?
                    }
                };
                let quorum_collapse = completes && others_active > 0;
                if !completes {
                    // WOR-73: ZEN koşulu SON kol da varınca hâlâ `false` ise join asla
                    // dolmayacaktır — WFE paralel modda sessizce kilitlenirdi. Bu bir
                    // TASARIM hatasıdır (validator tatmin edilebilirliği kanıtlayamaz),
                    // engine-defined fail ile yüzeye çıkarılır: WFE `error` olur ve
                    // `end_response.reason` neyin olduğunu söyler.
                    if others_active == 0 {
                        return Ok((
                            CommitOutcome::Failed {
                                end_response: json!({
                                    "reason": "WFD.JoinUnsatisfied",
                                    "join_rule": rule.kind(),
                                    "arrived": arrived_entries,
                                }),
                            },
                            staged,
                            None,
                        ));
                    }
                    // Engine'in görüşü: join henüz dolmadı — yarış varsa adapter
                    // doğrulaması + executor retry düzeltir (T3).
                    return Ok((
                        CommitOutcome::BranchArrived {
                            from_node: from_node.to_string(),
                            arrived_entries: arrived_entries.to_vec(),
                        },
                        staged,
                        None,
                    ));
                }
                // Join doldu: paralel mod biter, join hedefine promotion.
                return match join {
                    WftTarget::Node { node } => Ok((
                        CommitOutcome::JoinComplete {
                            from_node: from_node.to_string(),
                            quorum_collapse,
                            arrived_entries: arrived_entries.to_vec(),
                            next: Box::new(CommitOutcome::MoveTo { node: node.clone() }),
                        },
                        staged,
                        Some(CallSite::Node(node.clone())),
                    )),
                    WftTarget::Terminal { terminal } => {
                        let (end_response, final_ctx) = self.terminal_outcome(
                            terminal,
                            wfd,
                            staged,
                            actor,
                            wfe_id,
                            action_input,
                            call,
                        )?;
                        Ok((
                            CommitOutcome::JoinComplete {
                                from_node: from_node.to_string(),
                                quorum_collapse,
                                arrived_entries: arrived_entries.to_vec(),
                                next: Box::new(CommitOutcome::Terminal { end_response }),
                            },
                            final_ctx,
                            Some(CallSite::Terminal(terminal.clone())),
                        ))
                    }
                };
            }
            if let Target::Node(node_key) = &target {
                // Ç4-EK/S4: fork alt-grafının DIŞINA geri gönderme bir COLLAPSE'tır.
                // `BranchMoveTo` üretmek kolu fork dışına oturtur ve paralel modu
                // AÇIK bırakırdı — join o kolu sonsuza kadar bekler, WFE ÖLÜ
                // KİLİTLENİR. Kardeşleri düşürüp paralel modu bitirmek tek doğru
                // cevaptır; hedef zaten uğranmış bir node'dur (K-2 süzgeci).
                if send_back && !fork_subgraph(wfd, all_entries, join).contains(node_key.as_str())
                {
                    return Ok((
                        CommitOutcome::CollapseTo {
                            from_node: Some(from_node.to_string()),
                            node: node_key.clone(),
                            cause: CollapseCause::SentBack,
                        },
                        staged,
                        Some(CallSite::Node(node_key.clone())),
                    ));
                }
                // Normal kol hareketi — paralel mod sürer; kol claim'i +
                // entered_at adapter'da sıfırlanır (T3).
                return Ok((
                    CommitOutcome::BranchMoveTo {
                        from_node: from_node.to_string(),
                        node: node_key.clone(),
                    },
                    staged,
                    Some(CallSite::Node(node_key.clone())),
                ));
            }
        }

        match target {
            Target::Node(node_key) => {
                let site = CallSite::Node(node_key.clone());
                Ok((CommitOutcome::MoveTo { node: node_key }, staged, Some(site)))
            }
            Target::Terminal(terminal_id) => {
                let (end_response, final_ctx) = self.terminal_outcome(
                    &terminal_id,
                    wfd,
                    staged,
                    actor,
                    wfe_id,
                    action_input,
                    call,
                )?;
                Ok((
                    CommitOutcome::Terminal { end_response },
                    final_ctx,
                    Some(CallSite::Terminal(terminal_id)),
                ))
            }
        }
    }

    /// "Varılan node'u ŞU AN kim yapabilir?" — `current_c_a` cache'i.
    ///
    /// `resolve_wft`ten AYRI ve ondan SONRA çağrılır, çünkü cevabı aksiyonun kendisine
    /// bağlı olabilir: WFAH-çapalı bir `c_orgu` ("başlatanın biriminin müdürü",
    /// `{from: {wfah: "<aksiyon>", field: "actor.orgu"}}`) o aksiyonun kaydını
    /// göremezse çapa çözülmez ve `resolve_c_orgu` BOŞ küme döner (aktörün birimine
    /// DÜŞMEZ — bkz. `resolver.rs`), yani node adaysız açılır. Bu yüzden çağıranlar
    /// `wfah`'ı `Wfah::extended` ile TAMAMLADIKTAN sonra buraya girer; koşulların
    /// gördüğü aksiyon-öncesi geçmişle karıştırılmaz.
    ///
    /// Aday YOKTUR: terminal (iş bitti), kol varışı ve join-doldurmama (WFE bir
    /// node'a inmedi).
    #[allow(clippy::too_many_arguments)]
    /// `anchor_orgu`: node c_a'sındaki ORGTRVLANG selector'larının çapası — WFE'nin
    /// KENDİ birimi (`origin_orgu_id`), geçişi yapan aktörünki DEĞİL (2026-08-13).
    /// Aksi halde aynı akış, ona dokunan kişiye göre farklı bir aday kümesi
    /// yazardı: `self` çapası kaydıkça iş bir sonraki adımda başka bir şubenin
    /// havuzuna düşerdi.
    async fn candidates_at(
        &self,
        outcome: &CommitOutcome,
        landed: Option<&CallSite>,
        wfd: &Wfd,
        ctx: &Value,
        wfah: &Wfah,
        anchor_orgu: Uuid,
        orgtnt_id: Uuid,
    ) -> Result<Vec<ResolvedCandidate>, EngineError> {
        // Fork TEK bir node'a inmez (`landed` de bu yüzden None): cache tüm kol giriş
        // node'larının birleşimidir — kol-bazlı havuz görünümü T3'te wfe_branch
        // satırlarından türetilir, buradaki union liste görünürlüğü içindir.
        if let CommitOutcome::ForkTo { branches, .. } = outcome {
            let mut resolved = Vec::new();
            for b in branches {
                let node = wfd.nodes.get(b).ok_or_else(|| {
                    EngineError::InvalidWfd(format!("parallel branch bilinmeyen node '{b}'"))
                })?;
                let mut extra = self
                    .resolve_candidates(&node.c_a, ctx, wfah, anchor_orgu, orgtnt_id)
                    .await?;
                resolved.append(&mut extra);
            }
            return Ok(resolved);
        }
        // E03/a: JOKER KAPANDI. Eski gövde `landed` üzerinde jokerliydi ve `landed`
        // bir `Option<&CallSite>` olduğu için `E02`/S1-EK'in `CommitOutcome` joker
        // yasağı bu satırı KAPSAMIYORDU: `StayAt` eklendiğinde derleyici burayı
        // işaret ETMEZ, `resolved_c_a` sessizce boş kalırdı.
        //
        // Node artık `resolution()`dan okunur — soru "hangi outcome" değil, **işin
        // DURDUĞU node**. ⚠️ `to_node()` KULLANILAMAZ: `Ç2` gereği `StayAt`te `None`dır
        // (marker satırı hareket taşımaz) ve tam da kapatılmak istenen deliği açardı.
        match landed {
            Some(CallSite::Node(node_key)) => {
                self.node_candidates(node_key, wfd, ctx, wfah, anchor_orgu, orgtnt_id)
                    .await
            }
            // Ardıl akış terminalden doğar: iş BİTTİ, node havuzu yok.
            Some(CallSite::Terminal(_)) => Ok(vec![]),
            // Hareket bir çağrı sitesine inmedi — işin durduğu node varsa onun havuzu
            // (`StayAt`), yoksa boş (kol varışı, join dolmadı, terminal sınıfı).
            None => match outcome.resolution().1 {
                Some(node_key) => {
                    self.node_candidates(node_key, wfd, ctx, wfah, anchor_orgu, orgtnt_id)
                        .await
                }
                None => Ok(vec![]),
            },
        }
    }

    /// Node hedefinin aday cache'i: node c_a + WOR-44 listable[] union'ı
    /// (VIEW-only; `when` guard'ları burada yok sayılır — over-inclusive cache
    /// kabul edilir, claim/act gerçek kuralda matcher-gated kalır).
    /// Bir node'un aday listesi — `wf.wfe.current_c_a` cache'ine yazılır.
    ///
    /// 2026-08-13: `listable[]` katlaması BURADAN KALKTI. Katlanmış hâli iki
    /// yanlış üretiyordu: (1) `when` guard'ı yok sayılıyordu (havuz over-inclusive
    /// kabul ediyordu), (2) terminal'de kolon boşaltıldığı için kalıcı olması
    /// gereken grant kayboluyordu. Görünürlük grant'ları artık AYRI ve KALICI bir
    /// kolonda: `view_c_a` (bkz. `Engine::view_grants`). Bu kolon adının söylediği
    /// şeydir: yalnız node'un adayları.
    pub async fn node_candidates(
        &self,
        node_key: &str,
        wfd: &Wfd,
        staged: &Value,
        wfah: &Wfah,
        anchor_orgu: Uuid,
        orgtnt_id: Uuid,
    ) -> Result<Vec<ResolvedCandidate>, EngineError> {
        let node = wfd.nodes.get(node_key).ok_or_else(|| {
            EngineError::InvalidWfd(format!("wft hedefi bilinmeyen node '{node_key}'"))
        })?;
        // ⚠️ v2.3 (`E04`): **`node.c_a ∪ AÇILMIŞ GRANTLAR`.** Escalation havuzu
        // genişletiyor (`Ç9`), dolayısıyla bu kolonun değeri artık yalnız node'un
        // kendi kuralı DEĞİL. Genişletmenin BURADA olması bilinçli: havuz kolonlarını
        // (`wfe.current_c_a`, `wfe_branch.c_a`) yazan her yol bu fonksiyondan geçiyor,
        // yani atlanabilecek ikinci bir yol yok.
        //
        // ⚠️ **Guard'lar YOK SAYILIR** — kolon over-inclusive bir cache'tir (`E03`):
        // commit anında viewer bilinmez ve `when` kişiye bağlı olabilir. Ayrım OKUMA
        // anında yapılır ("görebilir ≠ yapabilir", `P05`/G).
        let mut out = self
            .resolve_candidates(node.act_c_a(), staged, wfah, anchor_orgu, orgtnt_id)
            .await?;
        for grant in crate::v22::grants::open_grants(wfd, wfah, node_key) {
            let extra = self
                .resolve_candidates(&grant.c_a, staged, wfah, anchor_orgu, orgtnt_id)
                .await?;
            for cand in extra {
                if !out.contains(&cand) {
                    out.push(cand);
                }
            }
        }
        Ok(out)
    }

    /// Terminal hedefi: terminal.wfes_effects uygulanır, wfe_end_response
    /// $-string'leri FINAL ctx ile çözülür (M9/WOR-42). `(end_response, final_ctx)`.
    fn terminal_outcome(
        &self,
        terminal_id: &str,
        wfd: &Wfd,
        staged: Value,
        actor: &Actor,
        wfe_id: Uuid,
        action_input: Option<&Value>,
        // WFC-RETURN'den gelen terminal `wfe_end_response` içinde `$call.*` kullanabilir.
        call: Option<&CallOutcome>,
    ) -> Result<(Value, Value), EngineError> {
        let terminal = wfd
            .terminals
            .iter()
            .find(|t| t.id == terminal_id)
            .ok_or_else(|| {
                EngineError::InvalidWfd(format!("wft hedefi bilinmeyen terminal '{terminal_id}'"))
            })?;
        let now = Utc::now();
        let env = EffectEnv {
            env: self.env.public(),
            call,
            actor,
            wfe_id,
            node: None,
            action_input,
            exec_result: None,
            now,
        };
        let final_ctx = match &terminal.wfes_effects {
            Some(effects) => apply_effects(&staged, effects, &env)?,
            None => staged,
        };
        let mut end_response = Map::new();
        for (k, raw) in &terminal.wfe_end_response {
            end_response.insert(k.clone(), resolve_value(raw, &final_ctx, &env)?);
        }
        Ok((Value::Object(end_response), final_ctx))
    }

    /// Yeni node'un c_a'sını (orgu × rol, orgu × user) aday listesine çözer —
    /// pool cache'i (WOR-44). c_r girdileri role-only ResolvedCandidate, c_u
    /// girdileri user-only ResolvedCandidate üretir (role boş string).
    /// Kimlik kanalı matcher.rs (§3.3) ile birebir: c_u önce UUID string
    /// olarak parse edilir (user_id), parse başarısızsa ident olarak saklanır
    /// (user_ident) — pool sorgusu actor'ün kendi ident'ini org.user_ident ile
    /// çözüp aynı kanaldan eşler. Claim yetkisi HER ZAMAN matcher ile
    /// runtime'da yeniden doğrulanır; bu cache yalnızca liste görünürlüğü içindir.
    /// `anchor_orgu`: ORGTRVLANG selector'larının çapası (`self`, `parent`, …).
    ///
    /// Node c_a'sında bu, geçişi yapan AKTÖRün birimidir (kural "geçişi yapanın
    /// birimine göre" çözülür). `listable`/`wf_admin` grant'larında ise WFE'nin
    /// KENDİ birimidir (`view_grants`) — o kurallar viewer'a bağlı çözülemez,
    /// çünkü grant'lar viewer bilinmezken (commit anında) yazılır.
    async fn resolve_candidates(
        &self,
        rule: &CandidateActor,
        ctx: &Value,
        wfah: &Wfah,
        anchor_orgu: Uuid,
        orgtnt_id: Uuid,
    ) -> Result<Vec<ResolvedCandidate>, EngineError> {
        // Çapasız kural (c_orgu yok): birim kümesi YOK — aday tek satır, `any_orgu` işaretli
        // (bkz. `types::actor::CandidateActor`). Tenant'taki her ORGU için satır üretmek
        // hem sınırsız hem de yanlış olurdu: küme org ağacı değiştikçe değişir, cache ise
        // node'a girişte donar.
        let units = match &rule.c_orgu {
            Some(c_orgu) => {
                Some(resolve_c_orgu(c_orgu, anchor_orgu, ctx, wfah, orgtnt_id, self.org).await?)
            }
            None => None,
        };
        let mut out = Vec::new();
        // Rol kanalı yalnız ÇAPALI kuralda vardır (matcher ile aynı kısıt).
        if let (Some(roles), Some(units)) = (&rule.c_r, units.as_ref()) {
            for unit in units {
                for role in roles {
                    out.push(ResolvedCandidate {
                        orgu_id: Some(unit.orgu_id),
                        role: role.clone(),
                        user_id: None,
                        user_ident: None,
                        any_orgu: false,
                    });
                }
            }
        }
        if let Some(users) = &rule.c_u {
            // `Ref` öğeleri BURADA çözülür: bu fonksiyon node'a girişte, ctx bilinirken
            // koşuyor ve çıktısı denormalize `current_c_a` cache'ine yazılıyor. Havuz
            // listelemesi (portal/pool.rs) o cache'i jsonb containment ile sorguladığı için
            // SQL tarafı değişmez — cache zaten "çözülmüş adaylar" tutuyor.
            // Çözülemeyen referans aday üretmez (matcher'la aynı: eksik = eşleşme yok).
            let idents: Vec<String> = users
                .iter()
                .filter_map(|item| match item {
                    CuItem::Literal(s) => Some(s.clone()),
                    CuItem::Ref { from } => resolve_cu_ident(from, ctx),
                })
                .collect();
            for u in &idents {
                let (user_id, user_ident) = match Uuid::parse_str(u) {
                    Ok(uuid) => (Some(uuid), None),
                    Err(_) => (None, Some(u.clone())),
                };
                match units.as_ref() {
                    Some(units) => {
                        for unit in units {
                            out.push(ResolvedCandidate {
                                orgu_id: Some(unit.orgu_id),
                                role: String::new(),
                                user_id,
                                user_ident: user_ident.clone(),
                                any_orgu: false,
                            });
                        }
                    }
                    // Çapasız: kişi başına TEK girdi, birim yazılmaz.
                    None => out.push(ResolvedCandidate {
                        orgu_id: None,
                        role: String::new(),
                        user_id,
                        user_ident: user_ident.clone(),
                        any_orgu: true,
                    }),
                }
            }
        }
        Ok(out)
    }
}

enum Target {
    Node(String),
    Terminal(String),
}

/// Ç4-EK/S4 — **fork alt-grafı**: kol giriş node'larından İLERİ yürüyüşle ulaşılan
/// node kümesi. Join node'unda DURULUR (join fork'un dışıdır; oraya varış zaten
/// `BranchArrived`/`JoinComplete` yolundan geçer).
///
/// Hangi kenarın izlendiği bu testin TAMAMIDIR — bkz. `subgraph_edges`.
///
/// Validator'ın `check_parallel` / `parallel_interior_nodes` yürüyüşü send-back
/// kenarını İZLER; oradaki soru *erişilebilirlik ve ayrıklık*, buradaki soru
/// *"kol bu hareketle fork'un dışına çıkıyor mu"*. İki yürüyüş bilerek AYRIDIR.
///
/// Escalation/claim-timeout hedefleri de İZLENMEZ: bu test yalnız geri gönderme
/// yolunda sorulur (`resolve_wft`in `send_back` parametresi), SLA yolları bu
/// kararın kapsamı dışındadır.
fn fork_subgraph(wfd: &Wfd, entries: &[String], join: &WftTarget) -> BTreeSet<String> {
    let join_node = match join {
        WftTarget::Node { node } => Some(node.as_str()),
        WftTarget::Terminal { .. } => None,
    };
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    for e in entries {
        if seen.insert(e.clone()) {
            queue.push_back(e.clone());
        }
    }
    while let Some(node_key) = queue.pop_front() {
        if Some(node_key.as_str()) == join_node {
            continue;
        }
        // Ç5: yönlendirme kuralı aksiyonun KENDİ kaydında; `from` TEK node'dur
        // (eskiden `transitions[].from` bir listeydi ve `contains` sorulurdu).
        for t in wfd.actions.values() {
            if t.from != node_key {
                continue;
            }
            for target in subgraph_edges(&t.wft) {
                if seen.insert(target.to_string()) {
                    queue.push_back(target.to_string());
                }
            }
        }
    }
    seen
}

/// `fork_subgraph` yürüyüşünün İZLEDİĞİ node kenarları.
///
/// Terminal hedefleri yoktur (akış bir node'a gitmez, alt-graf node kümesidir).
/// Üç form BOŞ döner ve bu, alt-graf tanımının kendisidir:
/// `SendBack` (alt-graftan ÇIKAN kenar — izlenirse test daima "içeride" der),
/// `Collapse` (WOR-56: zaten kapsam dışına çıkar),
/// `Parallel` (nested fork validator tarafından yasak).
fn subgraph_edges(wft: &Wft) -> Vec<&str> {
    match wft {
        Wft::Node { node } => vec![node.as_str()],
        Wft::Conditional {
            conditions,
            default,
        } => {
            let mut out: Vec<&str> = conditions
                .iter()
                .filter_map(|c| c.node.as_deref())
                .collect();
            if let Some(WftTarget::Node { node }) = default {
                out.push(node.as_str());
            }
            out
        }
        Wft::Terminal { .. }
        | Wft::SendBack { .. }
        | Wft::Collapse { .. }
        | Wft::Parallel { .. } => Vec::new(),
    }
}

/// WOR-31 — `resolve_wft`'in çalıştığı bağlam.
#[derive(Clone, Copy)]
enum WftMode<'p> {
    /// Start kuralı: Parallel hedef yasak (validator da reddeder).
    Start,
    /// Tekil (paralel olmayan) mod: Parallel hedef `ForkTo` üretir.
    Single,
    /// Paralel mod, tek kolun wft'i: hedef join'e eşitse varış
    /// (`BranchArrived`/`JoinComplete`), normal node ise `BranchMoveTo`,
    /// join'den farklı terminal ise WFE-terminal (sibling'ler iptal).
    Branch {
        join: &'p WftTarget,
        from_node: &'p str,
        others_active: usize,
        /// WOR-72/WOR-73: çözülmüş join kuralı (`Wfes::join_rule`).
        rule: &'p JoinRule,
        /// WOR-73: fork'un TÜM kollarının giriş node'ları — `$branches` namespace'i
        /// hiç varmamış kollar için de `false` taşıyabilsin diye gerekir.
        all_entries: &'p [String],
        /// WOR-73: bu varış DAHİL, join'e varmış kolların giriş node'ları (sıralı).
        /// Hem quorum sayımı hem ZEN koşulu hem adapter doğrulaması bunu kullanır.
        arrived_entries: &'p [String],
    },
}

/// Ç2: commit'in HAREKET satırına (asıl aksiyon) akış izini yazar. Marker satırlarına
/// DOKUNULMAZ — onları üreten kod `None` ile kurar, ayrım marker ADINDAN türetilmez.
///
/// `fallback_from`: outcome kaynağı taşımıyorsa (`MoveTo`/`ForkTo`/`Terminal`/`Failed`/
/// `Terminated`) satırı üreten yolun bildiği kaynak node — tek-kol yolda
/// `wfes.current_node`, kol yolunda kolun o anki node'u.
fn stamp_movement(entry: &mut WfahEntry, outcome: &CommitOutcome, fallback_from: Option<&str>) {
    entry.from_node = outcome.from_node().or(fallback_from).map(str::to_string);
    entry.to_node = outcome.to_node().map(str::to_string);
}

/// Ç4 + E14: kol node'u (KONUM) → satırın KOL ETİKETİ = (kimlik, tur). Kol
/// bağlamında üretilen her satır bu ikiliyi taşır; paralel-olmayan yolda
/// `(None, None)` ("bu satır bir kolda değil").
///
/// İkisi TEK fonksiyondan çıkar çünkü *"`branch_entry` NULL ⇔ `branch_round` NULL"*
/// bir DEĞİŞMEZDİR (E14/S3): ayrı ayrı yazılsalar bir üretici birini doldurup
/// diğerini atlayabilirdi.
/// `E02`/S2 — claim'i TUTAN aktörü defterden okur (`Ç13`in `claim_taken:<node>` satırı).
///
/// `Wfes.assigned_to` yalnız `user_id`dir; yetki sorusu ise birim ve rol ister. Sahipliği
/// doğuran satır `actor`ı da yazdığı için kaynak odur. En SON eşleşen satır alınır: aynı
/// node'da sahiplik el değiştirmiş olabilir.
fn claimant_actor(wfah: &Wfah, node_key: &str, owner: Uuid) -> Option<Actor> {
    let marker = format!("claim_taken:{node_key}");
    wfah.entries()
        .iter()
        .rev()
        .find(|e| e.action == marker && e.actor.user_id == owner)
        .map(|e| e.actor.clone())
}

fn branch_label(wfes: &Wfes, branch: Option<&str>) -> (Option<String>, Option<u32>) {
    let entry = branch
        .and_then(|b| active_branch(wfes, b))
        .map(|b| b.entry_node.clone());
    let round = branch_round_of(&wfes.wfah, entry.as_deref());
    (entry, round)
}

/// E14: satırın TURU — kolu açan `_fork` satırlarının defterdeki sayısı; **1'den
/// başlar, FORK BAŞINA sayar** (global sayaç DEĞİL). Türetim, çapa gerekçesi ve tur
/// eleme kuralı `v22::valid` modülündedir — sayım iki yerde AYRI yazılmaz.
///
/// `branch_entry` NULL ⇔ dönüş NULL (E14/S3 değişmezi). Taban 1'e sabitlenir: kol
/// bağlamında `_fork` satırı olmadan satır üretilemez, ama sayının 0 çıktığı bir hâlde
/// `None` dönmek *"bu satır bir kolda değil"* anlamına gelir ve değişmezi delerdi.
fn branch_round_of(wfah: &Wfah, branch_entry: Option<&str>) -> Option<u32> {
    valid::round_of_opt(wfah, branch_entry)
}

/// Kol node'una göre AKTİF branch state'i.
fn active_branch<'w>(wfes: &'w Wfes, node: &str) -> Option<&'w BranchState> {
    wfes.branches
        .iter()
        .find(|b| b.status == BranchStatus::Active && b.branch_node == node)
}

/// WOR-73: fork'un TÜM kollarının giriş node'ları (kol kimlikleri, `branches`
/// sırasında). `$branches` namespace'i hiç varmamış kollar için de alan taşısın diye.
fn all_entry_nodes(wfes: &Wfes) -> Vec<String> {
    wfes.branches.iter().map(|b| b.entry_node.clone()).collect()
}

/// WOR-73: join'e varmış kol kimlikleri + `acting` kolun kendisi (varış ANINDA
/// değerlendirildiği için karar kümesine dahildir). Sıralı döner: küme
/// karşılaştırması (adapter doğrulaması) sıraya duyarsız olsun.
fn arrived_entries_with(wfes: &Wfes, acting_branch: &str) -> Vec<String> {
    let mut out: Vec<String> = wfes
        .branches
        .iter()
        .filter(|b| b.status == BranchStatus::Arrived)
        .map(|b| b.entry_node.clone())
        .collect();
    if let Some(acting) = wfes
        .branches
        .iter()
        .find(|b| b.status == BranchStatus::Active && b.branch_node == acting_branch)
    {
        out.push(acting.entry_node.clone());
    }
    out.sort();
    out.dedup();
    out
}

/// WOR-31 sistem marker'ları — ENGINE tarafından staged edilir; tek istisna
/// `_join`: o, son-varış doğrulamasıyla aynı transaction'da ADAPTER tarafından
/// eklenir (dokümante edilmiş istisna).
/// - `ForkTo` → `_fork` {branches, join, join_threshold} (WOR-72: null = AND)
/// - `BranchArrived`/`JoinComplete` → `_branch_arrived` {branch_entry, at_node,
///   approved_by, approved_at, claimed_at} (WOR-68: claim başlangıcı;
///   hold = approved_at − claimed_at)
/// - WOR-72: quorum (OR) join eşiği dolup geride aktif kol kalırsa `JoinComplete`
///   de aşağıdaki collapse yoluna girer (`kind`/`reason` = `join_quorum`); eşiğin
///   ÜYESİ olan varmış kardeşler `superseded` işaretlenMEZ (onayları sayıldı).
/// - paralel modda Terminal/Failed/Terminated/CollapseTo → önce `_collapse` özeti,
///   sonra acting kol DIŞINDAKİ her AKTİF kol için `_branch_cancelled`
///   {branch_entry, at_node, reason, claimed_by, claimed_at, trigger_*}, her ARRIVED
///   kol için `_branch_superseded` {branch_entry, at_node, reason, approved_by,
///   approved_at, trigger_*}
///
/// Ç3 (v2.3): kol marker'ları kolu İKİ ayrı alanla taşır — kimlik `branch_entry`
/// (kolun DEĞİŞMEZ `entry_node`'u), konum `at_node` (kolun o anki `branch_node`'u).
/// Belirsiz `node` alanı KALKTI; detay marker'larının `trigger_node`'u
/// `trigger_branch` oldu ve değeri kol kimliğidir (`_collapse` manşetiyle aynı ad).
/// `_fork` DEĞİŞMEDİ (zaten giriş node'larını taşıyor). Marker ADLARI değişmez
/// (Değişmez #2).
///
/// WOR-59: iptal edilen kolun claim'i adapter tarafında düşürülür (`claimed_by`
/// NULL'lanır) — düşen claim'in SAHİBİ ve TUTULMA BAŞLANGICI bu marker'a yazılır,
/// yoksa "kim ne kadar süre tutuyordu" bilgisi collapse anında kaybolur.
///
/// WOR-60: join'e VARMIŞ (onaylanmış) kol collapse'ta hiçbir iz bırakmıyordu —
/// `cancel_active_branches` yalnız `active` satırları vurur, marker döngüsü de
/// yalnız aktif kardeşleri gezerdi. Onay WFAH'ta duruyor ama "bu onay geçersizleşti"
/// bilgisi yoktu; onaylanmış kol yan etki üretmiş olabileceği için kritik.
///
/// WOR-61: kol-başına marker'ların ÜSTÜNE tek bir `_collapse` özeti eklenir —
/// "ne oldu" sorusu tek kayıttan cevaplanır (detay marker'ları KALIR).
///
/// WOR-63: kol marker'larının `reason`'ı tek başına "neden düştü"yü anlatmıyordu
/// (sabit, dar bir string). Tetikleyen kol/aksiyon/actor ek alanlar olarak eklendi;
/// `reason` alanı DEĞİŞMEDİ — mevcut tüketiciler kırılmaz.
///
/// WOR-67: acting kol (collapse'ı tetikleyen) marker döngüsünden dışlanır ama
/// adapter onun da claim'ini düşürür. Düşen claim `_collapse` manşetine
/// `trigger_claimed_by`/`trigger_claimed_at` olarak yazılır (ayrı marker YOK) —
/// yoksa "reddeden kişi claim'i ne kadar tuttu" collapse anında kaybolur.
fn stage_parallel_markers(
    wfes: &Wfes,
    trigger: &Trigger<'_>,
    outcome: &CommitOutcome,
    wfah_entries: &mut Vec<WfahEntry>,
    seq: &mut u32,
    now: DateTime<Utc>,
) {
    let (acting_branch, actor) = (trigger.branch, trigger.actor);
    let system = system_actor();
    // Ç2: kol/collapse marker'ları HAREKET taşımaz — bu commit'in akış izi aynı
    // commit'teki aksiyon satırındadır.
    // Ç4: `branch_entry` marker'ın KONUSU olan kolun kimliğidir; WFE-geneli
    // marker'larda (`_fork` kolları YARATIR, `_collapse` paralel modu KAPATIR) `None`.
    let mut push = |action: &str, branch_entry: Option<&str>, input: Value| {
        wfah_entries.push(WfahEntry {
            seq: *seq,
            action: action.to_string(),
            actor: system.clone(),
            input: Some(input),
            applied_at: now,
            from_node: None,
            to_node: None,
            branch_entry: branch_entry.map(str::to_string),
            branch_round: branch_round_of(&wfes.wfah, branch_entry),
        });
        *seq += 1;
    };
    match outcome {
        CommitOutcome::ForkTo {
            branches,
            join,
            join_rule,
        } => {
            // WOR-72/WOR-73: audit'te join kuralının kaydı ŞART — "neden 3 kolun
            // 2'siyle devam etti" sorusu yalnız buradan cevaplanır.
            let (threshold, when) = match join_rule {
                JoinRule::All => (Value::Null, Value::Null),
                JoinRule::Quorum(k) => (json!(k), Value::Null),
                JoinRule::Expr(e) => (Value::Null, json!(e)),
            };
            push(
                "_fork",
                // Ç3/Ç4: `_fork` DEĞİŞMEZ — `branches` listesi fork ANINDA yazılıyor ve
                // o an `branch_node == entry_node`, yani zaten giriş node'larını
                // taşıyor. Satırın kendisi bir kolun içinde DEĞİLDİR.
                None,
                json!({
                    "branches": branches,
                    "join": join,
                    "join_mode": join_rule.kind(),
                    "join_threshold": threshold,
                    "join_when": when,
                }),
            );
        }
        CommitOutcome::BranchArrived { from_node, .. }
        | CommitOutcome::JoinComplete { from_node, .. } => {
            // WOR-60: varış anındaki onaylayan + zaman burada kalıcılaşır. Kol satırı
            // varışta claim'ini kaybettiği (`mark_branch_arrived`) için sonradan
            // collapse olursa "kimin onayı geçersizleşti" YALNIZCA buradan okunabilir.
            //
            // WOR-68: claim BAŞLANGICI (`claimed_at`) da burada kalıcılaşır — adapter
            // varışta NULL'ladığı için "onaylayan kolu ne kadar tuttu" (hold süresi =
            // approved_at − claimed_at) sonradan yalnız bu marker'dan hesaplanabilir.
            // Snapshot (`wfes.branches`) commit ÖNCESİ olduğu için claim hâlâ duruyor.
            //
            // Ç3: kolun KİMLİĞİ (`entry_node`) ile KONUMU (varış node'u) ayrı
            // alanlarda taşınır; belirsiz `node` alanı KALKTI. `branch_approval`
            // geri okuması kimlik anahtarıyla çalışır — çok adımlı kolda
            // `branch_node` ile arama "onay geçersizleşti" bilgisini kaybediyordu.
            let arriving = wfes
                .branches
                .iter()
                .find(|b| b.branch_node.as_str() == from_node.as_str());
            let claimed_at = arriving.and_then(|b| b.claimed_at);
            push(
                "_branch_arrived",
                arriving.map(|b| b.entry_node.as_str()),
                json!({
                    "branch_entry": arriving.map(|b| b.entry_node.as_str()),
                    "at_node": from_node,
                    "approved_by": actor,
                    "approved_at": now,
                    "claimed_at": claimed_at,
                    // Ç13: "claim üç yoldan düşer" okuma kuralının (c) ayağı. Kol
                    // kapanış marker'ı örtük bir bırakmadır ve KİMİN sahipliğinin
                    // düştüğünü yalnız bu alan söyler — `approved_by` eylemi alanı
                    // gösterir, sahibi DEĞİL (vekaleten alınmış kolda ikisi ayrışır).
                    "claimed_by": arriving.and_then(|b| b.claimed_by),
                }),
            );
        }
        _ => {}
    }

    // Paralel modu bitiren yolların ORTAK iptal nedeni + hedefi. WOR-56'da collapse
    // ayrı bir arm'dı; iptal semantiği Terminal/Failed/Terminated ile birebir aynı
    // olduğu için tek yerde toplandı (WOR-59: claim düşürme bilgisi tek yerden yazılsın).
    // `target`: yalnız node hedefli collapse'ta anlamlı — terminal yollarında akış
    // bir node'a GİTMEZ, sonucu `wfe.end_response` taşır.
    let (cancel_reason, collapse_kind, target) = match outcome {
        CommitOutcome::Terminal { .. } if wfes.join_target.is_some() => {
            ("sibling_terminal", "terminal", Value::Null)
        }
        CommitOutcome::Failed { .. } if wfes.join_target.is_some() => {
            ("failed", "failed", Value::Null)
        }
        CommitOutcome::Terminated { .. } if wfes.join_target.is_some() => {
            ("terminated", "terminated", Value::Null)
        }
        // Node hedefli collapse (WOR-56). Terminal hedefli collapse yukarıya düşer.
        CommitOutcome::CollapseTo {
            node,
            cause: CollapseCause::Collapse,
            ..
        } => ("collapsed", "collapse_to", json!(node)),
        // Ç4-EK/S4: fork alt-grafının dışına geri gönderme. Tasarımcının `collapse`
        // wft'iyle AYNI mekanizma ama AYRI olay — audit ikisini karıştıramaz, ve
        // `$valid` eleme kuralı 2'nin collapse dalı tam olarak bu `reason`ı arar.
        CommitOutcome::CollapseTo {
            node,
            cause: CollapseCause::SentBack,
            ..
        } => ("sent_back", "sent_back", json!(node)),
        // WOR-72: quorum (OR) join eşiği doldu ve geride aktif kol kaldı — iptal
        // semantiği collapse ile AYNI, nedeni farklı: kimse "reddetmedi", join
        // yeterli onayı topladı. `target` join node'u (terminal hedefte null).
        CommitOutcome::JoinComplete {
            quorum_collapse: true,
            next,
            ..
        } => (
            "join_quorum",
            "join_quorum",
            match next.as_ref() {
                CommitOutcome::MoveTo { node } => json!(node),
                _ => Value::Null,
            },
        ),
        _ => return,
    };
    // WOR-72: quorum join'de ZATEN VARMIŞ kardeşler eşiğin ÜYESİdir (onayları
    // sayıldı) — `superseded` işaretlenmezler. Diğer tüm yollarda (collapse /
    // terminal / failed / terminated) varmış kolun onayı geçersizleşir (WOR-60).
    let supersede_arrived = !matches!(
        outcome,
        CommitOutcome::JoinComplete {
            quorum_collapse: true,
            ..
        }
    );

    // Etkilenen kolları ÖNCE sınıflandır: WOR-61 özet marker'ı listeleri taşıdığı
    // için detay marker'larından ÖNCE (manşet olarak) yazılmak zorunda.
    let mut cancelled = Vec::new();
    let mut superseded = Vec::new();
    for b in &wfes.branches {
        if Some(b.branch_node.as_str()) == acting_branch {
            continue;
        }
        match b.status {
            // Henüz çalışılan kol: işi yarıda kaldı.
            BranchStatus::Active => cancelled.push(b),
            // WOR-60: onaylanmış ama join'lenmemiş kol: onayı geçersizleşti.
            // Kol satırının statüsü `arrived` KALIR (bkz. decisions.md) —
            // izlenebilirlik marker ile sağlanır, şema değişmez.
            // WOR-72: quorum join'de bu kollar eşiğin üyesidir → atlanır.
            BranchStatus::Arrived if supersede_arrived => superseded.push(b),
            BranchStatus::Arrived => {}
            BranchStatus::Cancelled => {}
        }
    }
    // Ç3: özet listeleri kol KİMLİKLERİNİ taşır (konumları değil) — `$valid` eleme
    // kuralı 1 ve portalın kol eşleştirmesi kimlikle çalışır.
    fn nodes<'b>(bs: &[&'b BranchState]) -> Vec<&'b str> {
        bs.iter().map(|b| b.entry_node.as_str()).collect()
    }

    // WOR-67: collapse'ı TETİKLEYEN (acting) kolun düşen claim'i. Acting kol marker
    // döngüsünden dışlanır (aksiyon kaydı zaten WFAH'ta) ama adapter onun claim'ini de
    // NULL'lar; sahip + başlangıç yalnız burada kalır. Ayrı marker YOK (bkz. WOR-67 a′):
    // manşet zaten `trigger_*` taşıyor, claim de doğal olarak buraya ait. Kardeş
    // kollarla simetri için `claimed_by` de yazılır (çoğu yolda == trigger_actor).
    // Snapshot commit ÖNCESİ olduğu için acting kolun claim'i hâlâ duruyor.
    let acting =
        acting_branch.and_then(|n| wfes.branches.iter().find(|b| b.branch_node.as_str() == n));
    // WOR-61 manşet: collapse'ın tamamı tek kayıtta. Detaylar (aşağıdaki kol-başına
    // marker'lar) KALIR — bu özet onların yerine değil, üstüne geçer.
    push(
        "_collapse",
        // Manşet paralel modun TAMAMINI özetler — bir kolun satırı değildir.
        None,
        json!({
            // Ç4-EK/S5: tetikleyicinin cinsi AÇIKÇA yazılır — aşağıdaki
            // `trigger_branch`/`trigger_at_node` `null` ise hiçbir tüketici
            // "kol yok mu, bilinmiyor mu" diye tahmin etmek zorunda kalmaz.
            "trigger_kind": trigger.kind.as_str(),
            // Ç3: tetikleyen kolun KİMLİĞİ; konumu ayrı alanda.
            "trigger_branch": acting.map(|b| b.entry_node.as_str()),
            "trigger_at_node": acting_branch,
            "trigger_action": trigger.action,
            "trigger_actor": actor,
            "trigger_claimed_by": acting.and_then(|b| b.claimed_by),
            "trigger_claimed_at": acting.and_then(|b| b.claimed_at),
            "kind": collapse_kind,
            "reason": cancel_reason,
            "target": target,
            "cancelled": nodes(&cancelled),
            "superseded": nodes(&superseded),
        }),
    );

    for b in cancelled {
        push(
            "_branch_cancelled",
            Some(b.entry_node.as_str()),
            json!({
                // Ç3: kol kimliği + kolun iptal ANINDAKİ konumu.
                "branch_entry": b.entry_node,
                "at_node": b.branch_node,
                "reason": cancel_reason,
                // WOR-59: cancel ANINDAKİ claim sahibi/başlangıcı — adapter bu
                // alanları hemen ardından NULL'ladığı için tek kayıt yeri burası.
                "claimed_by": b.claimed_by,
                "claimed_at": b.claimed_at,
                // WOR-63: tetikleyici bağlam (bkz. `Trigger`). Ç3: ad `_collapse`
                // manşetiyle aynı (`trigger_branch`), değeri kol KİMLİĞİ.
                // Ç4-EK/S5: `trigger_kind` de manşetle aynı ad ve aynı değer.
                "trigger_kind": trigger.kind.as_str(),
                "trigger_branch": acting.map(|a| a.entry_node.as_str()),
                "trigger_action": trigger.action,
                "trigger_actor": actor,
            }),
        );
    }
    for b in superseded {
        let (approved_by, approved_at) = branch_approval(&wfes.wfah, &b.entry_node);
        push(
            "_branch_superseded",
            Some(b.entry_node.as_str()),
            json!({
                "branch_entry": b.entry_node,
                "at_node": b.branch_node,
                "reason": cancel_reason,
                "approved_by": approved_by,
                "approved_at": approved_at,
                "trigger_kind": trigger.kind.as_str(),
                "trigger_branch": acting.map(|a| a.entry_node.as_str()),
                "trigger_action": trigger.action,
                "trigger_actor": actor,
            }),
        );
    }
}

/// WOR-61/WOR-63: collapse marker'larına yazılan TETİKLEYİCİ bağlam — "bu collapse'ı
/// kim, hangi koldan, hangi aksiyonla başlattı". Sistem yollarında (SLA deadline /
/// escalation / claim timeout) `actor` system aktörüdür ve `action` ilgili sistem
/// marker'ının adıdır (`timeout:deadline`, `escalate:<node>:<idx>` gibi).
struct Trigger<'a> {
    /// Ç4-EK/S5: tetikleyicinin CİNSİ. `branch` alanının `None` olması iki ayrı
    /// şey anlatıyordu ("kol yok" / "bu yolda kol kavramı yok"); bir alan iki
    /// anlam taşımaz (Ç3'ün `branch_entry`/`at_node` ayrımıyla aynı ilke).
    kind: TriggerKind,
    /// Aksiyonu uygulayan kol node'u; paralel-olmayan veya WFE-geneli yollarda None.
    branch: Option<&'a str>,
    action: Option<&'a str>,
    actor: &'a Actor,
}

/// Ç4-EK/S5 — collapse marker'larındaki `trigger_kind` alanının KAPALI listesi.
/// Değer kümesinin tek kaynağı burasıdır; okuyucu `null` görürse satır bu karardan
/// ÖNCE yazılmıştır (backfill YOK — Değişmez #9, R01 kapsamı).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TriggerKind {
    /// Bir KOL tetikledi: `trigger_branch`/`trigger_at_node` DOLUDUR.
    Branch,
    /// WF Admin global aksiyonu (`admin:send_back` / `admin:send_to_start` /
    /// `admin:cancel`) tetikledi. Adminin kolu yoktur → `trigger_branch` `null`
    /// ve bu "bilinmiyor" DEĞİL, "kol yok" demektir. Rastgele bir kol acting
    /// SAYILMAZ; `trigger_actor` gerçek admindir.
    Admin,
    /// Motorun kendi yolları: SLA-3 deadline, escalation, claim timeout, WFC
    /// dönüşü. `trigger_actor` sistem aktörüdür; kol bağlamı varsa `Branch`
    /// kullanılır (kol escalation'ı bir KOLU tetikleyicidir).
    System,
}

impl TriggerKind {
    fn as_str(self) -> &'static str {
        match self {
            TriggerKind::Branch => "branch",
            TriggerKind::Admin => "admin",
            TriggerKind::System => "system",
        }
    }
}

/// WOR-60: bir kolun onay bilgisini (`approved_by`/`approved_at`) kendi
/// `_branch_arrived` marker'ından okur — kol satırı varışta claim'ini kaybettiği
/// için onaylayan RUNTIME state'te değil, yalnız WFAH'ta durur. En SON varış
/// kaydı esastır (bir kol kol-içi hareketle aynı node'a dönebilir).
///
/// WOR-60 ÖNCESİ yazılmış `_branch_arrived` kayıtlarında bu alanlar yoktur →
/// null döner; eski WFE'ler için marker yine üretilir, alanları boş kalır.
///
/// Ç3: arama anahtarı kolun KİMLİĞİDİR (`branch_entry` = `entry_node`), o anki
/// konumu değil — kol varıştan sonra hareket etmiş olabilir ve `branch_node` ile
/// arama çok adımlı kolda hiçbir şey bulamazdı (`approved_by: null`).
///
/// E14 (UYGULAMA NOTU — bu bağımlılık SÖZLEŞMEDİR): aynı fork'a ikinci kez
/// girildiğinde defterde AYNI kimliğe ait İKİ turun `_branch_arrived` satırı bulunur.
/// Doğru cevabı veren şey `.rev()`tir: tarama sondan başladığı için YAŞAYAN turun
/// varışı ilk bulunur. Yön değişirse (ya da `find_map` `filter().next()`e çevrilirse)
/// birinci turun onaylayanı ikinci turun collapse marker'ına yazılır — sessiz yanlış.
/// Tur ALANINA bakmak gerekmez, ama yön değiştirilemez.
fn branch_approval(wfah: &Wfah, branch_entry: &str) -> (Value, Value) {
    let field = |input: &Value, key: &str| input.get(key).cloned().unwrap_or(Value::Null);
    wfah.entries()
        .iter()
        .rev()
        .filter(|e| e.action == "_branch_arrived")
        .find_map(|e| {
            let input = e.input.as_ref()?;
            (input.get("branch_entry")?.as_str()? == branch_entry)
                .then(|| (field(input, "approved_by"), field(input, "approved_at")))
        })
        .unwrap_or((Value::Null, Value::Null))
}

fn escalation_marker(node_key: &str, idx: usize) -> String {
    format!("escalate:{node_key}:{idx}")
}

/// WF Admin atlamasının marker'ı. `escalate:` önekini PAYLAŞIR — bkz.
/// `Engine::skip_escalation` yorumu (önek olmadan escalation tabanı kayar).
fn skipped_escalation_marker(node_key: &str, idx: usize) -> String {
    format!("{}:skipped", escalation_marker(node_key, idx))
}

/// Engine-tetiklemeli kenarlar (WFC-RETURN, SLA) için **çapa aktörü**.
///
/// `c_orgu` çözümü DAİMA `actor.orgu_id`'ye çapalanır (`resolve_c_orgu`). Saf sistem
/// aktörünün orgu'su `nil` olduğundan, hedef node'un kuralı `self`-çapalıysa çözüm
/// "orgu 00000000-...-0000 bulunamadı" ile patlar — yani akış o kenardan geçemez.
///
/// Çapa olarak WFAH'taki SON GERÇEK aktörün orgu'su kullanılır: `self` "bu işin
/// yaşadığı birim" demektir ve o birim, işi oraya getiren aktörün birimidir.
/// Rol `system` kalır — audit izinde tetikleyicinin insan olmadığı görünmeye devam eder.
///
/// Gerçek aktör yoksa (WFAH boş) nil orgu'ya düşer; o durumda `self`-çapalı hedef
/// zaten çözülemez ve hata anlaşılır biçimde yüzeye çıkar.
/// Çağıranın WFAH'ına satır satır işlenecek azami alt akış kaydı. Aşılırsa kalanı
/// kırpılır ve bir `call:<anahtar>/…` satırı kaç kaydın atlandığını + tam geçmişin
/// hangi WFE'de olduğunu söyler. Sınırın nedeni: WFAH her `load`'da TÜMÜYLE okunur.
const MAX_INLINED_CALL_ENTRIES: usize = 100;

/// Global aksiyonun WFAH kaydındaki adı: `admin:<aksiyon>`.
///
/// Önek ZORUNLU. İki sebep: (1) WFD aksiyon id'leri ile çakışmasın — `cancel` adında
/// bir akış aksiyonu tanımlamak serbesttir ve `$wfah` izdüşümüne bakan bir `when`
/// ifadesi ikisini ayırt edemezdi; (2) `escalate:` önekinde öğrenilen ders: marker
/// adı sözleşmedir, yayınlanmış akışlar `count($wfah, ...)` ile karar veriyor.
fn global_action_marker(action: GlobalAction) -> String {
    format!("admin:{}", action.as_str())
}

fn system_actor_anchored(wfes: &Wfes) -> Actor {
    let anchor = wfes
        .wfah
        .entries()
        .iter()
        .rev()
        .map(|e| e.actor.orgu_id)
        .find(|id| !id.is_nil())
        .unwrap_or_else(Uuid::nil);
    Actor {
        orgu_id: anchor,
        user_id: Uuid::nil(),
        role: "system".into(),
    }
}

fn system_actor() -> Actor {
    Actor {
        orgu_id: Uuid::nil(),
        user_id: Uuid::nil(),
        role: "system".into(),
    }
}

fn parse_wfd_uuid(wfd: &Wfd) -> Result<Uuid, EngineError> {
    // WFD JSON id'si insan-okur slug olabilir; store katmanı UUID satır id'sini bilir.
    // Burada parse edilebiliyorsa kullanılır, yoksa nil döner ve executor doldurur.
    Ok(Uuid::parse_str(&wfd.id).unwrap_or(Uuid::nil()))
}

/// §7.5 — aksiyon girdisi sözleşme denetimi: `input.required` yolları mevcut VE
/// non-null olmalı, `required ∪ optional` dışında kalan leaf yol reddedilir.
///
/// WOR-70: bu fonksiyon ctx'e ARTIK YAZMAZ. Girdinin ctx'e taşınması yalnız
/// `wfes_effects.set` üzerinden `$action.input.<yol>` ile olur — context'e tek yazma
/// yolu effects'tir. Böylece "bu değer ctx'e nereden geldi" sorusu akışa bakılarak
/// cevaplanabilir; validator de her declared input'un tüketildiğini zorlar
/// (`unused_action_input`) ve hiç yazılmayan context alanını reddeder
/// (`context_field_never_written`).
///
/// `required` ↔ `optional` ayrımı (WOR-70b): ikisi de `wfes_effects` ile ctx'e
/// eşlenmek ZORUNDADIR (validator `unused_action_input`); fark yalnız değerdedir —
/// `required` gönderilmek zorunda ve `null` OLAMAZ, `optional` gönderilmeyebilir ve
/// gönderilmediğinde ctx'e `null` yazılır. Null denetimi YALNIZ bildirilen yolun
/// kendisine bakar: `required: ["applicant"]` ile `{"applicant": {"name": null}}`
/// geçerlidir; `name`'in de dolu olması isteniyorsa `applicant.name` ayrıca
/// `input.required`'a yazılır.
/// §7.5 + TİP: `context` verilirse bildirilen yolların DEĞERLERİ de context şemasına
/// göre denetlenir (2026-08-19 — `v22::ctx_types`). Motor bilir kişidir: bildirilen bir
/// tip varsa ve değer o tipte gelmiyorsa reddi BURADA verir, istemcinin kendi kuralını
/// koymasını beklemez. `null` her tipte geçerlidir (WOR-70b gönderilmeyen `optional`
/// ctx'e `null` yazar); `required`ın null olamaması aşağıdaki ayrı kuraldır.
fn validate_action_input(
    action: &ActionDef,
    input: &Value,
    context: &Value,
) -> Result<(), EngineError> {
    let declared: Vec<&String> = action
        .input
        .required
        .iter()
        .chain(action.input.optional.iter())
        .collect();

    for required in &action.input.required {
        match get_path(input, required) {
            None => {
                return Err(EngineError::InvalidInput(format!(
                    "zorunlu input '{required}' eksik"
                )))
            }
            Some(Value::Null) => {
                return Err(EngineError::InvalidInput(format!(
                    "zorunlu input '{required}' null olamaz"
                )))
            }
            Some(_) => {}
        }
    }

    // declared olmayan leaf path reddedilir
    let mut leaves = Vec::new();
    collect_leaf_paths(input, String::new(), &mut leaves);
    for leaf in &leaves {
        let covered = declared.iter().any(|d| {
            leaf == *d || leaf.starts_with(&format!("{d}.")) || d.starts_with(&format!("{leaf}."))
        });
        if !covered {
            return Err(EngineError::InvalidInput(format!(
                "input yolu '{leaf}' bu action'da tanımlı değil"
            )));
        }
    }

    // TİP denetimi EN SON: önce "yol bildirildi mi" sorusu yanıtlanır, sonra değer.
    // Ters sırada, tanımsız bir yola gönderilen değer için tip hatası verilir ve
    // kullanıcı asıl sorunu (yol bildirilmemiş) göremezdi.
    let declared_owned: Vec<String> = declared.iter().map(|s| (*s).clone()).collect();
    let violations = crate::v22::ctx_types::validate_input(context, &declared_owned, input);
    if !violations.is_empty() {
        return Err(EngineError::InputTypeMismatch(violations));
    }

    Ok(())
}

fn collect_leaf_paths(value: &Value, prefix: String, out: &mut Vec<String>) {
    match value {
        Value::Object(map) if !map.is_empty() => {
            for (k, v) in map {
                let path = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                collect_leaf_paths(v, path, out);
            }
        }
        _ => {
            if !prefix.is_empty() {
                out.push(prefix);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! WOR-44: resolve_candidates artık c_r YANINDA c_u için de aday üretmeli
    //! (pool cache'ine önceden hiç girmeyen c_u-only node'lar için).
    use super::*;
    use crate::types::actor::OrgUnit;
    use crate::types::wfd_v22::COrgu;
    use async_trait::async_trait;

    struct MockOrg;

    #[async_trait]
    impl OrgPort for MockOrg {
        async fn resolve_c_orgu(
            &self,
            anchor: Uuid,
            _expr: &str,
            _orgtnt: Uuid,
        ) -> Result<Vec<OrgUnit>, EngineError> {
            Ok(vec![OrgUnit {
                orgu_id: anchor,
                orgu_type: json!({"type": "branch"}),
                path: "1".into(),
            }])
        }
        async fn check_user_role(&self, _: Uuid, _: Uuid, _: &str) -> Result<bool, EngineError> {
            Ok(true)
        }
        async fn orgtnt_for_orgu(&self, _: Uuid) -> Result<Uuid, EngineError> {
            Ok(Uuid::nil())
        }
    }

    struct DummyRunner;

    #[async_trait]
    impl AutoexecRunner for DummyRunner {
        async fn run(&self, _def: &AutoexecDef, _env: &ExecEnv) -> Result<Value, ExecFailure> {
            unimplemented!("resolve_candidates testlerinde autoexec çalışmaz")
        }
    }

    fn rule(c_r: Option<Vec<&str>>, c_u: Option<Vec<&str>>) -> CandidateActor {
        CandidateActor {
            c_orgu: Some(COrgu::Selector("self".into())),
            c_r: c_r.map(|v| v.into_iter().map(String::from).collect()),
            c_u: c_u.map(|v| v.into_iter().map(|x| CuItem::Literal(x.into())).collect()),
        }
    }

    /// Çapasız kural: aday cache'ine kişi başına TEK girdi yazılır — birim YOK,
    /// `any_orgu: true`. Tenant'taki her ORGU için satır üretmek hem sınırsız olurdu hem
    /// de org ağacı değişince yanlışa düşerdi (cache node'a girişte donar).
    #[tokio::test]
    async fn resolve_candidates_anchorless_emits_single_any_orgu_entry() {
        let org = MockOrg;
        let runner = DummyRunner;
        let engine = Engine {
            org: &org,
            exec: &runner,
            env: Default::default(),
        };
        let actor = Actor {
            orgu_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            role: "clerk".into(),
        };
        let wfah = Wfah::empty();
        let anchorless = CandidateActor {
            c_orgu: None,
            c_r: None,
            c_u: Some(vec![CuItem::Literal("ayse".into())]),
        };

        let out = engine
            .resolve_candidates(&anchorless, &json!({}), &wfah, actor.orgu_id, Uuid::nil())
            .await
            .unwrap();

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].orgu_id, None);
        assert!(out[0].any_orgu);
        assert_eq!(out[0].user_ident.as_deref(), Some("ayse"));
        // Havuz sorgusunun containment filtresi tam bu biçime bakıyor (portal/pool.rs).
        assert_eq!(
            serde_json::to_value(&out[0]).unwrap(),
            json!({ "role": "", "user_ident": "ayse", "any_orgu": true })
        );
    }

    #[tokio::test]
    async fn resolve_candidates_still_emits_role_candidates() {
        let org = MockOrg;
        let runner = DummyRunner;
        let engine = Engine {
            org: &org,
            exec: &runner,
            env: Default::default(),
        };
        let actor = Actor {
            orgu_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            role: "clerk".into(),
        };
        let wfah = Wfah::empty();

        let out = engine
            .resolve_candidates(
                &rule(Some(vec!["branchClerk"]), None),
                &json!({}),
                &wfah,
                actor.orgu_id,
                Uuid::nil(),
            )
            .await
            .unwrap();

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].orgu_id, Some(actor.orgu_id));
        assert_eq!(out[0].role, "branchClerk");
        assert_eq!(out[0].user_id, None);
        assert_eq!(out[0].user_ident, None);
    }

    #[tokio::test]
    async fn resolve_candidates_emits_user_candidate_for_uuid_c_u() {
        let org = MockOrg;
        let runner = DummyRunner;
        let engine = Engine {
            org: &org,
            exec: &runner,
            env: Default::default(),
        };
        let actor = Actor {
            orgu_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            role: "clerk".into(),
        };
        let wfah = Wfah::empty();
        let target_user = Uuid::new_v4();
        let target_user_str = target_user.to_string();

        let out = engine
            .resolve_candidates(
                &rule(None, Some(vec![target_user_str.as_str()])),
                &json!({}),
                &wfah,
                actor.orgu_id,
                Uuid::nil(),
            )
            .await
            .unwrap();

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].orgu_id, Some(actor.orgu_id));
        assert_eq!(out[0].role, "");
        assert_eq!(out[0].user_id, Some(target_user));
        assert_eq!(out[0].user_ident, None);
    }

    #[tokio::test]
    async fn resolve_candidates_emits_ident_candidate_for_non_uuid_c_u() {
        let org = MockOrg;
        let runner = DummyRunner;
        let engine = Engine {
            org: &org,
            exec: &runner,
            env: Default::default(),
        };
        let actor = Actor {
            orgu_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            role: "clerk".into(),
        };
        let wfah = Wfah::empty();

        let out = engine
            .resolve_candidates(
                &rule(None, Some(vec!["jdoe"])),
                &json!({}),
                &wfah,
                actor.orgu_id,
                Uuid::nil(),
            )
            .await
            .unwrap();

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].role, "");
        assert_eq!(out[0].user_id, None);
        assert_eq!(out[0].user_ident.as_deref(), Some("jdoe"));
    }

    #[tokio::test]
    async fn resolve_candidates_unions_role_and_user_entries() {
        let org = MockOrg;
        let runner = DummyRunner;
        let engine = Engine {
            org: &org,
            exec: &runner,
            env: Default::default(),
        };
        let actor = Actor {
            orgu_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            role: "clerk".into(),
        };
        let wfah = Wfah::empty();

        let out = engine
            .resolve_candidates(
                &rule(Some(vec!["creditAnalyst"]), Some(vec!["jdoe"])),
                &json!({}),
                &wfah,
                actor.orgu_id,
                Uuid::nil(),
            )
            .await
            .unwrap();

        assert_eq!(out.len(), 2);
        assert!(out
            .iter()
            .any(|c| c.role == "creditAnalyst" && c.user_id.is_none() && c.user_ident.is_none()));
        assert!(out
            .iter()
            .any(|c| c.role.is_empty() && c.user_ident.as_deref() == Some("jdoe")));
    }

    // ---- 2026-08-13: görünürlük projeksiyonunun yazıcısı ----

    /// `view_grants` için minimum WFD: iki grant kuralı (biri guard'lı) + bir node.
    /// Ham JSON kullanılır çünkü şema kapısı (`from_json`) de birlikte sınanmış olur.
    fn grants_wfd(when: Option<&str>) -> Wfd {
        let guard = match when {
            Some(w) => format!(r#", "when": "{w}""#),
            None => String::new(),
        };
        let doc = format!(
            r#"{{
              "wfd_version": "2.3",
              "id": "grant-test",
              "name": "Grant Test",
              "version": "1.0.0",
              "context": {{ "type": "object", "properties": {{}} }},
              "listable": [
                {{ "c_a": {{ "c_orgu": "self", "c_r": ["mudur"] }}{guard} }}
              ],
              "wf_admin": [
                {{ "c_a": {{ "c_orgu": "self", "c_r": ["wfAdmin"] }} }}
              ],
              "start": [ {{ "id": "start__adim", "action": "basla" }} ],
              "nodes": {{ "adim": {{ "c_a": {{ "c_orgu": "self", "c_r": ["memur"] }} }} }},
              "actions": {{
                "basla": {{ "input": {{ "required": [], "optional": [] }},
                            "from": "adim", "wft": {{ "terminal": "bitti" }} }}
              }},
              "terminals": [
                {{ "id": "bitti", "label": "Bitti", "wfe_end_response": {{ "status": "ok" }} }}
              ]
            }}"#
        );
        Wfd::from_json(&doc).expect("fixture geçerli olmalı")
    }

    fn grants_engine<'a>(org: &'a MockOrg, runner: &'a DummyRunner) -> Engine<'a> {
        Engine {
            org,
            exec: runner,
            env: Default::default(),
        }
    }

    /// `listable` ∪ `wf_admin` BİRLEŞİR ve çapa (origin) her ikisine uygulanır.
    #[tokio::test]
    async fn view_grants_unions_listable_and_wf_admin() {
        let org = MockOrg;
        let runner = DummyRunner;
        let engine = grants_engine(&org, &runner);
        let origin = Uuid::new_v4();

        let out = engine
            .view_grants(
                &grants_wfd(None),
                &json!({}),
                &Wfah::empty(),
                Some("adim"),
                Uuid::new_v4(),
                origin,
                Uuid::nil(),
            )
            .await
            .unwrap();

        // MockOrg `self`i çapaya çözer → iki kural da origin birimine yazılır.
        let roles: Vec<&str> = out.iter().map(|c| c.role.as_str()).collect();
        assert!(roles.contains(&"mudur"), "listable grant'ı yok: {roles:?}");
        assert!(
            roles.contains(&"wfAdmin"),
            "wf_admin grant'ı yok: {roles:?}"
        );
        assert!(out.iter().all(|c| c.orgu_id == Some(origin)));
        // Node c_a'sı (memur) BURAYA GİRMEZ: o `current_c_a`nın işi, ve iş
        // bitince silinir. Karıştırılırsa bitmiş işin görünürlüğü sızar.
        assert!(
            !roles.contains(&"memur"),
            "node c_a grant'a karışmış: {roles:?}"
        );
    }

    /// `when` guard'ı FALSE olan kural grant ÜRETMEZ — havuzun eski
    /// "guard'ı yok say, over-inclusive kabul et" davranışının kapandığı yer.
    #[tokio::test]
    async fn view_grants_applies_when_guard() {
        let org = MockOrg;
        let runner = DummyRunner;
        let engine = grants_engine(&org, &runner);
        let origin = Uuid::new_v4();
        let wfd = grants_wfd(Some("$ctx.tutar >= 100"));

        let low = engine
            .view_grants(
                &wfd,
                &json!({"tutar": 10}),
                &Wfah::empty(),
                Some("adim"),
                Uuid::new_v4(),
                origin,
                Uuid::nil(),
            )
            .await
            .unwrap();
        assert!(
            !low.iter().any(|c| c.role == "mudur"),
            "guard false iken listable grant'ı yazılmamalı: {low:?}"
        );
        // wf_admin guard'sız → o kalır (guard kural BAŞINA işler).
        assert!(low.iter().any(|c| c.role == "wfAdmin"));

        let high = engine
            .view_grants(
                &wfd,
                &json!({"tutar": 250}),
                &Wfah::empty(),
                Some("adim"),
                Uuid::new_v4(),
                origin,
                Uuid::nil(),
            )
            .await
            .unwrap();
        assert!(high.iter().any(|c| c.role == "mudur"));
    }

    /// Çapa değişince grant'ın BİRİMİ değişir — projeksiyonun WFE'ye bağlı
    /// olduğunun kanıtı (aynı belge, iki farklı WFE → iki farklı grant kümesi).
    #[tokio::test]
    async fn view_grants_are_anchored_per_wfe() {
        let org = MockOrg;
        let runner = DummyRunner;
        let engine = grants_engine(&org, &runner);
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let wfd = grants_wfd(None);

        let ga = engine
            .view_grants(
                &wfd,
                &json!({}),
                &Wfah::empty(),
                None,
                Uuid::new_v4(),
                a,
                Uuid::nil(),
            )
            .await
            .unwrap();
        let gb = engine
            .view_grants(
                &wfd,
                &json!({}),
                &Wfah::empty(),
                None,
                Uuid::new_v4(),
                b,
                Uuid::nil(),
            )
            .await
            .unwrap();

        assert!(ga.iter().all(|c| c.orgu_id == Some(a)));
        assert!(gb.iter().all(|c| c.orgu_id == Some(b)));
    }

    // ---- 2026-08-13: NODE listable projeksiyonu (`node_view_grants`) ----

    /// Node listable'ı olan minimum WFD. Node c_a'sı (`memur`) ile node
    /// listable'ı (`izleyen`) AYRI rollerdir: projeksiyonun ikisini karıştırıp
    /// karıştırmadığı ancak böyle görülür. Ham JSON → şema kapısı da koşar.
    fn node_grants_wfd(when: Option<&str>) -> Wfd {
        let guard = match when {
            Some(w) => format!(r#", "when": "{w}""#),
            None => String::new(),
        };
        let doc = format!(
            r#"{{
              "wfd_version": "2.3",
              "id": "node-grant-test",
              "name": "Node Grant Test",
              "version": "1.0.0",
              "context": {{ "type": "object", "properties": {{}} }},
              "start": [ {{ "id": "start__adim", "action": "basla" }} ],
              "nodes": {{
                "adim": {{
                  "c_a": {{ "c_orgu": "self", "c_r": ["memur"] }},
                  "listable": [
                    {{ "c_a": {{ "c_orgu": "self", "c_r": ["izleyen"] }}{guard} }}
                  ]
                }},
                "sessiz": {{ "c_a": {{ "c_orgu": "self", "c_r": ["memur"] }} }}
              }},
              "actions": {{
                "basla": {{ "input": {{ "required": [], "optional": [] }},
                            "from": "adim", "wft": {{ "terminal": "bitti" }} }}
              }},
              "terminals": [
                {{ "id": "bitti", "label": "Bitti", "wfe_end_response": {{ "status": "ok" }} }}
              ]
            }}"#
        );
        Wfd::from_json(&doc).expect("fixture geçerli olmalı")
    }

    /// Node listable ÇÖZÜLÜR, node c_a'sı KARIŞMAZ ve çapa `origin`dir.
    /// Karışsa `current_view_c_a` ACT adayı taşır ve kolon "görme" anlamını
    /// yitirirdi (WOR-44 katlamasının aynı hatası, node ekseninde).
    #[tokio::test]
    async fn node_view_grants_resolves_only_node_listable() {
        let org = MockOrg;
        let runner = DummyRunner;
        let engine = grants_engine(&org, &runner);
        let origin = Uuid::new_v4();

        let out = engine
            .node_view_grants(
                &node_grants_wfd(None),
                "adim",
                &json!({}),
                &Wfah::empty(),
                Some("adim"),
                Uuid::new_v4(),
                origin,
                Uuid::nil(),
            )
            .await
            .unwrap();

        let roles: Vec<&str> = out.iter().map(|c| c.role.as_str()).collect();
        assert!(roles.contains(&"izleyen"), "node listable yok: {roles:?}");
        assert!(
            !roles.contains(&"memur"),
            "node c_a'sı görünürlük projeksiyonuna karışmış: {roles:?}"
        );
        assert!(
            out.iter().all(|c| c.orgu_id == Some(origin)),
            "çapa origin değil"
        );
    }

    /// `when` guard'ı UYGULANIR — kök `listable` ile birebir aynı semantik.
    /// Uygulanmazsa kolon over-inclusive olur ve tam da 2026-08-13'te kapatılan
    /// "guard'ı yok say" davranışı node ekseninde geri gelir.
    #[tokio::test]
    async fn node_view_grants_applies_when_guard() {
        let org = MockOrg;
        let runner = DummyRunner;
        let engine = grants_engine(&org, &runner);
        let origin = Uuid::new_v4();
        let wfd = node_grants_wfd(Some("$ctx.tutar >= 100"));

        let low = engine
            .node_view_grants(
                &wfd,
                "adim",
                &json!({"tutar": 10}),
                &Wfah::empty(),
                Some("adim"),
                Uuid::new_v4(),
                origin,
                Uuid::nil(),
            )
            .await
            .unwrap();
        assert!(
            low.is_empty(),
            "guard false iken node listable yazılmamalı: {low:?}"
        );

        let high = engine
            .node_view_grants(
                &wfd,
                "adim",
                &json!({"tutar": 250}),
                &Wfah::empty(),
                Some("adim"),
                Uuid::new_v4(),
                origin,
                Uuid::nil(),
            )
            .await
            .unwrap();
        assert!(high.iter().any(|c| c.role == "izleyen"));
    }

    /// Guard'ın gördüğü `$node` ÇAĞIRANDAN gelir: tek-kol yolunda varılan node,
    /// paralel kol projeksiyonunda `None` (wfe-seviyesi `current_node` NULL'dır).
    /// `can_view` okuma anında hangi değeri görüyorsa projeksiyon da onu
    /// görmelidir; ayrışırsa `$node`e bakan bir guard iki okumada farklı cevap
    /// verir ve kontrat denetçisi ayrışma raporlar.
    #[tokio::test]
    async fn node_view_grants_guard_sees_the_caller_supplied_node() {
        let org = MockOrg;
        let runner = DummyRunner;
        let engine = grants_engine(&org, &runner);
        let origin = Uuid::new_v4();
        // JSON içinde geçtiği için tırnaklar kaçışlıdır: guard ZEN'e
        // `$node == "adim"` olarak varır.
        let wfd = node_grants_wfd(Some(r#"$node == \"adim\""#));

        let with_node = engine
            .node_view_grants(
                &wfd,
                "adim",
                &json!({}),
                &Wfah::empty(),
                Some("adim"),
                Uuid::new_v4(),
                origin,
                Uuid::nil(),
            )
            .await
            .unwrap();
        assert!(with_node.iter().any(|c| c.role == "izleyen"));

        let without_node = engine
            .node_view_grants(
                &wfd,
                "adim",
                &json!({}),
                &Wfah::empty(),
                None,
                Uuid::new_v4(),
                origin,
                Uuid::nil(),
            )
            .await
            .unwrap();
        assert!(
            without_node.is_empty(),
            "guard `$node`u çağırandan almıyor: {without_node:?}"
        );
    }

    /// `listable`ı OLMAYAN node ve BİLİNMEYEN node boş liste verir — bu bir
    /// yetki sorusu değil cache üretimidir, commit'i düşürmemeli.
    #[tokio::test]
    async fn node_view_grants_is_empty_without_rules_or_node() {
        let org = MockOrg;
        let runner = DummyRunner;
        let engine = grants_engine(&org, &runner);
        let wfd = node_grants_wfd(None);

        for key in ["sessiz", "hiç-yok"] {
            let out = engine
                .node_view_grants(
                    &wfd,
                    key,
                    &json!({}),
                    &Wfah::empty(),
                    Some(key),
                    Uuid::new_v4(),
                    Uuid::new_v4(),
                    Uuid::nil(),
                )
                .await
                .unwrap();
            assert!(out.is_empty(), "'{key}' için boş liste beklenir: {out:?}");
        }
    }

    /// Kök `view_grants` node listable'ı TOPLAMAZ: kalıcı kolon durum-bağımlı
    /// grant taşırsa node'dan çıkmış (hatta bitmiş) iş görünür kalır.
    #[tokio::test]
    async fn view_grants_does_not_pick_up_node_listable() {
        let org = MockOrg;
        let runner = DummyRunner;
        let engine = grants_engine(&org, &runner);

        let out = engine
            .view_grants(
                &node_grants_wfd(None),
                &json!({}),
                &Wfah::empty(),
                Some("adim"),
                Uuid::new_v4(),
                Uuid::new_v4(),
                Uuid::nil(),
            )
            .await
            .unwrap();

        assert!(
            !out.iter().any(|c| c.role == "izleyen"),
            "node listable kalıcı `view_c_a` projeksiyonuna sızmış: {out:?}"
        );
    }
}
