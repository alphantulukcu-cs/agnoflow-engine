//! WFD v2.2 custom validator — şemanın yakalayamadığı kurallar.
//! Spec: docs/spec/runtime-semantics.md §1, §2b, §5, §6.
//! Linear: WOR-32 (cross-ref, slug/uniqueness), WOR-33 (graf), WOR-34 (context/expression/retry).

use crate::error::EngineError;
use crate::expr_types::{self, ExprEnv};
use crate::types::wfd_v22::{
    AutoexecType, CallMode, CallRef, JoinMode, ParallelSpec, StartAs, Wfd, WfesEffects, Wft,
    WftCondition, WftTarget,
};
use crate::v22::dollar::{self, DollarForm};
use crate::v22::duration::parse_iso8601_duration;
use crate::v22::env;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

#[derive(Debug, Clone)]
pub struct ValidationIssue {
    pub code: String,
    pub path: String,
    pub message: String,
}

#[derive(Debug, Default)]
pub struct ValidationReport {
    pub errors: Vec<ValidationIssue>,
    pub warnings: Vec<ValidationIssue>,
}

impl ValidationReport {
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }

    fn error(&mut self, code: &str, path: String, message: String) {
        self.errors.push(ValidationIssue {
            code: code.into(),
            path,
            message,
        });
    }

    fn warn(&mut self, code: &str, path: String, message: String) {
        self.warnings.push(ValidationIssue {
            code: code.into(),
            path,
            message,
        });
    }
}

/// Çağrılan WFD'leri çözebilen kaynak — WFC'nin cross-WFD kuralları (girdi kümesi,
/// tip uyumu, `wfe_end_response` anahtarları, döngü) için gerekir.
///
/// Neden opsiyonel: `wfe-core` saf bir crate'tir, I/O yapmaz. Saf unit testler
/// resolver vermez ve yalnız yerel kurallar koşar. Upload yolunda (`wfd` crate)
/// resolver DAİMA verilir — yani üretimde tam kontrol vardır.
pub trait WfdProvider {
    /// `(wfd_id, version)` → çağrılan WFD. `version: None` = en son yayınlanmış.
    /// `None` dönmek "bulunamadı / yayınlanmamış" demektir.
    fn resolve(&self, wfd_id: &str, version: Option<&str>) -> Option<Wfd>;
}

pub fn validate(wfd: &Wfd) -> ValidationReport {
    validate_with(wfd, None)
}

/// WFC cross-WFD kurallarını da koşan tam validasyon.
pub fn validate_with(wfd: &Wfd, provider: Option<&dyn WfdProvider>) -> ValidationReport {
    let mut report = validate_local(wfd);
    check_calls_cross_wfd(wfd, provider, &mut report);
    report
}

fn validate_local(wfd: &Wfd) -> ValidationReport {
    let mut report = ValidationReport::default();
    check_calls(wfd, &mut report);
    check_uniqueness(wfd, &mut report);
    check_slugs(wfd, &mut report);
    check_cross_refs(wfd, &mut report);
    check_start_rules(wfd, &mut report);
    check_wft_conditions(wfd, &mut report);
    check_graph(wfd, &mut report);
    check_parallel(wfd, &mut report);
    check_expressions(wfd, &mut report);
    check_action_inputs(wfd, &mut report);
    check_context_required_removed(wfd, &mut report);
    check_context_field_writers(wfd, &mut report);
    check_action_input_consumed(wfd, &mut report);
    check_effect_value_types(wfd, &mut report);
    check_dollar_refs(wfd, &mut report);
    check_optional_input_overwrites(wfd, &mut report);
    check_attachments(wfd, &mut report);
    check_effect_paths(wfd, &mut report);
    check_retries(wfd, &mut report);
    check_string_namespaces(wfd, &mut report);
    check_sla(wfd, &mut report);
    check_env_references(wfd, &mut report);
    check_c_orgu_anchor_kinds(wfd, &mut report);
    check_c_u_items(wfd, &mut report);
    report
}

/// Dokümandaki TÜM `$env.*` referanslarını toplar.
///
/// Server publish ucunda bu küme, WFD'nin tanımlı her ortamındaki anahtarlarla
/// karşılaştırılır (eksik anahtar → 422). Bu fonksiyon I/O YAPMAZ — çekirdek yalnız
/// referansları çıkarır, DB'yle karşılaştırma server'ın işidir.
///
/// Yürüyüş, alan alan gezmek yerine serileştirilmiş dokümanın TÜM string'leri üzerinde
/// yapılır: yeni bir `$env` taşıyabilen alan (yeni bir autoexec config anahtarı, yeni bir
/// effect yeri) eklendiğinde bu fonksiyonu güncellemeyi unutmak, prod'da runtime hatası
/// demek olurdu.
///
/// Serbest metin alanları (`description` vb.) ELENMEZ. Elenmeleri düşünüldü ve reddedildi:
/// bir REST gövdesindeki `{"name": "$env.TENANT"}` çok yaygın bir kalıptır ve ad bazlı
/// eleme onu da atlardı — runtime'da çözülen ama publish'te doğrulanmayan bir referans,
/// yani prod'da hata. Ters yöndeki bedel çok daha ucuz: bir açıklamada geçen "$env.FOO"
/// publish'i "tanımlı değil" diye bloklar, tasarımcı metni değiştirir.
pub fn env_references(wfd: &Wfd) -> Result<BTreeSet<String>, EngineError> {
    let doc = serde_json::to_value(wfd)
        .map_err(|e| EngineError::InvalidWfd(format!("WFD serileştirilemedi: {e}")))?;
    let mut out = BTreeSet::new();
    collect_env_refs(&doc, &mut out)?;
    Ok(out)
}

