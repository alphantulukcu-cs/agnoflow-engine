//! WFD v2.2 modeli — Named Nodes, Single-Rule C_A.
//! Kanonik referans: docs/spec/reference-types.rs + schema.json.
//! Kural: kod ile spec çelişirse spec kazanır.

use crate::error::EngineError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const SUPPORTED_WFD_VERSION: &str = "2.2";

fn default_true() -> bool {
    true
}
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_true(b: &bool) -> bool {
    *b
}
fn default_timeout() -> u32 {
    60
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wfd {
    pub wfd_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression_language: Option<String>,
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Root WFD timeout — ISO 8601 duration (örn. "P30D").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<String>,
    /// JSON Schema 2020-12 + x-visibility uzantısı.
    pub context: Value,
    pub nodes: BTreeMap<String, NodeDef>,
    pub start: Vec<StartRule>,
    pub actions: BTreeMap<String, ActionDef>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub autoexec: BTreeMap<String, AutoexecDef>,
    /// WFC katalogu — başka bir WFD'yi çağırma sözleşmeleri. **Ne** çağrılacağını ve
    /// hangi girdiyle çağrılacağını tutar; **nasıl** çağrıldığı referans yerindedir
    /// (`nodes.<k>.call` veya `terminals[].call`). `autoexec` ↔ `trigger` ayrımının aynısı.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub calls: BTreeMap<String, CallDef>,
    pub transitions: Vec<Transition>,
    pub terminals: Vec<Terminal>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub listable: Vec<ListableRule>,
    /// Opsiyonel ek-belge katalogu (grup adı → grup). Node'lar `NodeDef.attachments`
    /// ile bu grupları adıyla referanslar. Engine yalnız metadata taşır; dosya I/O
    /// portal katmanındadır (bkz. server/routes/portal/attachments.rs).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attachments: BTreeMap<String, AttachmentGroup>,
    /// **DEPRECATED (WOR-84) — motorda DEĞERLENDİRİLMEZ.** v1'den kalmıştır: o modelde
    /// her aksiyondan sonra koşan global bir "akış bitti mi" guard'ıydı. v2.2'de terminal
    /// `wft: {terminal}` ile açıkça verilir, dolayısıyla bu alan ikinci ve çelişebilen bir
    /// terminal-belirleme yolu olurdu. Eski dosyalar parse hatası almasın diye kabul
    /// edilmeye devam eder; validator `terminal_when_ignored` uyarısı basar ve yeniden
    /// serileştirmede alan DÜŞER (`skip_serializing`) — dosya bir kez açılıp kaydedilince
    /// kendiliğinden temizlenir.
    #[serde(default, skip_serializing)]
    pub terminal_when: Option<String>,
}

impl Wfd {
    /// Yükleme kapısı (M14): tanınmayan `wfd_version` = red; root'ta bilinmeyen alan = red.
    pub fn from_value(v: Value) -> Result<Wfd, EngineError> {
        Self::check_version(&v)?;
        serde_json::from_value(v).map_err(|e| EngineError::InvalidWfd(e.to_string()))
    }

    fn check_version(v: &Value) -> Result<(), EngineError> {
        let version = v
            .get("wfd_version")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                EngineError::UnsupportedWfdVersion("(yok — wfd_version zorunlu)".into())
            })?;
        if version != SUPPORTED_WFD_VERSION {
            return Err(EngineError::UnsupportedWfdVersion(version.to_string()));
        }
        Ok(())
    }

    pub fn from_json(s: &str) -> Result<Wfd, EngineError> {
        let v: Value =
            serde_json::from_str(s).map_err(|e| EngineError::InvalidWfd(e.to_string()))?;
        Wfd::from_value(v)
    }

    /// `from_value` + **kanonik JSON Schema kapısı** (`crate::schema`).
    ///
    /// Serde tek başına şemayı karşılamaz: `#[serde(default)]`li alanları eksik kabul eder,
    /// `minItems`/`uniqueItems`/`pattern` gibi kısıtları bilmez. Yani `"c_r": []` gibi şemanın
    /// yasakladığı bir belge serde'den geçip motorda "rol kanalı kapalı" olarak çalışıyordu.
    /// Elle yazılıp API'ye POST edilen JSON'un kapısı budur.
    ///
    /// Ham `from_value` KASITLI olarak açık kalır: taslak kaydı (`save_draft`) yarım belgeyi
    /// saklayabilmeli ve testler tek kuralı sınamak için iskelet belge kurabilmeli.
    pub fn from_value_checked(v: Value) -> Result<Wfd, EngineError> {
        // Sürüm kapısı ÖNCE koşar (M14): eski formatın cevabı "desteklenmeyen sürüm"dür,
        // "şema ihlali" değil — 2.1 belgesi 2.2 şemasına karşı onlarca ihlal üretir ve
        // gerçek sebep o gürültünün içinde kaybolur.
        Self::check_version(&v)?;
        if let Err(errors) = crate::schema::validate_document(&v) {
            return Err(EngineError::InvalidWfd(format!(
                "şema ihlali ({} sorun): {}",
                errors.len(),
                errors.join("; ")
            )));
        }
        Wfd::from_value(v)
    }

    /// `from_json` + şema kapısı — bkz. `from_value_checked`.
    pub fn from_json_checked(s: &str) -> Result<Wfd, EngineError> {
        let v: Value =
            serde_json::from_str(s).map_err(|e| EngineError::InvalidWfd(e.to_string()))?;
        Wfd::from_value_checked(v)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeDef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// v2.2: TEK kural (obje). Eski array formu deserialize edilmez.
    pub c_a: CandidateActor,
    /// Madde 7: opsiyonel claim devri yetkisi. `c_a` ile birebir aynı C_A şekli;
    /// bu kurala uyan aktör (amir) bu node'daki claim'i başkasına devredebilir ya da
    /// havuza bırakabilir. Verilmezse devir bu node'da tamamen kapalıdır (403).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reassign: Option<CandidateActor>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub escalation: Vec<EscalationStep>,
    /// SLA-1 (2026-07-16): claim eden aktör `after` içinde aksiyon almazsa tetiklenir.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_timeout: Option<ClaimTimeout>,
    /// Root `attachments` katalogundaki grup key'lerine referans — bu node'da hangi
    /// dosyaların TOPLANDIĞI. Düz string biçimi grubu o node'un TÜM aksiyonlarına kapı
    /// yapar; `{group, actions}` biçimi yalnız sayılan aksiyonlara (bkz. `AttachmentRef`).
    /// Gate portal katmanındadır; engine core dosya I/O yapmaz, yalnız referansı taşır ve
    /// validator ile katalogda var olduğunu doğrular.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<AttachmentRef>,
    /// WFC — alt akış çağrısı (`mode: wait | detached`). Bu bloğu taşıyan node bir
    /// **WFC node**'udur: insan ACT'i alınamaz, çıkışı `call.wft`'dir. Bekleme bir
    /// DURUMdur; bu yüzden çağrı transition'da değil node'da durur.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<CallRef>,
}

