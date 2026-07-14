// wfd_types_v2_2.rs — Engine icin v2.2 referans serde modeli + canonical slug turetme.
// Kabul testleri (main): (1) golden fixture kayipsiz parse, (2) her node key == slug(c_a),
// (3) canonical c_a uniqueness.
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
    pub transitions: Vec<Transition>,
    pub terminals: Vec<Terminal>,
    #[serde(default)]
    pub listable: Vec<ListableRule>,
    #[serde(default)]
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
    pub escalation: Vec<EscalationStep>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EscalationStep {
    pub after: String,
    #[serde(default)]
    pub wfes_effects: Option<WfesEffects>,
    pub wft: Wft,
}

/// Tek C_A kurali. match = resolved(c_orgu) AND (rol_match OR user_match).
/// Verilmeyen alan o kanaldan match uretmez (yok = false). c_u match'i rol-agnostiktir.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateActor {
    pub c_orgu: COrgu,
    #[serde(default)]
    pub c_r: Option<Vec<String>>,
    #[serde(default)]
    pub c_u: Option<Vec<String>>,
}

impl CandidateActor {
    pub fn matches(&self, actor_orgu: &str, actor_role: &str, actor_user: &str,
                   resolved_orgu: &str) -> bool {
        let in_orgu  = actor_orgu == resolved_orgu;
        let role_hit = self.c_r.as_ref().map_or(false, |r| r.iter().any(|x| x == actor_role));
        let user_hit = self.c_u.as_ref().map_or(false, |u| u.iter().any(|x| x == actor_user));
        in_orgu && (role_hit || user_hit)
    }

    /// Canonical node slug: orgu_slug [+ "__" + sirali_roller] [+ "__u_" + sirali_userlar]
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

    /// Uniqueness karsilastirmasi icin canonical form (rol/user siralari normalize).
    pub fn canonical(&self) -> String {
        let mut r = self.c_r.clone().unwrap_or_default(); r.sort();
        let mut u = self.c_u.clone().unwrap_or_default(); u.sort();
        format!("{:?}|r:{:?}|u:{:?}", self.c_orgu.slug(), r, u)
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
    Conditional {
        conditions: Vec<WftCondition>,
        #[serde(default)]
        default: Option<WftTarget>,
    },
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
    pub id: String,
    #[serde(default)]
    pub wfes_effects: Option<WfesEffects>,
    pub wfe_end_response: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListableRule {
    pub c_a: CandidateActor,          // v2.2: TEK kural (coklu grant = coklu kayit)
    #[serde(default)]
    pub when: Option<String>,
}

// ---- kabul testleri ----
fn main() {
    let path = std::env::args().nth(1).expect("kullanim: wfd_types_v2_2 <wfd.json>");
    let wfd: Wfd = serde_json::from_str(&std::fs::read_to_string(&path).unwrap())
        .expect("v2.2 parse FAIL");
    println!("1) parse OK: {} (wfd_version={})", wfd.id, wfd.wfd_version);

    let mut ok = true;
    let mut seen = std::collections::HashMap::new();
    for (key, node) in &wfd.nodes {
        let slug = node.c_a.slug();
        let m = if *key == slug { "OK " } else { ok = false; "FAIL" };
        println!("2) [{}] key={:<28} slug={:<28} label={:?}",
            m, key, slug, node.label.as_deref().unwrap_or("-"));
        if let Some(prev) = seen.insert(node.c_a.canonical(), key.clone()) {
            ok = false;
            println!("3) [FAIL] canonical c_a tekrari: {} == {}", prev, key);
        }
    }
    if ok { println!("3) canonical c_a uniqueness OK ({} node)", wfd.nodes.len()); }

    // matcher smoke test: c_u rol-agnostik
    let rule = CandidateActor {
        c_orgu: COrgu::Selector("self".into()),
        c_r: None, c_u: Some(vec!["user_ayse".into()]),
    };
    println!("4) c_u-only matcher: analist-ayse={} memur-ayse={} mehmet={}",
        rule.matches("sube_5", "creditAnalyst", "user_ayse", "sube_5"),
        rule.matches("sube_5", "branchClerk",   "user_ayse", "sube_5"),
        rule.matches("sube_5", "creditAnalyst", "user_mehmet", "sube_5"));
}