fn collect_env_refs(v: &Value, out: &mut BTreeSet<String>) -> Result<(), EngineError> {
    match v {
        Value::String(s) => {
            out.extend(env::references(s)?);
        }
        Value::Array(a) => {
            for item in a {
                collect_env_refs(item, out)?;
            }
        }
        Value::Object(m) => {
            for item in m.values() {
                collect_env_refs(item, out)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Bozuk `$env` referansı (`$env.foo`, `$env.` gibi) yayını ENGELLER: runtime'da düz metin
/// olarak dışarı sızar ve URL'ye "$env.foo" yazılmış bir istek gider.
fn check_env_references(wfd: &Wfd, report: &mut ValidationReport) {
    if let Err(e) = env_references(wfd) {
        report.error("env_reference_malformed", "$".into(), e.to_string());
    }
}

// ---- x-wf-kind: c_orgu anchor'ının context alanına bağlanması ----
//
// `c_orgu`'nun anchor formu (`{from: "$ctx.<yol>", traverse}`) bir context alanından ORGU
// çözer. Alanın gerçekten bir ORGU tuttuğu tasarım-zamanında bilinmelidir: aksi halde
// runtime'da anchor çözülemez ve KİMSE yetkilenmez (bkz. `resolver::resolve_c_orgu`) —
// yani akış sessizce kilitlenir. Anlamsal tip context şemasında `x-wf-kind` ile bildirilir.

/// `x-wf-kind` değerleri — `actor` ORGU'yu KAPSAR (içindeki `orgu_id` anchor'a yeter).
/// `actor` = terminology.md'deki (ORGU,(U,R)) üçlüsü, yani `$actor`'ün yazdığı şekil.
const ORGU_CAPABLE_KINDS: [&str; 2] = ["orgu", "actor"];

/// `actor`/`orgu` kind'lı bir nesnenin içinde ORGU tutan alan adları
/// (`resolver::extract_orgu_uuid` bu iki anahtarı arar).
const ORGU_CHILD_KEYS: [&str; 2] = ["orgu", "orgu_id"];

/// `#/$defs/<Ad>` referanslarını çözer.
///
/// Motor `$ref`'i başka hiçbir yerde çözmüyor (`expr_types`'ta hiç yok, `resolve_schema_path`
/// onu `Opaque` sayıyor) ama EDİTÖR çözüyor: Context Studio alanı `{"$ref":"#/$defs/Musteri"}`
/// olarak yazabiliyor. Bu asimetri kalırsa `$defs` arkasındaki kind'lı alanı göremeyip MEŞRU
/// bir belgeyi reddederdik. Kapsam bilinçli olarak dar: yalnız editörün ürettiği tek biçim
/// (`#/$defs/<Ad>`, iç içe yol yok), zincir uzunluğu sınırlı → döngü asla dönmez.
fn deref_defs<'a>(root: &'a Value, node: &'a Value) -> Option<&'a Value> {
    let mut current = node;
    for _ in 0..16 {
        let Some(reference) = current.get("$ref").and_then(Value::as_str) else {
            return Some(current);
        };
        let name = reference.strip_prefix("#/$defs/")?;
        if name.contains('/') {
            return None; // iç içe pointer — çözmüyoruz, opak kalır
        }
        current = root.get("$defs")?.get(name)?;
    }
    None // 16 hop'tan uzun zincir = döngü kabul edilir
}

enum NodeAt<'a> {
    Found(&'a Value),
    Missing,
    /// Şema bu derinliği kısıtlamıyor (`properties` yok, çözülemeyen `$ref`, ...).
    Opaque,
}

/// Bir context yolundaki şema düğümü, `$defs` çözülerek.
fn context_node_at<'a>(context: &'a Value, dotted: &str) -> NodeAt<'a> {
    let Some(mut current) = deref_defs(context, context) else {
        return NodeAt::Opaque;
    };
    for segment in dotted.split('.') {
        let Some(props) = current.get("properties").and_then(Value::as_object) else {
            return NodeAt::Opaque;
        };
        let Some(next) = props.get(segment) else {
            return NodeAt::Missing;
        };
        let Some(next) = deref_defs(context, next) else {
            return NodeAt::Opaque;
        };
        current = next;
    }
    NodeAt::Found(current)
}

fn is_orgu_capable(node: &Value) -> bool {
    node.get("x-wf-kind")
        .and_then(Value::as_str)
        .is_some_and(|k| ORGU_CAPABLE_KINDS.contains(&k))
}

/// Dokümandaki TÜM `<key_name>` yerlerini toplar (yol, değer) — `c_orgu` ve `c_u` ortak.
///
/// `env_references` ile aynı gerekçe: alan alan gezmek yerine serileştirilmiş doküman
/// taranır. Bugün beş taşıyıcı var (`nodes.*.c_a`, `nodes.*.reassign`, `transitions[].c_a`,
/// `listable[].c_a`, context şemasındaki `x-visibility`); altıncısı eklendiğinde bu kuralı
/// güncellemeyi unutmak, kind denetimi olmadan yayınlanan bir belge demek olurdu.
fn collect_key_sites<'a>(
    v: &'a Value,
    key_name: &str,
    path: &str,
    out: &mut Vec<(String, &'a Value)>,
) {
    match v {
        Value::Object(m) => {
            for (key, child) in m {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                if key == key_name {
                    out.push((child_path.clone(), child));
                }
                collect_key_sites(child, key_name, &child_path, out);
            }
        }
        Value::Array(a) => {
            for (i, item) in a.iter().enumerate() {
                collect_key_sites(item, key_name, &format!("{path}[{i}]"), out);
            }
        }
        _ => {}
    }
}

fn check_c_orgu_anchor_kinds(wfd: &Wfd, report: &mut ValidationReport) {
    let Ok(doc) = serde_json::to_value(wfd) else {
        return; // serileştirme hatası başka kuralların işi
    };
    let mut sites = Vec::new();
    collect_key_sites(&doc, "c_orgu", "", &mut sites);

    for (path, c_orgu) in sites {
        // Yalnız ctx-anchor formu: `from` STRING. Selector formu düz string, wfah formunda
        // `from` bir objedir — ikisi de bu kuralın dışında.
        let Some(from) = c_orgu.get("from").and_then(Value::as_str) else {
            continue;
        };
        // `$ctx.` öneki opsiyonel — `resolver::anchor_from_ctx` da onu soyup devam ediyor.
        let bare = from.strip_prefix("$ctx.").unwrap_or(from);

        match context_node_at(&wfd.context, bare) {
            // Şema bu derinliği kısıtlamıyor (ör. `{"type":"object"}` alanının alt yolu).
            // HATA değil — bu biçim meşru ve yaygın; ama SESSİZ de geçemez: sessizlik
            // kuralı tümüyle atlatmanın yolu olurdu. Uyarı, tasarımcıyı alanı `orgu`
            // tipiyle bildirmeye yönlendirir.
            NodeAt::Opaque => report.warn(
                "c_orgu_anchor_kind_unverifiable",
                format!("{path}.from"),
                format!(
                    "c_orgu anchor'ı '{from}' şemanın kısıtlamadığı bir derinliğe düşüyor — \
                     bu yolun bir ORGU tuttuğu doğrulanamıyor. Alanı Context Studio'da \
                     `orgu` (ya da bir aktör tutuyorsa `actor`) tipiyle bildirin; aksi halde \
                     anchor runtime'da çözülemezse o node'da KİMSE yetkilenmez."
                ),
            ),
            NodeAt::Missing => report.error(
                "c_orgu_anchor_unknown_field",
                format!("{path}.from"),
                format!(
                    "c_orgu anchor'ı '{from}' context şemasında olmayan bir alanı işaret ediyor. \
                     Alanı Context Studio'da tanımlayın (tip: orgu) ya da anchor'ı var olan bir \
                     alana çevirin."
                ),
            ),
            NodeAt::Found(node) => {
                if is_orgu_capable(node) {
                    continue;
                }
                // Yol bir ORGU alt-alanını gösteriyorsa ebeveynin kind'ı yeter:
                // `$ctx.talep_sahibi.orgu_id` ↔ `talep_sahibi` = actor/orgu.
                if let Some((parent, last)) = bare.rsplit_once('.') {
                    if ORGU_CHILD_KEYS.contains(&last) {
                        if let NodeAt::Found(p) = context_node_at(&wfd.context, parent) {
                            if is_orgu_capable(p) {
                                continue;
                            }
                        }
                    }
                }
                report.error(
                    "c_orgu_anchor_not_orgu_kind",
                    format!("{path}.from"),
                    format!(
                        "c_orgu anchor'ı '{from}' bir ORGU tutmayan alanı işaret ediyor. \
                         Context Studio'da o alanın tipini `orgu` (ya da bir aktör tutuyorsa `actor`) \
                         yapın — anchor yalnız `x-wf-kind: orgu|actor` bildirilmiş bir alandan \
                         ya da onun orgu_id/orgu çocuğundan çözülebilir."
                    ),
                );
            }
        }
    }
}

// ---- c_u: sabit kimlik ile context referansının ayrımı ----
//
// `c_u` öğesi ya `Literal` (kullanıcı adı / UUID) ya `Ref { from: "$ctx..." }`dır. İki kural,
// birleşimin iki ucundaki sessiz başarısızlıkları kapatır.

/// `actor` kind'lı bir nesnenin içinde KİŞİ tutan alan adları
/// (`resolver::resolve_cu_ident` bu iki anahtarı arar).
const USER_CHILD_KEYS: [&str; 2] = ["user", "user_id"];

fn is_actor_kind(node: &Value) -> bool {
    node.get("x-wf-kind").and_then(Value::as_str) == Some("actor")
}

fn check_c_u_items(wfd: &Wfd, report: &mut ValidationReport) {
    let Ok(doc) = serde_json::to_value(wfd) else {
        return;
    };
    let mut sites = Vec::new();
    collect_key_sites(&doc, "c_u", "", &mut sites);

    for (path, c_u) in sites {
        let Some(items) = c_u.as_array() else { continue };
        for i in 0..items.len() {
            let item = &items[i];
            let item_path = format!("{path}[{i}]");

            // (a) Sabit kimlik `$` ile başlayamaz. Başlarsa motor onu "böyle bir kullanıcı
            //     adı" sanar ve kural sessizce HİÇ eşleşmez — `$ctx.x` yazım hatası, ya da
            //     `Ref` yazmayı unutmuş bir tasarımcı. İkisi de publish'te durmalı.
            if let Some(literal) = item.as_str() {
                if literal.starts_with('$') {
                    report.error(
                        "c_u_literal_dollar_prefix",
                        item_path.clone(),
                        format!(
                            "c_u öğesi '{literal}' bir KULLANICI ADI olarak yorumlanır ama `$` ile \
                             başlıyor — böyle bir kullanıcı yoktur, kural hiç eşleşmez. Context'ten \
                             kişi çözmek istiyorsanız referans biçimini kullanın: \
                             {{\"from\": \"{literal}\"}}."
                        ),
                    );
                }
                continue;
            }

            // (b) Referansın hedefi `actor` kind'lı bir alan (ya da onun user_id/user
            //     çocuğu) olmalı. Değilse runtime'da çözülemez ve o öğe aday üretmez —
            //     yani havuz sessizce daralır.
            let Some(from) = item.get("from").and_then(Value::as_str) else {
                continue; // şema zorunlu kılıyor; buraya düşen şey şema hatasıdır
            };
            let bare = from.strip_prefix("$ctx.").unwrap_or(from);
            match context_node_at(&wfd.context, bare) {
                NodeAt::Opaque => report.warn(
                    "c_u_ref_kind_unverifiable",
                    format!("{item_path}.from"),
                    format!(
                        "c_u referansı '{from}' şemanın kısıtlamadığı bir derinliğe düşüyor — bu \
                         yolun bir kişi tuttuğu doğrulanamıyor. Alanı Context Studio'da `actor` \
                         tipiyle bildirin."
                    ),
                ),
                NodeAt::Missing => report.error(
                    "c_u_ref_unknown_field",
                    format!("{item_path}.from"),
                    format!(
                        "c_u referansı '{from}' context şemasında olmayan bir alanı işaret ediyor."
                    ),
                ),
                NodeAt::Found(node) => {
                    if is_actor_kind(node) {
                        continue;
                    }
                    // Yol `user_id`/`user` çocuğunu gösteriyorsa ebeveynin kind'ı yeter.
                    if let Some((parent, last)) = bare.rsplit_once('.') {
                        if USER_CHILD_KEYS.contains(&last) {
                            if let NodeAt::Found(p) = context_node_at(&wfd.context, parent) {
                                if is_actor_kind(p) {
                                    continue;
                                }
                            }
                        }
                    }
                    report.error(
                        "c_u_ref_not_actor_kind",
                        format!("{item_path}.from"),
                        format!(
                            "c_u referansı '{from}' bir kişi tutmayan alanı işaret ediyor. Context \
                             Studio'da o alanın tipini `actor` yapın — referans yalnız \
                             `x-wf-kind: actor` bildirilmiş bir alandan ya da onun user_id/user \
                             çocuğundan çözülebilir. (`orgu` kind'ı YETMEZ: içinde kişi yoktur.)"
                        ),
                    );
                }
            }
        }
    }
}

// ---- WFC — İş Akışı Çağrısı: yerel kurallar ----
//
// Cross-WFD kurallar (girdi kümesi, tip uyumu, sonuç anahtarları, döngü)
// `check_calls_cross_wfd`'de, `WfdProvider` varsa koşar.

/// WFC-IN'de izin verilen namespace'ler. `$action.input.*` YASAK — iki gerekçe:
/// (1) moddan bağımsızlık: `terminal` modunda ACT girdisi güvenilir biçimde mevcut
/// değil (SLA-3 ile ulaşılan terminal'de hiç yok), (2) WOR-70 tutarlılığı: ctx'e tek
/// yazma yolu effects'tir, böylece "çağrılana ne gitti" DynCtx'te denetlenebilir kalır.
const CALL_INPUT_BANNED: &[&str] = &["$action.input.", "$exec.result.", "$call.", "$node"];

/// Bir WFD'deki tüm WFC referanslarını yerleşimiyle birlikte gezer.
fn call_sites(wfd: &Wfd) -> Vec<(String, &CallRef, bool)> {
    let mut out: Vec<(String, &CallRef, bool)> = Vec::new();
    for (key, node) in &wfd.nodes {
        if let Some(call) = &node.call {
            out.push((format!("nodes[{key}].call"), call, true));
        }
    }
    for t in &wfd.terminals {
        if let Some(call) = &t.call {
            out.push((format!("terminals[{}].call", t.id), call, false));
        }
    }
    out
}

fn check_calls(wfd: &Wfd, report: &mut ValidationReport) {
    let sites = call_sites(wfd);

    // Katalog referansları + moda göre yerleşim.
    let mut used: HashSet<&str> = HashSet::new();
    for (path, call, is_node_site) in &sites {
        used.insert(call.use_.as_str());
        if !wfd.calls.contains_key(&call.use_) {
            report.error(
                "call_unknown_use",
                format!("{path}.use"),
                format!("'{}' `calls` katalogunda tanımlı değil", call.use_),
            );
        }
        if call.mode.is_node_site() != *is_node_site {
            let (yer, dogru) = if *is_node_site {
                ("bir node'da", "wait ya da detached")
            } else {
                ("bir terminal'de", "terminal")
            };
            report.error(
                "call_mode_placement",
                format!("{path}.mode"),
                format!(
                    "`mode: {}` {yer} kullanılamaz — bu yerleşimde geçerli mod: {dogru}. \
                     (wait/detached çağıranı yaşatır ve node'da bekletir; terminal çağıranı bitirip \
                     ardıl akışı başlatır.)",
                    call.mode.as_str()
                ),
            );
            continue; // mod yanlışsa moda özel kuralları koşmak gürültü üretir
        }
        if *is_node_site {
            check_node_call(wfd, path, call, report);
        } else {
            check_next_call(wfd, path, call, report);
        }
    }

    // Katalogda tanımlı ama hiç kullanılmayan kayıt (autoexec'in ikizi).
    for key in wfd.calls.keys() {
        if !used.contains(key.as_str()) {
            report.warn(
                "call_unused_catalog_entry",
                format!("calls[{key}]"),
                format!("'{key}' çağrı tanımı hiçbir node ya da terminal tarafından kullanılmıyor"),
            );
        }
    }

    // WFC-IN: namespace kısıtı + kaynak alanın çağıranın şemasında bildirilmiş olması.
    for (key, def) in &wfd.calls {
        if def.wfd_id == wfd.id {
            // Kendi kendini çağırma: node yerleşiminde sonsuz yuvalanma, terminal
            // yerleşiminde sonsuz zincir. İkincisi `max_next` ile açıkça istenebilir.
            let self_via_terminal = sites
                .iter()
                .any(|(_, c, is_node)| c.use_ == *key && !is_node && c.max_next.is_some());
            if !self_via_terminal {
                report.error(
                    "call_self_recursion",
                    format!("calls[{key}].wfd_id"),
                    format!(
                        "'{key}' akışın kendisini ('{}') çağırıyor — sonsuz özyineleme. \
                         Ardıl (terminal) modunda bilinçli bir döngü isteniyorsa `max_next` ile üst sınır verin",
                        wfd.id
                    ),
                );
            }
        }
        for (input_key, raw) in &def.input {
            let path = format!("calls[{key}].input.{input_key}");
            walk_strings(raw, &path, &mut |s, p| {
                for bad in CALL_INPUT_BANNED {
                    if s.contains(bad) {
                        report.error(
                            "call_input_namespace",
                            p.to_string(),
                            format!(
                                "çağrı girdisinde '{bad}*' kullanılamaz: '{s}'. Çağrı girdisi yalnız \
                                 `$ctx.*`, `$actor`, `$timestamp`, `$wfe_id` ve sabit değerler görür — \
                                 aksiyon girdisini geçirmek için önce `wfes_effects` ile ctx'e yazın"
                            ),
                        );
                    }
                }
                // WFC-IN kaynağı çağıranın context şemasında bildirilmiş olmalı:
                // "çağrılan akışın girdileri çağıranın context'inde de bulunmalı" kuralı.
                if let Some(token) = s.strip_prefix("$ctx.") {
                    let token: String = token
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '.')
                        .collect();
                    let token = token.trim_end_matches('.');
                    if !token.is_empty() {
                        if let PathResolution::Missing = resolve_schema_path(&wfd.context, token) {
                            report.error(
                                "call_input_source_undeclared",
                                p.to_string(),
                                format!(
                                    "'$ctx.{token}' bu akışın context şemasında yok — çağrılan akışın \
                                     '{input_key}' girdisi buradan beslenemez. Ya var olan bir alana \
                                     eşleyin ya da alanı context'e ekleyip bir `wfes_effects` ile doldurun"
                                ),
                            );
                        }
                    }
                }
            });
        }
    }
}

/// `mode: wait | detached` — node yerleşimine özel kurallar.
fn check_node_call(wfd: &Wfd, path: &str, call: &CallRef, report: &mut ValidationReport) {
    let node_key = path
        .strip_prefix("nodes[")
        .and_then(|s| s.split(']').next())
        .unwrap_or_default();

    if call.wft.is_none() {
        report.error(
            "call_wft_required",
            path.to_string(),
            "alt akış çağrısı bir hedef (`wft`) içermelidir — çağrılan bittiğinde akışın \
             nereye gideceği belirsiz kalamaz"
                .into(),
        );
    }
    for (field, present) in [
        ("start_as", call.start_as.is_some()),
        ("max_next", call.max_next.is_some()),
    ] {
        if present {
            report.error(
                "call_node_forbidden_field",
                format!("{path}.{field}"),
                format!("`{field}` yalnız ardıl (terminal) çağrısında geçerlidir"),
            );
        }
    }
    if let Some(t) = &call.timeout {
        if call.mode == CallMode::Detached {
            report.error(
                "call_node_forbidden_field",
                format!("{path}.timeout"),
                "`timeout` yalnız `wait` modunda anlamlıdır — `detached` çağrının sonucunu \
                 hiç beklemez"
                    .into(),
            );
        } else if let Err(e) = parse_iso8601_duration(t) {
            report.error("duration_format", format!("{path}.timeout"), e.to_string());
        }
    }
    // `detached` sonucu hiç görmez — `$call.result.*` daima null olurdu.
    if call.mode == CallMode::Detached {
        if let Some(effects) = &call.wfes_effects {
            for (target, raw) in &effects.set {
                walk_strings(
                    raw,
                    &format!("{path}.wfes_effects.set[{target}]"),
                    &mut |s, p| {
                        if s.starts_with("$call.result.") {
                            report.error(
                            "call_result_in_detached",
                            p.to_string(),
                            format!(
                                "`detached` modda '{s}' daima null'dur — çağrılanın sonucu beklenmiyor. \
                                 Sonuca göre karar verilecekse modu `wait` yapın"
                            ),
                        );
                        }
                    },
                );
            }
        }
    }
    // WFC node'u insan ACT'i almaz: bekleme bir durumdur, havuz değildir.
    for t in &wfd.transitions {
        if t.from.contains(node_key) {
            report.error(
                "call_node_has_action",
                format!("transitions[{}].from", t.id),
                format!(
                    "'{node_key}' bir alt akış çağrısı node'u — buradan aksiyon alınamaz. \
                     Akış çağrılan bittiğinde `call.wft` ile kendi ilerler"
                ),
            );
        }
    }
    if let Some(node) = wfd.nodes.get(node_key) {
        let forbidden = [
            ("escalation", !node.escalation.is_empty()),
            ("claim_timeout", node.claim_timeout.is_some()),
            ("attachments", !node.attachments.is_empty()),
            ("reassign", node.reassign.is_some()),
        ];
        for (field, present) in forbidden {
            if present {
                report.error(
                    "call_node_forbidden_field",
                    format!("nodes[{node_key}].{field}"),
                    format!(
                        "alt akış çağrısı node'unda `{field}` kullanılamaz — çağrılanı terkedip \
                         başka bir node'a taşımak sahipsiz bir WFE bırakır. Üst sınır için \
                         `call.timeout` ya da akışın kök `timeout`'unu kullanın"
                    ),
                );
            }
        }
    }
    if wfd.start.iter().any(|s| s.from == node_key) {
        report.error(
            "call_node_is_start",
            format!("nodes[{node_key}]"),
            format!(
                "'{node_key}' hem start node'u hem alt akış çağrısı — akış henüz başlamadan \
                 çağrı yapılamaz"
            ),
        );
    }
    if let Some(effects) = &call.wfes_effects {
        for (target, raw) in &effects.set {
            walk_strings(
                raw,
                &format!("{path}.wfes_effects.set[{target}]"),
                &mut |s, p| {
                    if s.contains("$action.input.") {
                        report.error(
                        "call_effect_namespace",
                        p.to_string(),
                        format!(
                            "çağrı dönüşü effects'inde '$action.input.*' kullanılamaz (dönüşü system \
                             tetikler, aksiyon girdisi yok): '{s}'"
                        ),
                    );
                    }
                },
            );
        }
    }
}

/// `mode: terminal` — ardıl yerleşimine özel kurallar.
fn check_next_call(wfd: &Wfd, path: &str, call: &CallRef, report: &mut ValidationReport) {
    let terminal_id = path
        .strip_prefix("terminals[")
        .and_then(|s| s.split(']').next())
        .unwrap_or_default();

    // Dönüş olmadığı için dönüşe ait alanların hepsi anlamsızdır.
    for (field, present) in [
        ("wfes_effects", call.wfes_effects.is_some()),
        ("wft", call.wft.is_some()),
        ("timeout", call.timeout.is_some()),
    ] {
        if present {
            report.error(
                "call_next_forbidden_field",
                format!("{path}.{field}"),
                format!(
                    "ardıl çağrıda `{field}` kullanılamaz — akış bu terminal'de biter, dönecek bir \
                     yer yoktur. Ardıla taşınacak veriyi terminal'in kendi `wfes_effects`'i ile \
                     ctx'e yazıp çağrı girdisinde `$ctx.*` olarak eşleyin"
                ),
            );
        }
    }
    // Ardılda WFC-OUT yoktur (çağıran zaten bitti, çağrılan henüz başlamadı).
    if let Some(t) = wfd.terminals.iter().find(|t| t.id == terminal_id) {
        if let Some(effects) = &t.wfes_effects {
            for (target, raw) in &effects.set {
                walk_strings(
                    raw,
                    &format!("terminals[{terminal_id}].wfes_effects.set[{target}]"),
                    &mut |s, p| {
                        if s.contains("$call.") {
                            report.error(
                                "call_next_result_ref",
                                p.to_string(),
                                format!(
                                    "'{s}' burada çözülemez — ardıl çağrının sonucu yoktur (akış bu \
                                     terminal'de biter, ardıl bağımsız koşar)"
                                ),
                            );
                        }
                    },
                );
            }
        }
    }
    // `start_as: actor` yalnız bir ACT ile ulaşılan terminal'de güvenlidir. SLA-3
    // (kök timeout) ya da bir SLA ihlali bu terminal'e getirebiliyorsa aktör yoktur.
    // Kök `timeout` varsa HER terminal SLA yoluyla ulaşılabilir sayılır.
    let start_as = call.start_as.unwrap_or_default();
    if start_as == StartAs::Actor && wfd.timeout.is_some() {
        report.warn(
            "call_next_start_actor",
            format!("{path}.start_as"),
            format!(
                "bu akışın kök `timeout`'u var — '{terminal_id}' terminal'ine zaman aşımıyla da \
                 ulaşılabilir ve o yolda başlatacak bir aktör yoktur. Ardıl o durumda başlamaz; \
                 `start_as: \"system\"` kullanın"
            ),
        );
    }
    if let Some(0) = call.max_next {
        report.error(
            "call_next_max",
            format!("{path}.max_next"),
            "`max_next: 0` ardılı hiç başlatmaz — çağrıyı kaldırın".into(),
        );
    }
}

// ---- WFC: cross-WFD kuralları (WfdProvider gerektirir) ----

/// Çağrılanın start ACT'inin bildirdiği girdiler + o girdilerin ctx'teki tipleri.
struct CalleeInputs {
    /// input adı → zorunlu mu
    declared: Vec<(String, bool)>,
    /// input adı → çağrılanın ctx şemasındaki hedef alanın tipi (biliniyorsa)
    types: HashMap<String, String>,
}

/// Çağrılanın start kuralını ve girdi sözleşmesini çıkarır.
/// Girdi kümesi WOR-70 zinciriyle okunur: `start[]` → ACT → `input.required/optional`.
/// Tip ise start ACT'inin `wfes_effects`'i üzerinden `$action.input.<x>` → `ctx.<y>`
/// izlenerek çağrılanın kendi şemasından alınır.
fn callee_inputs(callee: &Wfd, start_id: Option<&str>) -> Option<CalleeInputs> {
    let rule = match start_id {
        Some(id) => callee.start.iter().find(|s| s.id == id)?,
        None => callee.start.first()?,
    };
    let action = callee.actions.get(&rule.action)?;
    let declared: Vec<(String, bool)> = action
        .input
        .required
        .iter()
        .map(|p| (p.clone(), true))
        .chain(action.input.optional.iter().map(|p| (p.clone(), false)))
        .collect();

    let mut types = HashMap::new();
    if let Some(effects) = &rule.wfes_effects {
        for (ctx_path, raw) in &effects.set {
            if let Some(input_path) = raw.as_str().and_then(|s| s.strip_prefix("$action.input.")) {
                if let Some(ty) = schema_type_at(&callee.context, ctx_path) {
                    types.insert(input_path.to_string(), ty);
                }
            }
        }
    }
    Some(CalleeInputs { declared, types })
}

/// Bir context şeması yolundaki `type` değeri (biliniyorsa).
pub(crate) fn schema_type_at(context: &Value, dotted: &str) -> Option<String> {
    let mut current = context;
    for segment in dotted.split('.') {
        current = current.get("properties")?.get(segment)?;
    }
    current
        .get("type")
        .and_then(Value::as_str)
        .map(String::from)
}

fn check_calls_cross_wfd(
    wfd: &Wfd,
    provider: Option<&dyn WfdProvider>,
    report: &mut ValidationReport,
) {
    let Some(provider) = provider else {
        // Resolver yok (saf çekirdek testi) — cross-WFD kuralları atlanır.
        // Upload yolunda resolver DAİMA verilir, bkz. `validate_with` dokümantasyonu.
        return;
    };
    let sites = call_sites(wfd);

    for (key, def) in &wfd.calls {
        let path = format!("calls[{key}]");
        let Some(callee) = provider.resolve(&def.wfd_id, def.version.as_deref()) else {
            report.error(
                "call_version_not_published",
                format!("{path}.wfd_id"),
                match &def.version {
                    Some(v) => format!(
                        "çağrılan akış '{}' sürüm '{v}' bulunamadı ya da yayınlanmamış",
                        def.wfd_id
                    ),
                    None => format!(
                        "çağrılan akış '{}' bulunamadı ya da yayınlanmış bir sürümü yok",
                        def.wfd_id
                    ),
                },
            );
            continue;
        };

        // Start kuralı seçimi: ≥2 start varsa `start` zorunlu.
        if def.start.is_none() && callee.start.len() > 1 {
            report.error(
                "call_start_ambiguous",
                format!("{path}.start"),
                format!(
                    "'{}' akışının {} başlatma kuralı var — hangisiyle başlatılacağını `start` ile belirtin",
                    def.wfd_id,
                    callee.start.len()
                ),
            );
        }
        if let Some(id) = &def.start {
            if !callee.start.iter().any(|s| s.id == *id) {
                report.error(
                    "call_start_ambiguous",
                    format!("{path}.start"),
                    format!(
                        "'{}' akışında '{id}' adlı bir başlatma kuralı yok",
                        def.wfd_id
                    ),
                );
            }
        }

        let Some(inputs) = callee_inputs(&callee, def.start.as_deref()) else {
            continue; // çağrılanın kendi validasyonunun işi
        };

        // Zorunlu girdi eksik / bilinmeyen girdi verilmiş.
        for (name, required) in &inputs.declared {
            if *required && !def.input.contains_key(name) {
                report.error(
                    "call_input_missing",
                    format!("{path}.input"),
                    format!(
                        "'{}' akışı '{name}' girdisini zorunlu istiyor ama çağrıda verilmemiş — \
                         bir `$ctx.*` alanına eşleyin ya da sabit bir değer verin",
                        def.wfd_id
                    ),
                );
            }
        }
        for name in def.input.keys() {
            if !inputs.declared.iter().any(|(d, _)| d == name) {
                report.error(
                    "call_input_unknown",
                    format!("{path}.input.{name}"),
                    format!("'{}' akışı '{name}' adlı bir girdi bildirmiyor", def.wfd_id),
                );
            }
        }

        // Tip uyumu: kaynak (çağıranın şeması) ↔ hedef (çağrılanın şeması).
        for (name, raw) in &def.input {
            let Some(want) = inputs.types.get(name) else {
                continue;
            };
            let got = match raw.as_str().and_then(|s| s.strip_prefix("$ctx.")) {
                Some(src) => schema_type_at(&wfd.context, src),
                None => json_literal_type(raw),
            };
            let Some(got) = got else { continue };
            if !types_compatible(&got, want) {
                report.error(
                    "call_input_type_mismatch",
                    format!("{path}.input.{name}"),
                    format!(
                        "'{name}' girdisi '{}' akışında `{want}` bekliyor ama verilen kaynak `{got}` — \
                         tipleri eşitleyin",
                        def.wfd_id
                    ),
                );
            }
        }

        // `$call.result.<k>` çağrılanın hiçbir terminal'inin yanıtında yoksa daima null.
        let result_keys: HashSet<&str> = callee
            .terminals
            .iter()
            .flat_map(|t| t.wfe_end_response.keys().map(String::as_str))
            .collect();
        for (site_path, call, is_node) in sites.iter().filter(|(_, c, _)| c.use_ == *key) {
            if !is_node || call.mode != CallMode::Wait {
                continue;
            }
            let Some(effects) = &call.wfes_effects else {
                continue;
            };
            for (target, raw) in &effects.set {
                walk_strings(
                    raw,
                    &format!("{site_path}.wfes_effects.set[{target}]"),
                    &mut |s, p| {
                        let Some(field) = s.strip_prefix("$call.result.") else {
                            return;
                        };
                        let head = field.split('.').next().unwrap_or(field);
                        if !result_keys.contains(head) {
                            report.error(
                                "call_result_unknown",
                                p.to_string(),
                                format!(
                                    "'{}' akışının hiçbir bitişi '{head}' alanını döndürmüyor — \
                                     '{s}' daima null olur. Çağrılanın bitiş yanıtına bu alanı ekleyin",
                                    def.wfd_id
                                ),
                            );
                        }
                    },
                );
            }
        }
    }

    check_call_cycles(wfd, provider, &sites, report);
}

fn json_literal_type(v: &Value) -> Option<String> {
    Some(
        match v {
            Value::String(_) => "string",
            Value::Bool(_) => "boolean",
            Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
            Value::Number(_) => "number",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
            Value::Null => return None,
        }
        .into(),
    )
}

/// `integer` bir `number` yerine geçebilir; gerisi tam eşleşme ister.
pub(crate) fn types_compatible(got: &str, want: &str) -> bool {
    got == want || (got == "integer" && want == "number")
}

/// Yuvalanma döngüsü (`wait`/`detached`) ve ardıl döngüsü (`terminal`) ayrı ayrı
/// aranır — biri sonsuz yuvalanma, diğeri sonsuz WFE üretimi demektir.
/// Ardıl döngüsü `max_next` ile AÇIKÇA istenebilir; yuvalanma döngüsü istenemez.
fn check_call_cycles(
    root: &Wfd,
    provider: &dyn WfdProvider,
    sites: &[(String, &CallRef, bool)],
    report: &mut ValidationReport,
) {
    // (kod, hata mı, yalnız bu modları izle)
    for (node_site, code) in [(true, "call_cycle"), (false, "call_next_cycle")] {
        // Ardıl döngüsüne `max_next` ile açıkça izin verilmiş mi? (Yuvalanma
        // döngüsünün böyle bir kaçışı yoktur.)
        if !node_site
            && sites
                .iter()
                .any(|(_, c, is_node)| !*is_node && c.max_next.is_some())
        {
            continue;
        }
        let mut visiting: Vec<String> = Vec::new();
        let mut done: HashSet<String> = HashSet::new();
        if let Some(chain) = find_cycle(root, provider, node_site, &mut visiting, &mut done) {
            let noun = if node_site {
                "alt akış çağrısı"
            } else {
                "ardıl akış"
            };
            let hint = if node_site {
                "Döngüyü kırın — yuvalanma döngüsüne izin verilemez"
            } else {
                "Bilinçli bir tekrar isteniyorsa terminal çağrısında `max_next` ile üst sınır verin"
            };
            report.error(
                code,
                "calls".into(),
                format!("{noun} döngüsü: {}. {hint}", chain.join(" → ")),
            );
        }
    }
}

/// DFS ile döngü arar; bulursa döngü zincirini (wfd_id listesi) döner.
///
/// Döngü **kenar üzerinde** tespit edilir (hedefe inmeden önce): kökün kendisi
/// `WfdProvider`'dan çözülemeyebilir (henüz yayınlanmamış bir taslak olabilir), o yüzden
/// "hedefe git, orada kendini gör" yaklaşımı döngüyü kaçırırdı.
fn find_cycle(
    wfd: &Wfd,
    provider: &dyn WfdProvider,
    node_site: bool,
    visiting: &mut Vec<String>,
    done: &mut HashSet<String>,
) -> Option<Vec<String>> {
    if done.contains(&wfd.id) {
        return None;
    }
    visiting.push(wfd.id.clone());

    // Yalnız ilgili yerleşimden referanslanan katalog kayıtlarını izle.
    let used: HashSet<&str> = call_sites(wfd)
        .into_iter()
        .filter(|(_, _, is_node)| *is_node == node_site)
        .map(|(_, c, _)| c.use_.as_str())
        .collect();

    for (key, def) in &wfd.calls {
        if !used.contains(key.as_str()) {
            continue;
        }
        // Kenar, halihazırda DFS yığınında olan bir WFD'ye mi gidiyor?
        if let Some(pos) = visiting.iter().position(|id| id == &def.wfd_id) {
            let mut chain = visiting[pos..].to_vec();
            chain.push(def.wfd_id.clone());
            visiting.pop();
            return Some(chain);
        }
        if let Some(callee) = provider.resolve(&def.wfd_id, def.version.as_deref()) {
            if let Some(chain) = find_cycle(&callee, provider, node_site, visiting, done) {
                visiting.pop();
                return Some(chain);
            }
        }
    }
    visiting.pop();
    done.insert(wfd.id.clone());
    None
}

// ---- §1: uniqueness ----

fn check_uniqueness(wfd: &Wfd, report: &mut ValidationReport) {
    let mut seen = HashSet::new();
    for (i, t) in wfd.transitions.iter().enumerate() {
        if !seen.insert(t.id.clone()) {
            report.error(
                "unique",
                format!("transitions[{i}]"),
                format!("transition id '{}' birden fazla kez tanımlı", t.id),
            );
        }
    }
    let mut seen = HashSet::new();
    for (i, s) in wfd.start.iter().enumerate() {
        if !seen.insert(s.id.clone()) {
            report.error(
                "unique",
                format!("start[{i}]"),
                format!("start id '{}' birden fazla kez tanımlı", s.id),
            );
        }
    }
    let mut terminal_ids = HashSet::new();
    let mut terminal_ids_ci: HashMap<String, &str> = HashMap::new();
    for (i, t) in wfd.terminals.iter().enumerate() {
        if !terminal_ids.insert(t.id.clone()) {
            report.error(
                "unique",
                format!("terminals[{i}]"),
                format!("terminal id '{}' birden fazla kez tanımlı", t.id),
            );
        }
        // Terminal id'leri case-insensitive unique olmak zorunda (editor id = kullanıcı
        // adı; "Start" ile "sTaRT" aynı isim sayılır).
        let lower = t.id.to_lowercase();
        if let Some(prev) = terminal_ids_ci.insert(lower, &t.id) {
            if prev != t.id {
                report.error(
                    "unique",
                    format!("terminals[{i}]"),
                    format!(
                        "terminal id '{}' ile '{}' büyük/küçük harf farkı hariç aynı",
                        t.id, prev
                    ),
                );
            }
        }
    }
    // node ve terminal id'leri global namespace'te çakışamaz
    for key in wfd.nodes.keys() {
        if terminal_ids.contains(key) {
            report.error(
                "unique",
                format!("nodes[{key}]"),
                format!("'{key}' hem node key hem terminal id — global namespace çakışması"),
            );
        }
    }
}

// ---- §2b: slug + canonical c_a uniqueness ----

fn check_slugs(wfd: &Wfd, report: &mut ValidationReport) {
    for (key, node) in &wfd.nodes {
        let slug = node.c_a.slug();
        if key != &slug && !is_collision_suffixed(key, &slug) {
            report.error(
                "slug",
                format!("nodes[{key}]"),
                format!("node key '{key}' != slug(c_a) '{slug}'"),
            );
        }
    }
    let mut seen: HashMap<String, &String> = HashMap::new();
    for (key, node) in &wfd.nodes {
        let canonical = node.c_a.canonical();
        if let Some(prev) = seen.insert(canonical, key) {
            report.error(
                "duplicate_c_a",
                format!("nodes[{key}]"),
                format!("aynı canonical c_a iki node'da: '{prev}' ve '{key}'"),
            );
        }
    }
}

/// Collision durumunda editör `_<fnv1a16>` (4 hex) son eki ekler; validator kabul eder.
fn is_collision_suffixed(key: &str, slug: &str) -> bool {
    key.strip_prefix(slug)
        .and_then(|rest| rest.strip_prefix('_'))
        .map(|hex| hex.len() == 4 && hex.chars().all(|c| c.is_ascii_hexdigit()))
        .unwrap_or(false)
}

// ---- §1: cross-reference ----

fn check_cross_refs(wfd: &Wfd, report: &mut ValidationReport) {
    for (i, t) in wfd.transitions.iter().enumerate() {
        let path = format!("transitions[{}]", t.id);
        for node in t.from.iter() {
            if !wfd.nodes.contains_key(node) {
                report.error(
                    "cross_ref",
                    format!("{path}.from"),
                    format!("bilinmeyen node '{node}'"),
                );
            }
        }
        if !wfd.actions.contains_key(&t.action) {
            report.error(
                "cross_ref",
                format!("{path}.action"),
                format!("bilinmeyen action '{}'", t.action),
            );
        }
        for (j, trig) in t.trigger.iter().enumerate() {
            if !wfd.autoexec.contains_key(&trig.use_) {
                report.error(
                    "cross_ref",
                    format!("{path}.trigger[{j}]"),
                    format!("bilinmeyen autoexec '{}'", trig.use_),
                );
            }
        }
        check_wft_refs(wfd, &t.wft, &format!("{path}.wft"), report);
        let _ = i;
    }
    for s in &wfd.start {
        let path = format!("start[{}]", s.id);
        for (j, trig) in s.trigger.iter().enumerate() {
            if !wfd.autoexec.contains_key(&trig.use_) {
                report.error(
                    "cross_ref",
                    format!("{path}.trigger[{j}]"),
                    format!("bilinmeyen autoexec '{}'", trig.use_),
                );
            }
        }
        check_wft_refs(wfd, &s.wft, &format!("{path}.wft"), report);
    }
    for (key, node) in &wfd.nodes {
        for (j, esc) in node.escalation.iter().enumerate() {
            if let Some(wft) = &esc.wft {
                check_wft_refs(
                    wfd,
                    wft,
                    &format!("nodes[{key}].escalation[{j}].wft"),
                    report,
                );
            }
        }
        // WFC node'unun çıkışı `call.wft`'dir — normal bir wft kenarı gibi doğrulanır.
        if let Some(call) = &node.call {
            if let Some(wft) = &call.wft {
                check_wft_refs(wfd, wft, &format!("nodes[{key}].call.wft"), report);
            }
        }
    }
}

fn check_wft_refs(wfd: &Wfd, wft: &Wft, path: &str, report: &mut ValidationReport) {
    for (kind, target) in wft_targets(wft) {
        let known = match kind {
            TargetKind::Node => wfd.nodes.contains_key(target),
            TargetKind::Terminal => wfd.terminals.iter().any(|t| t.id == target),
        };
        if !known {
            let noun = match kind {
                TargetKind::Node => "node",
                TargetKind::Terminal => "terminal",
            };
            report.error(
                "cross_ref",
                path.to_string(),
                format!("bilinmeyen {noun} '{target}'"),
            );
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum TargetKind {
    Node,
    Terminal,
}

fn wft_targets(wft: &Wft) -> Vec<(TargetKind, &str)> {
    let mut out = Vec::new();
    match wft {
        Wft::Node { node } => out.push((TargetKind::Node, node.as_str())),
        Wft::Terminal { terminal } => out.push((TargetKind::Terminal, terminal.as_str())),
        Wft::Conditional {
            conditions,
            default,
        } => {
            for c in conditions {
                if let Some(n) = &c.node {
                    out.push((TargetKind::Node, n.as_str()));
                }
                if let Some(t) = &c.terminal {
                    out.push((TargetKind::Terminal, t.as_str()));
                }
            }
            match default {
                Some(WftTarget::Node { node }) => out.push((TargetKind::Node, node.as_str())),
                Some(WftTarget::Terminal { terminal }) => {
                    out.push((TargetKind::Terminal, terminal.as_str()))
                }
                None => {}
            }
        }
        // WOR-31: fork/join — her branch başlangıç node'u VE join hedefi birer
        // çıkış kenarıdır (cross_ref + graf BFS bunları otomatik kapsar).
        Wft::Parallel { parallel } => {
            for b in &parallel.branches {
                out.push((TargetKind::Node, b.as_str()));
            }
            match &parallel.join {
                WftTarget::Node { node } => out.push((TargetKind::Node, node.as_str())),
                WftTarget::Terminal { terminal } => {
                    out.push((TargetKind::Terminal, terminal.as_str()))
                }
            }
        }
        // WOR-56: collapse hedefi bir çıkış kenarıdır (cross_ref + graf
        // reachability kapsasın); ama branch subgraph BFS'i bu kenarı İZLEMEZ
        // (aşağıda check_parallel'de atlanır — kapsam dışına çıkar).
        Wft::Collapse { collapse } => match collapse {
            WftTarget::Node { node } => out.push((TargetKind::Node, node.as_str())),
            WftTarget::Terminal { terminal } => out.push((TargetKind::Terminal, terminal.as_str())),
        },
    }
    out
}

// ---- V1, V4, V5: start kuralları (spec runtime-semantics, M16). V2/V3 kaldırıldı
// (2026-07-16): start node yeniden girilebilir; mid-flow'da normal node gibi
// davranır, wft hedefi ve escalation geçerlidir. ----

fn check_start_rules(wfd: &Wfd, report: &mut ValidationReport) {
    // V5: en az 1 start
    if wfd.start.is_empty() {
        report.error(
            "start_required",
            "start".into(),
            "en az bir start kuralı gerekli".into(),
        );
    }
    for s in &wfd.start {
        let path = format!("start[{}]", s.id);
        // V4 (M16): start.action gerçek bir action adıdır — actions{} içinde tanımlı
        // olmalı (transition'lardaki action ile aynı kural).
        if !wfd.actions.contains_key(&s.action) {
            report.error(
                "start_action",
                format!("{path}.action"),
                format!("bilinmeyen action '{}'", s.action),
            );
        }
        // V1: from var olan bir node'a işaret etmeli
        if wfd.nodes.get(&s.from).is_none() {
            report.error(
                "cross_ref",
                format!("{path}.from"),
                format!("start.from bilinmeyen node '{}'", s.from),
            );
        }
    }
}

// ---- M3: wft.conditions hedef tekilliği ----

fn check_wft_conditions(wfd: &Wfd, report: &mut ValidationReport) {
    let visit = |wft: &Wft, path: String, report: &mut ValidationReport| {
        if let Wft::Conditional { conditions, .. } = wft {
            for (i, c) in conditions.iter().enumerate() {
                check_condition_target(c, &format!("{path}.conditions[{i}]"), report);
            }
        }
        check_dead_conditions(wft, &path, report);
    };
    for t in &wfd.transitions {
        visit(&t.wft, format!("transitions[{}].wft", t.id), report);
    }
    for s in &wfd.start {
        visit(&s.wft, format!("start[{}].wft", s.id), report);
    }
    for (key, node) in &wfd.nodes {
        for (j, esc) in node.escalation.iter().enumerate() {
            if let Some(wft) = &esc.wft {
                visit(wft, format!("nodes[{key}].escalation[{j}].wft"), report);
            }
        }
        if let Some(call) = &node.call {
            if let Some(wft) = &call.wft {
                visit(wft, format!("nodes[{key}].call.wft"), report);
            }
        }
    }
}

/// Koşulsuz (`true`) bir koşuldan SONRA gelen hiçbir koşul — ve `default` — asla
/// değerlendirilmez: `wft.conditions` İLK-MATCH'tir.
///
/// Bu, editörde "aynı adımdan birden fazla ok" çizildiğinde üretilen şeklin motor
/// tarafındaki karşılığıdır: çoklu ham kenar `when: "true"` koşullarına derlenir ve
/// yalnız ilki erişilebilir olur. Sessiz bırakılırsa akış yazarı iki hedef tanımladığını
/// sanır, biri hiç çalışmaz.
fn check_dead_conditions(wft: &Wft, path: &str, report: &mut ValidationReport) {
    let Wft::Conditional {
        conditions,
        default,
    } = wft
    else {
        return;
    };
    let Some(idx) = conditions.iter().position(|c| is_unconditional(&c.when)) else {
        return;
    };
    let dead_after = conditions.len() - idx - 1;
    if dead_after == 0 && default.is_none() {
        return; // koşulsuz koşul SON ve default yok → `default` yerine geçer, sorun değil
    }
    report.error(
        "wft_dead_condition",
        format!("{path}.conditions[{idx}]"),
        format!(
            "koşulsuz (her zaman doğru) bir dal {}. sırada; kendisinden sonraki {} koşul{}              asla değerlendirilmez (wft ilk-match'tir). Aynı adımdan birden fazla hedef              çıkarmak istiyorsanız her dala GERÇEK bir koşul verin ya da son dalı `default`              yapın.",
            idx + 1,
            dead_after,
            if default.is_some() {
                " ve `default`"
            } else {
                ""
            }
        ),
    );
}

/// `when` her zaman doğru mu? Yalnız apaçık biçimleri sayar — genel bir teorem
/// kanıtlayıcı DEĞİL. Amaç, editörün çoklu-kenar için ürettiği `"true"`yu ve elle
/// yazılmış apaçık eşdeğerlerini yakalamak.
fn is_unconditional(when: &str) -> bool {
    matches!(when.trim(), "true" | "1 == 1" | "true == true")
}

fn check_condition_target(c: &WftCondition, path: &str, report: &mut ValidationReport) {
    match (&c.node, &c.terminal) {
        (Some(_), Some(_)) => report.error(
            "wft_target",
            path.to_string(),
            "condition hem node hem terminal hedefliyor — tam olarak biri olmalı".into(),
        ),
        (None, None) => report.error(
            "wft_target",
            path.to_string(),
            "condition hedefsiz — node veya terminal zorunlu".into(),
        ),
        _ => {}
    }
}

// ---- §5: graf — BFS reachability (escalation DAHİL) + çıkışsız node + ilk-match belirsizliği ----

fn check_graph(wfd: &Wfd, report: &mut ValidationReport) {
    // BFS
    let mut reached_nodes: HashSet<String> = HashSet::new();
    let mut reached_terminals: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();

    fn absorb(
        targets: Vec<(TargetKind, &str)>,
        reached_nodes: &mut HashSet<String>,
        reached_terminals: &mut HashSet<String>,
        queue: &mut VecDeque<String>,
    ) {
        for (kind, target) in targets {
            match kind {
                TargetKind::Node => {
                    if reached_nodes.insert(target.to_string()) {
                        queue.push_back(target.to_string());
                    }
                }
                TargetKind::Terminal => {
                    reached_terminals.insert(target.to_string());
                }
            }
        }
    }

    for s in &wfd.start {
        // Simetrik start: `from` node bir KAYNAKtır — hiçbir wft hedefi olmasa da
        // (V2 zaten yasaklar) erişilebilir sayılır, dead-node uyarısı vermemeli.
        reached_nodes.insert(s.from.clone());
        absorb(
            wft_targets(&s.wft),
            &mut reached_nodes,
            &mut reached_terminals,
            &mut queue,
        );
    }

    while let Some(node_key) = queue.pop_front() {
        for t in &wfd.transitions {
            if t.from.contains(&node_key) {
                absorb(
                    wft_targets(&t.wft),
                    &mut reached_nodes,
                    &mut reached_terminals,
                    &mut queue,
                );
            }
        }
        if let Some(node) = wfd.nodes.get(&node_key) {
            for esc in &node.escalation {
                if let Some(wft) = &esc.wft {
                    absorb(
                        wft_targets(wft),
                        &mut reached_nodes,
                        &mut reached_terminals,
                        &mut queue,
                    );
                }
            }
            // WFC-RETURN de bir çıkıştır (BFS'e girmezse hedefi "unreachable" görünür).
            if let Some(call) = &node.call {
                if let Some(wft) = &call.wft {
                    absorb(
                        wft_targets(wft),
                        &mut reached_nodes,
                        &mut reached_terminals,
                        &mut queue,
                    );
                }
            }
            // SLA-1: claim_timeout.wft de bir çıkıştır (node/terminal hedefi
            // BFS'e dahil edilmezse hedef yanlışlıkla "unreachable" görünür).
            if let Some(ct) = &node.claim_timeout {
                if let Some(target) = &ct.wft {
                    let kind = if wfd.nodes.contains_key(target) {
                        TargetKind::Node
                    } else {
                        TargetKind::Terminal
                    };
                    absorb(
                        vec![(kind, target.as_str())],
                        &mut reached_nodes,
                        &mut reached_terminals,
                        &mut queue,
                    );
                }
            }
        }
    }

    for key in wfd.nodes.keys() {
        if !reached_nodes.contains(key.as_str()) {
            report.error(
                "unreachable",
                format!("nodes[{key}]"),
                format!("WFD.Unreachable: '{key}' start'tan erişilemiyor"),
            );
        }
    }
    for t in &wfd.terminals {
        if !reached_terminals.contains(t.id.as_str()) {
            report.error(
                "unreachable",
                format!("terminals[{}]", t.id),
                format!(
                    "WFD.Unreachable: terminal '{}' hiçbir wft'den referans almıyor",
                    t.id
                ),
            );
        }
    }

    // çıkışsız node: ne transition kaynağı ne escalation'ı var
    // (start node'unun çıkışı start kuralının wft'sidir — no_exit muaf)
    let start_from: HashSet<&str> = wfd.start.iter().map(|s| s.from.as_str()).collect();
    for (key, node) in &wfd.nodes {
        if start_from.contains(key.as_str()) {
            continue;
        }
        // WFC node'unun çıkışı `call.wft`'dir — insan ACT'i almadığı için transition
        // aramak yanlış olur (aksine `call_node_has_action` bunu yasaklar).
        if node.call.is_some() {
            continue;
        }
        let has_transition = wfd.transitions.iter().any(|t| t.from.contains(key));
        if !has_transition && node.escalation.is_empty() {
            report.error(
                "no_exit",
                format!("nodes[{key}]"),
                format!("'{key}' çıkışsız — transition veya escalation gerekli"),
            );
        }
    }

    // aynı (node, action) için çoklu transition
    let mut groups: HashMap<(&str, &str), Vec<&crate::types::wfd_v22::Transition>> = HashMap::new();
    for t in &wfd.transitions {
        for node in t.from.iter() {
            groups.entry((node, t.action.as_str())).or_default().push(t);
        }
    }
    for ((node, action), group) in groups {
        if group.len() < 2 {
            continue;
        }
        let without_when = group.iter().filter(|t| t.when.is_none()).count();
        let ids: Vec<&str> = group.iter().map(|t| t.id.as_str()).collect();
        if without_when >= 2 {
            report.error(
                "ambiguous_transition",
                format!("transitions[{}]", ids.join(",")),
                format!("({node}, {action}) için birden fazla when'siz transition — belirsiz"),
            );
        } else {
            report.warn(
                "ambiguous_transition",
                format!("transitions[{}]", ids.join(",")),
                format!("({node}, {action}) için çoklu transition — runtime ilk-match uygular"),
            );
        }
    }
}

// ---- WOR-31: Parallel fork/join — branch/join şekli + subgraph kısıtları ----
// Restrictions v1 (design doc §Validation): start wft'de Parallel yasak;
// branches len>=2 + distinct + var olan node; join var olan node/terminal ve
// branches'ten biri olamaz; branch subgraph'ları (fork'tan join'e/terminale
// kadar transition wft kenarları) birbirinden ayrık; subgraph içinde iç içe
// (nested) Parallel yasak; her subgraph join'e veya bir terminal'e ulaşmalı.
// WOR-72: join_mode: or (quorum) eklendi — eşik 1..N-1 olmalı, AND'de eşik verilemez.
// Kol subgraph kuralları OR'da AYNEN geçerlidir: quorum dolmadan iptal edilmeyecek
// kolların da bir çıkışı olmak zorundadır (yoksa quorum hiç dolmayabilir).

/// WOR-73: `join_when` içindeki `$branches.<kol>` referanslarını çıkarır.
///
/// Neden ZEN AST'si değil: `zen_expression` ayrıştırılmış ağacı public API'de
/// vermiyor (yalnız `validate_expression` + `evaluate_expression`). Sözdizimi
/// yeterince dar: referans daima `$branches.` + identifier'dır (`$branches['x']`
/// biçimi DESTEKLENMEZ ve zaten kullanılmasına gerek yoktur; node slug'ları
/// identifier-uyumludur). Yanlış-pozitif olamaz: eşleşen her şey gerçekten bir
/// kol referansıdır.
fn branch_refs_in(expr: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = expr.as_bytes();
    let needle = "$branches.";
    let mut i = 0;
    while let Some(pos) = expr[i..].find(needle) {
        let start = i + pos + needle.len();
        let mut end = start;
        while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
            end += 1;
        }
        if end > start {
            out.push(expr[start..end].to_string());
        }
        i = start.max(i + pos + 1);
    }
    out
}

fn check_parallel(wfd: &Wfd, report: &mut ValidationReport) {
    // Parallel wft start kuralında kullanılamaz.
    for s in &wfd.start {
        if matches!(&s.wft, Wft::Parallel { .. }) {
            report.error(
                "parallel_start",
                format!("start[{}].wft", s.id),
                "Parallel wft start kuralında kullanılamaz".into(),
            );
        }
        // WOR-56: collapse yalnız paralel dal içinde anlamlıdır — start'ta yasak.
        if matches!(&s.wft, Wft::Collapse { .. }) {
            report.error(
                "collapse_start",
                format!("start[{}].wft", s.id),
                "Collapse wft start kuralında kullanılamaz (WOR-56)".into(),
            );
        }
    }

    // Fork noktalarını topla (yalnızca transitions.wft — start zaten yasak;
    // nested fork da ayrıca aşağıda yasaklanıyor).
    struct Fork<'a> {
        path: String,
        spec: &'a ParallelSpec,
    }
    let mut forks: Vec<Fork> = Vec::new();
    for t in &wfd.transitions {
        if let Wft::Parallel { parallel } = &t.wft {
            forks.push(Fork {
                path: format!("transitions[{}].wft", t.id),
                spec: parallel,
            });
        }
    }

    for fork in &forks {
        let path = &fork.path;
        let spec = fork.spec;

        if spec.branches.len() < 2 {
            report.error(
                "parallel_branches",
                format!("{path}.parallel.branches"),
                "parallel.branches en az 2 kol içermeli".into(),
            );
        }
        let mut seen_names = HashSet::new();
        for b in &spec.branches {
            if !seen_names.insert(b.as_str()) {
                report.error(
                    "parallel_branches",
                    format!("{path}.parallel.branches"),
                    format!("branch '{b}' tekrarlanıyor — kollar distinct olmalı"),
                );
            }
        }
        // branch/join'in var olan node/terminal'e işaret etmesi generic
        // cross_ref (check_cross_refs → wft_targets) tarafından zaten kontrol
        // edilir; burada sadece Parallel'e özgü kısıt: join, kollardan biri
        // olamaz.
        if let WftTarget::Node { node: join_node } = &spec.join {
            if spec.branches.iter().any(|b| b == join_node) {
                report.error(
                    "parallel_join",
                    format!("{path}.parallel.join"),
                    format!(
                        "join node '{join_node}' branches listesinde de var — join kollardan biri olamaz"
                    ),
                );
            }
        }

        // WOR-72: join_mode / join_threshold. Tek temsil kuralı: AND'in eşiği YOK,
        // OR'un eşiği 1..N-1 aralığındadır. K == N matematiksel olarak AND'dir ve
        // aynı davranışın ikinci bir yazımı olurdu → reddedilir (runtime AND yolunda
        // kalan-aktif-kol sayımı, OR yolunda varış sayımı yapar; iki kod yolunun
        // aynı anlama gelen iki girdiyle beslenmesi audit'i de ikiye böler).
        // WOR-73: `join_when` yalnız `expr` ile verilebilir ve `expr` modunda
        // ZORUNLUDUR; ifade parse edilebilmeli ve YALNIZ bu fork'un kollarına
        // referans vermeli (yazım hatası sessizce `false` dönen bir alan olur ve
        // join asla dolmaz → runtime'da WFD.JoinUnsatisfied ile patlar; statik
        // olarak burada yakalanır).
        if spec.join_mode != JoinMode::Expr && spec.join_when.is_some() {
            report.error(
                "parallel_join_when",
                format!("{path}.parallel.join_when"),
                "join_when yalnız join_mode: expr ile verilebilir".into(),
            );
        }
        match spec.join_mode {
            JoinMode::And => {
                if spec.join_threshold.is_some() {
                    report.error(
                        "parallel_join_threshold",
                        format!("{path}.parallel.join_threshold"),
                        "join_threshold yalnız join_mode: or ile verilebilir (AND tüm kolları bekler)"
                            .into(),
                    );
                }
            }
            JoinMode::Expr => {
                if spec.join_threshold.is_some() {
                    report.error(
                        "parallel_join_threshold",
                        format!("{path}.parallel.join_threshold"),
                        "join_threshold yalnız join_mode: or ile verilebilir (expr koşulu sayıyı kendi ifade eder: len($arrived) >= k)"
                            .into(),
                    );
                }
                match spec.join_when.as_deref().map(str::trim) {
                    None | Some("") => report.error(
                        "parallel_join_when",
                        format!("{path}.parallel.join_when"),
                        "join_mode: expr için join_when ZEN koşulu zorunludur".into(),
                    ),
                    Some(expr) => {
                        if let Err(e) = zen_expression::validate::validate_expression(expr) {
                            report.error(
                                "parallel_join_when",
                                format!("{path}.parallel.join_when"),
                                format!("join_when ZEN ifadesi parse edilemedi: {e}"),
                            );
                        }
                        for referenced in branch_refs_in(expr) {
                            if !spec.branches.iter().any(|b| b == &referenced) {
                                report.error(
                                    "parallel_join_when_unknown_branch",
                                    format!("{path}.parallel.join_when"),
                                    format!(
                                        "join_when '$branches.{referenced}' referansı bu fork'un kolu değil — kol kimliği kolun GİRİŞ node'udur"
                                    ),
                                );
                            }
                        }
                    }
                }
            }
            JoinMode::Or => {
                let n = spec.branches.len() as u32;
                if let Some(k) = spec.join_threshold {
                    if k == 0 {
                        report.error(
                            "parallel_join_threshold",
                            format!("{path}.parallel.join_threshold"),
                            "join_threshold en az 1 olmalı".into(),
                        );
                    } else if n >= 2 && k >= n {
                        report.error(
                            "parallel_join_threshold",
                            format!("{path}.parallel.join_threshold"),
                            format!(
                                "join_threshold ({k}) kol sayısından ({n}) küçük olmalı — k = kol sayısı ise join_mode: and kullan"
                            ),
                        );
                    }
                }
            }
        }
    }

    // Branch subgraph'ları: fork'tan join'e (veya bir terminal'e) kadar,
    // transition wft node kenarları takip edilerek BFS. Join node'a
    // ulaşılınca DURULUR (ötesine geçilmez).
    for fork in &forks {
        let spec = fork.spec;
        let join_node: Option<&str> = match &spec.join {
            WftTarget::Node { node } => Some(node.as_str()),
            WftTarget::Terminal { .. } => None,
        };

        // node -> hangi branch index'inde ilk görüldü (fork içi ayrıklık için)
        let mut owner: HashMap<&str, usize> = HashMap::new();

        for (bi, start) in spec.branches.iter().enumerate() {
            let mut visited: HashSet<&str> = HashSet::new();
            let mut queue: VecDeque<&str> = VecDeque::new();
            visited.insert(start.as_str());
            queue.push_back(start.as_str());
            let mut reaches_exit = join_node == Some(start.as_str());

            while let Some(node_key) = queue.pop_front() {
                if let Some(prev_bi) = owner.get(node_key) {
                    if *prev_bi != bi {
                        report.error(
                            "parallel_disjoint",
                            format!("{}.parallel", fork.path),
                            format!(
                                "node '{node_key}' birden fazla branch subgraph'ında (branch[{prev_bi}] ve branch[{bi}]) — kollar ayrık olmalı"
                            ),
                        );
                    }
                } else {
                    owner.insert(node_key, bi);
                }

                if Some(node_key) == join_node {
                    // join'e ulaşıldı — ötesine geçme.
                    continue;
                }

                for t in &wfd.transitions {
                    if !t.from.contains(node_key) {
                        continue;
                    }
                    if matches!(&t.wft, Wft::Parallel { .. }) {
                        report.error(
                            "parallel_nested",
                            format!("transitions[{}].wft", t.id),
                            "branch subgraph içinde iç içe (nested) Parallel yasak".into(),
                        );
                        continue;
                    }
                    // WOR-56: collapse kenarı subgraph dışına çıkar (kardeşleri düşürüp
                    // WFE'yi rastgele hedefe götürür) → BFS izlemez, disjoint/dead-end
                    // kurallarından muaf. Kol subgraph'ı normal (join/terminal) kenarlarla
                    // çıkışa ulaşmalıdır; collapse tek başına reaches_exit üretmez.
                    if matches!(&t.wft, Wft::Collapse { .. }) {
                        continue;
                    }
                    for (kind, target) in wft_targets(&t.wft) {
                        match kind {
                            TargetKind::Terminal => reaches_exit = true,
                            TargetKind::Node => {
                                if Some(target) == join_node {
                                    reaches_exit = true;
                                    // join node'u da ayrıklık defterine düş
                                    // (üstte tekrar işlenecek ve durulacak).
                                    if !owner.contains_key(target) {
                                        owner.insert(target, bi);
                                    }
                                } else if visited.insert(target) {
                                    queue.push_back(target);
                                }
                            }
                        }
                    }
                }
            }

            if !reaches_exit {
                report.error(
                    "parallel_dead_end",
                    format!("{}.parallel.branches[{}]", fork.path, bi),
                    format!("branch '{start}' join node'a veya bir terminal'e ulaşamıyor"),
                );
            }
        }
    }
}

// ---- §6: ZEN parse ----

/// WOR-84: `[` ... `]` içinde LİTERAL negatif indeks var mı (`$wfah[-1]`).
/// Neden ayrı bir tarama: `validate_expression` bunu KABUL eder (sözdizimi geçerli),
/// runtime'da `Fetch: Failed to convert to usize` ile patlar — yani parse kapısı
/// çalışan/çalışmayan ayrımını yapamıyor. Zen'de negatif indeks YOK; karşılığı
/// `$prev` / `$first` namespace'leri ya da `[len(x) - n]`.
fn has_negative_index(expr: &str) -> bool {
    let b = expr.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'[' {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        // `-` hemen ardından rakam → dilim/indeks konumunda negatif sabit.
        if j < b.len() && b[j] == b'-' && b.get(j + 1).is_some_and(u8::is_ascii_digit) {
            return true;
        }
        i += 1;
    }
    false
}

/// WOR-84: `$wfah` doğrudan indeksleniyor mu (`$wfah[...]`). Geçmiş yeterince uzun
/// değilse VM patlar — `$prev`/`$first` boş geçmişte null döner, patlamaz.
fn indexes_wfah_directly(expr: &str) -> bool {
    let mut rest = expr;
    while let Some(pos) = rest.find("$wfah") {
        rest = &rest[pos + "$wfah".len()..];
        if rest.trim_start().starts_with('[') {
            return true;
        }
    }
    false
}

/// TEK bir ZEN ifadesinin yüzey kontrolleri — `(kod, hata_mı, mesaj)` üçlüleri.
///
/// Neden ayrı ve **public**: editörün koşul kurucusu aynı verdiği almak zorundadır.
/// JS tarafında zen grameri taklit edilirse "editörde yeşil, motorda parse hatası"
/// sınıfı (WOR-84'ün ta kendisi) geri döner. Editör bu fonksiyonu
/// `POST /wfd/validate-expression` üzerinden çağırır; WFD validator'ı da aynı
/// listeyi kullanır — iki yol ayrışamaz.
///
/// `is_error = false` olan girdiler uyarıdır (yayını engellemez).
pub fn expression_issues(expr: &str) -> Vec<(&'static str, bool, String)> {
    if let Err(e) = zen_expression::validate::validate_expression(expr) {
        // Parse edilemeyen ifadede diğer kontroller anlamsız — tek hata döner.
        return vec![(
            "zen_parse",
            true,
            format!("ZEN ifadesi parse edilemedi: {e}"),
        )];
    }
    let mut out = Vec::new();
    if has_negative_index(expr) {
        out.push((
            "zen_negative_index",
            true,
            "negatif indeks zen'de desteklenmez — parse edilir ama runtime'da patlar. \
             Son/ilk girdi için $prev / $first kullan."
                .to_string(),
        ));
    }
    // `$env` referans biçimi — editör ve WFD validator'ı AYNI kaynaktan beslensin diye
    // burada. Bozuk referans runtime'da düz metin olarak dışarı sızar.
    if let Err(e) = env::references(expr) {
        out.push(("env_reference_malformed", true, e.to_string()));
    }
    if indexes_wfah_directly(expr) {
        out.push((
            "wfah_index_unguarded",
            false,
            "$wfah doğrudan indeksleniyor — geçmiş o kadar uzun değilse ifade \
             runtime'da patlar (boş geçmişte kesin patlar). $prev (son girdi) / \
             $first (ilk girdi) bu durumda null döner."
                .to_string(),
        ));
    }
    out
}

/// İfade tip denetiminin ihtiyaç duyduğu WFD bilgisi. Editör bunları JSON'dan çıkarıyorsa
/// motor da çıkarabilir — elle yazılmış dosya bu bilgileri taşımak ZORUNDADIR (taşımıyorsa
/// eksiklik kendi kuralıyla ayrıca reddedilir: `input_path` / `unused_action_input`).
///
/// **public**: `POST /wfd/validate-expression` de aynı bağlamı kurar — editörün serbest ZEN
/// satırı, yayın kapısıyla AYNI cevabı satır yazılırken almak zorundadır.
pub fn expr_env(wfd: &Wfd) -> ExprEnv<'_> {
    let mut input_ctx_map: HashMap<String, String> = HashMap::new();
    for (_, effects) in each_effects(wfd) {
        for (target, raw) in &effects.set {
            let Some(path) = raw.as_str().and_then(|s| s.strip_prefix("$action.input.")) else {
                continue;
            };
            input_ctx_map
                .entry(path.to_string())
                .or_insert_with(|| target.clone());
        }
    }
    let declared_inputs = wfd
        .actions
        .values()
        .flat_map(|a| a.input.required.iter().chain(&a.input.optional))
        .cloned()
        .collect();
    ExprEnv {
        context: &wfd.context,
        input_ctx_map,
        declared_inputs,
    }
}

fn check_expressions(wfd: &Wfd, report: &mut ValidationReport) {
    let env = expr_env(wfd);
    let check = |expr: &str, path: String, report: &mut ValidationReport| {
        // Yüzey kontrolleri (parse/indeks) + TİP kontrolleri aynı kapıdan geçer: editörün
        // koşul kurucusundaki kural setiyle motor tarafı ayrışmasın.
        let issues = expression_issues(expr)
            .into_iter()
            .chain(expr_types::expression_type_issues(expr, &env));
        for (code, is_error, message) in issues {
            if is_error {
                report.error(code, path.clone(), message);
            } else {
                report.warn(code, path.clone(), message);
            }
        }
    };

    let visit_wft = |wft: &Wft, path: &str, report: &mut ValidationReport| {
        if let Wft::Conditional { conditions, .. } = wft {
            for (i, c) in conditions.iter().enumerate() {
                check(&c.when, format!("{path}.conditions[{i}].when"), report);
            }
        }
    };

    for t in &wfd.transitions {
        let path = format!("transitions[{}]", t.id);
        if let Some(when) = &t.when {
            check(when, format!("{path}.when"), report);
        }
        for (j, trig) in t.trigger.iter().enumerate() {
            if let Some(when) = &trig.when {
                check(when, format!("{path}.trigger[{j}].when"), report);
            }
        }
        visit_wft(&t.wft, &format!("{path}.wft"), report);
    }
    for s in &wfd.start {
        let path = format!("start[{}]", s.id);
        for (j, trig) in s.trigger.iter().enumerate() {
            if let Some(when) = &trig.when {
                check(when, format!("{path}.trigger[{j}].when"), report);
            }
        }
        visit_wft(&s.wft, &format!("{path}.wft"), report);
    }
    for (key, node) in &wfd.nodes {
        for (j, esc) in node.escalation.iter().enumerate() {
            if let Some(wft) = &esc.wft {
                visit_wft(wft, &format!("nodes[{key}].escalation[{j}].wft"), report);
            }
        }
        if let Some(call) = &node.call {
            if let Some(wft) = &call.wft {
                visit_wft(wft, &format!("nodes[{key}].call.wft"), report);
            }
        }
    }
    for (i, l) in wfd.listable.iter().enumerate() {
        if let Some(when) = &l.when {
            check(when, format!("listable[{i}].when"), report);
        }
    }
    // WOR-84: `calc` autoexec ifadeleri. `config` şemasız `Value` olduğu için upload
    // kapısı buraya HİÇ bakmıyordu — bozuk ifade yayınlanıp akış koşarken patlıyordu
    // (`ExecFailure`), yani tasarımcı hatayı üretimde öğreniyordu.
    for (key, def) in &wfd.autoexec {
        if def.kind != AutoexecType::Calc {
            continue;
        }
        let Some(exprs) = def.config.get("expressions").and_then(Value::as_object) else {
            report.error(
                "calc_expressions_missing",
                format!("autoexec[{key}].config"),
                "calc autoexec'te config.expressions nesnesi zorunlu".into(),
            );
            continue;
        };
        for (name, expr) in exprs {
            let path = format!("autoexec[{key}].config.expressions.{name}");
            match expr.as_str() {
                Some(s) => check(s, path, report),
                None => report.error(
                    "calc_expression_not_string",
                    path,
                    "calc ifadesi string olmalı".into(),
                ),
            }
        }
    }
    // WOR-84: `terminal_when` v1 kalıntısıdır ve MOTORDA HİÇ DEĞERLENDİRİLMEZ.
    // v2.2'de terminal `wft: {terminal}` ile açıkça verilir; ikinci bir global
    // "bittiyse bitir" guard'ı tek-kural ilkesine aykırı olurdu. Alan parse edilmeye
    // devam eder (eski dosyalar reddedilmesin) ama sessizce yok sayıldığı SÖYLENİR —
    // "yazdım ama çalışmıyor" en pahalı hata sınıfıdır.
    if wfd.terminal_when.is_some() {
        report.warn(
            "terminal_when_ignored",
            "terminal_when".into(),
            "terminal_when motorda değerlendirilmez (v1 kalıntısı) — terminal'i \
             wft: {terminal} ile ver, bu alanı kaldır."
                .into(),
        );
    }
}

// ---- §6: action input yolları ----

fn check_action_inputs(wfd: &Wfd, report: &mut ValidationReport) {
    for (name, action) in &wfd.actions {
        for path in action.input.required.iter().chain(&action.input.optional) {
            match resolve_schema_path(&wfd.context, path) {
                PathResolution::Missing => report.error(
                    "input_path",
                    format!("actions[{name}].input"),
                    format!("input yolu '{path}' context şemasında yok"),
                ),
                PathResolution::Found | PathResolution::Opaque => {}
            }
        }
    }
}

// ---- WOR-70: context yazma sözleşmesi ----
//
// Kural seti üç parçadır ve birlikte "context.required"ın yerini alır:
//   1. `context.required` / `properties.*.required` YASAK  (context_required_removed)
//   2. Her context alanı en az bir `wfes_effects.set` tarafından yazılmalı
//      (context_field_never_written) — hiç dolmayacak alan tutulamaz.
//   3. Bir aksiyonun bildirdiği her input, o aksiyonu kullanan kuralın effects'inde
//      `$action.input.<yol>` ile tüketilmeli (unused_action_input) — istekten alınan
//      değer sessizce düşmesin.
//   4. Effect'in yazdığı değerin tipi hedef alanın şemasıyla uyuşmalı
//      (effect_type_mismatch) — `$actor` NESNEDİR, `string` bir alana yazılırsa o alanı
//      okuyan koşullar sessizce hep-false olur.
// Çalışma anında ctx doluluk denetimi YOKTUR; her şey tasarım zamanında yakalanır.

/// Kural 1 — kaldırılan `required` bildirimleri hard reject.
fn check_context_required_removed(wfd: &Wfd, report: &mut ValidationReport) {
    if wfd.context.get("required").is_some() {
        report.error(
            "context_required_removed",
            "context.required".into(),
            "`context.required` kaldırıldı (WOR-70) — zorunluluk artık aksiyonun \
             `input.required` listesinde bildirilir. Bu listeyi context şemasından silin."
                .into(),
        );
    }
    check_nested_required(&wfd.context, "context", report);
}

fn check_nested_required(schema: &Value, path: &str, report: &mut ValidationReport) {
    let Some(props) = schema.get("properties").and_then(Value::as_object) else {
        return;
    };
    for (name, node) in props {
        let node_path = format!("{path}.properties.{name}");
        if node.get("required").is_some() {
            report.error(
                "context_required_removed",
                format!("{node_path}.required"),
                format!(
                    "'{name}' içindeki `required` listesi kaldırıldı (WOR-70) — motor bunu hiç \
                     okumuyordu. Zorunluluk aksiyonun `input.required` listesinde bildirilir."
                ),
            );
        }
        check_nested_required(node, &node_path, report);
    }
}

/// Bir yolun diğerini kapsayıp kapsamadığı: eşit, ata veya torun.
/// (`credit_info` ↔ `credit_info.amount_requested` her iki yönde de kapsar.)
fn paths_overlap(a: &str, b: &str) -> bool {
    a == b || a.starts_with(&format!("{b}.")) || b.starts_with(&format!("{a}."))
}

/// WFD'deki TÜM `wfes_effects` blokları, site yoluyla (hata mesajının konumu için).
/// `collect_effect_targets` ve `check_effect_value_types` AYNI listeyi görür — iki ayrı
/// yürüyüş, biri (örn. WFC-RETURN effects'i) sessizce kapsam dışında kalmaya açıktı.
fn each_effects(wfd: &Wfd) -> Vec<(String, &WfesEffects)> {
    let mut out: Vec<(String, &WfesEffects)> = Vec::new();

    for s in &wfd.start {
        let path = format!("start[{}]", s.id);
        if let Some(e) = &s.wfes_effects {
            out.push((path.clone(), e));
        }
        for trig in &s.trigger {
            if let Some(c) = &trig.catch {
                out.push((
                    format!("{path}.trigger[{}].catch", trig.use_),
                    &c.wfes_effects,
                ));
            }
        }
    }
    for (i, t) in wfd.transitions.iter().enumerate() {
        let path = format!("transitions[{i}]");
        if let Some(e) = &t.wfes_effects {
            out.push((path.clone(), e));
        }
        for trig in &t.trigger {
            if let Some(c) = &trig.catch {
                out.push((
                    format!("{path}.trigger[{}].catch", trig.use_),
                    &c.wfes_effects,
                ));
            }
        }
    }
    for (key, node) in &wfd.nodes {
        for (i, esc) in node.escalation.iter().enumerate() {
            if let Some(e) = &esc.wfes_effects {
                out.push((format!("nodes[{key}].escalation[{i}]"), e));
            }
        }
        if let Some(ct) = &node.claim_timeout {
            if let Some(e) = &ct.wfes_effects {
                out.push((format!("nodes[{key}].claim_timeout"), e));
            }
        }
        // WFC-RETURN effects de bir yazardır — yoksa yalnız çağrı sonucundan dolan
        // alan `context_field_never_written` ile yanlışlıkla reddedilirdi.
        if let Some(call) = &node.call {
            if let Some(e) = &call.wfes_effects {
                out.push((format!("nodes[{key}].call"), e));
            }
        }
    }
    for t in &wfd.terminals {
        if let Some(e) = &t.wfes_effects {
            out.push((format!("terminals[{}]", t.id), e));
        }
    }
    for (key, ax) in &wfd.autoexec {
        if let Some(e) = &ax.wfes_effects {
            out.push((format!("autoexec[{key}]"), e));
        }
    }
    out
}

/// WFD'deki TÜM `wfes_effects.set` hedef yollarını toplar.
fn collect_effect_targets(wfd: &Wfd) -> Vec<String> {
    each_effects(wfd)
        .into_iter()
        .flat_map(|(_, e)| e.set.keys().cloned().collect::<Vec<_>>())
        .collect()
}

/// Bir effect değerinin ÜRETTİĞİ JSON tipi — bilinmiyorsa `None` (kural sessiz kalır).
///
/// `$actor` bir NESNEDİR (`effects::resolve_dollar_string` → `serde_json::to_value(Actor)`
/// = `{orgu_id, user_id, role}`); `$timestamp`/`$wfe_id`/`$node`/`$call.status`/
/// `$call.wfe_id` metne serileşir. `$action.input.*` / `$exec.result.*` / `$call.result.*`
/// TİPSİZDİR: aksiyon girdisi yalnız yol listesi olarak bildirilir, autoexec/çağrı
/// sonucunun şeması WFD'de durmaz — tahmin etmek yanlış pozitif üretirdi.
fn effect_value_type(raw: &Value, context: &Value) -> Option<String> {
    let Some(s) = raw.as_str() else {
        return json_literal_type(raw);
    };
    match s {
        "$actor" => Some("object".into()),
        "$timestamp" | "$wfe_id" | "$node" | "$call.status" | "$call.wfe_id" => {
            Some("string".into())
        }
        _ => match s.strip_prefix("$ctx.") {
            Some(path) => schema_type_at(context, path),
            // Tanınmayan `$...` referansı tipsizdir; `$` ile başlamayan her şey metin sabiti.
            None if s.starts_with('$') => None,
            None => Some("string".into()),
        },
    }
}

/// Kural 4 — effect'in yazdığı değerin tipi hedef context alanının şemasıyla uyuşmalı.
///
/// Motor yazmayı REDDETMEZ (çalışma anında context şeması zorlanmaz), dolayısıyla hata
/// yayında da görünmez: `$actor`'ü `string` bir alana yazan akışta o alanı okuyan her
/// koşul (`$ctx.basvuran == "ali"`) bir objeyi metinle karşılaştırdığı için sessizce
/// hep-false olur. Tek yakalama noktası tasarım-zamanı doğrulamasıdır. Aynı mekanizma
/// çağrı girdilerinde zaten var (`call_input_type_mismatch`) — effects'te yoktu.
fn check_effect_value_types(wfd: &Wfd, report: &mut ValidationReport) {
    for (site, effects) in each_effects(wfd) {
        for (target, raw) in &effects.set {
            let Some(want) = schema_type_at(&wfd.context, target) else {
                continue; // hedef şemasız/tipsiz — kıyaslanacak bir şey yok
            };
            let Some(got) = effect_value_type(raw, &wfd.context) else {
                continue;
            };
            if types_compatible(&got, &want) {
                continue;
            }
            report.error(
                "effect_type_mismatch",
                format!("{site}.wfes_effects.set[{target}]"),
                format!(
                    "context alanı '{target}' `{want}` tipinde ama effects ona `{got}` yazıyor \
                     ({raw}) — alanın tipini düzeltin ya da başka bir kaynak seçin. \
                     (`$actor` bir nesnedir: orgu_id, user_id, role)"
                ),
            );
        }
    }
}

/// Bir DEĞER ağacındaki `$`-string'leri denetler. Obje/dizi de gezilir: motor effect
/// değerlerini recursive çözer (`effects::resolve_value`), yani `{"rol": "$actor.role"}`
/// içindeki yazım hatası da yayına sızardı.
fn check_dollar_value(raw: &Value, site: &str, report: &mut ValidationReport) {
    match raw {
        Value::String(s) => {
            if dollar::classify(s) == DollarForm::Unknown {
                report.error(
                    "unknown_dollar_ref",
                    site.to_string(),
                    format!(
                        "'{s}' motorun tanıdığı bir referans DEĞİL — çözülmez, alana bu METİN \
                         yazılır ve hata hiçbir yerde görünmez. Tanınan biçimler: $actor · \
                         $timestamp · $wfe_id · $node · $call.status · $call.wfe_id · $ctx.<yol> · \
                         $action.input.<yol> · $exec.result.<yol> · $call.result.<yol> · \
                         $env.ANAHTAR. (`$actor` bir NESNEDİR; alt alanı `$actor.role` diye \
                         okunamaz — önce effects ile bir context alanına yazın.)"
                    ),
                );
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                check_dollar_value(v, &format!("{site}.{k}"), report);
            }
        }
        Value::Array(arr) => {
            for (i, v) in arr.iter().enumerate() {
                check_dollar_value(v, &format!("{site}[{i}]"), report);
            }
        }
        _ => {}
    }
}

/// Tanınmayan `$` referansları — motorun DÜZ METİN yazdığı sessiz yazım hataları.
///
/// Denetlenen yerler, çözücülerin bulunduğu yerlerdir (bkz. `v22::dollar` modül dokümanı):
/// `wfes_effects.set`, WFC `call.input`, terminal `wfe_end_response`, autoexec `config`.
/// `when` / `calc` gibi ZEN İFADELERİ buraya GİRMEZ: onların namespace kümesi ayrıdır
/// (`$wfah`, `$prev`, `$first`…) ve kendi kuralları vardır (`expression_issues`).
fn check_dollar_refs(wfd: &Wfd, report: &mut ValidationReport) {
    for (site, effects) in each_effects(wfd) {
        for (target, raw) in &effects.set {
            check_dollar_value(raw, &format!("{site}.wfes_effects.set[{target}]"), report);
        }
    }
    // WFC-IN: girdi eşlemesi katalogdadır (`calls`), yerleşimde (`CallRef`) değil.
    for (key, def) in &wfd.calls {
        for (k, raw) in &def.input {
            check_dollar_value(raw, &format!("calls[{key}].input[{k}]"), report);
        }
    }
    for t in &wfd.terminals {
        for (k, raw) in &t.wfe_end_response {
            check_dollar_value(raw, &format!("terminals[{}].wfe_end_response[{k}]", t.id), report);
        }
    }
    for (key, ax) in &wfd.autoexec {
        check_dollar_value(&ax.config, &format!("autoexec[{key}].config"), report);
    }
}

/// Context şemasının yazılabilir yaprak yolları. Yaprak = altında `properties` olmayan
/// düğüm (`$ref` opaktır, yaprak sayılır).
fn collect_context_leaves(schema: &Value, prefix: &str, out: &mut Vec<String>) {
    let props = schema
        .get("properties")
        .and_then(Value::as_object)
        .filter(|p| !p.is_empty());
    let Some(props) = props else {
        if !prefix.is_empty() {
            out.push(prefix.to_string());
        }
        return;
    };
    if schema.get("$ref").is_some() && !prefix.is_empty() {
        out.push(prefix.to_string());
        return;
    }
    for (name, node) in props {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };
        collect_context_leaves(node, &path, out);
    }
}

