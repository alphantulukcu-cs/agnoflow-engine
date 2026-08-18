// reference-types.rs — v2.2 referans serde modeli + canonical c_a formu + §2a slug.
// Kabul testleri (main): (1) golden fixture kayipsiz parse, (2) node key + slug DOKUMU
// (kimlik TASARIMCININ - 2026-08-12'den beri key == slug(c_a) BEKLENMEZ, slug yalniz bilgi),
// (3) canonical c_a uniqueness (2026-08-14: validator duplicate_c_a ile HATA).
//
// SLUG'IN YERI (2026-08-14): `CandidateActor::slug` burada spec §2a'nin referans
// implementasyonu olarak DURUR - tuketicisi EDITORDUR (yeni node'a varsayilan anahtar
// onerir), motor DEGIL. agnoflow-backend'deki ikizi (`wfe_core::types::wfd_v22`) bu
// yuzden SILINDI: orada kimlik uretmiyordu, cagirani yoktu. Motorda duran tek sey
// `canonical()` (tekillik) ve onun `COrgu::canonical_key` parcasidir.
//
// BU DOSYA ARTIK DERLENIYOR (2026-08-18). `crates/wfe-core/tests/reference_types_parity.rs`
// onu `#[path]` ile modul olarak alir; ayni test motorun modeliyle TIP ve ALAN paritesini
// dogrular ve `docs/spec/examples/*.json`in hepsini bu modelle parse eder. Sebep: dosya
// `docs/` altinda oldugu icin hicbir derleyici bakmiyordu ve SESSIZCE curumustu — 2026-08-17
// olcumunde 8 tip (CallDef/CallRef/CallMode/StartAs/CuItem/GlobalTarget/CaGrantRule) ve
// 10'dan fazla alan eksikti, `c_u` hala `Vec<String>` idi. Motora alan eklendiginde BURASI
// da guncellenir; unutulursa parite testi patlar.
#![allow(dead_code)]
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

