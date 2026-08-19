//! Ek-belge KURALLARI — dosya I/O'suz, saf. Engine core hâlâ dosyaya değmez:
//! burada yalnız "hangi grup hangi aksiyonu kapar", "zorunlu slotlardan hangisi
//! eksik" ve "bu tip/boyut kabul edilir mi" soruları yaşar.
//!
//! NEDEN wfe-core: bu kuralları İKİ tüketici uygular — `wf_server::attachments`
//! (gerçek akış; `uploaded` bilgisi depodan gelir) ve `wf_wfe::sim` (simülasyon/
//! senaryo; `uploaded` bilgisi `SimState`ten gelir). İki yerde yaşasalardı
//! simülasyonda geçen bir senaryo portalda kalabilirdi — `check_expectations`ın
//! motora taşınmasıyla aynı gerekçe (bkz. `wf_wfe::scenario`).
//! `wf_server::attachments` bu fonksiyonları `pub use` ile yeniden ihraç eder;
//! oradaki çağrı yerleri değişmedi.
//!
//! Burada OLMAYAN: depo erişimi, DB metadata'sı, magic-byte sniff
//! (`wf_server::attachments::sniff_content_type` — baytlar route katmanında).

use crate::types::wfd_v22::{AttachmentFormatRule, AttachmentItem, Wfd};

// ---- format / boyut kuralı ----