/// Kural 2 — hiçbir effect tarafından yazılmayan context alanı reddedilir.
fn check_context_field_writers(wfd: &Wfd, report: &mut ValidationReport) {
    let targets = collect_effect_targets(wfd);
    let mut leaves = Vec::new();
    collect_context_leaves(&wfd.context, "", &mut leaves);

    for leaf in leaves {
        if targets.iter().any(|t| paths_overlap(t, &leaf)) {
            continue;
        }
        report.error(
            "context_field_never_written",
            format!("context.properties.{}", leaf.replace('.', ".properties.")),
            format!(
                "context alanı '{leaf}' hiçbir `wfes_effects` tarafından yazılmıyor — bu alan hiç \
                 dolmayacak. Ya bu alanı yazan bir aksiyona \"{leaf}\": \"$action.input.{leaf}\" \
                 effect'i ekleyin, ya da alanı context şemasından silin."
            ),
        );
    }
}

/// Bir effect bloğundaki `$action.input.<yol>` referanslarını toplar (nested değerler dahil).
fn collect_input_refs(effects: &WfesEffects, out: &mut Vec<String>) {
    for raw in effects.set.values() {
        walk_strings(raw, "", &mut |s, _| {
            if let Some(path) = s.strip_prefix("$action.input.") {
                out.push(path.to_string());
            }
        });
    }
}