fn default_true() -> bool { true }
fn default_timeout() -> u32 { 60 }

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wfd {
    pub wfd_version: String,
    #[serde(default)]
    pub expression_language: Option<String>,
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub timeout: Option<String>,
    pub context: Value,
    pub nodes: BTreeMap<String, NodeDef>,
    pub start: Vec<StartRule>,
    pub actions: BTreeMap<String, ActionDef>,
    #[serde(default)]
    pub autoexec: BTreeMap<String, AutoexecDef>,
    // WFC katalogu (2026-07-30) — baska bir WFD'yi cagirma sozlesmeleri. NE cagrilacagini
    // ve hangi girdiyle cagrilacagini tutar; NASIL cagrildigi referans yerindedir
    // (`nodes.<k>.call` ya da `terminals[].call`). autoexec <-> trigger ayriminin aynisi.
    #[serde(default)]
    pub calls: BTreeMap<String, CallDef>,
    pub transitions: Vec<Transition>,
    pub terminals: Vec<Terminal>,
    #[serde(default)]
    pub listable: Vec<ListableRule>,
    // T-A5 (2026-08-11) — akis-ici yetkili havuzu. `listable` ile AYNI kayit sekli ama
    // AYRI dizi: biri gorme hakki verir, oteki akisi yonetme (claim devri + escalation
    // mudahalesi + gorunurluk). AKSIYON yetkisi VERMEZ.
    #[serde(default)]
    pub wf_admin: Vec<CaGrantRule>,
    #[serde(default)]
    pub attachments: BTreeMap<String, AttachmentGroup>, // opsiyonel ek-belge katalogu
    // WOR-84: DEPRECATED — motor HIC okumaz (v1 kalintisi). Terminal wft: {terminal} ile
    // verilir. Parse edilmeye devam eder ama validator terminal_when_ignored uyarisi basar
    // ve yeniden serilestirmede DUSER (skip_serializing).
    #[serde(default, skip_serializing)]
    pub terminal_when: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeDef {
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub c_a: CandidateActor,          // v2.2: TEK kural
    #[serde(default)]
    pub reassign: Option<CandidateActor>, // Madde 7: opsiyonel claim devri yetkisi (amir)
    #[serde(default)]
    pub escalation: Vec<EscalationStep>,
    #[serde(default)]
    pub claim_timeout: Option<ClaimTimeout>, // SLA-1: claim eden aktor zamaninda aksiyon almazsa
    #[serde(default)]
    pub attachments: Vec<AttachmentRef>, // root attachments katalogundaki grup referanslari
    // WFC (2026-07-30) — alt akis cagrisi (mode: wait | detached). Bu blogu tasiyan node
    // bir WFC node'udur: insan ACT'i ALINAMAZ (transitions[].from icinde yer alamaz),
    // cikisi call.wft'dir, escalation/claim_timeout/attachments/reassign tasiyamaz.
    #[serde(default)]
    pub call: Option<CallRef>,
    // 2026-08-13: node-seviyesi gorunurluk grant'i - kok `listable` ile AYNI tip. WFE bu
    // node'dayken kurallardan birine uyan aktor gorebilir; kok listable KALICIDIR (terminal'de
    // de gorur), bu DURUMA BAGLIDIR (node'dan cikinca biter). ACT/claim VERMEZ.
    #[serde(default)]
    pub listable: Vec<ListableRule>,
}

/// Node'un ek-belge referansi. Iki bicim de "bu grup burada TOPLANIR" der; fark KAPIDIR.
/// - `"grup"`                                -> node'un TUM aksiyonlarina kapi
/// - `{"group":"grup","actions":["onayla"]}` -> yalniz sayilan aksiyonlara kapi
/// - `{"group":"grup","actions":[]}`         -> hicbir aksiyonu kapamaz (opsiyonel yukleme)
///
/// `actions` Option'dir: "verilmedi" (tumu) ile "bos verildi" (hicbiri) zit anlamlidir.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum AttachmentRef {
    Group(String),
    Scoped(ScopedAttachmentRef),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopedAttachmentRef {
    pub group: String,
    #[serde(default)]
    pub actions: Option<Vec<String>>, // None = tum aksiyonlar
}

/// SLA-1. Sure claim anindan itibaren olculur. `wft` verilmezse claim temizlenir
/// ve is ayni havuza doner; verilirse WFE o hedefe tasinir.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimTimeout {
    pub after: String,
    #[serde(default)]
    pub wfes_effects: Option<WfesEffects>,
    /// Bare NODE key — {node}/{terminal} sarmalayicisi YOK. Terminal YASAK
    /// (2026-07-28, validator `sla_terminal_target`): SLA akisi bitirmez.
    #[serde(default)]
    pub wft: Option<String>,
    /// 2026-08-03 (WOR-56): paralel kolda tetiklendiginde kardes kollari dusurup
    /// paraleli sonlandirir (bu bicimde `wft` ZORUNLU).
    #[serde(default)]
    pub collapses_parallel: bool,
}

/// Ek-belge katalog grubu. Dosyalar engine'de degil portal opendal storage'inda tutulur.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachmentGroup {
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub items: Vec<AttachmentItem>,
}

/// Katalog grubundaki tek dosya slotu. id = "verilen dosya ismi"; grup icinde tekil.
/// Storage anahtari: attachments/{wfe_id}/{grup}/{id}.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachmentItem {
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_true")]
    pub required: bool,               // yuklenmeden gruba bagli node'dan aksiyon submit edilemez
    #[serde(default)]
    pub formats: Vec<AttachmentFormatRule>, // per-format MIME grubu + o gruba ozel boyut siniri
}

/// Tek format kurali: MIME grubu + o gruba ozel opsiyonel boyut siniri.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachmentFormatRule {
    pub accept: Vec<String>,
    #[serde(default)]
    pub max_size_mb: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EscalationStep {
    pub after: String,
    #[serde(default)]
    pub wfes_effects: Option<WfesEffects>,
    /// wft XOR terminate — biri zorunlu (2026-07-16 SLA sozlesmesi, validator XOR).
    #[serde(default)]
    pub wft: Option<Wft>,
    /// SLA-2: true ise instance `terminated` olur (end_response reason = SLA.Dwell).
    #[serde(default)]
    pub terminate: Option<bool>,
}