/// SLA-1 — claim timeout (havuz node'u üzerinde). `wft` yoksa aynı havuza döner
/// (claimed_by/claimed_at temizlenir); varsa belirtilen node'a taşınır.
/// 2026-07-28: `wft` TERMINAL olamaz (SLA node'lar arası devirdir — validator
/// `sla_terminal_target`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimTimeout {
    /// ISO 8601 duration — claim anından itibaren.
    pub after: String,
    /// 2026-07-28 (SLA-1 effects): opsiyonel DynCtx yazımı — süre dolduğunda
    /// `$actor` = system aktörü, `$node` = SLA'nın tetiklendiği node. `$action.input.*`
    /// ve `$exec.result.*` bu bağlamda YOKTUR (validator `sla_effect_namespace`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wfes_effects: Option<WfesEffects>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wft: Option<String>,
    /// WOR-56/SLA-1 (2026-08-03): TASARIMCININ TERCİHİ — bu node bir paralel kolun
    /// içindeyken süre dolarsa yalnız kolu taşımak yerine PARALELİ SONLANDIR:
    /// kardeş kollar iptal edilir, WFE paralel moddan çıkar ve `wft` hedefine gider
    /// (aksiyon tarafındaki `Wft::Collapse` ile birebir aynı semantik).
    ///
    /// Sözleşme:
    /// - `wft` ZORUNLU olur (validator `claim_timeout_collapse_requires_wft`) —
    ///   "aynı havuza dön" ile collapse birlikte anlamsızdır: gidilecek hedef yok.
    /// - Hedef hâlâ yalnız NODE olabilir (`sla_terminal_target` değişmedi): collapse
    ///   paraleli bitirir, AKIŞI bitirmez — zaman aşımıyla akışı bitiren tek kural
    ///   root `timeout` (SLA-3).
    /// - Node paralel modda DEĞİLKEN tetiklenirse bayrak yok sayılır ve normal
    ///   `{node}` devri uygulanır (bkz. `Pipeline::fire_claim_timeout`) — aynı node
    ///   hem kol içinde hem dışında erişilebilir olabilir, runtime hatası vermek
    ///   WFE'yi kilitlerdi.
    #[serde(default, skip_serializing_if = "is_false")]
    pub collapses_parallel: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EscalationStep {
    /// ISO 8601 duration — node'a girişten itibaren.
    pub after: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wfes_effects: Option<WfesEffects>,
    /// SLA-2 hedefi — ZORUNLU (validator `escalation_wft_required`). Yalnız NODE
    /// olabilir: terminal hedef 2026-07-28'de yasaklandı (`sla_terminal_target`).
    /// `Option` kalır ki eksikliği serde parse hatası yerine anlaşılır bir
    /// validasyon mesajı üretsin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wft: Option<Wft>,
    /// KALDIRILDI (2026-07-28) — SLA-2 akışı BİTİREMEZ; yalnız SLA-3 (root `timeout`)
    /// bitirir. Alan sırf eski WFD'lere anlaşılır hata verebilmek için deserialize
    /// edilir (`deny_unknown_fields` yüzünden aksi halde ham parse hatası olurdu);
    /// validator `escalation_terminate_removed` ile reddeder ve yeni dokümanlara
    /// asla yazılmaz.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminate: Option<bool>,
}