/// Kural 3 — kuralın aksiyonunun bildirdiği her input, o kuralın effects'inde tüketilmeli.
fn check_action_input_consumed(wfd: &Wfd, report: &mut ValidationReport) {
    // (kural yolu, aksiyon adı, kuralın kendi effects'i, tetiklediği trigger'lar)
    let mut rules: Vec<(String, &String, Vec<String>)> = Vec::new();

    let refs_for = |own: Option<&WfesEffects>,
                    triggers: &[crate::types::wfd_v22::TriggerInvocation]| {
        let mut refs = Vec::new();
        if let Some(e) = own {
            collect_input_refs(e, &mut refs);
        }
        for trig in triggers {
            if let Some(c) = &trig.catch {
                collect_input_refs(&c.wfes_effects, &mut refs);
            }
            // Tetiklenen autoexec'in kendi effects'i de aksiyon girdisini görebilir.
            if let Some(ax) = wfd.autoexec.get(&trig.use_) {
                if let Some(e) = &ax.wfes_effects {
                    collect_input_refs(e, &mut refs);
                }
            }
        }
        refs
    };

    for s in &wfd.start {
        rules.push((
            format!("start[{}]", s.id),
            &s.action,
            refs_for(s.wfes_effects.as_ref(), &s.trigger),
        ));
    }
    for t in &wfd.transitions {
        rules.push((
            format!("transitions[{}]", t.id),
            &t.action,
            refs_for(t.wfes_effects.as_ref(), &t.trigger),
        ));
    }

    for (path, action_name, refs) in rules {
        let Some(action) = wfd.actions.get(action_name) else {
            continue; // tanımsız aksiyon check_cross_refs'in işi
        };
        for declared in action.input.required.iter().chain(&action.input.optional) {
            if refs.iter().any(|r| paths_overlap(r, declared)) {
                continue;
            }
            report.error(
                "unused_action_input",
                path.clone(),
                format!(
                    "'{action_name}' aksiyonu '{declared}' girdisini istiyor ama bu kuralın \
                     `wfes_effects` bloğu onu hiçbir yere yazmıyor — istekten gelen değer \
                     kayboluyor. Şunu ekleyin: \"{declared}\": \"$action.input.{declared}\"."
                ),
            );
        }
    }
}