/// Basit MIME eşleşmesi: tam eşit, `*/*`, ya da `type/*` joker.
pub fn mime_matches(pattern: &str, ct: &str) -> bool {
    if pattern == ct || pattern == "*/*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix("/*") {
        return ct.split('/').next() == Some(prefix);
    }
    false
}

/// Upload reddi — route katmanı HTTP statüsüne çevirir.
#[derive(Debug, PartialEq)]
pub enum UploadReject {
    /// İçerik tipi hiçbir format kuralına uymadı (415).
    UnsupportedType(String),
    /// Eşleşen kuralın boyut sınırı aşıldı (413) — taşınan değer sınır (MB).
    TooLarge(f64),
    /// Beyan edilen `Content-Type` ile dosya baytlarından sezilen gerçek tip çelişiyor
    /// (route katmanı bunu 415 sayar) — bkz. `wf_server::attachments::detect_mismatch`.
    TypeMismatch { declared: String, detected: String },
}

impl std::fmt::Display for UploadReject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedType(ct) => write!(f, "kabul edilmeyen içerik tipi: {ct}"),
            Self::TooLarge(mb) => write!(f, "dosya bu slot için çok büyük (sınır {mb} MB)"),
            Self::TypeMismatch { declared, detected } => write!(
                f,
                "beyan edilen tip ({declared}) dosyanın gerçek tipiyle ({detected}) çelişiyor"
            ),
        }
    }
}

/// Yüklenen dosyayı item'ın format kurallarına göre doğrular.
/// - `formats` boşsa: her tip/boyut kabul (yalnız boşluk kontrolü route'ta).
/// - Aksi halde: `content_type` bir kurala UYMALI; uyan kuralın `max_size_mb`'si uygulanır.
///   İçerik tipi verilmemişse (`None`) ve kural varsa → `UnsupportedType`.
pub fn check_upload(
    item: &AttachmentItem,
    content_type: Option<&str>,
    len: usize,
) -> Result<(), UploadReject> {
    if item.formats.is_empty() {
        return Ok(());
    }
    let ct = content_type.unwrap_or("").trim();
    let rule = item
        .formats
        .iter()
        .find(|r| r.accept.iter().any(|a| mime_matches(a, ct)));
    let Some(rule) = rule else {
        return Err(UploadReject::UnsupportedType(if ct.is_empty() {
            "(içerik tipi yok)".into()
        } else {
            ct.into()
        }));
    };
    if let Some(max_mb) = rule.max_size_mb {
        let max_bytes = (max_mb * 1024.0 * 1024.0) as usize;
        if len > max_bytes {
            return Err(UploadReject::TooLarge(max_mb));
        }
    }
    Ok(())
}

/// `AttachmentItem` içinden bir dosya için geçerli tüm MIME kalıpları (accept birleşimi) —
/// UI `accept` attribute'u / hata mesajı için.
pub fn all_accept_patterns(item: &AttachmentItem) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for r in &item.formats {
        for a in &r.accept {
            if !out.contains(a) {
                out.push(a.clone());
            }
        }
    }
    out
}

// ---- kapı (gate) kuralı ----

/// Bir node'da toplanan TEK dosya slotu — katalog (`wfd.attachments`) ile node
/// referansının (`NodeDef.attachments`) birleşiminden düzleştirilmiş hâli.
/// (`PartialEq` YOK: `AttachmentFormatRule` türetmiyor; karşılaştırma zaten
/// `key()`/`missing_required` çıktısı üzerinden yapılır.)
#[derive(Debug, Clone)]
pub struct GateSlot {
    pub group: String,
    pub item: String,
    pub label: Option<String>,
    pub required: bool,
    /// Bu slot SORULAN aksiyonu kapıyor mu? `action: None` sorulduğunda (node geneli
    /// durum listesi) daima `true` — süzme çağırana bırakılır.
    pub gates: bool,
    pub formats: Vec<AttachmentFormatRule>,
}

impl GateSlot {
    /// `"grup/item"` — kullanıcıya gösterilen ve `missing_required`'ın döndürdüğü kimlik.
    pub fn key(&self) -> String {
        format!("{}/{}", self.group, self.item)
    }
}

/// Verilen node'un topladığı tüm slotlar (grup sırası korunur). Node yoksa, attachment
/// referansı yoksa ya da referans edilen grup katalogda yoksa (validator yakalar;
/// runtime'da sessiz atlanır) boş döner.
pub fn gate_slots(wfd: &Wfd, node_key: &str, action: Option<&str>) -> Vec<GateSlot> {
    let Some(node) = wfd.nodes.get(node_key) else {
        return vec![];
    };
    let mut out = Vec::new();
    for aref in &node.attachments {
        let group_key = aref.group();
        let Some(group) = wfd.attachments.get(group_key) else {
            continue;
        };
        let gates = aref.gates_action(action);
        for item in &group.items {
            out.push(GateSlot {
                group: group_key.to_string(),
                item: item.id.clone(),
                label: item.label.clone(),
                required: item.required,
                gates,
                formats: item.formats.clone(),
            });
        }
    }
    out
}

/// Katalogdaki slot tanımını bulur — `attach` doğrulaması (format/boyut) için.
pub fn find_item<'a>(wfd: &'a Wfd, group: &str, item: &str) -> Option<&'a AttachmentItem> {
    wfd.attachments
        .get(group)?
        .items
        .iter()
        .find(|i| i.id == item)
}

/// Eksik zorunlu slotlar (`"grup/item"`) — yalnız `gates: true` slotlar sayılır.
/// `uploaded(group, item)` çağıranın gerçeğidir: gerçek akışta depo + staging,
/// simülasyonda `SimState.attachments`.
pub fn missing_required(
    slots: &[GateSlot],
    uploaded: impl Fn(&str, &str) -> bool,
) -> Vec<String> {
    slots
        .iter()
        .filter(|s| s.gates && s.required && !uploaded(&s.group, &s.item))
        .map(GateSlot::key)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn wfd_with_gate(node_attachments: serde_json::Value) -> Wfd {
        serde_json::from_value(json!({
            "wfd_version": "2.2",
            "id": "belgeli-v1",
            "name": "Belgeli",
            "version": "1.0.0",
            "context": { "type": "object", "properties": {} },
            "attachments": {
                "kimlik": { "items": [
                    { "id": "kimlik.pdf", "required": true,
                      "formats": [{ "accept": ["application/pdf"], "max_size_mb": 4 }] },
                    { "id": "ekstra.pdf", "required": false }
                ]}
            },
            "start": [{ "id": "s1", "from": "basvuru", "action": "gonder",
                        "wft": { "terminal": "Bitti" } }],
            "nodes": {
                "basvuru": {
                    "c_a": { "c_orgu": "self" },
                    "attachments": node_attachments
                }
            },
            "actions": { "gonder": { "input": { "required": [], "optional": [] } },
                         "iptal": { "input": { "required": [], "optional": [] } } },
            "transitions": [
                { "id": "t1", "from": "basvuru", "action": "gonder",
                  "wft": { "terminal": "Bitti" } },
                { "id": "t2", "from": "basvuru", "action": "iptal",
                  "wft": { "terminal": "Bitti" } }
            ],
            "terminals": [{ "id": "Bitti", "wfe_end_response": {} }]
        }))
        .expect("fixture parse")
    }

    #[test]
    fn plain_group_ref_gates_every_action() {
        let wfd = wfd_with_gate(json!(["kimlik"]));
        let slots = gate_slots(&wfd, "basvuru", Some("iptal"));
        assert_eq!(slots.len(), 2);
        assert!(slots.iter().all(|s| s.gates));
        assert_eq!(missing_required(&slots, |_, _| false), vec!["kimlik/kimlik.pdf"]);
        // Yüklenmişse kapı açılır; `required: false` slot hiç kapı değildir.
        assert!(missing_required(&slots, |g, i| g == "kimlik" && i == "kimlik.pdf").is_empty());
    }

    #[test]
    fn scoped_ref_only_gates_listed_actions() {
        let wfd = wfd_with_gate(json!([{ "group": "kimlik", "actions": ["gonder"] }]));
        assert_eq!(
            missing_required(&gate_slots(&wfd, "basvuru", Some("gonder")), |_, _| false),
            vec!["kimlik/kimlik.pdf"]
        );
        // Kapsam dışı aksiyon belgesiz de submit edilebilir.
        assert!(
            missing_required(&gate_slots(&wfd, "basvuru", Some("iptal")), |_, _| false).is_empty()
        );
    }

    #[test]
    fn empty_actions_list_gates_nothing() {
        let wfd = wfd_with_gate(json!([{ "group": "kimlik", "actions": [] }]));
        let slots = gate_slots(&wfd, "basvuru", Some("gonder"));
        assert_eq!(slots.len(), 2, "grup toplanır");
        assert!(
            missing_required(&slots, |_, _| false).is_empty(),
            "ama hiçbir aksiyonu kapamaz"
        );
    }

    #[test]
    fn node_wide_query_marks_every_slot_as_gating() {
        let wfd = wfd_with_gate(json!([{ "group": "kimlik", "actions": ["gonder"] }]));
        let slots = gate_slots(&wfd, "basvuru", None);
        assert!(slots.iter().all(|s| s.gates), "süzme istemcide");
    }

    #[test]
    fn unknown_node_or_group_yields_no_slots() {
        let wfd = wfd_with_gate(json!(["kimlik"]));
        assert!(gate_slots(&wfd, "yok", None).is_empty());
    }

    #[test]
    fn upload_check_applies_the_matching_rule() {
        let wfd = wfd_with_gate(json!(["kimlik"]));
        let item = find_item(&wfd, "kimlik", "kimlik.pdf").unwrap();
        assert!(check_upload(item, Some("application/pdf"), 1024).is_ok());
        assert_eq!(
            check_upload(item, Some("image/png"), 1024),
            Err(UploadReject::UnsupportedType("image/png".into()))
        );
        assert_eq!(
            check_upload(item, Some("application/pdf"), 5 * 1024 * 1024),
            Err(UploadReject::TooLarge(4.0))
        );
        // Kuralı olmayan slot her tipi/boyutu kabul eder.
        let free = find_item(&wfd, "kimlik", "ekstra.pdf").unwrap();
        assert!(check_upload(free, None, 999_999_999).is_ok());
    }

    #[test]
    fn mime_wildcards() {
        assert!(mime_matches("image/*", "image/png"));
        assert!(mime_matches("*/*", "application/zip"));
        assert!(!mime_matches("image/*", "application/pdf"));
    }
}