/// `c_u` listesinin bir öğesi: sabit kimlik ya da context referansı.
///
/// `COrgu`'nun aynası — o da `#[serde(untagged)]` bir birleşimdir
/// (`Selector(String) | Anchor{from, traverse}`) ve aynı `from` anahtar adını kullanır.
/// Düz string'ler AYNEN çalışır: eski belgeler `Literal`'a deserialize olur, migration
/// gerekmez, node key'leri değişmez (bkz. `CandidateActor::slug`).
///
/// Neden sihirli önek (`"$ctx.x.user_id"` düz string olarak) DEĞİL: `c_u` büyüyecek bir
/// alandır (aday havuzunu kişiyle daraltma birinci sınıf bir yetenek olacak). Büyüyecek
/// bir alana önek konvansiyonu koymak her yeni yeteneği şemadan denetlenemez ve editör tip
/// sistemine görünmez kılar — bu projenin defalarca temizlediği drift (C_A array'i,
/// `terminal_when`, `x-wf-readonly`). Yeni yetenekler `Ref`'e alan eklenerek gelir,
/// `COrgu::Anchor`'ın `occurrence` kazanması gibi.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(untagged)]
pub enum CuItem {
    /// Kullanıcı adı ya da UUID string'i — `matcher` ikisini de dener.
    /// Validator `$` ile başlamasını YASAKLAR (`c_u_literal_dollar_prefix`): aksi halde
    /// `$ctx.x` yazım hatası sessizce "böyle bir kullanıcı adı" sanılıp hiç eşleşmezdi.
    Literal(String),
    /// `{ "from": "$ctx.<yol>" }` — çalışma anında context'ten çözülen kişi.
    /// Yol bir `actor` kind'lı alana (ya da onun `user_id`/`user` çocuğuna) işaret eder.
    Ref { from: String },
}

impl CuItem {
    /// Slug/canonical için kaynak metin. `sanitize()` `$` ve `.` karakterlerini attığı için
    /// `Literal("$ctx.x.user_id")` ile `Ref{from:"$ctx.x.user_id"}` AYNI slug'ı üretir —
    /// yani §2a node key'i bu birleşimden etkilenmez. (Belirsizlik doğmaz: `Literal`'ın
    /// `$` ile başlaması validator tarafından yasaklıdır.)
    pub fn slug_source(&self) -> &str {
        match self {
            CuItem::Literal(s) => s,
            CuItem::Ref { from } => from,
        }
    }

    /// Sabit kimlik (varsa) — `matcher`/`resolve_candidates` bunu doğrudan karşılaştırır.
    pub fn literal(&self) -> Option<&str> {
        match self {
            CuItem::Literal(s) => Some(s),
            CuItem::Ref { .. } => None,
        }
    }

    /// Context yolu (varsa), `$ctx.` öneki soyulmadan.
    pub fn ctx_ref(&self) -> Option<&str> {
        match self {
            CuItem::Literal(_) => None,
            CuItem::Ref { from } => Some(from),
        }
    }
}

// Çıplak string DAİMA `Literal`dır — JSON'daki untagged davranışın aynısı.
impl From<String> for CuItem {
    fn from(s: String) -> Self {
        CuItem::Literal(s)
    }
}

impl From<&str> for CuItem {
    fn from(s: &str) -> Self {
        CuItem::Literal(s.to_string())
    }
}

/// Tek C_A kuralı — İKİ biçim.
///
/// **Çapalı** (`c_orgu` verilir): match = resolved(c_orgu) AND (rol_match OR user_match).
/// Verilmeyen alan o kanaldan match üretmez (yok = false, wildcard DEĞİL).
///
/// **Çapasız** (`c_orgu` HİÇ verilmez): match = user_match. Kişi tenant genelinde, hangi
/// ORGU'da olursa olsun eşleşir. Bu biçimde `c_u` ZORUNLU, `c_r` YASAKtır (şema + validator
/// `c_a_anchorless_role`): çapasız bir ROL kanalı ("tenant'taki tüm müdürler") kazara
/// kurulabilecek en geniş kapıdır, kişi kanalı ise adı adı sayılmış bir istisna listesidir.
/// `matcher` çapasız kuralda rol kanalını hiç sormaz — belge bir yolla sızsa bile.
///
/// c_u match'i her iki biçimde de rol-agnostiktir.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateActor {
    /// `None` = çapasız biçim (orgu kanalı kısıtsız). Bkz. tip dokümantasyonu.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c_orgu: Option<COrgu>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c_r: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c_u: Option<Vec<CuItem>>,
}

impl CandidateActor {
    /// Kimlik kanalı: rol_match OR user_match (orgu kontrolü çağıran tarafta —
    /// resolve edilmiş ORGU kümesine üyelik gerektirir).
    pub fn matches_identity(&self, actor_role: &str, actor_user: &str) -> bool {
        let role_hit = self
            .c_r
            .as_ref()
            .map_or(false, |r| r.iter().any(|x| x == actor_role));
        // Yalnız SABİT kimlikler — `Ref` çözümü ctx ister, bu fonksiyon saf ve ctx'siz.
        // Ctx-farkında yol `matcher::authorize`'dır.
        let user_hit = self
            .c_u
            .as_ref()
            .map_or(false, |u| u.iter().any(|x| x.literal() == Some(actor_user)));
        role_hit || user_hit
    }

    /// Canonical node slug (runtime-semantics §2a):
    /// orgu_slug [+ "__" + sıralı_roller] [+ "__u_" + sıralı_userlar]
    ///
    /// Çapasız kuralda orgu parçası `ANCHORLESS_SLUG`'dır. Bir Selector bu metni ÜRETEMEZ:
    /// ORGTRVLANG ifadesi `self` ya da `*:` ile başlamak zorundadır (`ParseError::MissingSelf`),
    /// yani çapasız slug hiçbir çapalı kuralla çakışmaz.
    pub fn slug(&self) -> String {
        let mut parts = vec![match &self.c_orgu {
            Some(c) => c.slug(),
            None => ANCHORLESS_SLUG.to_string(),
        }];
        if let Some(r) = &self.c_r {
            let mut r: Vec<String> = r.iter().map(|x| sanitize(x)).collect();
            r.sort();
            parts.push(r.join("-"));
        }
        if let Some(u) = &self.c_u {
            // `slug_source()` üzerinden: `Literal("x")` ve `Ref{from:"x"}` aynı slug'ı verir.
            // Düz string c_u taşıyan ESKİ belgelerin node key'leri byte-byte aynı kalır.
            let mut u: Vec<String> = u.iter().map(|x| sanitize(x.slug_source())).collect();
            u.sort();
            parts.push(format!("u_{}", u.join("-")));
        }
        parts.join("__")
    }