/// Kural 4 (UYARI) — gönderilmeyen opsiyonel girdi, başka bir yazarın değerini `null`'a
/// çevirir. Hata değil (bilinçli tasarım olabilir), ama akış yazarı bunu tasarım anında
/// görmeli: `optional` bir girdiyle yazılan alanı BAŞKA bir yazar da yazıyorsa
/// (escalation/autoexec/terminal ya da başka bir kural), girdi gönderilmediğinde o
/// değer kaybolur (bkz. effects::apply_effects).
/// Bir effect yazarı.
///
/// `excl`: **karşılıklı dışlama grubu** — aynı (node, action) için birden fazla
/// transition varsa runtime İLK-MATCH uygular, yani bu kurallardan yalnız BİRİ koşar.
/// Birbirlerinin değerini ezmeleri imkansızdır; bu yüzden aynı gruptaki yazarlar
/// karşılaştırmadan muaftır (aksi halde "X yazıyor — aynı alanı X da yazıyor" gibi
/// kendi kendini gösteren bir yanlış pozitif üretilirdi).
struct EffectWriter {
    path: String,
    site: String,
    optional_sourced: bool,
    excl: Option<(String, Vec<String>)>,
}

/// İki yazar ilk-match kardeşi mi? Aynı aksiyon adı VE kesişen `from` kümesi →
/// runtime yalnız birini seçer, ikisi birlikte koşmaz.
fn mutually_exclusive(
    a: Option<&(String, Vec<String>)>,
    b: Option<&(String, Vec<String>)>,
) -> bool {
    let (Some((a_action, a_from)), Some((b_action, b_from))) = (a, b) else {
        return false;
    };
    a_action == b_action && a_from.iter().any(|n| b_from.contains(n))
}