/// Tek C_A kurali — IKI bicim. CAPALI: c_orgu verilir, match = resolved(c_orgu) AND
/// (rol_match OR user_match). CAPASIZ: c_orgu HIC verilmez, c_u zorunlu / c_r yasak,
/// match = user_match (kisi tenant genelinde eslesir).
/// Verilmeyen alan o kanaldan match uretmez (yok = false). c_u match'i rol-agnostiktir.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateActor {
    /// None = capasiz bicim (orgu kanali kisitsiz).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c_orgu: Option<COrgu>,
    #[serde(default)]
    pub c_r: Option<Vec<String>>,
    /// 2026-08: sabit kimlik ya da `$ctx` referansi (bkz. `CuItem`).
    #[serde(default)]
    pub c_u: Option<Vec<CuItem>>,
}

/// `c_u` ogesi — IKI bicim.
/// - `"user_ayse"`            -> sabit kimlik (uuid ya da kullanici adi)
/// - `{"from": "$ctx.musteri.temsilci"}` -> ctx'ten OKUNAN kimlik
///
/// Neden sihirli onek (`"$ctx.x"` duz string) DEGIL: `c_u` buyuyecek bir alandir; bir
/// onek konvansiyonu her yeni yetenegi semadan denetlenemez ve editorun tip sistemine
/// gorunmez kilardi. Tekillik karsilastirmasi (`canonical`) iki variant'i AYIRIR:
/// `Literal("x")` ile `Ref { from: "x" }` ayni c_a sayilmaz.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(untagged)]
pub enum CuItem {
    Literal(String),
    Ref { from: String },
}

impl CuItem {
    /// Slug/karsilastirma icin metin kaynagi. Referansta YOL kullanilir — cozulmus
    /// kimlik calisma anindadir, belgede yoktur.
    pub fn source(&self) -> &str {
        match self { CuItem::Literal(s) => s, CuItem::Ref { from } => from }
    }
}

impl CandidateActor {
    pub fn matches(&self, actor_orgu: &str, actor_role: &str, actor_user: &str,
                   resolved_orgu: &str) -> bool {
        let in_orgu  = actor_orgu == resolved_orgu;
        let role_hit = self.c_r.as_ref().is_some_and(|r| r.iter().any(|x| x == actor_role));
        // Referans matcher YALNIZ sabit kimligi bilir: `CuItem::Ref` ctx'ten cozulur ve
        // ctx bu saf modelin disindadir (motorda `resolver::resolve_cu_ident`).
        let user_hit = self.c_u.as_ref().is_some_and(|u| {
            u.iter().any(|x| matches!(x, CuItem::Literal(s) if s == actor_user))
        });
        in_orgu && (role_hit || user_hit)
    }

    /// §2a slug: orgu_slug [+ "__" + sirali_roller] [+ "__u_" + sirali_userlar].
    ///
    /// KIMLIK DEGILDIR (2026-08-12): node anahtarini tasarimci yazar. Bu hesap yalnizca
    /// EDITORUN yeni node'a onerdigi VARSAYILAN anahtardir; motor `c_a`'dan anahtar
    /// turetmez ve bu fonksiyonun motor tarafinda karsiligi da yoktur. Tekillik
    /// karsilastirmasi `canonical()` ile yapilir, slug ile DEGIL.
    pub fn slug(&self) -> String {
        let mut parts = vec![match &self.c_orgu {
            Some(c) => c.slug(),
            None => "any".to_string(),
        }];
        if let Some(r) = &self.c_r {
            let mut r: Vec<String> = r.iter().map(|x| sanitize(x)).collect();
            r.sort();
            parts.push(r.join("-"));
        }
        if let Some(u) = &self.c_u {
            let mut u: Vec<String> = u.iter().map(|x| sanitize(x.source())).collect();
            u.sort();
            parts.push(format!("u_{}", u.join("-")));
        }
        parts.join("__")
    }

    /// Uniqueness karsilastirmasi icin canonical form (rol/user siralari normalize).
    pub fn canonical(&self) -> String {
        let mut r = self.c_r.clone().unwrap_or_default(); r.sort();
        let mut u = self.c_u.clone().unwrap_or_default(); u.sort();
        format!("{:?}|r:{:?}|u:{:?}", self.c_orgu.as_ref().map(COrgu::slug), r, u)
    }
}

