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
use crate::v22::wfah_kind::{parse_marker, WfahKind};
use bumpalo::Bump;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use zen_expression::lexer::Lexer;
use zen_expression::parser::{Node, Parser};

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
    check_duplicate_c_a(wfd, &mut report);
    check_escalation_grant_noop(wfd, &mut report);
    check_cross_refs(wfd, &mut report);
    check_send_back(wfd, &mut report);
    check_start_rules(wfd, &mut report);
    check_wft_conditions(wfd, &mut report);
    check_graph(wfd, &mut report);
    check_parallel(wfd, &mut report);
    check_expressions(wfd, &mut report);
    check_action_inputs(wfd, &mut report);
    check_input_required_optional_overlap(wfd, &mut report);
    check_context_required_removed(wfd, &mut report);
    check_context_named_types(wfd, &mut report);
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
    check_c_orgu_wfah_anchors(wfd, &mut report);
    check_c_u_items(wfd, &mut report);
    check_c_a_shape(wfd, &mut report);
    report
}

/// C_A'nın iki biçiminden hangisinde olduğumuzu ve o biçimin kısıtlarını denetler.
///
/// **Çapasız** (`c_orgu` yok) biçim yalnız KİŞİ kanalına açıktır: `c_u` zorunlu, `c_r`
/// yasak. Şema da aynı kısıtı koyar; kural BURADA da durur çünkü validator elle yazılmış
/// JSON'un (şema kapısını bir yolla atlatan bir istemcinin) son savunma hattıdır ve
/// mesajı tasarımcıya dönük olandır. `matcher` üçüncü katman: çapasız kuralda rol
/// kanalını hiç sormaz.
///
/// Yürüyüş ANAHTAR bazlıdır (`c_a` / `reassign`) — `x-visibility` objeleri de
/// `c_orgu`/`c_r`/`c_u` taşır ama SEMANTİĞİ farklıdır (kriterler arası OR, scope'suz
/// rol/kişi meşru), o yüzden bu kuralın dışında kalmaları gerekir.
/// `listable[]`/`wf_admin[]` guard'ında `$actor` YASAK (2026-08-13).
///
/// Bu guard'lar görünürlük projeksiyonuna (`wf.wfe.view_c_a`) commit anında,
/// yani soruyu soracak kişi HENÜZ BİLİNMEZKEN yazılır. `$actor` referansı
/// guard'ı viewer'a bağlar: aynı WFE iki kişiye iki farklı cevap verirdi ve
/// tek bir kolona yazılamazdı. Aynı ihtiyaç `c_a`'nın kendisiyle karşılanır
/// (kural zaten "kim" sorusunun cevabıdır).
///
/// Node aksiyonlarının `when`'i bu kısıttan ETKİLENMEZ — orada `$actor`
/// geçişi yapan kişidir ve karar anında bilinir.
fn grant_when_actor_ref(when: &str, path: String, report: &mut ValidationReport) {
    if when.contains("$actor") {
        report.error(
            "grant_when_actor_ref",
            path,
            "listable/wf_admin guard'ında $actor kullanılamaz: bu kurallar görünürlük \
             projeksiyonuna viewer bilinmezken yazılır. Kişi/rol kısıtını c_a ile ifade edin."
                .into(),
        );
    }
}