fn check_optional_input_overwrites(wfd: &Wfd, report: &mut ValidationReport) {
    let mut writers: Vec<EffectWriter> = Vec::new();

    let push_rule = |path: &str,
                     action_name: &str,
                     effects: &WfesEffects,
                     site: String,
                     excl: Option<(String, Vec<String>)>,
                     writers: &mut Vec<EffectWriter>| {
        let optional_sourced = effects.set.get(path).is_some_and(|raw| {
            let Some(input_path) = raw.as_str().and_then(|s| s.strip_prefix("$action.input."))
            else {
                return false;
            };
            let Some(action) = wfd.actions.get(action_name) else {
                return false;
            };
            // Yalnız opsiyonel bildirimi karşılıyorsa; zorunlu bildirimi de karşılıyorsa
            // gönderilmesi garanti olduğundan null'a dönmez.
            action
                .input
                .optional
                .iter()
                .any(|o| paths_overlap(o, input_path))
                && !action
                    .input
                    .required
                    .iter()
                    .any(|r| paths_overlap(r, input_path))
        });
        writers.push(EffectWriter {
            path: path.to_string(),
            site,
            optional_sourced,
            excl,
        });
    };

    for s in &wfd.start {
        if let Some(e) = &s.wfes_effects {
            for path in e.set.keys() {
                let site = format!("start[{}]", s.id);
                // Start kuralları da (from, action) üzerinden ilk-match'tir.
                push_rule(
                    path,
                    &s.action,
                    e,
                    site,
                    Some((s.action.clone(), vec![s.from.clone()])),
                    &mut writers,
                );
            }
        }
        for trig in &s.trigger {
            if let Some(c) = &trig.catch {
                for path in c.wfes_effects.set.keys() {
                    writers.push(EffectWriter {
                        path: path.clone(),
                        site: format!("start[{}] catch", s.id),
                        optional_sourced: false,
                        excl: None,
                    });
                }
            }
        }
    }
    for t in &wfd.transitions {
        if let Some(e) = &t.wfes_effects {
            for path in e.set.keys() {
                // Site etiketi node'u da taşır: aynı aksiyonun iki kuralı varsa
                // "X — aynı alanı X da yazıyor" okunmaz bir mesaj üretiyordu.
                let mut froms: Vec<String> = t.from.iter().into_iter().map(String::from).collect();
                froms.sort();
                let site = format!("'{}' aksiyonu ({})", t.action, froms.join(", "));
                push_rule(
                    path,
                    &t.action,
                    e,
                    site,
                    Some((t.action.clone(), froms)),
                    &mut writers,
                );
            }
        }
        for trig in &t.trigger {
            if let Some(c) = &trig.catch {
                for path in c.wfes_effects.set.keys() {
                    writers.push(EffectWriter {
                        path: path.clone(),
                        site: format!("transitions[{}] catch", t.id),
                        optional_sourced: false,
                        excl: None,
                    });
                }
            }
        }
    }
    for (key, node) in &wfd.nodes {
        for esc in &node.escalation {
            if let Some(e) = &esc.wfes_effects {
                for path in e.set.keys() {
                    writers.push(EffectWriter {
                        path: path.clone(),
                        site: format!("'{key}' escalation'ı"),
                        optional_sourced: false,
                        excl: None,
                    });
                }
            }
        }
        if let Some(ct) = &node.claim_timeout {
            if let Some(e) = &ct.wfes_effects {
                for path in e.set.keys() {
                    writers.push(EffectWriter {
                        path: path.clone(),
                        site: format!("'{key}' claim süresi"),
                        optional_sourced: false,
                        excl: None,
                    });
                }
            }
        }
    }
    for t in &wfd.terminals {
        if let Some(e) = &t.wfes_effects {
            for path in e.set.keys() {
                writers.push(EffectWriter {
                    path: path.clone(),
                    site: format!("'{}' terminali", t.id),
                    optional_sourced: false,
                    excl: None,
                });
            }
        }
    }
    for (name, ax) in &wfd.autoexec {
        if let Some(e) = &ax.wfes_effects {
            for path in e.set.keys() {
                writers.push(EffectWriter {
                    path: path.clone(),
                    site: format!("'{name}' otomasyonu"),
                    optional_sourced: false,
                    excl: None,
                });
            }
        }
    }

    let mut reported: HashSet<String> = HashSet::new();
    for w in &writers {
        if !w.optional_sourced || !reported.insert(w.path.clone()) {
            continue;
        }
        let others: Vec<&str> = writers
            .iter()
            .filter(|o| o.site != w.site && paths_overlap(&o.path, &w.path))
            // İLK-MATCH kardeşleri muaf: aynı (node, action) için yalnız BİRİ koşar,
            // dolayısıyla birbirinin değerini ezemezler.
            .filter(|o| !mutually_exclusive(o.excl.as_ref(), w.excl.as_ref()))
            .map(|o| o.site.as_str())
            .collect();
        // Aynı etiket birden fazla kez listelenmesin: ilk-match kardeşleri aynı
        // (aksiyon, node) etiketini taşır, ham liste "X, X, Y" gibi okunurdu.
        let mut others = others;
        others.dedup();
        others.sort();
        others.dedup();
        if others.is_empty() {
            continue;
        }
        report.warn(
            "optional_input_nulls_other_writer",
            w.site.clone(),
            format!(
                "'{}' alanını opsiyonel bir girdi yazıyor; aynı alanı {} da yazıyor. \
                 Girdi gönderilmezse bu alan null olur ve diğer yazarın değeri kaybolur.",
                w.path,
                others.join(", ")
            ),
        );
    }
}