fn sanitize(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() { out.push(c); }
        else if !out.is_empty() && !out.ends_with('_') { out.push('_'); }
    }
    out.trim_matches('_').to_string()
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum COrgu {
    Selector(String),
    Anchor { from: AnchorFrom, traverse: String },
}

impl COrgu {
    pub fn slug(&self) -> String {
        match self {
            COrgu::Selector(s) => sanitize(s),
            COrgu::Anchor { from, traverse } => match from {
                AnchorFrom::Ctx(p) => format!("{}_{}", sanitize(p), sanitize(traverse)),
                AnchorFrom::Wfah { wfah, .. } =>
                    format!("wfah_{}_{}", sanitize(wfah), sanitize(traverse)),
            },
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum AnchorFrom {
    Ctx(String),
    Wfah { wfah: String, field: String, #[serde(default)] occurrence: Option<String> },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionDef {
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub input: InputDef,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputDef {
    pub required: Vec<String>,
    pub optional: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutoexecDef {
    #[serde(rename = "type")]
    pub kind: AutoexecType,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u32,
    pub config: Value,
    #[serde(default)]
    pub wfes_effects: Option<WfesEffects>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AutoexecType { Rest, Sql, Calc, Python, Lambda }

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WfesEffects {
    pub set: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerInvocation {
    #[serde(rename = "use")]
    pub use_: String,
    #[serde(default)]
    pub when: Option<String>,
    #[serde(default = "default_true")]
    pub required: bool,
    #[serde(default)]
    pub retry: Vec<Retrier>,
    #[serde(default)]
    pub catch: Option<CatchDef>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Retrier {
    pub error_equals: Vec<String>,
    #[serde(default = "Retrier::d_interval")]
    pub interval_seconds: u32,
    #[serde(default = "Retrier::d_attempts")]
    pub max_attempts: u32,
    #[serde(default = "Retrier::d_backoff")]
    pub backoff_rate: f64,
    #[serde(default)]
    pub max_delay_seconds: Option<u32>,
}
impl Retrier {
    fn d_interval() -> u32 { 1 }
    fn d_attempts() -> u32 { 3 }
    fn d_backoff() -> f64 { 2.0 }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatchDef {
    #[serde(default = "CatchDef::d_all")]
    pub error_equals: Vec<String>,
    pub wfes_effects: WfesEffects,
}
impl CatchDef { fn d_all() -> Vec<String> { vec!["WFD.ALL".into()] } }

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Wft {
    Node { node: String },
    Terminal { terminal: String },
    // GLB (api-contract-v2, 2026-08-12): hedefi BELGE degil aksiyonu alan KISI secer.
    // Eskiden hedef aksiyon ANAHTARINA kodlaniyordu (`Geri_Gonder__gt__self__mudur`) ve
    // hedef basina ayri aksiyon + ayri transition uretiliyordu; artik TEK aksiyon, TEK
    // transition var ve secim calisma aninda `apply(..., target)` ile gelir. Secim bir
    // action input DEGILDIR: $ctx'e yazilmaz, wfes_effects gerektirmez, $wfah'a girmez.
    // Yalniz `transitions[].wft` icinde gecerli (validator `global_action_placement`).
    Targets { targets: Vec<GlobalTarget> },
    Conditional {
        conditions: Vec<WftCondition>,
        #[serde(default)]
        default: Option<WftTarget>,
    },
    // WOR-31: fork/join. branches >= 2, distinct (custom validator); join = AND-join
    // hedefi (son aktif kol vardiginda). Start kuralinda yasak, nested fork yasak.
    Parallel { parallel: ParallelSpec },
    // WOR-56: paralel dalda sonlandiran aksiyon (collapse+goto). Alininca TUM kardes
    // kollar cancelled, WFE paralel moddan cikip `collapse` hedefine gider (node ya da
    // terminal, RASTGELE). Node hedef = paralel modu bitirir (BranchMoveTo DEGIL);
    // terminal hedef = WOR-31 kol->terminal collapse'unun ozel hali. Yalniz kol
    // baglaminda gecerli (start'ta yasak).
    Collapse { collapse: WftTarget },
}

/// `Wft::Targets` ogesi — secilebilir TEK bir hedef. Duz `Vec<String>` yerine obje
/// olmasinin gerekcesi: hedef basina `when` guard'i / etiket gibi alanlar eklenirse
/// sekil kirilmadan buyur. Bugun yalniz `node` tasir — TERMINAL hedef YOKTUR.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlobalTarget {
    pub node: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParallelSpec {
    pub branches: Vec<String>,
    pub join: WftTarget,
    // WOR-72: birlestirme mantigi. Verilmezse `and` (WOR-31) — serialize'da atlanir.
    #[serde(default)]
    pub join_mode: JoinMode,
    // WOR-72: OR modunda yeterli varis sayisi (K-of-N). Yalniz join_mode: or ile
    // verilebilir; verilmezse 1 = ilk varan kazanir. 1 <= K < branches.len().
    #[serde(default)]
    pub join_threshold: Option<u32>,
    // WOR-73: ZEN join kosulu. Yalniz join_mode: expr ile verilebilir ve o modda
    // ZORUNLUDUR. Namespace: $branches.<kolGirisNodeKey> (bool), $arrived (dizi);
    // $ctx/$wfah/$prev/$first/$actor da acik. Referanslar bu forkun kollari olmali.
    #[serde(default)]
    pub join_when: Option<String>,
}

// WOR-72: `and` = tum kollar beklenir (WOR-31). `or` = K-of-N quorum; esik dolunca
// paralel mod OTORITER biter, kalan aktif kollar cancelled (collapse semantigi).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JoinMode {
    #[default]
    And,
    Or,
    // WOR-73: kosul `join_when` ZEN ifadesidir; true olunca Or ile ayni iptal
    // semantigi uygulanir. Son kol da varip ifade false ise WFD.JoinUnsatisfied.
    Expr,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum WftTarget {
    Node { node: String },
    Terminal { terminal: String },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WftCondition {
    pub when: String,
    #[serde(default)]
    pub node: Option<String>,
    #[serde(default)]
    pub terminal: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartRule {
    pub id: String,
    pub from: String,                 // v2.2 simetrik start: giris node id (nodes katalogunda, c_a'yi tasir)
    pub action: String,               // start aksiyonunun gercek adi; actions{} icinde normal bir ACT'e karsilik gelir
    #[serde(default)]
    pub wfes_effects: Option<WfesEffects>,
    #[serde(default)]
    pub trigger: Vec<TriggerInvocation>,
    pub wft: Wft,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transition {
    pub id: String,
    pub from: FromNodes,
    #[serde(default)]
    pub when: Option<String>,
    pub action: String,
    #[serde(default)]
    pub c_a: Option<CandidateActor>,  // v2.2: TEK kural (EK kisit)
    #[serde(default)]
    pub wfes_effects: Option<WfesEffects>,
    #[serde(default)]
    pub trigger: Vec<TriggerInvocation>,
    pub wft: Wft,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum FromNodes {
    One(String),
    Many(Vec<String>),
}
impl FromNodes {
    pub fn iter(&self) -> Vec<&str> {
        match self { FromNodes::One(s) => vec![s.as_str()],
                     FromNodes::Many(v) => v.iter().map(String::as_str).collect() }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Terminal {
    /// MAKINE kimligi (`^[a-zA-Z0-9_]+$`, belge icinde benzersiz) — kullanici metni
    /// DEGILDIR (api-contract-v2, 2026-08-12). Ekrana `label` basilir.
    pub id: String,
    /// Gosterim adi. Verilmezse motor id'nin okunur halini uretir (`display::humanize_key`).
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub wfes_effects: Option<WfesEffects>,
    pub wfe_end_response: BTreeMap<String, Value>,
    /// 2026-08-17: terminal-seviyesi gorunurluk grant'i — WFE BU terminal'de bittiyse
    /// kurallardan birine uyan aktor gorebilir. Kok `listable` ve `nodes.<k>.listable`
    /// ile AYNI sekil; omru node listable'in TERSI: terminal'den cikis olmadigi icin
    /// KALICI, ama kok listable'dan farkli olarak SONUCA BAGLI. Yalniz BASARILI Terminal
    /// sonucunda islerlidir (Failed/Terminated'da varilmis bir terminal YOKTUR).
    /// ACT/claim VERMEZ.
    #[serde(default)]
    pub listable: Vec<CaGrantRule>,
    /// WFC — ardil akis cagrisi (mode: terminal). "Bir is akisinin bitisi baska bir is
    /// akisinin baslangici." Donus YOKTUR.
    #[serde(default)]
    pub call: Option<CallRef>,
}

// ---- WFC: is akisi cagrisi (2026-07-30) ----
// Katalog <-> referans ayrimi autoexec <-> trigger ile AYNIDIR: `wfd.calls` NE
// cagrilacagini tutar (paylasilabilir), `nodes.<k>.call` / `terminals[].call` NASIL
// cagrildigini (yerlesime ozel).

/// Root `calls` katalog kaydi — moddan BAGIMSIZDIR, ucu modda da kullanilabilir.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallDef {
    /// Cagrilan WFD'nin DOKUMAN kimligi (`wfd.id`), DB uuid'si DEGIL.
    pub wfd_id: String,
    /// Yoksa cagri anindaki en son yayinlanmis surum; varsa o surume pinlenir.
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Cagrilanin start[] kurali >=2 ise ZORUNLU (StartRule.id).
    #[serde(default)]
    pub start: Option<String>,
    /// WFC-IN — cagrilanin girdi adi -> cagiran baglamindaki kaynak. Izinli: `$ctx.<yol>`,
    /// `$actor`, `$timestamp`, `$wfe_id`, sabit degerler. `$action.input.*` YASAKTIR:
    /// (1) moddan bagimsizlik (terminal modunda ACT girdisi guvenilir bicimde yok),
    /// (2) WOR-70 — ctx'e tek yazma yolu effects'tir.
    #[serde(default)]
    pub input: BTreeMap<String, Value>,
}

/// Bir katalog kaydina yapilan REFERANS. Node ve terminal yerlesimleri AYNI sekli
/// kullanir; yanlis yere yazilmis alani validator reddeder.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallRef {
    #[serde(rename = "use")]
    pub use_: String,
    #[serde(default)]
    pub mode: CallMode,
    /// Yalniz `wait` — asilirsa `$call.status == "timeout"`.
    #[serde(default)]
    pub timeout: Option<String>,
    /// WFC-RETURN effects — `$call.result.*` / `$call.status` / `$call.wfe_id` gorunur.
    #[serde(default)]
    pub wfes_effects: Option<WfesEffects>,
    /// WFC-RETURN hedefi — NODE yerlesiminde ZORUNLU. Node ya da terminal olabilir:
    /// SLA'nin "terminal hedef yasak" kisiti BURADA GECERSIZDIR, cunku bu bir
    /// zamanlayici degil cagrilanin sonucuna dayanan bir karardir.
    #[serde(default)]
    pub wft: Option<Wft>,
    /// Yalniz `mode: terminal`. `actor` = terminale getiren ACT'in aktoru, `system` =
    /// sistem aktoru. Kok timeout (SLA-3) da bu terminale getirebiliyorsa `system` SARTTIR.
    #[serde(default)]
    pub start_as: Option<StartAs>,
    /// Yalniz `mode: terminal`. Ardil dongusune ACIK izin + ust sinir; verilmezse dongu
    /// validator tarafindan reddedilir (`call_next_cycle`).
    #[serde(default)]
    pub max_next: Option<u32>,
    /// K8: alt akisin PUBLISHED notlari cagiranin not listesine girsin mi. Motor bu alani
    /// OKUMAZ, yalniz tasir (not defteri motorun disinda ucuncu bir katmandir).
    #[serde(default)]
    pub notes_visible_to_caller: bool,
}

/// WFC modu — cagrinin TEK belirleyici ekseni. Yerlesim de moda baglidir:
/// `wait`/`detached` yalniz `nodes.<k>.call`, `terminal` yalniz `terminals[].call`
/// (validator `call_mode_placement`).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CallMode {
    /// Cagiran node'da BEKLER; cagrilan bitince WFC-RETURN islenir.
    #[default]
    Wait,
    /// Cagrilan baslatilir, cagiran HEMEN devam eder (`$call.result.*` daima bos).
    Detached,
    /// Cagiran BITER, ardil akis baslar. Donus YOKTUR.
    Terminal,
}

/// Ardil akisi kimin baslattigi sayilir (yalniz `mode: terminal`).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StartAs {
    #[default]
    Actor,
    System,
}

/// C_A tabanli grant kaydi: kural + opsiyonel `when` guard'i.
///
/// DORT yer bu sekli paylasir — `wfd.listable[]` (kalici gorme), `nodes.<k>.listable[]`
/// (2026-08-13, duruma bagli), `terminals[].listable[]` (2026-08-17, sonuca bagli) ve
/// `wfd.wf_admin[]` (akis-ici yetkili). Farklari NE VERDIKLERIDIR, nasil yazildiklari degil.
///
/// `when` guard'inda `$actor` YASAKTIR (validator `grant_when_actor_ref`): grant'lar
/// commit aninda, viewer BILINMEZKEN projeksiyona yazilir.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaGrantRule {
    pub c_a: CandidateActor,          // v2.2: TEK kural (coklu grant = coklu kayit)
    #[serde(default)]
    pub when: Option<String>,
}

/// `wfd.listable[]` ogesi — `CaGrantRule`'un alias'i (motorda da oyle).
pub type ListableRule = CaGrantRule;

// ---- kabul testleri ----
fn main() {
    let path = std::env::args().nth(1).expect("kullanim: wfd_types_v2_2 <wfd.json>");
    let wfd: Wfd = serde_json::from_str(&std::fs::read_to_string(&path).unwrap())
        .expect("v2.2 parse FAIL");
    println!("1) parse OK: {} (wfd_version={})", wfd.id, wfd.wfd_version);

    let mut ok = true;
    let mut seen = std::collections::HashMap::new();
    for (key, node) in &wfd.nodes {
        // 2026-08-12: kimlik TASARIMCININ — `key == slug(c_a)` ARTIK BEKLENMEZ. Slug yalnız
        // BİLGİ olarak basılır: editörün aynı c_a için önereceği varsayılan anahtarı
        // gösterir, doğrulanan bir şey değildir. (2026-08-14 düzeltmesi: burada eskiden
        // "aday cache ve eski belgelerin okunuşu için üretiliyor" yazıyordu — İKİSİ DE
        // YANLIŞTI. Aday cache'i `resolve_candidates` kurar ve satırları
        // {orgu_id, role, user_id, any_orgu}'dur, slug HİÇ geçmez; eski belgelerin parse'ı
        // da slug'a dokunmaz, yalnız anahtarları tarihsel olarak slug biçimindedir.)
        println!("2) key={:<28} slug(bilgi)={:<28} label={:?}",
            key, node.c_a.slug(), node.label.as_deref().unwrap_or("-"));
        // 2026-08-14: tekillik kısıtı HATA olarak geri geldi (validator `duplicate_c_a`) —
        // aynı canonical c_a ikinci bir node'da bulunamaz, aynı key daima aynı c_a'yı taşır.
        if let Some(prev) = seen.insert(node.c_a.canonical(), key.clone()) {
            ok = false;
            println!("3) [FAIL] canonical c_a tekrari: {} == {}", prev, key);
        }
    }
    if ok { println!("3) canonical c_a uniqueness OK ({} node)", wfd.nodes.len()); }

    // matcher smoke test: c_u rol-agnostik
    let rule = CandidateActor {
        c_orgu: Some(COrgu::Selector("self".into())),
        c_r: None, c_u: Some(vec![CuItem::Literal("user_ayse".into())]),
    };
    println!("4) c_u-only matcher: analist-ayse={} memur-ayse={} mehmet={}",
        rule.matches("sube_5", "creditAnalyst", "user_ayse", "sube_5"),
        rule.matches("sube_5", "branchClerk",   "user_ayse", "sube_5"),
        rule.matches("sube_5", "creditAnalyst", "user_mehmet", "sube_5"));
}