    /// Uniqueness karşılaştırması için canonical form (rol/user sıraları normalize).
    ///
    /// `CuItem`'ın `Debug`'ı variant'ı ayırır (`Literal("x")` ≠ `Ref { from: "x" }`), yani
    /// sabit kimlik ile aynı metni taşıyan bir referans aynı c_a sayılmaz. Bu form yalnız
    /// doküman İÇİNDE karşılaştırılır, hiçbir yere yazılmaz — biçim değişikliği kalıcı
    /// veriyi etkilemez (`slug` etkiler, o korundu).
    pub fn canonical(&self) -> String {
        let mut r = self.c_r.clone().unwrap_or_default();
        r.sort();
        let mut u = self.c_u.clone().unwrap_or_default();
        u.sort();
        // `Option`'ın Debug'ı çapasızı ayırır: `None` ≠ `Some("...")`.
        format!(
            "{:?}|r:{:?}|u:{:?}",
            self.c_orgu.as_ref().map(COrgu::slug),
            r,
            u
        )
    }
}

/// Çapasız (`c_orgu` yok) kuralın node key parçası — bkz. `CandidateActor::slug`.
pub const ANCHORLESS_SLUG: &str = "any";

/// §2a sanitize: [A-Za-z0-9] korunur, diğerleri '_', ardışık '_' tekilleştirilir,
/// baş/son '_' kırpılır. Case korunur.
pub fn sanitize(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_matches('_').to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum COrgu {
    /// ORGTRVLANG ifadesi ("self", "parent", "*:[type:branch]" ...)
    Selector(String),
    /// DynCtx veya WFAH anchor'ından göreli traversal.
    Anchor { from: AnchorFrom, traverse: String },
}

impl COrgu {
    pub fn slug(&self) -> String {
        match self {
            COrgu::Selector(s) => sanitize(s),
            COrgu::Anchor { from, traverse } => match from {
                AnchorFrom::Ctx(p) => format!("{}_{}", sanitize(p), sanitize(traverse)),
                AnchorFrom::Wfah { wfah, .. } => {
                    format!("wfah_{}_{}", sanitize(wfah), sanitize(traverse))
                }
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AnchorFrom {
    Ctx(String),
    Wfah {
        wfah: String,
        field: String,
        /// "first" | "last" (default: last) — M9.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        occurrence: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionDef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input: InputDef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputDef {
    pub required: Vec<String>,
    pub optional: Vec<String>,
}

/// Bir node'un ek-belge referansı. İki biçim de "bu grup bu node'da TOPLANIR" der;
/// ayrıldıkları yer KAPIDIR (hangi aksiyon yükleme olmadan submit edilemez):
/// - `"grup"` — node'un TÜM aksiyonlarına kapı (v2.2'nin ilk biçimi; eski dosyalar aynen çalışır).
/// - `{"group":"grup","actions":["onayla"]}` — yalnız sayılan aksiyonlara kapı; diğer
///   aksiyonlar dosya yüklenmeden submit edilebilir.
/// - `{"group":"grup","actions":[]}` — hiçbir aksiyonu kapamaz (opsiyonel yükleme).
/// - `{"group":"grup"}` (`actions` yok) — düz string ile aynı: tüm aksiyonlar.
///
/// `actions` bir `Vec` DEĞİL `Option<Vec>`tir: "verilmedi" (tümü) ile "boş verildi"
/// (hiçbiri) zıt anlamlıdır, `#[serde(default)]` bir Vec bu ikisini aynı gösterirdi.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AttachmentRef {
    /// Düz grup key'i — tüm aksiyonlara kapı.
    Group(String),
    /// Aksiyon kapsamlı referans.
    Scoped(ScopedAttachmentRef),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopedAttachmentRef {
    pub group: String,
    /// `None` = tüm aksiyonlar; `Some([])` = hiçbiri; `Some([a, b])` = yalnız a ve b.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actions: Option<Vec<String>>,
}

impl AttachmentRef {
    /// Referans edilen katalog grubunun key'i.
    pub fn group(&self) -> &str {
        match self {
            AttachmentRef::Group(g) => g,
            AttachmentRef::Scoped(s) => &s.group,
        }
    }

    /// Kapsam listesi — `None` ise tüm aksiyonlar kapılıdır.
    pub fn actions(&self) -> Option<&[String]> {
        match self {
            AttachmentRef::Group(_) => None,
            AttachmentRef::Scoped(s) => s.actions.as_deref(),
        }
    }

    /// Bu referans verilen aksiyonu kapıyor mu? `action` bilinmiyorsa (`None` — node
    /// geneli durum sorgusu) kapsam gözetilmeden `true`: durum listesi her şeyi gösterir,
    /// hangi satırın gerçekten kapı olduğunu istemci `actions` alanından okur.
    pub fn gates_action(&self, action: Option<&str>) -> bool {
        match (self.actions(), action) {
            (None, _) => true,
            (Some(_), None) => true,
            (Some(list), Some(a)) => list.iter().any(|x| x == a),
        }
    }
}

/// Ek-belge katalog grubu. Bir veya daha fazla dosya slotu (item) içerir.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachmentGroup {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub items: Vec<AttachmentItem>,
}

/// Katalog grubundaki tek dosya slotu. `id` = "verilen dosya ismi"; grup içinde tekildir.
/// Storage anahtarı: `attachments/{wfe_id}/{grup}/{id}` (portal katmanı).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachmentItem {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// true (default): yüklenmeden gruba bağlı node'dan aksiyon submit edilemez.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub required: bool,
    /// Kabul edilen format kuralları — her biri bir MIME grubu + o gruba özel boyut
    /// sınırı. Farklı formatlar farklı MB (örn. pdf/jpg→4MB, xml/zip→20MB). Boş = her
    /// tip, sınırsız. Portal upload katmanı uygular (engine core dosyaya değmez).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub formats: Vec<AttachmentFormatRule>,
}

/// Tek format kuralı: bir MIME grubu + o gruba özel opsiyonel boyut sınırı.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachmentFormatRule {
    /// MIME tipleri (örn. `["application/pdf","image/jpeg"]`) veya `image/*` joker.
    pub accept: Vec<String>,
    /// Bu format grubu için üst boyut (MB). Yoksa bu grup için sınır yok.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_size_mb: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutoexecDef {
    #[serde(rename = "type")]
    pub kind: AutoexecType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u32,
    pub config: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wfes_effects: Option<WfesEffects>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AutoexecType {
    Rest,
    Sql,
    Calc,
    Python,
    Lambda,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WfesEffects {
    pub set: BTreeMap<String, Value>,
}

// ---- WFC — İş Akışı Çağrısı (Workflow Call) ----
//
// Katalog (`Wfd.calls`) = NE çağrılır + hangi girdiyle. Referans (`NodeDef.call` /
// `Terminal.call`) = NASIL çağrılır. Tek belirleyici eksen `mode`'dur; yerleşimi de
// mod belirler (validator `call_mode_placement`).

/// Root `calls` kataloğundaki bir kayıt. Moddan BAĞIMSIZdır — aynı kayıt hem alt akış
/// hem ardıl olarak kullanılabilir. Bunu mümkün kılan şey `input`'ta
/// `$action.input.*` yasağıdır (validator `call_input_namespace`): ACT girdisi terminal
/// bağlamında güvenilir biçimde mevcut değildir.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallDef {
    /// Çağrılacak WFD'nin id'si. Aynı ORGTNT olmak zorundadır (`call_cross_tenant`).
    pub wfd_id: String,
    /// Verilmezse çağrı anındaki EN SON yayınlanmış versiyon; verilirse o versiyona
    /// pinlenir. Yaratılan WFE her hâlde start anında bir versiyona sabitlenir — yani
    /// pin'siz çağrıda yeni versiyon yayınlamak koşan WFE'leri etkilemez.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Çağrılanın `start[]` kuralı ≥2 ise zorunlu (`startRule.id`); tek start varsa
    /// opsiyonel (`call_start_ambiguous`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    /// WFC-IN — çağrılanın start ACT girdisi → çağıran bağlamındaki kaynak.
    /// Değer: `$ctx.<yol>` / `$wfe_id` / `$actor` / `$timestamp` / literal.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub input: BTreeMap<String, Value>,
}

/// Bir katalog kaydına yapılan referans. Node ve terminal yerleşimleri AYNI tipi
/// kullanır — böylece yanlış yere yazılmış alan serde parse hatası yerine anlaşılır
/// bir validasyon mesajı üretir (`call_next_forbidden_field` / `call_wft_required`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallRef {
    /// Root `calls` kataloğundaki key.
    #[serde(rename = "use")]
    pub use_: String,
    /// Yerleşimden çıkarılabilir olsa da AÇIKÇA yazılır: JSON kendi kendini anlatır,
    /// editör/validator tek alan okur, ileride yeni mod eklenirse şema kırılmaz.
    #[serde(default)]
    pub mode: CallMode,
    /// ISO 8601 duration — yalnız `wait`. Aşılırsa çağrılan iptal edilir ve WFC-RETURN
    /// `$call.status == "timeout"` ile işler.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<String>,
    /// WFC-RETURN effects — yalnız node yerleşimi. `$call.*` burada görünür.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wfes_effects: Option<WfesEffects>,
    /// WFC-RETURN hedefi — node yerleşiminde ZORUNLU. Node veya terminal olabilir;
    /// SLA'nın "terminal hedef yasak" kısıtı burada GEÇERLİ DEĞİL (bu bir zamanlayıcı
    /// değil, çağrılanın sonucuna dayanan bir karardır). `Option` kalır ki eksikliği
    /// parse hatası yerine `call_wft_required` üretsin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wft: Option<Wft>,
    /// Yalnız `terminal`: ardılı kimin başlattığı.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_as: Option<StartAs>,
    /// Yalnız `terminal`: ardıl döngüsüne AÇIK izin + üst sınır. Verilmezse döngü
    /// `call_next_cycle` ile reddedilir ve global ardıl derinliği sınırı geçerlidir.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_next: Option<u32>,
}

/// WFC modu — çağrının TEK belirleyici ekseni.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum CallMode {
    /// Çağıran node'da BEKLER; çağrılan bitince WFC-RETURN işler. Sonuç `$call.*`.
    #[default]
    Wait,
    /// Çağrılan başlatılır, çağıran HEMEN devam eder. `$call.result.*` daima boş.
    Detached,
    /// Çağıran BİTER; ardıl akış onun bittiği yerden başlar. Dönüş yok.
    Terminal,
}

impl CallMode {
    /// Node yerleşiminde geçerli mi (aksi halde terminal yerleşimi).
    pub fn is_node_site(&self) -> bool {
        matches!(self, CallMode::Wait | CallMode::Detached)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            CallMode::Wait => "wait",
            CallMode::Detached => "detached",
            CallMode::Terminal => "terminal",
        }
    }
}

/// Ardılı hangi aktör başlatır. SLA-3/`Failed`/`Terminated` yoluyla da ulaşılabilen
/// bir terminal'de aktör YOKTUR — orada `System` şarttır (`call_next_start_actor`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum StartAs {
    /// Terminal'e getiren ACT'in aktörü ile başlat (default).
    #[default]
    Actor,
    /// Sistem aktörü ile başlat.
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerInvocation {
    /// Root autoexec kataloğundaki key.
    #[serde(rename = "use")]
    pub use_: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retry: Vec<Retrier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catch: Option<CatchDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Retrier {
    pub error_equals: Vec<String>,
    #[serde(default = "Retrier::d_interval")]
    pub interval_seconds: u32,
    #[serde(default = "Retrier::d_attempts")]
    pub max_attempts: u32,
    #[serde(default = "Retrier::d_backoff")]
    pub backoff_rate: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_delay_seconds: Option<u32>,
}

impl Retrier {
    fn d_interval() -> u32 {
        1
    }
    fn d_attempts() -> u32 {
        3
    }
    fn d_backoff() -> f64 {
        2.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatchDef {
    #[serde(default = "CatchDef::d_all")]
    pub error_equals: Vec<String>,
    pub wfes_effects: WfesEffects,
}

impl CatchDef {
    fn d_all() -> Vec<String> {
        vec!["WFD.ALL".into()]
    }
}

/// WFT dört formdan biridir (M3, WOR-31): {node} / {terminal} /
/// {conditions, default?} / {parallel}. Inline `wft.c_a` YOKTUR.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Wft {
    Node {
        node: String,
    },
    Terminal {
        terminal: String,
    },
    Conditional {
        conditions: Vec<WftCondition>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<WftTarget>,
    },
    /// WOR-31: fork/join. `branches` paralel kollara giriş node'larıdır (≥2,
    /// distinct — validator zorunlu kılar); `join` kollar bittiğinde WFE'nin
    /// gideceği hedeftir (node veya terminal). WOR-72: birleştirme mantığı
    /// `join_mode` ile seçilir (AND = tüm kollar, OR = K-of-N quorum).
    Parallel {
        parallel: ParallelSpec,
    },
    /// WOR-56: paralel dalda "sonlandıran aksiyon" (collapse+goto). Yalnız kol
    /// bağlamında geçerli: alındığında TÜM kardeş kollar `cancelled`, WFE paralel
    /// moddan çıkar ve `collapse` hedefine gider — hedef RASTGELE node ya da
    /// terminal olabilir (WOR-31'deki "kol→terminal" collapse'un genellenmişi;
    /// terminal-hedef artık özel hal). Normal `Wft::Node`'dan farkı: node hedefli
    /// collapse paralel modu bitirir + kardeşleri düşürür (BranchMoveTo DEĞİL).
    Collapse {
        collapse: WftTarget,
    },
}

/// WOR-31: `Wft::Parallel`'in gövdesi.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParallelSpec {
    pub branches: Vec<String>,
    pub join: WftTarget,
    /// WOR-72: birleştirme mantığı. Verilmezse `and` (WOR-31 davranışı) —
    /// serileştirmede de atlanır, eski dosyalar birebir aynı kalır.
    #[serde(default, skip_serializing_if = "JoinMode::is_and")]
    pub join_mode: JoinMode,
    /// WOR-72: OR modunda yeterli varış sayısı (K-of-N). Yalnız `join_mode: or`
    /// ile birlikte verilebilir (validator zorunlu kılar); verilmezse 1 = saf OR.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join_threshold: Option<u32>,
    /// WOR-73: `join_mode: expr` için ZEN join koşulu. Yalnız `expr` ile birlikte
    /// verilebilir ve `expr` modunda ZORUNLUDUR (validator). Her kol varışında
    /// değerlendirilir; `true` olunca join dolar.
    ///
    /// Namespace (bkz. `EvalEnv::with_join`): `$branches.<kolGirişNode'u>` (bool —
    /// o kol join'e vardı mı; DEĞERLENDİRİLEN varış dahil), `$arrived` (varmış kol
    /// giriş node'larının dizisi → `len($arrived) >= 2`). Mevcut namespace'ler
    /// (`$ctx`, `$wfah`, `$actor`, …) de açıktır — "tutar 1M üstündeyse GM de
    /// onaylasın" gibi kurallar yazılabilir.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join_when: Option<String>,
}

/// WOR-72: join birleştirme mantığı.
///
/// - `And` (varsayılan, WOR-31): TÜM kollar join hedefine varmalı; son varan kol
///   paralel modu kapatır.
/// - `Or`: **K-of-N quorum**. `join_threshold` kadar kol varır varmaz paralel mod
///   OTORİTER biçimde kapanır — kalan aktif kollar `cancelled`, daha önce varmış
///   fazla kollar `superseded` (collapse ile aynı iptal semantiği, bkz.
///   `stage_parallel_markers`). Eşik verilmezse 1 (ilk varan kazanır).
/// - `Expr` (WOR-73): join koşulu `join_when` ZEN ifadesidir — "(finans VE hukuk)
///   YA DA genel müdür" gibi sayıyla anlatılamayan kurallar için. İfade `true`
///   olunca `Or` ile aynı iptal semantiği uygulanır.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JoinMode {
    #[default]
    And,
    Or,
    Expr,
}

impl JoinMode {
    /// `skip_serializing_if` — varsayılan mod wire'a yazılmaz.
    pub fn is_and(&self) -> bool {
        matches!(self, JoinMode::And)
    }
}

/// WOR-72/WOR-73: **çözülmüş** join kuralı — mod + eşik/ifade üçlüsü tek değere
/// indirgenmiş hâli. Runtime'ın taşıdığı TEK temsil budur (`Wfes::join_rule`,
/// `wf.wfe.join_threshold` + `wf.wfe.join_when`): "mod expr ama ifade yok" gibi
/// tutarsız ara durumlar runtime'a hiç ulaşmaz.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum JoinRule {
    /// AND-join (WOR-31): iptal edilmemiş tüm kollar varmalı.
    #[default]
    All,
    /// K-of-N quorum (WOR-72).
    Quorum(u32),
    /// ZEN join koşulu (WOR-73).
    Expr(String),
}