// ---- §6b: attachments katalogu + node referansları ----

fn check_attachments(wfd: &Wfd, report: &mut ValidationReport) {
    // Katalog içi: item.id grup içinde tekil olmalı.
    for (group, def) in &wfd.attachments {
        let mut seen_ids = HashSet::new();
        for item in &def.items {
            if !seen_ids.insert(item.id.clone()) {
                report.error(
                    "attachment_item_dup",
                    format!("attachments[{group}].items"),
                    format!(
                        "attachment item id '{}' grup içinde birden fazla tanımlı",
                        item.id
                    ),
                );
            }
        }
    }
    // Node referansları: katalogda var olmalı; aynı grup bir node'da tekrar edilmemeli.
    for (node_key, node) in &wfd.nodes {
        let mut seen_refs = HashSet::new();
        for aref in &node.attachments {
            let group_ref = aref.group();
            if !seen_refs.insert(group_ref.to_string()) {
                report.error(
                    "attachment_ref_dup",
                    format!("nodes[{node_key}].attachments"),
                    format!("attachment grubu '{group_ref}' bu node'da birden fazla referanslı"),
                );
            }
            if !wfd.attachments.contains_key(group_ref) {
                report.error(
                    "attachment_ref",
                    format!("nodes[{node_key}].attachments"),
                    format!("attachment grubu '{group_ref}' root attachments katalogunda yok"),
                );
            }
            // Aksiyon kapsamı: sayılan her aksiyon bu node'dan GERÇEKTEN çıkabilmeli.
            // Yoksa kapı hiç kapanmaz — dosya zorunlu sanılır, hiçbir submit'i durdurmaz.
            let Some(scoped) = aref.actions() else { continue };
            let mut seen_actions = HashSet::new();
            for action in scoped {
                if !seen_actions.insert(action.clone()) {
                    report.error(
                        "attachment_action_dup",
                        format!("nodes[{node_key}].attachments[{group_ref}].actions"),
                        format!("aksiyon '{action}' bu kapsamda birden fazla sayılmış"),
                    );
                }
                // Start bloğu da bir aksiyondur (M16: `start[].action` actions{} içinde
                // normal bir ACT'tir) — yalnız transition'lara bakmak, başlatma
                // aksiyonuna konan belge kapısını "ulaşılmaz" sanıp reddederdi.
                let reachable = wfd
                    .transitions
                    .iter()
                    .any(|t| t.action == *action && t.from.contains(node_key))
                    || wfd
                        .start
                        .iter()
                        .any(|s| s.action == *action && s.from == *node_key);
                if !reachable {
                    report.error(
                        "attachment_action_ref",
                        format!("nodes[{node_key}].attachments[{group_ref}].actions"),
                        format!(
                            "aksiyon '{action}' bu node'dan çıkmıyor — kapsam hiçbir zaman uygulanmaz"
                        ),
                    );
                }
            }
        }
    }
}

// ---- §6: wfes_effects.set yolları (catch ve escalation dahil) ----

fn check_effect_paths(wfd: &Wfd, report: &mut ValidationReport) {
    let check_effects = |effects: &Option<crate::types::wfd_v22::WfesEffects>,
                         path: &str,
                         report: &mut ValidationReport| {
        let Some(effects) = effects else { return };
        for key in effects.set.keys() {
            if let PathResolution::Missing = resolve_schema_path(&wfd.context, key) {
                report.error(
                    "effect_path",
                    path.to_string(),
                    format!("effect yolu '{key}' context şemasında yok"),
                );
            }
        }
    };

    for s in &wfd.start {
        check_effects(&s.wfes_effects, &format!("start[{}]", s.id), report);
        for (j, trig) in s.trigger.iter().enumerate() {
            if let Some(c) = &trig.catch {
                check_effects(
                    &Some(c.wfes_effects.clone()),
                    &format!("start[{}].trigger[{j}].catch", s.id),
                    report,
                );
            }
        }
    }
    for t in &wfd.transitions {
        check_effects(&t.wfes_effects, &format!("transitions[{}]", t.id), report);
        for (j, trig) in t.trigger.iter().enumerate() {
            if let Some(c) = &trig.catch {
                check_effects(
                    &Some(c.wfes_effects.clone()),
                    &format!("transitions[{}].trigger[{j}].catch", t.id),
                    report,
                );
            }
        }
    }
    for (key, node) in &wfd.nodes {
        for (j, esc) in node.escalation.iter().enumerate() {
            check_effects(
                &esc.wfes_effects,
                &format!("nodes[{key}].escalation[{j}]"),
                report,
            );
        }
        if let Some(call) = &node.call {
            check_effects(&call.wfes_effects, &format!("nodes[{key}].call"), report);
        }
    }
    for t in &wfd.terminals {
        check_effects(&t.wfes_effects, &format!("terminals[{}]", t.id), report);
    }
    for (name, ax) in &wfd.autoexec {
        check_effects(&ax.wfes_effects, &format!("autoexec[{name}]"), report);
    }
}