fn check_c_a_shape(wfd: &Wfd, report: &mut ValidationReport) {
    let Ok(doc) = serde_json::to_value(wfd) else {
        return;
    };
    let mut sites = Vec::new();
    collect_key_sites(&doc, "c_a", "", &mut sites);
    collect_key_sites(&doc, "reassign", "", &mut sites);

    for (path, rule) in sites {
        let Some(obj) = rule.as_object() else {
            continue;
        };
        if obj.contains_key("c_orgu") {
            continue; // çapalı biçim — kısıtları şema + diğer kurallar taşıyor
        }
        let has_users = obj
            .get("c_u")
            .and_then(Value::as_array)
            .is_some_and(|a| !a.is_empty());

        if obj.contains_key("c_r") {
            report.error(
                "c_a_anchorless_role",
                path.clone(),
                "c_orgu verilmeyen (çapasız) bir C_A'da c_r YAZILAMAZ: çapasız bir rol kanalı \
                 'tenant'taki bu role sahip herkes' demektir ve kurulabilecek en geniş kapıdır. \
                 Rol havuzu istiyorsanız c_orgu ile bir çapa verin (ör. \"self\"); kapıyı belirli \
                 kişilere açmak istiyorsanız yalnız c_u kullanın."
                    .into(),
            );
        }
        if !has_users {
            report.error(
                "c_a_anchorless_needs_user",
                path.clone(),
                "C_A ne c_orgu ne de c_u taşıyor — bu kural HİÇ KİMSEYLE eşleşmez ve node \
                 kalıcı olarak durur. Bir çapa verin (c_orgu) ya da kişileri sayın (c_u)."
                    .into(),
            );
        }
    }
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

/// Adlandırılmış tipi (`format: "<Ad>"` → `$defs.<Ad>`) çözer.
///
/// `x-wf-kind` denetimleri bunu kullanır: kind'lı alan bir tanımın arkasında olabilir ve
/// çözülmezse MEŞRU bir belge reddedilirdi. Zincir uzunluğu sınırlı → döngü asla dönmez.
/// (2026-08-19: eski `$ref` sözdizimi okuyucusu KALDIRILDI — bkz. `v22::ctx_types`.)
fn deref_defs<'a>(root: &'a Value, node: &'a Value) -> Option<&'a Value> {
    let mut current = node;
    for _ in 0..16 {
        let Some(name) = current.get("format").and_then(Value::as_str) else {
            return Some(current);
        };
        current = root.get("$defs")?.get(name)?;
    }
    None // 16 hop'tan uzun zincir = döngü kabul edilir
}

enum NodeAt<'a> {
    Found(&'a Value),
    Missing,
    /// Şema bu derinliği kısıtlamıyor (`properties` yok, çözülemeyen tanım adı, ...).
    Opaque,
}

/// Bir context yolundaki şema düğümü, adlandırılmış tip çözülerek.
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
        // `from` bir objedir — ikisi de bu kuralın dışında (wfah formunun kendi kuralı
        // `check_c_orgu_wfah_anchors`tır).
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

/// wfah çapasının işaret ettiği aksiyon KATALOGDA var mı.
///
/// `check_c_orgu_anchor_kinds`ten ayrı durmasının sebebi sorunun ayrı olması: orada
/// context alanının TİPİ sorulur, burada aksiyon adının VARLIĞI. İkisi ayrı formlardır
/// (`from` STRING ↔ `from` OBJE) ve tek gövdede birleşince hangi dalın hangi kuralı
/// koştuğu okunamaz hale gelirdi.
///
/// Neden HATA: `resolver::anchor_from_wfah` adı geçmişte bulamazsa `Ok(None)` döner,
/// `resolve_c_orgu` boş birim listesi verir ve o node'da HİÇ KİMSE yetkilenmez — ne
/// hata, ne uyarı, ne log; akış sessizce kilitlenir. Sahada görülen hâl (2026-08-20):
/// aksiyon editörde yeniden adlandırıldı (`Baslat`), çapa eski adda kaldı (`Ba_lat`).
/// Aksiyon adı KİMLİKTİR ve etiketten türediği için yeniden adlandırma bu referansı
/// rutin olarak kırar. Koşuldaki ölü ad (hep-false) uyarıyla geçebilir, çapadaki
/// geçemez: çapa yetkinin kaynağıdır, bedeli duran bir akıştır.
///
/// Editör aynası: `utils/validation.ts` → `UNKNOWN_WFAH_ANCHOR`. İki taraf da
/// katalog anahtarlarına bakar; ayrışırlarsa editörde yeşil görünen belge yayında 422 alır.
fn check_c_orgu_wfah_anchors(wfd: &Wfd, report: &mut ValidationReport) {
    let Ok(doc) = serde_json::to_value(wfd) else {
        return; // serileştirme hatası başka kuralların işi
    };
    let mut sites = Vec::new();
    collect_key_sites(&doc, "c_orgu", "", &mut sites);

    for (path, c_orgu) in sites {
        // Yalnız wfah formu: `from` bir OBJE ve içinde `wfah` var. Selector düz string,
        // ctx-anchor'da `from` STRING — ikisi de bu kuralın dışında.
        let Some(action) = c_orgu
            .get("from")
            .and_then(|from| from.get("wfah"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        // Boş ad şemanın işi (`$defs/wfahAnchor.wfah.minLength: 1`); burada ikinci bir
        // kod üretmek aynı kusuru iki kez raporlamak olurdu.
        if action.is_empty() || wfd.actions.contains_key(action) {
            continue;
        }
        report.error(
            "c_orgu_anchor_unknown_action",
            format!("{path}.from"),
            format!(
                "c_orgu çapası '{action}' aksiyonunu işaret ediyor ama bu ad aksiyon \
                 katalogunda YOK. Aksiyon yeniden adlandırıldıysa çapayı yeni anahtara \
                 çevirin; adı doğruysa aksiyonu tanımlayın. Aksi halde çapa çalışma anında \
                 hiçbir WFAH kaydına oturmaz, kapsam boş çözülür ve o node'da KİMSE \
                 yetkilenmez — akış hatasız durur."
            ),
        );
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
        let Some(items) = c_u.as_array() else {
            continue;
        };
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
            for (target, raw) in effects.all_writes() {
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
    for (action_key, t) in &wfd.actions {
        if t.from == *node_key {
            report.error(
                "call_node_has_action",
                format!("actions.{action_key}.from"),
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
    // v2.3: "başlatan node" = start aksiyonunun `from`u (`Ç7+Ç8`: `start[].from` silindi,
    // aynı gerçek iki yerde tutulmaz).
    if wfd
        .start
        .iter()
        .any(|s| crate::types::wfd_v22::start_action(wfd, s).is_some_and(|a| a.from == *node_key))
    {
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
        for (target, raw) in effects.all_writes() {
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
            for (target, raw) in effects.all_writes() {
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

    // v2.3: start gövdesi aksiyon kaydında — `wfes_effects` de oradan okunur.
    let mut types = HashMap::new();
    if let Some(effects) = &action.wfes_effects {
        for (ctx_path, raw) in effects.all_writes() {
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
/// Bir context yolunun TİPİ — adlandırılmış tip (`format` → `$defs`, eski `$ref`)
/// ÇÖZÜLEREK. Çözüm `v22::ctx_types`tedir; burada ikinci bir gezici YAZILMAZ (iki
/// kopya ayrışırsa tasarım zamanı ile çalışma anı farklı cevap verirdi).
///
/// 2026-08-19 öncesinde bu fonksiyon `$ref`i hiç çözmüyordu: `$defs` arkasındaki
/// alanlar `effect_type_mismatch` denetiminin DIŞINDA kalıyordu (tip bilinmiyor →
/// kıyas yok). Adlandırılmış tip `format`a taşınırken o boşluk da kapandı.
pub(crate) fn schema_type_at(context: &Value, dotted: &str) -> Option<String> {
    let crate::v22::ctx_types::Resolved::Found(schema) =
        crate::v22::ctx_types::field_schema(context, dotted)
    else {
        return None;
    };
    schema.get("type").and_then(Value::as_str).map(String::from)
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
            for (target, raw) in effects.all_writes() {
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
    // v2.3 (`Ç5`): transition id KAVRAMI ÖLDÜ — yönlendirme kuralının kimliği
    // `actions` map ANAHTARIDIR ve map anahtarı yapısal olarak tekildir. Ham JSON'daki
    // çift anahtar ise ayrıştırıcıya hiç ulaşmadan `dupkeys` kapısında reddedilir
    // (`E10`), yani burada sayılacak bir tekrar kalmaz.
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
    // Terminal id'si artık MAKİNE kimliğidir: gösterim `terminals[].label`e taşındı.
    // Bu yüzden eski "isim" kuralı (case-insensitive benzersizlik) KALKTI — `onaylandi`
    // ile `Onaylandi` farklı iki kimliktir, tıpkı node key'leri gibi. Yerine gelen
    // kural desen + tam benzersizliktir.
    let mut terminal_ids = HashSet::new();
    for (i, t) in wfd.terminals.iter().enumerate() {
        if !t.id.is_empty() && !t.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            report.error(
                "terminal_id_pattern",
                format!("terminals[{i}].id"),
                format!(
                    "terminal id '{}' deseni ihlal ediyor (^[a-zA-Z0-9_]+$) — kullanıcı metni \
                     `label` alanına yazılır",
                    t.id
                ),
            );
        }
        if !terminal_ids.insert(t.id.clone()) {
            report.error(
                "terminal_id_dup",
                format!("terminals[{i}]"),
                format!("terminal id '{}' birden fazla kez tanımlı", t.id),
            );
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

// ---- §2b: node key `slug(c_a)`dan TÜRETİLMEZ, ama c_a ile BİRE BİRDİR ----
//
// TARİHÇE — iki ayrı karar, karıştırılmamalı:
//
// 2026-08-12: node kimliğini tasarımcı verir; `c_a` node'un bir ALANIDIR, kimliği değil.
// Eski kural iki şeyi birden dayatıyordu:
//
//   • `node key == slug(c_a)` → kimlik org yolunu (ORGTRVLANG) taşıyordu ve "bu adımı
//     kim yapar"ı değiştirmek adımın KİMLİĞİNİ değiştiriyordu: koşan işler eski
//     anahtarı gösterirken belge yeni anahtara geçiyordu. Bu kısıt KALKTI ve GERİ
//     GELMEDİ — kimlik hâlâ tasarımcınındır.
//   • `duplicate_c_a` → o gün bu da kaldırıldı (uyarıya, `shared_c_a`, döndü), gerekçe:
//     "aynı kişinin iki farklı adımı olamıyor" ve paralel kolda aynı havuzdan iki kol
//     açılamıyor.
//
// 2026-08-14 (GERİ GETİRİLDİ, HATA): `duplicate_c_a` yeniden HATA. Sebep: uyarı
// döneminde tasarımcılar aynı havuzu iki node olarak çizdiğinde motor iki AYRI bekleme
// noktası üretiyordu — aynı kişi havuzunda iki ayrı "sıra sende" satırı, iki ayrı claim,
// iki ayrı geçmiş dalı. Bu, çizeni de o havuzdaki insanı da yanıltıyordu. Yeni değişmez:
// **aynı c_a = aynı kimlik, aynı kimlik = aynı c_a** → bir canonical c_a belgede EN
// FAZLA BİR node'da bulunabilir. Ardışık adım farkı aksiyonların `when` koşuluyla
// ($wfah) verilir. FEDA EDİLEN: paralel kolda "aynı havuzdan iki kol" (K-of-N quorum'un
// N kolu aynı havuza bakamaz) — bilinçli ve GEÇİCİ kısıt.
//
// Anahtarın BİÇİMİ şemada zorlanır: `nodes` `propertyNames: idName`
// (`^[A-Za-z_][A-Za-z0-9_-]*$`). Anahtar benzersizliği yapısaldır (JSON objesi anahtarı)
// ve node/terminal ortak isim uzayı çakışması `check_uniqueness`te denetlenir.

/// Aynı `c_a`'yı taşıyan iki node — HATA (2026-08-14'te geri getirildi).
///
/// Değişmez: **aynı c_a = aynı kimlik**. Bir canonical c_a belgede EN FAZLA BİR node'da
/// bulunabilir; aynı kimlik de daima aynı c_a'yı taşır. Kimliği yine TASARIMCI verir
/// (2026-08-12 kararı duruyor, kimlik `slug(c_a)` DEĞİLDİR) — geri gelen tek şey
/// TEKİLLİK kısıtıdır.
///
/// **Ardışık adımlar:** "Müdür inceler" ve "müdür nihai onayı verir" aynı havuzdur →
/// AYNI node'dur; fark alınan AKSİYONDADIR ve aksiyonun `when` koşuluyla (`$wfah`
/// üzerinden "önceki aksiyon şuydu") ayrılır. İki node açmak motorda iki ayrı bekleme
/// noktası üretir: aynı havuzda iki "sıra sende" satırı, iki claim, bölünmüş geçmiş.
///
/// **Paralel kolda aynı havuzdan iki kol ŞİMDİLİK DESTEKLENMEZ:** kol kimliği node
/// anahtarıdır (`BranchState.branch_node`), dolayısıyla aynı havuza bakan iki kol iki
/// node ister — bu kural onu yasaklar. Bilinçli, geçici kısıt (kararın gerekçesi ve
/// feda edileni `docs/spec/decisions.md`, 2026-08-14).
fn check_duplicate_c_a(wfd: &Wfd, report: &mut ValidationReport) {
    let mut seen: HashMap<String, &String> = HashMap::new();
    for (key, node) in &wfd.nodes {
        if let Some(prev) = seen.insert(node.c_a.canonical(), key) {
            report.error(
                "duplicate_c_a",
                format!("nodes[{key}]"),
                format!(
                    "'{prev}' ve '{key}' AYNI c_a'yı taşıyor. Aynı c_a TEK node demektir: \
                     ardışık adımların farkı aksiyonların `when` koşuluyla ($wfah) \
                     verilir. Paralel kolda aynı havuzdan iki kol şimdilik desteklenmiyor."
                ),
            );
        }
    }
}

// ---- §1: cross-reference ----

fn check_cross_refs(wfd: &Wfd, report: &mut ValidationReport) {
    for (action_key, t) in &wfd.actions {
        let path = format!("actions.{action_key}");
        if !wfd.nodes.contains_key(&t.from) {
            report.error(
                "cross_ref",
                format!("{path}.from"),
                format!("bilinmeyen node '{}'", t.from),
            );
        }
        // v2.3 (`Ç5`): `transitions[].action` cross-ref denetimi ÖLDÜ. Kural, bir
        // transition'ın `actions` katalogunda var olmayan bir aksiyona işaret
        // edebilmesi yüzünden vardı; artık aksiyonun KENDİSİ katalog girdisidir
        // (kimlik = map anahtarı), yani "bilinmeyen aksiyon" yapısal olarak imkânsız.
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
    }
    // v2.3 (`Ç7+Ç8`): start için AYRI cross-ref döngüsü GEREKMEZ. Gövde (`trigger`,
    // `wft`) aksiyon kaydına indi ve yukarıdaki `wfd.actions` döngüsü onu zaten
    // denetliyor. İkinci bir döngü aynı hatayı iki kez raporlardı.
    for (key, node) in &wfd.nodes {
        // v2.3 (`Ç9`): escalation'ın `wft`i YOK — hedefi olmayan bir kademe için
        // cross-ref denetlenecek bir referans da yok. `grant.c_a`nın kendi kuralları
        // ayrı issue'nun işi (`E13`).
        // WFC node'unun çıkışı `call.wft`'dir — normal bir wft kenarı gibi doğrulanır.
        if let Some(call) = &node.call {
            if let Some(wft) = &call.wft {
                check_wft_refs(wfd, wft, &format!("nodes[{key}].call.wft"), report);
            }
        }
    }
}

fn check_wft_refs(wfd: &Wfd, wft: &Wft, path: &str, report: &mut ValidationReport) {
    // Geri gönderme hedefleri KENDİ kodlarıyla denetlenir (`send_back_target_unknown`) —
    // burada ikinci kez jenerik `cross_ref` basılsaydı tasarımcı aynı sorunu iki
    // farklı isimle görürdü.
    if matches!(wft, Wft::SendBack { .. }) {
        return;
    }
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

// ---- GERİ GÖNDER (`send_back`) — `wft: {targets}` ----

/// Geri gönderme denetimi. Hedef artık aksiyon ANAHTARINA kodlanmadığı için
/// (`Geri_Gonder__gt__self__mudur` kalktı) hataların hepsi burada, tasarımcıya dönük
/// adlarla yakalanır; runtime'ın gördüğü tek şey "listede var mı" sorusudur.
fn check_send_back(wfd: &Wfd, report: &mut ValidationReport) {
    // REZERVE AD YOKTUR (2026-08-21 akşamı geri alındı). Aksiyonu geri gönderme yapan
    // şey adı değil, `wft`inin bu formu olmasıdır; hangi aksiyonun hedef seçtirdiği
    // `possible_actions` yanıtındaki `target` alanından okunur — istemcinin ad
    // ayrıştırmasına gerek yoktur. Tek anahtar (`send_back`) denendi ve GERİ ALINDI:
    // tüm geri gönderme adımlarını TEK aksiyon kimliğine indiriyor, dolayısıyla
    // girdi sözleşmesini (`actions.<key>.input`) akış genelinde tekleştiriyordu —
    // bir node'un geri göndermesine zorunlu alan eklemek hepsine ekliyordu.
    for (action_key, t) in &wfd.actions {
        let Wft::SendBack { targets } = &t.wft else {
            continue;
        };
        let path = format!("actions.{action_key}.wft");
        if targets.is_empty() {
            report.error(
                "send_back_no_targets",
                path.clone(),
                "geri gönderme hedef listesi boş — en az bir hedef gerekir".into(),
            );
        }
        let mut seen: HashSet<&str> = HashSet::new();
        for (i, g) in targets.iter().enumerate() {
            let at = format!("{path}.targets[{i}]");
            if !wfd.nodes.contains_key(&g.node) {
                report.error(
                    "send_back_target_unknown",
                    at.clone(),
                    format!("bilinmeyen node '{}'", g.node),
                );
            }
            if !seen.insert(g.node.as_str()) {
                report.error(
                    "send_back_target_dup",
                    at.clone(),
                    format!("hedef '{}' listede birden fazla kez var", g.node),
                );
            }
            // Kendine dönen hedef bir "geri gönder" seçeneği DEĞİLDİR: aksiyon
            // uygulanır, WFE aynı node'da kalır ve claim sıfırlanır — kullanıcı
            // için hiçbir şey olmamış gibi görünen sessiz bir tuzak.
            if t.from.contains(&g.node) {
                report.error(
                    "send_back_target_self",
                    at,
                    format!(
                        "hedef '{}' transition'ın kendi `from` node'u — kendine geri göndermek anlamsızdır",
                        g.node
                    ),
                );
            }
        }
    }

    // Hedef menüsü YALNIZ transition'da anlamlıdır: hedefi bir KİŞİ seçer. Start / escalation /
    // çağrı dönüşü yollarında seçim yapacak kimse yoktur (sırasıyla: seçim taşıyan bir
    // API yok, tetikleyici system aktörü, karar çağrılanın sonucunda). Şema `$defs/wft`
    // paylaşıldığı için bu kapı burada durur — yoksa hata ancak RUNTIME'da, akış
    // tıkandığında görünürdü.
    let mut misplaced = Vec::new();
    for s in &wfd.start {
        // v2.3: start aksiyonunun `wft`i. Kural KALIR — geri gönderme bir başlatma
        // biçimi değildir; yalnız okuma yolu değişti.
        if crate::types::wfd_v22::start_action(wfd, s)
            .is_some_and(|a| matches!(a.wft, Wft::SendBack { .. }))
        {
            misplaced.push(format!("actions.{}.wft (start)", s.action));
        }
    }
    for (key, node) in &wfd.nodes {
        if let Some(call) = &node.call {
            if matches!(call.wft, Some(Wft::SendBack { .. })) {
                misplaced.push(format!("nodes[{key}].call.wft"));
            }
        }
        // v2.3: escalation `wft` taşımaz → orada yanlış yerleşmiş bir SendBack olamaz.
    }
    for path in misplaced {
        report.error(
            "send_back_wft_placement",
            path,
            "geri gönderme hedef seçimi (`targets`) yalnız transitions[].wft içinde kullanılabilir \
             — bu yolda hedefi seçecek bir aktör yoktur"
                .into(),
        );
    }
}

fn wft_targets(wft: &Wft) -> Vec<(TargetKind, &str)> {
    let mut out = Vec::new();
    match wft {
        Wft::Node { node } => out.push((TargetKind::Node, node.as_str())),
        Wft::Terminal { terminal } => out.push((TargetKind::Terminal, terminal.as_str())),
        // Geri gönderme: her hedef gerçek bir çıkış kenarıdır — graf erişilebilirliği
        // (BFS) bunları izlemek ZORUNDA, aksi halde yalnız geri göndermeyle ulaşılan
        // node'lar "erişilemez" görünürdü. Referans denetimi ise `check_send_back`te
        // kendi koduyla yapılır (bkz. `check_wft_refs`).
        Wft::SendBack { targets } => {
            for t in targets {
                out.push((TargetKind::Node, t.node.as_str()));
            }
        }
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

/// Bir ZEN ifadesindeki `$` REFERANSLARINI, en fazla iki nokta segmentiyle, çıkarır
/// (`$ctx`, `$actor`, `$action.input`, `$env.ANAHTAR` …).
///
/// Metin taraması, ZEN AST'si değil — `branch_refs_in`in aynı gerekçesi: `zen_expression`
/// ayrıştırılmış ağacı public API'de vermiyor.
fn dollar_refs_in(expr: &str) -> Vec<String> {
    fn ident(b: &[u8], mut i: usize) -> (usize, usize) {
        let start = i;
        while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
            i += 1;
        }
        (start, i)
    }
    let b = expr.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(rel) = expr[i..].find('$') {
        let at = i + rel;
        let (s1, e1) = ident(b, at + 1);
        if e1 == s1 {
            i = at + 1;
            continue;
        }
        let mut end = e1;
        if b.get(end) == Some(&b'.') {
            let (s2, e2) = ident(b, end + 1);
            if e2 > s2 {
                end = e2;
            }
        }
        out.push(expr[at..end].to_string());
        i = end.max(at + 1);
    }
    out
}

/// `E11` — **start `when`i BEYAZ LİSTEDİR.** Serbest beş kök:
/// `$action.input.*` · `$actor` · `$timestamp` · `$wfe_id` · `$env`.
/// Geri kalan HER kök yasak — bilinen ya da bilinmeyen. `$node` DAHİL yasak
/// (start anında sabittir: start aksiyonunun `from`u).
///
/// Neden kara liste DEĞİL: kara listede unutulan bir kök tasarımcıya SESSİZCE hiç
/// başlamayan bir akış verir. `$prev`/`$first` boş defterde boş kabuk döner ve
/// karşılaştırma false'a düşer — akış hiç başlamaz, hiçbir yerde hata görünmez.
/// `Ç7+Ç8`in kara listesi üç günde çürüdü (`$valid`, `$branch_round`, `$env`,
/// `$branches`/`$arrived` listede yoktu). Beyaz listede unutulan kök ise GÜRÜLTÜLÜ
/// bir hata verir: bedeli tasarımcının bir kez şikâyet etmesidir, sessiz bir akış değil.
///
/// Emsal: `grant_when_actor_ref`. Kapsam: yalnız START aksiyonunun `when`i — normal
/// aksiyonların guard'ı bu kısıttan ETKİLENMEZ (orada `$ctx` meşru ve gereklidir).
/// `E13`/S2 — grant guard'ının KAPALI kök listesi. Yasak altı kök:
/// `$action.*` · `$exec.*` · `$call.*` · `$env.*` · `$branches` · `$arrived`.
///
/// `$actor` bu kuralın DIŞINDADIR: onu mevcut `grant_when_actor_ref` yakalar (aynı
/// yasak, ayrı gerekçe — orada konu tutarlılıktır, `$actor` bağlıdır ama projeksiyon
/// yolu ile doğrudan sorgu yolu iki farklı cevap verir). İki kural aynı ifadeyi iki
/// kez raporlamaz.
///
/// Yasağın ölçülmüş bedeli (kayıt `E13`, `grants.rs:52-60` üzerinden):
/// - `$action.input.*` / `$exec.result.*` → `null`. `null > sayı` ZEN'de sessiz false
///   DEĞİL, `Compare: Unsupported type`. Guard her yetki sorgusunda koştuğu için bu
///   havuz listesinin HER AÇILIŞINDA HTTP 500 demektir.
/// - `$call.*` → boş kabuk, alanlar `null`; aynı karşılaştırma tuzağı.
/// - `$env.*` → bu yolda `with_env` HİÇ çağrılmıyor → "bu ortamda tanımlı değil".
/// - `$branches`/`$arrived` → join bağlamı yok; ifade patlamaz ama YANLIŞ cevap verir
///   ("hiç kol yokmuş").
///
/// ⚠️ **KARA LİSTE — ve bu bir risktir.** `E11` start `when`i için beyaz listeyi
/// seçerken kara listelerin çürüdüğünü ölçmüştü. Burada kayıt (`E13`/S2) yasak kümeyi
/// ADIYLA sayıyor, o yüzden harfiyen uygulanıyor; ama ölçüldü ki listede OLMAYAN bir
/// kök (`$hayaliKok`, ya da `E14`ün henüz yazılmamış `$branch_round`ü) validator'dan
/// HİÇ hata almadan geçiyor — expression yüzeyinde "bilinmeyen kök" diye bir kural yok.
/// Yeni bir ZEN kökü eklendiğinde bu liste ELLE güncellenmek zorunda.
fn grant_when_namespace(when: &str, path: String, report: &mut ValidationReport) {
    const FORBIDDEN: [&str; 6] = ["$action", "$exec", "$call", "$env", "$branches", "$arrived"];
    for r in dollar_refs_in(when) {
        let root = r.split('.').next().unwrap_or(&r);
        if FORBIDDEN.contains(&root) {
            report.error(
                "grant_when_namespace",
                path.clone(),
                format!(
                    "grant guard'ında '{r}' okunamaz. Guard yalnız ctx / defter / node / \
                     zaman görür: $ctx · $wfah · $valid · $prev · $first · $node · $wfe_id · \
                     $timestamp. Aksiyon, otomasyon, çağrı, ortam ve kol bağlamları bu \
                     yolda BAĞLANMAZ — okunursa ya 500 verir ya sessizce yanlış cevap"
                ),
            );
        }
    }
}

/// `E13`/S1 — `escalation_grant_noop`. Bir kademenin `grant.c_a`'sı, bulunduğu node'un
/// `c_a`'sıyla BİREBİR AYNIYSA hata: kademe hiç kimseyi eklemiyor, yani hiçbir şey
/// yapmıyor. §3.7'nin kopyala-yapıştır hatası (node'un `c_a`'sını escalation'a olduğu
/// gibi yapıştırmak) en olası tasarımcı hatasıdır.
///
/// Karşılaştırma `CandidateActor::canonical()` string eşitliğidir — `duplicate_c_a`nın
/// kullandığı fonksiyonun AYNISI. **Alt küme testi YAZILMAZ:** ne kanal kanal, ne
/// `canonical()` üzerinden. Gerekçe kayıtta: grant'ın gerçekten kimseyi eklemediği
/// diğer hâller (rol o birimde yok; selector çalışma anında aynı kümeye çözülüyor) ORG
/// AĞACINA bağlıdır ve belge okunarak cevaplanamaz. Motorun tasarım zamanında
/// bilebileceği tek KESİN sinyal birebir eşitliktir.
///
/// Grant'ın `when`i YOK SAYILIR (`Ç9` hükmü): guard eklemek birebir eşitliği
/// "genişletme" hâline getirmez.
fn check_escalation_grant_noop(wfd: &Wfd, report: &mut ValidationReport) {
    for (key, node) in &wfd.nodes {
        let node_canon = node.act_c_a().canonical();
        for (i, esc) in node.escalation.iter().enumerate() {
            if esc.grant.c_a.canonical() == node_canon {
                report.error(
                    "escalation_grant_noop",
                    format!("nodes[{key}].escalation[{i}].grant"),
                    format!(
                        "kademe '{key}' node'unun havuzunu BİREBİR tekrarlıyor — kimseyi \
                         eklemiyor, yani hiçbir şey yapmıyor. Kademe havuzu GENİŞLETMELİ"
                    ),
                );
            }
        }
    }
}

fn check_start_when_namespace(wfd: &Wfd, report: &mut ValidationReport) {
    const ALLOWED_ROOTS: [&str; 4] = ["$actor", "$timestamp", "$wfe_id", "$env"];
    for s in &wfd.start {
        let Some(action) = crate::types::wfd_v22::start_action(wfd, s) else {
            continue; // `start_action` kuralı raporlar
        };
        let Some(when) = &action.when else {
            continue;
        };
        let path = format!("actions.{}.when (start)", s.action);
        for r in dollar_refs_in(when) {
            let root = r.split('.').next().unwrap_or(&r);
            let ok = ALLOWED_ROOTS.contains(&root) || r == "$action.input";
            if !ok {
                report.error(
                    "start_when_namespace",
                    path.clone(),
                    format!(
                        "start `when`inde '{r}' okunamaz. Serbest olan BEŞ kök: \
                         $action.input.<yol> · $actor · $timestamp · $wfe_id · $env.ANAHTAR. \
                         Akış henüz başlamadığı için defter, context ve çalışma-anı \
                         bağlamları YOKTUR; okunsa sessizce false döner ve akış hiç başlamaz"
                    ),
                );
            }
        }
    }
}

/// `E09`/B1 — `wfes_effects.set_when[].when` içinde okunabilecek kökler: **BEYAZ LİSTE.**
///
/// Kara liste OLMAMASININ sebebi ölçülmüş bir bulgu: `evaluate_bool`un ön denetimi
/// (`check_env_refs`) YALNIZ `$env` anahtarlarını koruyor; öteki kökleri `zen_context`
/// DAİMA dolduruyor — bağlanmamışsa `null` ya da boş kabukla. Yani yasak kök serbest
/// bırakılsa **hata vermez, SESSİZCE false** döner ve koşullu yazım hiç ateşlenmez.
/// Beyaz listede unutulan kök gürültülü hata verir; kara listede unutulan kök sessiz
/// bozukluk üretir.
///
/// YASAK olanlar ve neden:
/// * `$branches` / `$arrived` — yalnız `join_when` değerlendirilirken (`EvalEnv.join`)
///   bağlanır; effect yolunda boş kabuk okunur. Yasak **yapısaldır**, geçici değil.
/// * `$branch_round` — `E14`ün kökü, `apply_effects`in kurduğu `EvalEnv`e bağlanmıyor
///   (paralel mod bilgisi effect bağlamına girmiyor) → `null` okur.
///
/// `$valid` SERBESTTİR: `E05` ile motor tarafında bağlandı (`with_wfah` `$valid`i de
/// kurar), kayıt onu *"bağlanana kadar yasaktır"* diye şartlı yasaklamıştı.
///
/// ⚠️ Taşıyıcı yerin KENDİ yasakları (`sla_effect_namespace`, `call_effect_namespace`)
/// bu kuralın ÜSTÜNE biner ve `set_when[].when` metnine de uygulanır — burada serbest
/// görünen `$action.input.*` SLA yolunda yine reddedilir.
fn check_set_when_namespace(wfd: &Wfd, report: &mut ValidationReport) {
    const ALLOWED_ROOTS: [&str; 12] = [
        "$ctx",
        "$wfah",
        "$valid",
        "$prev",
        "$first",
        "$actor",
        "$timestamp",
        "$wfe_id",
        "$node",
        "$env",
        "$exec",
        "$call",
    ];
    for (site, effects) in each_effects(wfd) {
        for (i, entry) in effects.set_when.iter().enumerate() {
            let path = format!("{site}.wfes_effects.set_when[{i}].when");
            for r in dollar_refs_in(&entry.when) {
                let root = r.split('.').next().unwrap_or(&r);
                if ALLOWED_ROOTS.contains(&root) || r == "$action.input" {
                    continue;
                }
                report.error(
                    "set_when_namespace",
                    path.clone(),
                    format!(
                        "koşullu yazımın `when`inde '{r}' okunamaz. `$branches`/`$arrived` \
                         yalnız join koşulunda, `$branch_round` ise paralel mod bağlamında \
                         bağlanır; effect yolunda boş okunur ve koşul SESSİZCE false döner \
                         — yazım hiç ateşlenmez. Serbest kökler: $ctx · $wfah · $valid · \
                         $prev · $first · $node · $actor · $timestamp · $wfe_id · $env \
                         (+ taşıyıcı yerin izin verdiği $action.input.* · $exec.result.* · $call.*)"
                    ),
                );
            }
        }
    }
}

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
        // v2.3 (`E11` hükmü): V1 — `start[].from` node varlık denetimi **SİLİNDİ**.
        // Alan artık YOK; `ActionDef.from` kendi cross-ref kuralıyla denetleniyor
        // (`check_cross_refs`), yani aynı garanti tek yerden geliyor.
    }
    check_start_when_namespace(wfd, report);
    check_set_when_namespace(wfd, report);
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
    for (action_key, t) in &wfd.actions {
        visit(&t.wft, format!("actions.{action_key}.wft"), report);
    }
    // v2.3: start'ın `wft`i aksiyon kaydında — yukarıdaki `wfd.actions` döngüsü kapsar.
    for (key, node) in &wfd.nodes {
        // v2.3: escalation `wft` taşımaz — ölü koşul aranacak bir hedef yok.
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
        // v2.3 (`Ç7+Ç8`): BFS'in START AYAĞI artık aksiyon kaydından okur. Başlatan
        // node = start aksiyonunun `from`u; o node bir KAYNAKtır ve hiçbir wft hedefi
        // olmasa da erişilebilir sayılır (dead-node uyarısı vermemeli).
        let Some(action) = crate::types::wfd_v22::start_action(wfd, s) else {
            continue; // bozuk belge — `start_action` kuralı raporlar
        };
        reached_nodes.insert(action.from.clone());
        absorb(
            wft_targets(&action.wft),
            &mut reached_nodes,
            &mut reached_terminals,
            &mut queue,
        );
    }

    while let Some(node_key) = queue.pop_front() {
        for t in wfd.actions.values() {
            if t.from == node_key {
                absorb(
                    wft_targets(&t.wft),
                    &mut reached_nodes,
                    &mut reached_terminals,
                    &mut queue,
                );
            }
        }
        if let Some(node) = wfd.nodes.get(&node_key) {
            // ⚠️ v2.3 (`E08` FAZ 2 + `Ç9`): **ESCALATION ARTIK BİR GRAF KENARI DEĞİL.**
            // BFS'in escalation ayağı ÇIKARILDI — kademe iş taşımadığı için hedefi de
            // yok; yalnız o node'un yetki havuzunu genişletiyor.
            //
            // Bunun iki bilinçli sonucu var ve ikisi de HATA seviyesindedir:
            //   (a) yalnız escalation ile "erişilen" bir node artık `WFD.Unreachable`
            //   (b) tek çıkışı escalation olan node artık `no_exit`
            // Bedel ÖLÇÜLDÜ ve sıfır: 6 örnek belgenin hiçbirinde tek çıkışı escalation
            // olan node yok (golden'ın iki escalation'lı node'unun ikisinin de aksiyon
            // çıkışı var).
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
            // v2.3 (K13 + K19): `claim_timeout.wft` KALKTI — claim timeout artık YALNIZ
            // claim'i bırakır, iş taşımaz. Dolayısıyla BFS'in claim_timeout ayağı da
            // yok; escalation'la aynı gerekçe (bkz. yukarıdaki Faz 2 notu).
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

    // Çıkışsız node.
    //
    // ⚠️ v2.3 (`E08` FAZ 2): şarttan **`&& node.escalation.is_empty()` DÜŞTÜ.**
    // Escalation artık iş taşımıyor (`Ç9`), yani bir ÇIKIŞ DEĞİL — tek "çıkışı"
    // escalation olan node'da iş sonsuza kadar orada kalırdı. K10'un vaadi
    // ("escalation iş taşımaz") ancak iş taşımayan bir çıkışı çıkış saymayı
    // bırakınca denetlenebilir hâle gelir.
    //
    // Start node'unun çıkışı start aksiyonunun `wft`sidir — `no_exit`ten muaf.
    let start_from: HashSet<&str> = wfd
        .start
        .iter()
        .filter_map(|s| crate::types::wfd_v22::start_action(wfd, s))
        .map(|a| a.from.as_str())
        .collect();
    for (key, node) in &wfd.nodes {
        if start_from.contains(key.as_str()) {
            continue;
        }
        // WFC node'unun çıkışı `call.wft`'dir — insan ACT'i almadığı için aksiyon
        // aramak yanlış olur (aksine `call_node_has_action` bunu yasaklar).
        if node.call.is_some() {
            continue;
        }
        if !wfd.actions.values().any(|t| t.from == *key) {
            report.error(
                "no_exit",
                format!("nodes[{key}]"),
                format!(
                    "'{key}' çıkışsız — buradan çıkan bir aksiyon gerekli. Escalation \
                     ÇIKIŞ SAYILMAZ: iş taşımaz, yalnız yetki havuzunu genişletir"
                ),
            );
        }
    }

    // v2.3 (`Ç5` + `Ç10`): `ambiguous_transition` KURALI ÖLDÜ.
    //
    // Kural, aynı `(node, action)` çiftine birden çok `transitions[]` girdisi
    // yazılabildiği için vardı ve "runtime ilk-match uygular" diyordu. v2.3'te
    // yönlendirme kuralı aksiyon kaydının İÇİNDE: bir aksiyon anahtarı bir kez
    // tanımlanır, dolayısıyla aynı çift için ikinci bir kural YAZILAMAZ — belirsizlik
    // yapısal olarak imkânsız. `Ç10` "sıra artık seçim yapmaz" hükmüyle ilk-match
    // semantiğini de kaldırdı.
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
    // v2.3 (`E08` Faz 1): `parallel_start` ve `collapse_start` YASAKLARI **KALKMAZ** —
    // yalnız okunan anahtar `start[<id>].wft` yerine `actions.<x>.wft` oldu.
    for s in &wfd.start {
        let Some(action) = crate::types::wfd_v22::start_action(wfd, s) else {
            continue;
        };
        let path = format!("actions.{}.wft (start)", s.action);
        if matches!(&action.wft, Wft::Parallel { .. }) {
            report.error(
                "parallel_start",
                path.clone(),
                "Parallel wft start kuralında kullanılamaz".into(),
            );
        }
        // WOR-56: collapse yalnız paralel dal içinde anlamlıdır — start'ta yasak.
        if matches!(&action.wft, Wft::Collapse { .. }) {
            report.error(
                "collapse_start",
                path,
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
    for (action_key, t) in &wfd.actions {
        if let Wft::Parallel { parallel } = &t.wft {
            forks.push(Fork {
                path: format!("actions.{action_key}.wft"),
                spec: parallel,
            });
        }
    }

    // E05/BAĞLI KURAL 3: bir node belgede EN FAZLA BİR fork'un GİRİŞ kolu olabilir.
    //
    // Ayrıklık denetimi (aşağıdaki `owner` haritası) FORK BAŞINA çalışıyor; iç içe
    // OLMAYAN iki fork aynı giriş node'unu kullanabiliyor ve hiçbir hata çıkmıyordu.
    // `branch_entry` ZEN'e açıldığı an bu BELİRSİZ bir KİMLİK demektir:
    // `#.branch_entry == "hukuk"` iki farklı fork'un kollarını aynı şey sayar.
    // Ayrıca `E14`in tur elemesinin ÖN ŞARTIDIR — çapa (`X`i açan son `_fork`)
    // ancak giriş node'u tekilse TEK olur.
    //
    // Emsal Değişmez #4 (bir canonical `c_a` belgede en fazla bir node'da): kimlik
    // olarak kullanılan ad belge genelinde tekil olmak ZORUNDADIR.
    //
    // Alt-grafların tamamen ayrık olması İSTENMEZ — aynı adımın iki paralel aşamada
    // kullanılması meşrudur; kısıt YALNIZ giriş node'larına konur.
    let mut entry_owner: HashMap<&str, &str> = HashMap::new();
    for fork in &forks {
        for b in &fork.spec.branches {
            match entry_owner.get(b.as_str()) {
                Some(first) if *first != fork.path.as_str() => report.error(
                    "parallel_entry_node_shared",
                    format!("{}.parallel.branches", fork.path),
                    format!(
                        "node '{b}' iki fork'un giriş kolu ({first} ve {}) — kol KİMLİĞİ giriş node'udur ve belge genelinde TEKİL olmak zorundadır (#.branch_entry aksi halde iki fork'un kollarını aynı sayar)",
                        fork.path
                    ),
                ),
                // Aynı fork içindeki tekrar `parallel_branches`ın işi.
                Some(_) => {}
                None => {
                    entry_owner.insert(b.as_str(), fork.path.as_str());
                }
            }
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
                        let referenced = branch_refs_in(expr);
                        for r in &referenced {
                            if !spec.branches.iter().any(|b| b == r) {
                                report.error(
                                    "parallel_join_when_unknown_branch",
                                    format!("{path}.parallel.join_when"),
                                    format!(
                                        "join_when '$branches.{r}' referansı bu fork'un kolu değil — kol kimliği kolun GİRİŞ node'udur"
                                    ),
                                );
                            }
                        }
                        // `E08` Faz 3 — TERS YÖN. Yukarıdaki kural yanlış YAZILAN kolu
                        // yakalar; DOĞRU yazılıp unutulan kolu hiçbir şey yakalamıyordu.
                        // Bilinçli tercih olabileceği için UYARI: koşul o kolu hiç
                        // anmıyorsa join, kol varsın diye beklemez.
                        for b in &spec.branches {
                            if !referenced.iter().any(|r| r == b) {
                                report.warn(
                                    "parallel_join_when_unused_branch",
                                    format!("{path}.parallel.join_when"),
                                    format!(
                                        "'{b}' kolu join_when içinde hiç geçmiyor — join bu kolu beklemez. \
                                         Bilinçliyse sorun yok; değilse koşul o kolu atlıyor"
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

                for (action_key, t) in &wfd.actions {
                    if t.from != *node_key {
                        continue;
                    }
                    if matches!(&t.wft, Wft::Parallel { .. }) {
                        report.error(
                            "parallel_nested",
                            format!("actions.{action_key}.wft"),
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

/// WOR-84: defter dizisi doğrudan indeksleniyor mu (`$wfah[...]` / `$valid[...]`).
/// Geçmiş yeterince uzun değilse VM patlar — `$prev`/`$first` boş geçmişte null
/// döner, patlamaz.
///
/// `E05`/A3: kural `$valid`e DE uygulanır ve orada DAHA tehlikelidir — `$valid`de
/// `seq` boşluklu olduğu için `len($valid)` son `seq`e eşit değildir, yani
/// "son satır" hesabı ham listede işleyen aritmetikle bile tutmaz.
fn indexes_wfah_directly(expr: &str) -> Option<&'static str> {
    for root in ["$wfah", "$valid"] {
        let mut rest = expr;
        while let Some(pos) = rest.find(root) {
            rest = &rest[pos + root.len()..];
            if rest.trim_start().starts_with('[') {
                return Some(root);
            }
        }
    }
    None
}

/// `A02`/S3 — ham `$wfah`ın LİSTE argümanı olduğunda not düşülen KAPALI fonksiyon
/// listesi. `flatMap` bilerek YOKTUR (kararın listesi bu ondur); listeyi büyütmek
/// KARAR ister.
const RAW_LIST_FNS: &[&str] = &[
    "count", "some", "all", "none", "one", "filter", "map", "sum", "avg", "len",
];

/// `A02`/S4 — muafiyet alanı. Yordam bu alana değiyorsa not ÇIKMAZ: `E14`/S3 tur
/// sorgusunu ham defter üzerinde yazmayı EMREDİYOR (`$valid` yalnız YAŞAYAN turu
/// taşır, orada tur sorusu anlamsızdır). Muafiyet niyet okumak değildir — alanın
/// ifadede geçmesi tasarımcının tur ayrımını bildiğinin OLGUSAL kanıtıdır.
/// Liste TEK elemanlıdır; büyütmek KARAR ister.
const RAW_LIST_EXEMPT_FIELD: &str = "branch_round";

/// Bu düğüm ham `$wfah` kökünün KENDİSİ mi (indekslenmemiş, alanı alınmamış hâli).
fn is_raw_wfah(node: &Node<'_>) -> bool {
    match node {
        Node::Parenthesized(inner) => is_raw_wfah(inner),
        Node::Identifier(name) => *name == "$wfah",
        _ => false,
    }
}

/// Bir düğümün DOĞRUDAN çocukları. `zen_expression`ın kendi `Node::walk`ı yerine
/// burada duruyor çünkü `A02` bir dalı ATLAMAK zorunda (`$wfah[...]`) ve `walk`
/// budama sunmuyor.
fn child_nodes<'a>(node: &'a Node<'a>) -> Vec<&'a Node<'a>> {
    match node {
        Node::TemplateString(parts) | Node::Array(parts) => parts.to_vec(),
        Node::Object(pairs) => pairs.iter().flat_map(|(k, v)| [*k, *v]).collect(),
        Node::Assignments { list, output } => list
            .iter()
            .flat_map(|(k, v)| [*k, *v])
            .chain(output.iter().copied())
            .collect(),
        Node::Closure { body, .. } => vec![*body],
        Node::Parenthesized(inner) => vec![*inner],
        Node::Member { node, property } => vec![*node, *property],
        Node::Slice { node, from, to } => std::iter::once(*node)
            .chain(from.iter().copied())
            .chain(to.iter().copied())
            .collect(),
        Node::Interval { left, right, .. } => vec![*left, *right],
        Node::Conditional {
            condition,
            on_true,
            on_false,
        } => vec![*condition, *on_true, *on_false],
        Node::Unary { node, .. } => vec![*node],
        Node::Binary { left, right, .. } => vec![*left, *right],
        Node::FunctionCall { arguments, .. } => arguments.to_vec(),
        Node::MethodCall { this, arguments, .. } => std::iter::once(*this)
            .chain(arguments.iter().copied())
            .collect(),
        Node::Error { node, .. } => node.iter().copied().collect(),
        _ => Vec::new(),
    }
}

/// Yordam (`#.` yaprakları) `branch_round` alanına değiyor mu — `A02`/S4 muafiyeti.
/// Yalnız `#` kökü sayılır: `$prev.branch_round` farklı bir sorudur (uçlar ham
/// listeye bakar, `R04`).
fn touches_branch_round(node: &Node<'_>) -> bool {
    if let Node::Member {
        node: base,
        property: Node::String(name),
    } = node
    {
        if *name == RAW_LIST_EXEMPT_FIELD && matches!(base, Node::Pointer | Node::Identifier("#")) {
            return true;
        }
    }
    child_nodes(node).into_iter().any(touches_branch_round)
}

/// `A02` — ham `$wfah` bir niceleme/toplama fonksiyonunun LİSTE argümanı mı.
///
/// `$wfah[...]` DALI ATLANIR: orayı `wfah_index_unguarded` zaten konuşuyor ve
/// `$wfah[len($wfah) - 1]` iki not almaz — indeksin içindeki `len($wfah)` bir
/// eşik sayımı değil, indeks aritmetiğidir.
fn scans_raw_wfah_list(node: &Node<'_>) -> bool {
    match node {
        Node::Member { node: base, .. } | Node::Slice { node: base, .. } if is_raw_wfah(base) => {
            false
        }
        Node::FunctionCall { kind, arguments } => {
            let fname = kind.to_string();
            let hit = RAW_LIST_FNS.contains(&fname.as_str())
                && arguments.first().is_some_and(|a| is_raw_wfah(a))
                && !arguments[1..].iter().any(|a| touches_branch_round(a));
            hit || arguments.iter().any(|a| scans_raw_wfah_list(a))
        }
        other => child_nodes(other).into_iter().any(scans_raw_wfah_list),
    }
}

/// `A02` — ifadenin AST'si üzerinden ham liste taraması. Metin taraması BİLEREK
/// yazılmadı: boşluk, satır sonu ve iç içe `filter(...)` sarmalı metin aramasını
/// kırar, kapsam kararı zaten "fonksiyonun liste argümanı" olduğu için ağaç ister.
fn scans_raw_wfah_list_expr(expr: &str) -> bool {
    let bump = Bump::new();
    let source = bump.alloc_str(expr);
    let mut lexer = Lexer::new();
    let Ok(tokens) = lexer.tokenize(source) else {
        return false;
    };
    let Ok(parser) = Parser::try_new(tokens, &bump) else {
        return false;
    };
    let result = parser.standard().parse();
    if result.error().is_err() {
        return false;
    }
    scans_raw_wfah_list(result.root)
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
             Son/ilk AKSİYON girdisi için $prev / $first kullan."
                .to_string(),
        ));
    }
    // `$env` referans biçimi — editör ve WFD validator'ı AYNI kaynaktan beslensin diye
    // burada. Bozuk referans runtime'da düz metin olarak dışarı sızar.
    if let Err(e) = env::references(expr) {
        out.push(("env_reference_malformed", true, e.to_string()));
    }
    if let Some(root) = indexes_wfah_directly(expr) {
        let mut msg = format!(
            "{root} doğrudan indeksleniyor — geçmiş o kadar uzun değilse ifade \
             runtime'da patlar (boş geçmişte kesin patlar). $prev (son AKSİYON girdisi) / \
             $first (ilk AKSİYON girdisi) bu durumda null döner. DİKKAT: R04'ten beri \
             $wfah[len($wfah)-1] ile $prev AYNI SATIRI VERMEZ — elle indeksleme marker \
             satırını görür, $prev görmez."
        );
        if root == "$valid" {
            // A3: elenmiş listede `seq` boşluklu — indeks aritmetiği ham listede
            // işlese bile burada başka satırı gösterir.
            msg.push_str(
                " $valid'de seq BOŞLUKLUDUR: len($valid) son seq'e eşit DEĞİLDİR, \
                 dolayısıyla indeksleme ham listeden DAHA tehlikelidir.",
            );
        }
        out.push(("wfah_index_unguarded", false, msg));
    }
    // `A02` — ham `$wfah` üzerindeki niceleme/toplamalara OLGU notu. Yayını
    // ENGELLEMEZ (`wfah_index_unguarded` emsali birebir). Çerçeve OLGUDUR: metin
    // "bunu mu demek istediniz" DEMEZ, ifadenin ŞU ANDA ne yaptığını söyler ve
    // alternatifini gösterir — motor niyet okumaz, olgu bildirir. İfade başına TEK
    // not düşer (`$wfah` kaç kez geçerse geçsin).
    if scans_raw_wfah_list_expr(expr) {
        out.push((
            "wfah_raw_list",
            false,
            "$wfah TÜM GEÇMİŞ üzerinde koşuyor: iptal olmuş kolun satırları, geri gönderme \
             penceresindeki satırlar, sahiplik satırları ve önceki turların satırları da \
             hesaba girer. Yalnız geçerli satırlar için $valid kullan."
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
        for (target, raw) in effects.all_writes() {
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

/// `#.action` TAM-KİMLİK literallerini çıkarır — `zen_action_unknown`ın girdisi.
///
/// Kayıtlı sınır (`E08` Faz 3): yalnız kimliğin BÜTÜNÜ ile karşılaştıran formlar
/// okunur. `contains(#.action, "escalate:")` gibi PARÇA karşılaştırmaları atlanır —
/// orada aranan şey bir kimlik değil bir metin parçasıdır ve kataloğa uyması
/// gerekmez.
///
/// Okunan formlar: `#.action == "X"`, `#.action != "X"`, `#.action in ["A", "B"]`.
///
/// ⚠️ `!=` kayıtta ADIYLA sayılmıyor (kayıt `==` ve `in`i sayıyor), ama kaydın
/// çizdiği sınır "tam kimlik / parça" ayrımıdır ve `!=` tam kimlik tarafındadır.
/// Bir yazım hatası `!=` içinde de aynı şekilde sessizdir (koşul hep true döner),
/// o yüzden dahil edildi. Ayna sırası (`"X" == #.action`) BİLİNÇLE okunmuyor:
/// kayıtta yok, ZEN'de deyimsel değil, ve geriye doğru tarama kuralı kırılgan yapar.
fn action_literals_in(expr: &str) -> Vec<String> {
    /// `pos`taki tırnaklı diziyi okur (kaçış işlemez — ZEN aksiyon adlarında `\\` yok).
    fn quoted(b: &[u8], pos: usize) -> Option<(String, usize)> {
        let q = *b.get(pos)?;
        if q != b'"' && q != b'\'' {
            return None;
        }
        let start = pos + 1;
        let mut end = start;
        while end < b.len() && b[end] != q {
            end += 1;
        }
        if end >= b.len() {
            return None;
        }
        Some((
            String::from_utf8_lossy(&b[start..end]).into_owned(),
            end + 1,
        ))
    }
    let b = expr.as_bytes();
    let needle = "#.action";
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(rel) = expr[i..].find(needle) {
        let mut j = i + rel + needle.len();
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        if expr[j..].starts_with("==") || expr[j..].starts_with("!=") {
            let mut k = j + 2;
            while k < b.len() && b[k].is_ascii_whitespace() {
                k += 1;
            }
            if let Some((lit, _)) = quoted(b, k) {
                out.push(lit);
            }
        } else if expr[j..].starts_with("in") {
            let mut k = j + 2;
            while k < b.len() && b[k].is_ascii_whitespace() {
                k += 1;
            }
            if b.get(k) == Some(&b'[') {
                k += 1;
                // Liste kapanana kadar her tırnaklı diziyi topla.
                while k < b.len() && b[k] != b']' {
                    match quoted(b, k) {
                        Some((lit, next)) => {
                            out.push(lit);
                            k = next;
                        }
                        None => k += 1,
                    }
                }
            }
        }
        i = i + rel + 1;
    }
    out
}

/// `E08` Faz 3 — `zen_action_unknown`. Bir `#.action` literali YA `actions{}`'te
/// çözülecek YA motorun kapalı marker gramerine uyacak; marker kalıbı bir node
/// anahtarı taşıyorsa o anahtar da `nodes{}`'te çözülecek.
///
/// Kalıp listesi `v22::wfah_kind::parse_marker`dan gelir — bu kuralın kendi listesi
/// YOKTUR. Motora yeni bir marker eklendiğinde kural onu kendiliğinden tanır.
///
/// Neden HATA: marker adları yayınlanmış akışların SAYIM sözleşmesidir. Yanlış
/// yazılmış bir ad hiçbir satırla eşleşmez, `count(...)` sessizce hep 0 döner ve
/// koşul yanlış dalı seçer — yayın sonrası, tasarımcı hiç uyarılmadan.
fn check_action_literals(wfd: &Wfd, expr: &str, path: &str, report: &mut ValidationReport) {
    for lit in action_literals_in(expr) {
        if wfd.actions.contains_key(&lit) {
            continue;
        }
        let parsed = parse_marker(&lit);
        if let Some(call_key) = &parsed.from_call {
            // `call:<key>/<action>` — İÇ kimlik BAŞKA belgeye aittir ve bu belgeden
            // çözülemez. Yerel olan tek parça çağrı anahtarıdır; yalnız o denetlenir.
            if !wfd.calls.contains_key(call_key) {
                report.error(
                    "zen_action_unknown",
                    path.to_string(),
                    format!(
                        "'{lit}' içindeki '{call_key}' çağrı anahtarı `calls{{}}`'te tanımlı değil"
                    ),
                );
            }
            continue;
        }
        if parsed.kind == WfahKind::Action {
            report.error(
                "zen_action_unknown",
                path.to_string(),
                format!(
                    "#.action == '{lit}' — bu ad ne `actions{{}}`'te tanımlı ne de motorun \
                     marker kalıplarından birine uyuyor. Sayım hiçbir satırla eşleşmez"
                ),
            );
            continue;
        }
        if let Some(node) = &parsed.node {
            if !wfd.nodes.contains_key(node) {
                report.error(
                    "zen_action_unknown",
                    path.to_string(),
                    format!(
                        "'{lit}' marker kalıbı doğru ama içindeki '{node}' node'u \
                         `nodes{{}}`'te tanımlı değil"
                    ),
                );
            }
        }
    }
}

fn check_expressions(wfd: &Wfd, report: &mut ValidationReport) {
    let env = expr_env(wfd);
    let check = |expr: &str, path: String, report: &mut ValidationReport| {
        // `E08` Faz 3: `#.action` literalleri katalog ∪ marker grameriyle çözülür.
        // Ayrı bir kapı çünkü BELGEYİ görmesi gerekiyor — `expression_issues` saf bir
        // metin fonksiyonudur ve `/wfd/validate-expression` ucundan WFD'siz de çağrılır.
        check_action_literals(wfd, expr, &path, report);
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

    for (action_key, t) in &wfd.actions {
        let path = format!("actions.{action_key}");
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
    // v2.3: start'ın `when`/`trigger`/`wft`i aksiyon kaydında — yukarıdaki `actions`
    // döngüsü kapsıyor. Escalation ise `wft` taşımıyor (`Ç9`).
    for (key, node) in &wfd.nodes {
        // `E13`: escalation grant'ının `when`i — `grant_when_actor_ref`in BEŞİNCİ
        // çağrı yeri. `listable` yolundaki SIRANIN aynısı: önce tip denetimi (`check`),
        // sonra iki namespace kapısı.
        for (i, esc) in node.escalation.iter().enumerate() {
            if let Some(when) = &esc.grant.when {
                let path = format!("nodes[{key}].escalation[{i}].grant.when");
                check(when, path.clone(), report);
                grant_when_actor_ref(when, path.clone(), report);
                grant_when_namespace(when, path, report);
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
            grant_when_actor_ref(when, format!("listable[{i}].when"), report);
            grant_when_namespace(when, format!("listable[{i}].when"), report);
        }
    }
    // T‑A5: `wf_admin[]` kuralları `listable` ile aynı şekli taşır (`CaGrantRule`) ve
    // aynı guard denetimine girer. `c_a` şekli ise `check_c_a_shape`'in genel
    // toplayıcısına (`collect_key_sites(doc, "c_a")`) kendiliğinden dahildir.
    for (i, a) in wfd.wf_admin.iter().enumerate() {
        if let Some(when) = &a.grant.when {
            check(when, format!("wf_admin[{i}].when"), report);
            grant_when_actor_ref(when, format!("wf_admin[{i}].when"), report);
            grant_when_namespace(when, format!("wf_admin[{i}].when"), report);
        }
        // A-1 (2026-08-21): `allowed_global_actions` boş = admin YALNIZ görür. Hata
        // DEĞİL (gözlemci admin meşru bir yapılandırmadır) ama tasarımcının niyeti
        // "yetki vermek" olup listeyi yazmayı unutması en olası hâl — sessiz kalmak
        // yayın sonrası "admin düğmeleri neden yok" sorusuna dönüşür.
        if a.allowed_global_actions.is_empty() {
            report.warn(
                "wf_admin_no_global_actions",
                format!("wf_admin[{i}]"),
                "kural hiçbir global aksiyon vermiyor: admin bu akışı yalnız GÖRÜR. \
                 Müdahale gerekiyorsa `allowed_global_actions` yazılmalı."
                    .into(),
            );
        }
        // Aynı aksiyonu iki kez yazmak sessizce yutulurdu (küme birleşimi) — belge
        // hatası olarak söylenir, `duplicate_c_a` deseninin aynısı.
        let mut seen = BTreeSet::new();
        for act in &a.allowed_global_actions {
            if !seen.insert(*act) {
                report.error(
                    "wf_admin_duplicate_global_action",
                    format!("wf_admin[{i}].allowed_global_actions"),
                    format!("global aksiyon iki kez yazılmış: '{}'", act.as_str()),
                );
            }
        }
    }
    // 2026-08-13: `nodes.<key>.listable[]` kök `listable`/`wf_admin` ile AYNI şekli
    // (`CaGrantRule`) ve AYNI matcher'ı (`matches_grant_rules`) paylaşır — `when` guard'ı
    // da aynı denetimden geçmeli (expr_types tip denetimi + `$actor` yasağı). `c_a` şekli
    // ve `c_orgu` anchor denetimi zaten doc-geniş toplayıcılardan geçiyor (`check_c_a_shape`,
    // `check_c_orgu_anchor_kinds`: `serde_json::to_value(wfd)` üzerinden yürüyorlar, yeni
    // alan otomatik dahil olur).
    for (key, node) in &wfd.nodes {
        for (i, l) in node.listable.iter().enumerate() {
            if let Some(when) = &l.when {
                let path = format!("nodes[{key}].listable[{i}].when");
                check(when, path.clone(), report);
                grant_when_actor_ref(when, path.clone(), report);
                grant_when_namespace(when, path, report);
            }
        }
    }
    // 2026-08-17: `terminals[].listable[]` — yine AYNI şekil, AYNI matcher, AYNI guard
    // denetimi. `$node` bu guard'da `None`'dır (terminal'de `current_node` yoktur), ama
    // bu bir yasak değil: `$node == "x"` yazan kural yalnız hiç eşleşmez ve `$actor`
    // yasağı burada da geçerlidir — projeksiyon viewer bilinmezken yazılır.
    for t in &wfd.terminals {
        for (i, l) in t.listable.iter().enumerate() {
            if let Some(when) = &l.when {
                let path = format!("terminals[{}].listable[{i}].when", t.id);
                check(when, path.clone(), report);
                grant_when_actor_ref(when, path.clone(), report);
                grant_when_namespace(when, path, report);
            }
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

/// Aynı yol HEM `input.required` HEM `input.optional` listesinde olamaz.
///
/// İki liste ÇELİŞİR: `required` "gönderilmek zorunda ve null olamaz" (pipeline
/// `validate_action_input` reddeder), `optional` "gönderilmeyebilir, gönderilmezse ctx'e
/// `null` yazılır" (WOR-70b) demektir. Aynı yol için ikisi birden bildirildiğinde motor
/// `required` gibi davranır ve `optional` bildirimi ölü kalır — tasarımcı ise alanı
/// atlanabilir sanır. Şema bunu YAKALAYAMAZ: `uniqueItems` her diziye tek tek bakar,
/// JSON Schema iki dizinin ayrık olmasını ifade edemez. Bu yüzden kural validator'da.
///
/// Ata/torun bildirimi (`user` + `user.boy`) HATA DEĞİLDİR ve burada aranmaz: pipeline
/// sözleşmesi null denetiminin yalnız bildirilen yola baktığını, iç alanın da dolu olması
/// istenirse ayrıca `required`'a yazılacağını söyler (`validate_action_input` doc'u).
fn check_input_required_optional_overlap(wfd: &Wfd, report: &mut ValidationReport) {
    for (name, action) in &wfd.actions {
        let required: BTreeSet<&String> = action.input.required.iter().collect();
        for path in &action.input.optional {
            if required.contains(path) {
                report.error(
                    "input_required_and_optional",
                    format!("actions[{name}].input.optional"),
                    format!(
                        "input yolu '{path}' hem `required` hem `optional` listesinde — \
                         ikisi çelişir (`required` null olamaz, `optional` gönderilmezse \
                         null yazılır). Yolu tek listede bırakın."
                    ),
                );
            }
        }
    }
}

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

// ---- Adlandırılmış tip (`format` → `$defs`) — 2026-08-19 ----
//
// Bir alan tipini `$defs`'teki bir tanıma ADLA bağlayabilir:
//     "$defs":      { "Tarih": { "type": "string", "pattern": "^[0-9]{14}$" } }
//     "properties": { "basvuru_tarihi": { "format": "Tarih" } }
//
// `format` bu belgede STANDART JSON Schema anlamında DEĞİL, `#/$defs/<Ad>` kısayolu
// olarak okunur. Sebep: standart `format` yalnız bir İSİMDİR, kuralı doğrulayıcı
// kütüphanenin format tablosunda durur — motorun sözleşmesini crate sürümüne bağlamak
// olurdu. `$defs` tanımı kuralı BELGEDE taşır ve motor onu çalışma anında da uygular
// (`v22::ctx_types`).
//
// Kurallar:
//   `context_format_unknown`   — `format` değeri `$defs`'te TANIMLI olmalı. Yani
//                                `format: "date-time"` "standart format" demek değil,
//                                "benim `$defs.date-time` tanımım" demektir.
//   `context_format_with_type` — `format` yanında TİP kuralı olamaz (`type`, `enum`,
//                                `pattern`, sınırlar, `items`, `properties`,
//                                `x-wf-kind`): tip tanımın içindedir. İzinli kardeşler
//                                yalnız anlatım/görünürlük (`title`, `description`,
//                                `x-visibility`).
//   `context_format_cycle`     — tanım zinciri döngü yapamaz (A → B → A).
//   `context_defs_name`        — tanım adı biçimi (`^[A-Za-z][A-Za-z0-9_]*$`).
//   `context_ref_removed`      — `$ref` YAZILAMAZ. Okuma tarafı onu ÇÖZMEYE devam eder
//                                (`ctx_types::field_schema`) çünkü yayınlanmış belge
//                                yeniden yazılamaz; kapı yalnız YAZMA yollarındadır
//                                (upload/publish/submit/approve — `fetch` bu kapıdan
//                                geçmez, yalnız şemaya bakar).

/// `$defs` tanım adı biçimi — editördeki `DEF_NAME_RE` ile aynı; tek kaynak MOTOR.
fn is_valid_def_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `format` yanında bulunması YASAK olan (tip taşıyan) anahtarlar.
const TYPE_KEYWORDS: [&str; 18] = [
    "type",
    "enum",
    "const",
    "properties",
    "items",
    "pattern",
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "multipleOf",
    "minLength",
    "maxLength",
    "minItems",
    "maxItems",
    "uniqueItems",
    "minProperties",
    "x-wf-kind",
];

fn check_context_named_types(wfd: &Wfd, report: &mut ValidationReport) {
    let defs = wfd.context.get("$defs").and_then(Value::as_object);

    // Tanım adları.
    if let Some(defs) = defs {
        for name in defs.keys() {
            if !is_valid_def_name(name) {
                report.error(
                    "context_defs_name",
                    format!("context.$defs.{name}"),
                    format!(
                        "'{name}' geçersiz tip adı — harfle başlamalı, yalnız harf/rakam/alt                          çizgi içerebilir (motor ve editör aynı kuralı uygular)"
                    ),
                );
            }
        }
    }

    // Kullanım yerleri: context ağacının her düğümü (tanımların İÇİ dahil).
    let mut nodes: Vec<(String, &Value)> = Vec::new();
    collect_schema_nodes(&wfd.context, "context", &mut nodes);
    if let Some(defs) = defs {
        for (name, def) in defs {
            collect_schema_nodes(def, &format!("context.$defs.{name}"), &mut nodes);
        }
    }

    for (path, node) in nodes {
        let Some(map) = node.as_object() else {
            continue;
        };

        if map.contains_key("$ref") {
            report.error(
                "context_ref_removed",
                path.clone(),
                "`$ref` artık YAZILAMAZ — adlandırılmış tip `\"format\": \"<Ad>\"` ile                  verilir (aynı `$defs` tanımına işaret eder). Yayınlanmış belgelerdeki                  `$ref` okunmaya devam eder; bu kapı yalnız yeni yazımlar içindir."
                    .into(),
            );
        }

        let Some(format_name) = map.get("format").and_then(Value::as_str) else {
            continue;
        };

        if defs.is_none_or(|d| !d.contains_key(format_name)) {
            report.error(
                "context_format_unknown",
                path.clone(),
                format!(
                    "'{format_name}' tipi `context.$defs` içinde tanımlı değil. `format` bu                      belgede standart JSON Schema formatı DEĞİL, `$defs`'teki bir tipin                      adıdır — tipi `$defs.{format_name}` olarak tanımlayın (ör.                      {{\"type\": \"string\", \"pattern\": \"…\"}})."
                ),
            );
        }

        let clashing: Vec<&str> = TYPE_KEYWORDS
            .iter()
            .copied()
            .filter(|k| map.contains_key(*k))
            .collect();
        if !clashing.is_empty() {
            report.error(
                "context_format_with_type",
                path.clone(),
                format!(
                    "`format: \"{format_name}\"` yanında tip kuralı olamaz ({}) — tip                      tanımın İÇİNDEDİR. Bu alana özel bir kural gerekiyorsa ayrı bir                      `$defs` tipi tanımlayın; yalnız `title`/`description`/`x-visibility`                      kullanım yerinde ezilebilir.",
                    clashing.join(", ")
                ),
            );
        }
    }

    // Döngü: tanım → tanım zinciri.
    if let Some(defs) = defs {
        for name in defs.keys() {
            let mut seen: Vec<&str> = Vec::new();
            let mut current: &str = name;
            loop {
                if seen.contains(&current) {
                    report.error(
                        "context_format_cycle",
                        format!("context.$defs.{name}"),
                        format!("tip tanımı döngüsü: {} → {current}", seen.join(" → ")),
                    );
                    break;
                }
                seen.push(current);
                let Some(next) = defs
                    .get(current)
                    .and_then(|d| d.get("format"))
                    .and_then(Value::as_str)
                else {
                    break;
                };
                current = next;
            }
        }
    }
}

/// Context şema ağacındaki TÜM düğümleri toplar (`properties` ve `items` içleri dahil).
/// `$defs`in kendisi ATLANIR — çağıran onları ayrıca gezer (yol adları farklı olsun).
fn collect_schema_nodes<'a>(node: &'a Value, path: &str, out: &mut Vec<(String, &'a Value)>) {
    out.push((path.to_string(), node));
    if let Some(props) = node.get("properties").and_then(Value::as_object) {
        for (name, sub) in props {
            collect_schema_nodes(sub, &format!("{path}.properties.{name}"), out);
        }
    }
    match node.get("items") {
        Some(Value::Object(_)) => {
            collect_schema_nodes(node.get("items").unwrap(), &format!("{path}.items"), out)
        }
        Some(Value::Array(arr)) => {
            for (i, sub) in arr.iter().enumerate() {
                collect_schema_nodes(sub, &format!("{path}.items[{i}]"), out);
            }
        }
        _ => {}
    }
}

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

    // v2.3: start'ın effects'i ve trigger catch'leri aksiyon kaydında — aşağıdaki
    // `actions` döngüsü ikisini de topluyor. Ayrı start döngüsü aynı effect'i İKİ KEZ
    // listeler ve `context_field_never_written` gibi sayım kuralları bozulurdu.
    // ⚠️ `E09` (`set_when`) bu fonksiyonun TÜKETİCİLERİNİ genişletiyor; burada yalnız
    // iterasyon yüzeyi `transitions` → `actions`a taşındı. İki iş aynı gövdede kesişir.
    for (action_key, t) in &wfd.actions {
        let path = format!("actions.{action_key}");
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
        .flat_map(|(_, e)| e.all_writes().map(|(k, _)| k.clone()).collect::<Vec<_>>())
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
            // WOR-70: aksiyon girdisi yolları CONTEXT yollarıdır — `check_action_inputs`
            // her bildirilen `input.required/optional` yolunu context şemasında arar
            // (`input_path`). Dolayısıyla `$action.input.<yol>`un tipi de aynı şemadan
            // okunur. Bu olmadan girdi kaynaklı yazımlar tip denetiminin DIŞINDA kalıyordu:
            // `"user.yas": "$action.input.user"` (number alana TÜM obje) sessizce geçiyordu.
            // Şemada olmayan/çözülemeyen yol → None, yani kıyas yapılmaz (eski davranış).
            None => match s.strip_prefix("$action.input.") {
                Some(path) => schema_type_at(context, path),
                // Tanınmayan `$...` referansı tipsizdir; `$` ile başlamayan her şey metin sabiti.
                None if s.starts_with('$') => None,
                None => Some("string".into()),
            },
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
        for (target, raw) in effects.all_writes() {
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
        for (target, raw) in effects.all_writes() {
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
            check_dollar_value(
                raw,
                &format!("terminals[{}].wfe_end_response[{k}]", t.id),
                report,
            );
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
    for (_, raw) in effects.all_writes() {
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

    // v2.3 (`Ç7+Ç8` hükmü): `unused_action_input`in START DÖNGÜSÜ **SİLİNDİ**. Start
    // gövdesi aksiyon kaydına indiği için start aksiyonu da aşağıdaki `actions`
    // döngüsünden geçer; ayrı döngü aynı aksiyonu iki kez kaydeder ve "bildirilen ama
    // tüketilmeyen girdi" sayımını bozardı.
    for (action_key, t) in &wfd.actions {
        rules.push((
            format!("actions.{action_key}"),
            action_key,
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

    // v2.3 (`Ç7+Ç8`): start için AYRI döngü GEREKMEZ — gövde (`wfes_effects`,
    // `trigger`) aksiyon kaydına indi ve aşağıdaki `wfd.actions` döngüsü onu kapsıyor.
    // İkinci bir döngü aynı effect'i iki kez sayar; `unused_action_input` için `Ç7+Ç8`
    // bu silmeyi ADIYLA emrediyor.
    for (action_key, t) in &wfd.actions {
        if let Some(e) = &t.wfes_effects {
            for (path, _) in e.all_writes() {
                // Site etiketi node'u da taşır. v2.3'te `from` TEK string (`K3`), yani
                // eskiden gereken sıralı birleştirme de düştü.
                let site = format!("'{action_key}' aksiyonu ({})", t.from);
                push_rule(
                    path,
                    action_key,
                    e,
                    site,
                    Some((action_key.clone(), vec![t.from.clone()])),
                    &mut writers,
                );
            }
        }
        for trig in &t.trigger {
            if let Some(c) = &trig.catch {
                for (path, _) in c.wfes_effects.all_writes() {
                    writers.push(EffectWriter {
                        path: path.clone(),
                        site: format!("actions.{action_key} catch"),
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
                for (path, _) in e.all_writes() {
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
                for (path, _) in e.all_writes() {
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
            for (path, _) in e.all_writes() {
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
            for (path, _) in e.all_writes() {
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
    // Katalog: item.id BÜTÜN belgede tekil olmalı — iki AYRI grupta bile tekrar edemez
    // (E10/e). `items` bir DİZİ olduğu için `dupkeys` kapısı buraya BAKAMAZ: ayrıştırıcı
    // iki girdiyi de tutar, tekilliği yalnız validator görebilir. Katman farkı bilinçli.
    //
    // Neden belge geneli: item id'si storage anahtarına giriyor
    // (`attachments/{wfe_id}/{grup}/{item}`) ve iki grupta aynı id, aynı slotu iki farklı
    // kapının arkasına koyar — hangi kapının o dosyayı istediği belirsizleşir.
    let mut item_owner: HashMap<&str, &str> = HashMap::new();
    for (group, def) in &wfd.attachments {
        for item in &def.items {
            match item_owner.insert(item.id.as_str(), group.as_str()) {
                None => {}
                Some(first) if first == group.as_str() => report.error(
                    "attachment_item_dup",
                    format!("attachments[{group}].items"),
                    format!(
                        "attachment item id '{}' '{group}' grubunda birden fazla tanımlı",
                        item.id
                    ),
                ),
                Some(first) => report.error(
                    "attachment_item_dup",
                    format!("attachments[{group}].items"),
                    format!(
                        "attachment item id '{}' iki ayrı grupta tanımlı: '{first}' ve \
                         '{group}' — item id'si belge genelinde tekil olmalıdır",
                        item.id
                    ),
                ),
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
            let Some(scoped) = aref.actions() else {
                continue;
            };
            let mut seen_actions = HashSet::new();
            for action in scoped {
                if !seen_actions.insert(action.clone()) {
                    report.error(
                        "attachment_action_dup",
                        format!("nodes[{node_key}].attachments[{group_ref}].actions"),
                        format!("aksiyon '{action}' bu kapsamda birden fazla sayılmış"),
                    );
                }
                // v2.3: aksiyon kaydı `from`u kendisi taşır, yani "bu aksiyon bu
                // node'dan alınabilir mi" tek bakışta sorulur. Start aksiyonu da aynı
                // kayıttan geçer (`start[]` yalnız `{id, action}`) — ayrı bir start
                // döngüsü GEREKMEZ, `Ç7+Ç8` o gövdeyi aksiyona indirdi.
                let reachable = wfd
                    .actions
                    .get(action.as_str())
                    .is_some_and(|a| a.from == *node_key);
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
        for (key, _) in effects.all_writes() {
            if let PathResolution::Missing = resolve_schema_path(&wfd.context, key) {
                report.error(
                    "effect_path",
                    path.to_string(),
                    format!("effect yolu '{key}' context şemasında yok"),
                );
            }
        }
    };

    // v2.3 (`Ç7+Ç8`): start için AYRI döngü GEREKMEZ — gövde (`wfes_effects`,
    // `trigger`) aksiyon kaydına indi ve aşağıdaki `wfd.actions` döngüsü onu kapsıyor.
    // İkinci bir döngü aynı effect'i iki kez sayar; `unused_action_input` için `Ç7+Ç8`
    // bu silmeyi ADIYLA emrediyor.
    for (action_key, t) in &wfd.actions {
        check_effects(&t.wfes_effects, &format!("actions.{action_key}"), report);
        for (j, trig) in t.trigger.iter().enumerate() {
            if let Some(c) = &trig.catch {
                check_effects(
                    &Some(c.wfes_effects.clone()),
                    &format!("actions.{action_key}.trigger[{j}].catch"),
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

    // v2.3 (`Ç7+Ç8`): start için AYRI döngü GEREKMEZ — gövde (`wfes_effects`,
    // `trigger`) aksiyon kaydına indi ve aşağıdaki `wfd.actions` döngüsü onu kapsıyor.
    // İkinci bir döngü aynı effect'i iki kez sayar; `unused_action_input` için `Ç7+Ç8`
    // bu silmeyi ADIYLA emrediyor.
    for (action_key, t) in &wfd.actions {
        check_triggers(&t.trigger, &format!("actions.{action_key}"), report);
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

/// SLA bağlamında `$action.input.*`, `$exec.result.*` ve `$call.*` YOKTUR (tetikleyici
/// system aktörü; ne aksiyon girdisi, ne autoexec sonucu, ne de bir çağrı dönüşü vardır)
/// — sessizce `null` yazmak yerine WFD reddedilir. `$ctx.*`, `$actor`, `$node`,
/// `$timestamp`, `$wfe_id`, `$env.*` geçerli.
fn check_sla_effect_namespaces(effects: &WfesEffects, path: &str, report: &mut ValidationReport) {
    for (target, raw) in effects.all_writes() {
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

// v2.3 (`Ç9` + `K13`): `wft_form_name` ve `parallel_interior_nodes` SİLİNDİ.
// İkisi de yalnız C ekseninin ölen kuralları tarafından kullanılıyordu —
// SLA hedef FORMU hatası (`escalation`/`claim_timeout` artık `wft` taşımıyor) ve
// `*_collapse_outside_parallel` (collapse'lı SLA yolu kalktı). Çağıranları gidince
// öksüz kaldılar; derleyici `never used` diye işaret etti.

fn check_sla(wfd: &Wfd, report: &mut ValidationReport) {
    // ⚠️ v2.3 (`E08` Faz 1) — BU FONKSİYONUN KALKANLARININ ÇOĞU ÖLDÜ.
    //
    // Silinen kurallar ve DAYANAKLARININ neden yok olduğu:
    //
    // | kural | neden öldü |
    // |---|---|
    // | `escalation_terminate_removed` | `EscalationStep.terminate` ALANI da silindi (`E08`/S2). Alan 2026-07-28'den beri ölüydü ve yalnız eski belgeye güzel hata vermek için deserialize ediliyordu; Değişmez #9 gereği o okuyucu saf borçtu. Bedeli: eski belge artık ham `deny_unknown_fields` hatası alır |
    // | `escalation_wft_required` | `esc.wft` YOK — yerine `grant` geldi ve **zorunlu alan** olduğu için eksikliği serde'de patlar (`Ç9`) |
    // | `sla_target_not_node` · `sla_terminal_target` (İKİ kullanımı) | escalation'ın HEDEFİ yok; "hedef node olmalı" diye bir kural kalmadı. Ad tamamen ölür (`E08`/S3) |
    // | `escalation_collapse_outside_parallel` | escalation `Wft::Collapse` taşıyamaz — `wft` yok |
    // | `claim_timeout_collapse_requires_wft` · `claim_timeout_collapse_outside_parallel` | `ClaimTimeout`tan `wft` ve `collapses_parallel` KALKTI (K13 + K19): claim timeout artık YALNIZ claim'i bırakır, iş taşımaz |
    //
    // `claim_timeout_collapse_no_parallel` kuralı motorda **HİÇ YAZILMAMIŞTI** — yalnız
    // `docs/spec/decisions.md`'de duruyordu; oradaki satır ayrı iş (Aşama 5).
    //
    // GERİYE KALAN: `claim_timeout` denetimi yalnız `after` süre biçimi + `wfes_effects`
    // namespace kontrolüne iner. Escalation tarafında `grant`ın tasarım zamanı kuralları
    // (`escalation_grant_noop` + `grant_when_namespace`) AYRI issue'nun işidir (`E13`).
    for (key, node) in &wfd.nodes {
        for (j, esc) in node.escalation.iter().enumerate() {
            let path = format!("nodes[{key}].escalation[{j}]");
            // ⚠️ `esc.after` süre biçimi v2.2'de de DENETLENMİYORDU ve bu Faz 1'de
            // DEĞİŞMEZ: yeni bir kural eklemek fazın "davranış korunur" şartını bozar.
            // Eksikliği ayrı bir kalem (bkz. `claim_timeout` tarafındaki emsal).
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

/// Yol çözümü — adlandırılmış tip (`format`/`$ref`) `v22::ctx_types` ile çözülür,
/// yani `$defs` arkasındaki alan artık `Opaque` değil `Found`dur ve girdi yolu
/// denetimi (`input_path`) onu görebilir.
fn resolve_schema_path(context: &Value, dotted: &str) -> PathResolution {
    match crate::v22::ctx_types::field_schema(context, dotted) {
        crate::v22::ctx_types::Resolved::Found(_) => PathResolution::Found,
        crate::v22::ctx_types::Resolved::Missing => PathResolution::Missing,
        crate::v22::ctx_types::Resolved::Opaque => PathResolution::Opaque,
    }
}