impl JoinRule {
    /// Audit/`_fork` marker'ı ve API görünümü için kısa etiket.
    pub fn kind(&self) -> &'static str {
        match self {
            JoinRule::All => "and",
            JoinRule::Quorum(_) => "or",
            JoinRule::Expr(_) => "expr",
        }
    }
}

impl ParallelSpec {
    /// Mod + eşik/ifade → tek çözülmüş kural. `join_mode: expr` olup `join_when`
    /// verilmemişse (validator bunu reddeder) güvenli tarafa, AND'e düşer —
    /// runtime "koşulsuz expr" ile hiç karşılaşmaz.
    pub fn join_rule(&self) -> JoinRule {
        match self.join_mode {
            JoinMode::And => JoinRule::All,
            JoinMode::Or => JoinRule::Quorum(self.join_threshold.unwrap_or(1).max(1)),
            JoinMode::Expr => match self.join_when.as_deref().map(str::trim) {
                Some(expr) if !expr.is_empty() => JoinRule::Expr(expr.to_string()),
                _ => JoinRule::All,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WftTarget {
    Node { node: String },
    Terminal { terminal: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WftCondition {
    pub when: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartRule {
    pub id: String,
    /// v2.2 simetrik start: giriş node id'si (nodes katalogunda; initiator c_a'sını taşır). Tekil.
    pub from: String,
    /// Start aksiyonunun gerçek adı (M16) — actions{} içinde normal bir ACT olarak tanımlıdır.
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wfes_effects: Option<WfesEffects>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trigger: Vec<TriggerInvocation>,
    pub wft: Wft,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transition {
    pub id: String,
    /// Kaynak node slug'ı veya slug listesi (M2).
    pub from: FromNodes,
    /// Opsiyonel ek veri guard'ı — state seçimi DEĞİL (M2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
    pub action: String,
    /// Opsiyonel EK yetki kısıtı — node c_a'sının üstüne AND'lenir.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c_a: Option<CandidateActor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wfes_effects: Option<WfesEffects>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trigger: Vec<TriggerInvocation>,
    pub wft: Wft,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FromNodes {
    One(String),
    Many(Vec<String>),
}

impl FromNodes {
    pub fn iter(&self) -> Vec<&str> {
        match self {
            FromNodes::One(s) => vec![s.as_str()],
            FromNodes::Many(v) => v.iter().map(String::as_str).collect(),
        }
    }

    pub fn contains(&self, node: &str) -> bool {
        self.iter().iter().any(|n| *n == node)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Terminal {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wfes_effects: Option<WfesEffects>,
    pub wfe_end_response: BTreeMap<String, Value>,
    /// WFC — ardıl akış çağrısı (`mode: terminal`). "Bir iş akışının bitişi başka bir
    /// iş akışının başlangıcı." WFE bu terminal'de NORMAL biçimde sonlanır (`completed`),
    /// ardından ardıl WFE başlar. Dönüş YOKTUR; `wfes_effects`/`wft`/`timeout` bu
    /// bağlamda yasaktır (validator `call_next_forbidden_field`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<CallRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListableRule {
    /// v2.2: TEK kural (çoklu grant = çoklu kayıt).
    pub c_a: CandidateActor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
}

#[cfg(test)]
mod wft_roundtrip_tests {
    use super::*;

    fn roundtrip(json: serde_json::Value) -> Wft {
        let wft: Wft = serde_json::from_value(json.clone()).unwrap_or_else(|e| {
            panic!("deserialize failed for {json}: {e}");
        });
        let back = serde_json::to_value(&wft).unwrap();
        assert_eq!(back, json, "roundtrip mismatch for {json}");
        wft
    }

    #[test]
    fn node_shape_roundtrips_as_node() {
        let wft = roundtrip(serde_json::json!({"node": "n1"}));
        assert!(matches!(wft, Wft::Node { node } if node == "n1"));
    }

    #[test]
    fn terminal_shape_roundtrips_as_terminal() {
        let wft = roundtrip(serde_json::json!({"terminal": "t1"}));
        assert!(matches!(wft, Wft::Terminal { terminal } if terminal == "t1"));
    }

    #[test]
    fn conditional_shape_roundtrips_as_conditional() {
        let wft = roundtrip(serde_json::json!({
            "conditions": [{"when": "true", "node": "n1"}],
            "default": {"terminal": "t1"},
        }));
        assert!(matches!(wft, Wft::Conditional { .. }));
    }

    #[test]
    fn conditional_shape_without_default_roundtrips() {
        let wft = roundtrip(serde_json::json!({
            "conditions": [{"when": "true", "terminal": "t1"}],
        }));
        assert!(matches!(wft, Wft::Conditional { default: None, .. }));
    }

    #[test]
    fn parallel_shape_with_node_join_roundtrips_as_parallel() {
        let wft = roundtrip(serde_json::json!({
            "parallel": {
                "branches": ["node-a", "node-b"],
                "join": {"node": "x"},
            }
        }));
        match wft {
            Wft::Parallel { parallel } => {
                assert_eq!(parallel.branches, vec!["node-a", "node-b"]);
                assert!(matches!(parallel.join, WftTarget::Node { node } if node == "x"));
            }
            other => panic!("expected Wft::Parallel, got {other:?}"),
        }
    }

    #[test]
    fn parallel_shape_with_terminal_join_roundtrips_as_parallel() {
        let wft = roundtrip(serde_json::json!({
            "parallel": {
                "branches": ["node-a", "node-b", "node-c"],
                "join": {"terminal": "t1"},
            }
        }));
        match wft {
            Wft::Parallel { parallel } => {
                assert!(
                    matches!(parallel.join, WftTarget::Terminal { terminal } if terminal == "t1")
                );
            }
            other => panic!("expected Wft::Parallel, got {other:?}"),
        }
    }

    /// Parallel şekli diğer üç varyant tarafından yutulmamalı (untagged enum
    /// sırası önemli değil çünkü her varyantın zorunlu alan adı farklı, ama
    /// yine de regresyona karşı test ediyoruz).
    #[test]
    fn parallel_shape_is_not_swallowed_by_other_variants() {
        let wft: Wft = serde_json::from_value(serde_json::json!({
            "parallel": {"branches": ["a", "b"], "join": {"node": "x"}}
        }))
        .unwrap();
        assert!(!matches!(wft, Wft::Node { .. }));
        assert!(!matches!(wft, Wft::Terminal { .. }));
        assert!(!matches!(wft, Wft::Conditional { .. }));
        assert!(matches!(wft, Wft::Parallel { .. }));
    }

    /// deny_unknown_fields: ParallelSpec fazladan alanı reddetmeli.
    #[test]
    fn parallel_spec_rejects_unknown_fields() {
        let res: Result<Wft, _> = serde_json::from_value(serde_json::json!({
            "parallel": {"branches": ["a", "b"], "join": {"node": "x"}, "extra": 1}
        }));
        assert!(res.is_err());
    }

    /// branches < 2 veya undistinct: tip seviyesinde serbest — bu yalnızca
    /// validator (WOR-31) tarafından reddedilir, burada sadece parse başarılı
    /// olmalı (deserialize aşamasında kısıt yok).
    #[test]
    fn parallel_spec_parses_regardless_of_branch_count_type_level() {
        let res: Result<Wft, _> = serde_json::from_value(serde_json::json!({
            "parallel": {"branches": ["a"], "join": {"node": "x"}}
        }));
        assert!(res.is_ok());
    }

    /// WOR-72: `join_mode` verilmemişse AND (WOR-31 davranışı) ve serileştirmede
    /// ALAN HİÇ YAZILMAZ — eski dosyalar/golden fixture'lar birebir aynı kalır.
    #[test]
    fn join_mode_defaults_to_and_and_is_omitted_when_serialized() {
        let wft: Wft = serde_json::from_value(serde_json::json!({
            "parallel": {"branches": ["a", "b"], "join": {"node": "x"}}
        }))
        .unwrap();
        match &wft {
            Wft::Parallel { parallel } => {
                assert_eq!(parallel.join_mode, JoinMode::And);
                assert_eq!(parallel.join_rule(), JoinRule::All);
            }
            other => panic!("expected Wft::Parallel, got {other:?}"),
        }
        let json = serde_json::to_value(&wft).unwrap();
        let spec = json.get("parallel").unwrap();
        assert!(spec.get("join_mode").is_none());
        assert!(spec.get("join_threshold").is_none());
    }

    /// WOR-72: `or` eşiksiz = saf OR (1-of-N).
    #[test]
    fn join_mode_or_without_threshold_is_one_of_n() {
        let wft: Wft = serde_json::from_value(serde_json::json!({
            "parallel": {"branches": ["a", "b"], "join": {"node": "x"}, "join_mode": "or"}
        }))
        .unwrap();
        match &wft {
            Wft::Parallel { parallel } => {
                assert_eq!(parallel.join_mode, JoinMode::Or);
                assert_eq!(parallel.join_rule(), JoinRule::Quorum(1));
            }
            other => panic!("expected Wft::Parallel, got {other:?}"),
        }
        // OR modu wire'a AÇIKÇA yazılır (varsayılan olmadığı için atlanamaz).
        let json = serde_json::to_value(&wft).unwrap();
        assert_eq!(json["parallel"]["join_mode"], serde_json::json!("or"));
    }

    /// WOR-72: K-of-N quorum roundtrip.
    #[test]
    fn join_mode_or_with_threshold_roundtrips() {
        let wft = roundtrip(serde_json::json!({
            "parallel": {
                "branches": ["a", "b", "c"],
                "join": {"node": "x"},
                "join_mode": "or",
                "join_threshold": 2,
            }
        }));
        match wft {
            Wft::Parallel { parallel } => {
                assert_eq!(parallel.join_rule(), JoinRule::Quorum(2));
                assert_eq!(parallel.join_threshold, Some(2));
            }
            other => panic!("expected Wft::Parallel, got {other:?}"),
        }
    }

    /// WOR-72: bilinmeyen mod adı reddedilir (yazım hatası sessizce AND'e düşmez).
    #[test]
    fn join_mode_rejects_unknown_value() {
        let res: Result<Wft, _> = serde_json::from_value(serde_json::json!({
            "parallel": {"branches": ["a", "b"], "join": {"node": "x"}, "join_mode": "xor"}
        }));
        assert!(res.is_err());
    }
}