// ---- §6: retry — WFD.ALL tek başına ve yalnızca son retrier'da ----

fn check_retries(wfd: &Wfd, report: &mut ValidationReport) {
    let check_triggers = |triggers: &[crate::types::wfd_v22::TriggerInvocation],
                          path: &str,
                          report: &mut ValidationReport| {
        for (j, trig) in triggers.iter().enumerate() {
            let last = trig.retry.len().saturating_sub(1);
            for (k, r) in trig.retry.iter().enumerate() {
                if r.error_equals.iter().any(|e| e == "WFD.ALL") {
                    if r.error_equals.len() > 1 {
                        report.error(
                            "retry_wfd_all",
                            format!("{path}.trigger[{j}].retry[{k}]"),
                            "WFD.ALL yalnızca tek başına kullanılabilir".into(),
                        );
                    }
                    if k != last {
                        report.error(
                            "retry_wfd_all",
                            format!("{path}.trigger[{j}].retry[{k}]"),
                            "WFD.ALL yalnızca son retrier'da kullanılabilir".into(),
                        );
                    }
                }
            }
        }
    };

    for s in &wfd.start {
        check_triggers(&s.trigger, &format!("start[{}]", s.id), report);
    }
    for t in &wfd.transitions {
        check_triggers(&t.trigger, &format!("transitions[{}]", t.id), report);
    }
}

// ---- 2026-07-16 SLA sözleşmesi: escalation + claim_timeout ----
// ---- 2026-07-28: SLA-1/SLA-2 YALNIZ bir node'a devreder. Akışı bitiremez
//      (`terminate` kaldırıldı, terminal hedef yasak — bitirme yalnız SLA-3'ün işi) ve
//      dallanma/fork kararı veremez (`wft` yalnız `{node}` formu).
//      + SLA effects namespace kısıtı ----
// ---- 2026-08-03 (WOR-56/SLA-1): HEDEF hâlâ yalnız node, ama SLA-1 tasarımcının
//      açık tercihiyle (`claim_timeout.collapses_parallel`) paralel kolları
//      düşürebilir. Bu bir DALLANMA kararı değil, "paralel modu kapat + hedefe git"
//      kararıdır; akışı yine bitirmez (terminal hedef hâlâ yasak). ----

/// `Wft`'in wire formunun kullanıcıya gösterilecek adı — SLA hedef formu hatasında
/// hangi biçimin kullanıldığını söylemek için.
fn wft_form_name(wft: &Wft) -> &'static str {
    match wft {
        Wft::Node { .. } => "node",
        Wft::Terminal { .. } => "terminal",
        Wft::Conditional { .. } => "conditions (koşullu dallanma)",
        Wft::Parallel { .. } => "parallel (fork/join)",
        Wft::Collapse { .. } => "collapse (kolları düşür)",
    }
}

/// SLA bağlamında `$action.input.*`, `$exec.result.*` ve `$call.*` YOKTUR (tetikleyici
/// system aktörü; ne aksiyon girdisi, ne autoexec sonucu, ne de bir çağrı dönüşü vardır)
/// — sessizce `null` yazmak yerine WFD reddedilir. `$ctx.*`, `$actor`, `$node`,
/// `$timestamp`, `$wfe_id`, `$env.*` geçerli.
fn check_sla_effect_namespaces(effects: &WfesEffects, path: &str, report: &mut ValidationReport) {
    for (target, raw) in &effects.set {
        walk_strings(raw, &format!("{path}.set[{target}]"), &mut |s, p| {
            for bad in ["$action.input.", "$exec.result.", "$call."] {
                if s.contains(bad) {
                    report.error(
                        "sla_effect_namespace",
                        p.to_string(),
                        format!(
                            "SLA effects'inde '{bad}*' kullanılamaz (system tetikler — aksiyon girdisi, autoexec sonucu ya da çağrı dönüşü yok): '{s}'"
                        ),
                    );
                }
            }
        });
    }
}

/// WOR-56 (2026-08-03) — bir PARALEL KOLUN İÇİNDE yer alan node key'lerinin kümesi.
///
/// SLA collapse'ı yalnız bu kümedeki node'larda anlamlıdır: paralel akışa bağlı olmayan
/// bir node'un süresi dolduğunda düşürülecek kardeş kol YOKTUR. Kural authoring-time'da
/// burada kapatılır (`*_collapse_outside_parallel`).
///
/// Yürüyüş `check_parallel`'in branch subgraph BFS'iyle AYNI: fork'un `branches` giriş
/// node'larından başlanır, transition wft kenarları izlenir, join node'unda durulur;
/// collapse kenarları (kapsam dışına çıkarlar) ve iç içe parallel izlenmez. SLA kenarları
/// (escalation / claim_timeout hedefleri) da izlenmez — onlar kolun İÇİNDEN dışarı çıkan
/// devirlerdir, hedefi kolun parçası yapmaz.
fn parallel_interior_nodes(wfd: &Wfd) -> HashSet<&str> {
    let mut interior: HashSet<&str> = HashSet::new();
    for t in &wfd.transitions {
        let Wft::Parallel { parallel: spec } = &t.wft else {
            continue;
        };
        let join_node: Option<&str> = match &spec.join {
            WftTarget::Node { node } => Some(node.as_str()),
            WftTarget::Terminal { .. } => None,
        };
        let mut queue: VecDeque<&str> = spec.branches.iter().map(|b| b.as_str()).collect();
        let mut visited: HashSet<&str> = queue.iter().copied().collect();
        while let Some(node_key) = queue.pop_front() {
            if Some(node_key) == join_node {
                continue; // join kolun parçası değildir — ötesine geçilmez.
            }
            interior.insert(node_key);
            for tr in &wfd.transitions {
                if !tr.from.contains(node_key) {
                    continue;
                }
                if matches!(&tr.wft, Wft::Collapse { .. } | Wft::Parallel { .. }) {
                    continue;
                }
                for (kind, target) in wft_targets(&tr.wft) {
                    if kind != TargetKind::Node || Some(target) == join_node {
                        continue;
                    }
                    if visited.insert(target) {
                        queue.push_back(target);
                    }
                }
            }
        }
    }
    interior
}

fn check_sla(wfd: &Wfd, report: &mut ValidationReport) {
    // Yalnız gerçekten collapse isteyen bir SLA görülürse hesaplanır (BFS bedeli).
    let wants_collapse = wfd.nodes.values().any(|n| {
        n.claim_timeout
            .as_ref()
            .is_some_and(|ct| ct.collapses_parallel)
            || n.escalation
                .iter()
                .any(|e| matches!(&e.wft, Some(Wft::Collapse { .. })))
    });
    let interior = if wants_collapse {
        parallel_interior_nodes(wfd)
    } else {
        HashSet::new()
    };

    for (key, node) in &wfd.nodes {
        for (j, esc) in node.escalation.iter().enumerate() {
            let path = format!("nodes[{key}].escalation[{j}]");
            // 2026-07-28: SLA-2 akışı BİTİREMEZ — `terminate` kaldırıldı, `wft` zorunlu.
            if esc.terminate.is_some() {
                report.error(
                    "escalation_terminate_removed",
                    format!("{path}.terminate"),
                    "`terminate` kaldırıldı — SLA-2 akışı bitiremez; yalnız root `timeout` (SLA-3) bitirir. Adımı bir node hedefine (`wft`) çevirin ya da adımı kaldırın".into(),
                );
            }
            if esc.wft.is_none() {
                report.error(
                    "escalation_wft_required",
                    path.clone(),
                    "escalation adımı bir node hedefi (`wft`) içermelidir".into(),
                );
            }
            // SLA-2 hedefi `{node}` ya da (2026-08-03, WOR-56/SLA-2) node hedefli
            // `{collapse:{node}}` olabilir. Terminal (akışı bitirir), conditions
            // (dallanma kararı) ve parallel (fork) formları hâlâ bir AKSİYONUN
            // verebileceği kararlardır — bir zamanlayıcının değil.
            //
            // Collapse İSTİSNASI: "kimse süresinde bakmadıysa paraleli kapat" bir
            // dallanma kararı DEĞİLDİR — hedef tektir ve tasarım anında sabittir;
            // yalnız "paralel modu bitir + kardeşleri düşür" yan etkisi eklenir.
            // Akışı yine bitirmez: collapse hedefi de terminal olamaz.
            match &esc.wft {
                None | Some(Wft::Node { .. }) => {}
                Some(Wft::Collapse {
                    collapse: WftTarget::Node { .. },
                }) => {}
                Some(Wft::Terminal { terminal })
                | Some(Wft::Collapse {
                    collapse: WftTarget::Terminal { terminal },
                }) => report.error(
                    "sla_terminal_target",
                    format!("{path}.wft"),
                    format!(
                        "SLA-2 escalation hedefi terminal olamaz ('{terminal}') — SLA yalnız node'lar arası devirdir; akışı zaman aşımıyla bitiren tek kural root `timeout` (SLA-3)"
                    ),
                ),
                Some(other) => report.error(
                    "sla_target_not_node",
                    format!("{path}.wft"),
                    format!(
                        "SLA-2 escalation hedefi `{{node}}` ya da `{{collapse:{{node}}}}` olabilir — '{}' formu kullanılamaz. Dallanma/fork bir aksiyonun kararıdır; SLA sıradaki havuza devreder (istenirse paraleli sonlandırarak)",
                        wft_form_name(other)
                    ),
                ),
            }
            // 2026-08-03 — collapse YALNIZ paralel kolun içindeki node'da kullanılabilir:
            // paralel akışa bağlı olmayan bir node'un süresi dolduğunda düşürülecek
            // kardeş kol yoktur, "paraleli sonlandır" anlamsız bir ayardır.
            if matches!(&esc.wft, Some(Wft::Collapse { .. })) && !interior.contains(key.as_str()) {
                report.error(
                    "escalation_collapse_outside_parallel",
                    format!("{path}.wft"),
                    format!(
                        "'{key}' bir paralel kolun içinde değil — SLA-2 collapse hedefi yalnız fork ile join arasındaki node'larda kullanılabilir. Hedefi düz `{{node}}` formuna çevirin"
                    ),
                );
            }
            if let Some(effects) = &esc.wfes_effects {
                check_sla_effect_namespaces(effects, &format!("{path}.wfes_effects"), report);
            }
        }
        if let Some(ct) = &node.claim_timeout {
            let path = format!("nodes[{key}].claim_timeout");
            if let Err(e) = parse_iso8601_duration(&ct.after) {
                report.error("duration_format", format!("{path}.after"), e.to_string());
            }
            if let Some(effects) = &ct.wfes_effects {
                check_sla_effect_namespaces(effects, &format!("{path}.wfes_effects"), report);
            }
            if let Some(target) = &ct.wft {
                // SLA-1 hedefi YALNIZ node olabilir (2026-07-28). Terminal referansı
                // ayrı bir hata verir; hiç bilinmiyorsa cross_ref.
                if wfd.terminals.iter().any(|t| t.id == *target) {
                    report.error(
                        "sla_terminal_target",
                        format!("{path}.wft"),
                        format!(
                            "SLA-1 claim_timeout hedefi terminal olamaz ('{target}') — bir node seçin ya da hedefi kaldırıp claim'i havuza bırakın"
                        ),
                    );
                } else if !wfd.nodes.contains_key(target) {
                    report.error(
                        "cross_ref",
                        format!("{path}.wft"),
                        format!("bilinmeyen node '{target}'"),
                    );
                }
            }
            // WOR-56/SLA-1 (2026-08-03): "paraleli sonlandır" tercihi. Collapse'ın
            // GİDECEĞİ bir hedef olmak zorunda — `wft` yoksa "aynı havuza dön"
            // demektir ve kolları düşürmenin anlamı kalmaz.
            if ct.collapses_parallel && ct.wft.is_none() {
                report.error(
                    "claim_timeout_collapse_requires_wft",
                    path.clone(),
                    "SLA-1 'collapses_parallel' bir node hedefi (`wft`) ister — paraleli sonlandırıp nereye gidileceği belirsiz kalamaz; hedef verin ya da bayrağı kaldırın".into(),
                );
            }
            // 2026-08-03 — collapse YALNIZ paralel kolun içindeki node'da kullanılabilir:
            // paralel akışa bağlı olmayan bir node'un süresi dolduğunda düşürülecek
            // kardeş kol yoktur, "paraleli sonlandır" anlamsız bir ayardır.
            if ct.collapses_parallel && !interior.contains(key.as_str()) {
                report.error(
                    "claim_timeout_collapse_outside_parallel",
                    format!("{path}.collapses_parallel"),
                    format!(
                        "'{key}' bir paralel kolun içinde değil — 'collapses_parallel' yalnız fork ile join arasındaki node'larda kullanılabilir. Bayrağı kaldırın"
                    ),
                );
            }
        }
    }
}

// ---- M7: $exec.response.* her yerde hata; $ctx.* referans yolları şemada olmalı ----

fn check_string_namespaces(wfd: &Wfd, report: &mut ValidationReport) {
    let value = match serde_json::to_value(wfd) {
        Ok(v) => v,
        Err(_) => return,
    };
    walk_strings(&value, "$", &mut |s, path| {
        if s.contains("$exec.response.") {
            report.error(
                "exec_response",
                path.to_string(),
                format!("'$exec.response.*' kaldırıldı (M7) — '$exec.result.*' kullanın: '{s}'"),
            );
        }
        // $ctx.<path> referansları — token'ı çıkar, şemada doğrula
        let mut rest = s;
        while let Some(idx) = rest.find("$ctx.") {
            let token: String = rest[idx + 5..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '.')
                .collect();
            let token = token.trim_end_matches('.').to_string();
            if !token.is_empty() {
                if let PathResolution::Missing = resolve_schema_path(&wfd.context, &token) {
                    report.error(
                        "ctx_ref",
                        path.to_string(),
                        format!("'$ctx.{token}' context şemasında yok"),
                    );
                }
            }
            rest = &rest[idx + 5..];
        }
    });
}

fn walk_strings<'a>(v: &'a Value, path: &str, f: &mut impl FnMut(&'a str, &str)) {
    match v {
        Value::String(s) => f(s, path),
        Value::Array(arr) => {
            for (i, item) in arr.iter().enumerate() {
                walk_strings(item, &format!("{path}[{i}]"), f);
            }
        }
        Value::Object(map) => {
            for (k, item) in map {
                // context şeması serbest metin içerebilir (description vb.) — atla.
                // `calls` de atlanır: WFC-IN'in kendi kuralları (`call_input_namespace`,
                // `call_input_source_undeclared`) daha iyi mesaj verir; generic ctx_ref
                // burada koşarsa aynı hata iki kez raporlanır.
                if path == "$" && (k == "context" || k == "calls") {
                    continue;
                }
                walk_strings(item, &format!("{path}.{k}"), f);
            }
        }
        _ => {}
    }
}

// ---- context şeması yol çözümü ----

enum PathResolution {
    Found,
    Missing,
    /// Şema bu derinliği kısıtlamıyor (properties tanımsız, $ref, vs.)
    Opaque,
}

fn resolve_schema_path(context: &Value, dotted: &str) -> PathResolution {
    let mut current = context;
    for segment in dotted.split('.') {
        if current.get("$ref").is_some() {
            return PathResolution::Opaque;
        }
        let Some(props) = current.get("properties").and_then(Value::as_object) else {
            return PathResolution::Opaque;
        };
        let Some(next) = props.get(segment) else {
            return PathResolution::Missing;
        };
        current = next;
    }
    PathResolution::Found
}
