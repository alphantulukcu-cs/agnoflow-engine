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
    pub transitions: Vec<Transition>,
    pub terminals: Vec<Terminal>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub listable: Vec<ListableRule>,
    /// Opsiyonel ek-belge katalogu (grup adı → grup). Node'lar `NodeDef.attachments`
    /// ile bu grupları adıyla referanslar. Engine yalnız metadata taşır; dosya I/O
    /// portal katmanındadır (bkz. server/routes/portal/attachments.rs).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attachments: BTreeMap<String, AttachmentGroup>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_when: Option<String>,
}

impl Wfd {
    /// Yükleme kapısı (M14): tanınmayan `wfd_version` = red; root'ta bilinmeyen alan = red.
    pub fn from_value(v: Value) -> Result<Wfd, EngineError> {
        let version = v
            .get("wfd_version")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                EngineError::UnsupportedWfdVersion("(yok — wfd_version zorunlu)".into())
            })?;
        if version != SUPPORTED_WFD_VERSION {
            return Err(EngineError::UnsupportedWfdVersion(version.to_string()));
        }
        serde_json::from_value(v).map_err(|e| EngineError::InvalidWfd(e.to_string()))
    }

    pub fn from_json(s: &str) -> Result<Wfd, EngineError> {
        let v: Value =
            serde_json::from_str(s).map_err(|e| EngineError::InvalidWfd(e.to_string()))?;
        Wfd::from_value(v)
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
    /// Root `attachments` katalogundaki grup key'lerine referans. WFE bu node'da
    /// beklerken listelenen grupların `required` dosyaları yüklenmeden bu node'dan
    /// hiçbir aksiyon submit edilemez (gate portal katmanındadır; engine core dosya
    /// I/O yapmaz, yalnız referansı taşır ve validator ile katalogda var olduğunu doğrular).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<String>,
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

/// Tek C_A kuralı. match = resolved(c_orgu) AND (rol_match OR user_match).
/// Verilmeyen alan o kanaldan match üretmez (yok = false, wildcard DEĞİL).
/// c_u match'i rol-agnostiktir.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateActor {
    pub c_orgu: COrgu,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c_r: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c_u: Option<Vec<String>>,
}

impl CandidateActor {
    /// Kimlik kanalı: rol_match OR user_match (orgu kontrolü çağıran tarafta —
    /// resolve edilmiş ORGU kümesine üyelik gerektirir).
    pub fn matches_identity(&self, actor_role: &str, actor_user: &str) -> bool {
        let role_hit = self
            .c_r
            .as_ref()
            .map_or(false, |r| r.iter().any(|x| x == actor_role));
        let user_hit = self
            .c_u
            .as_ref()
            .map_or(false, |u| u.iter().any(|x| x == actor_user));
        role_hit || user_hit
    }

    /// Canonical node slug (runtime-semantics §2a):
    /// orgu_slug [+ "__" + sıralı_roller] [+ "__u_" + sıralı_userlar]
    pub fn slug(&self) -> String {
        let mut parts = vec![self.c_orgu.slug()];
        if let Some(r) = &self.c_r {
            let mut r: Vec<String> = r.iter().map(|x| sanitize(x)).collect();
            r.sort();
            parts.push(r.join("-"));
        }
        if let Some(u) = &self.c_u {
            let mut u: Vec<String> = u.iter().map(|x| sanitize(x)).collect();
            u.sort();
            parts.push(format!("u_{}", u.join("-")));
        }
        parts.join("__")
    }

    /// Uniqueness karşılaştırması için canonical form (rol/user sıraları normalize).
    pub fn canonical(&self) -> String {
        let mut r = self.c_r.clone().unwrap_or_default();
        r.sort();
        let mut u = self.c_u.clone().unwrap_or_default();
        u.sort();
        format!("{:?}|r:{:?}|u:{:?}", self.c_orgu.slug(), r, u)
    }
}

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
    /// distinct — validator zorunlu kılar); `join` tüm kollar (AND-join)
    /// bittiğinde WFE'nin gideceği hedeftir (node veya terminal).
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
}
