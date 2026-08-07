//! Ek-belge (attachment) depolama — PORTAL/edge katmanı sorumluluğu.
//!
//! Engine core (wfe-core) dosya I/O yapmaz; yalnız WFD içindeki attachment
//! KATALOGUNU ve node referanslarını metadata olarak taşır. Dosyaların kendisi
//! bu store aracılığıyla opendal üzerinden dış bir konuma (varsayılan:
//! `work-pool-portal/storage`, S3'e geçince aynı arayüz) yazılır ve varlığı
//! burada kontrol edilir. Böylece engine dış kaynaklara bağımlı kalmaz.
//!
//! Storage anahtarı: `attachments/{wfe_id}/{grup}/{item}` — grup+item WFD
//! katalogundan gelir; wfe_id sayesinde her instance kendi dosyalarını izole tutar
//! ve aynı grubu referanslayan farklı node'lar dosyayı tekrar istemez.

use opendal::Operator;
use serde::Serialize;
use uuid::Uuid;
use wfe_core::types::wfd_v22::{AttachmentFormatRule, AttachmentItem, Wfd};

#[derive(Clone)]
pub struct AttachmentStore {
    op: Operator,
}

impl AttachmentStore {
    pub fn new(op: Operator) -> Self {
        Self { op }
    }

    fn key(wfe_id: Uuid, group: &str, item: &str) -> String {
        format!("attachments/{wfe_id}/{group}/{item}")
    }

    /// Dosya slotu yüklenmiş mi? Gate ve UI durum sorgusu bunu kullanır.
    pub async fn exists(
        &self,
        wfe_id: Uuid,
        group: &str,
        item: &str,
    ) -> Result<bool, opendal::Error> {
        self.op.exists(&Self::key(wfe_id, group, item)).await
    }

    /// Yükleme — opendal write (local fs veya S3, backend'e göre şeffaf).
    pub async fn write(
        &self,
        wfe_id: Uuid,
        group: &str,
        item: &str,
        bytes: Vec<u8>,
    ) -> Result<(), opendal::Error> {
        self.op
            .write(&Self::key(wfe_id, group, item), bytes)
            .await
            .map(|_| ())
    }

    /// İndirme.
    pub async fn read(
        &self,
        wfe_id: Uuid,
        group: &str,
        item: &str,
    ) -> Result<Vec<u8>, opendal::Error> {
        Ok(self
            .op
            .read(&Self::key(wfe_id, group, item))
            .await?
            .to_vec())
    }

    /// Silme (idempotent — yoksa da hata vermez opendal semantiğinde).
    pub async fn delete(
        &self,
        wfe_id: Uuid,
        group: &str,
        item: &str,
    ) -> Result<(), opendal::Error> {
        self.op.delete(&Self::key(wfe_id, group, item)).await
    }
}

// ---- attachment durum tipleri + gate yardımcıları (her iki route ağacı da kullanır) ----

#[derive(Debug, Serialize, Clone)]
pub struct AttachmentItemStatus {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub required: bool,
    /// Kabul edilen format kuralları (her biri MIME grubu + o gruba özel boyut sınırı).
    /// Boşsa: her tip, sınırsız boyut.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub formats: Vec<AttachmentFormatRule>,
    pub uploaded: bool,
}

#[derive(Debug, Serialize, Clone)]
pub struct AttachmentGroupStatus {
    pub group: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub items: Vec<AttachmentItemStatus>,
    /// Bu grup SORULAN aksiyonu kapıyor mu? Aksiyon verilmeden sorulan node geneli
    /// listede daima `true`'dur — orada "hangi aksiyon" sorusunun cevabı `actions`tır.
    /// `false` grup toplanır ama o aksiyonu bloklamaz (yükleme opsiyoneldir).
    pub gates: bool,
    /// Grubun kapı olduğu aksiyonlar; `None` = node'un tüm aksiyonları. İstemci node
    /// geneli listeyi seçili aksiyona göre kendisi süzebilsin diye taşınır.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actions: Option<Vec<String>>,
}

/// Verilen node'un referansladığı tüm grupların item bazlı yükleme durumu.
/// Node yoksa ya da attachment referansı yoksa boş döner.
///
/// `action`: hangi aksiyon submit edilmek üzere sorulduğu. `Some(a)` verilirse her
/// grubun `gates` alanı o aksiyona göre hesaplanır (kapsam dışı gruplar `false`);
/// `None` (node geneli durum listesi) hepsini `gates: true` bırakır — süzme istemcide.
pub async fn status_for_node(
    store: &AttachmentStore,
    wfd: &Wfd,
    wfe_id: Uuid,
    node_key: &str,
    action: Option<&str>,
) -> Result<Vec<AttachmentGroupStatus>, opendal::Error> {
    let Some(node) = wfd.nodes.get(node_key) else {
        return Ok(vec![]);
    };
    let mut out = Vec::new();
    for aref in &node.attachments {
        let group_ref = aref.group();
        let Some(group) = wfd.attachments.get(group_ref) else {
            continue; // validator zaten yakalar; runtime'da sessiz atla
        };
        let mut items = Vec::with_capacity(group.items.len());
        for item in &group.items {
            let uploaded = store.exists(wfe_id, group_ref, &item.id).await?;
            items.push(AttachmentItemStatus {
                id: item.id.clone(),
                label: item.label.clone(),
                required: item.required,
                formats: item.formats.clone(),
                uploaded,
            });
        }
        out.push(AttachmentGroupStatus {
            group: group_ref.to_string(),
            label: group.label.clone(),
            items,
            gates: aref.gates_action(action),
            actions: aref.actions().map(|a| a.to_vec()),
        });
    }
    Ok(out)
}

/// Gate koşulu: sorulan aksiyonu KAPAYAN grupların tüm `required` dosyaları yüklü mü?
/// Kapamayan gruplar (aksiyon kapsamı dışı) eksik olsa da engellemez.
pub fn satisfied(groups: &[AttachmentGroupStatus]) -> bool {
    groups
        .iter()
        .filter(|g| g.gates)
        .all(|g| g.items.iter().all(|i| !i.required || i.uploaded))
}

/// Eksik zorunlu item id'leri ("grup/item" biçiminde) — 422 mesajı için.
pub fn missing_required(groups: &[AttachmentGroupStatus]) -> Vec<String> {
    groups
        .iter()
        .filter(|g| g.gates)
        .flat_map(|g| {
            g.items
                .iter()
                .filter(|i| i.required && !i.uploaded)
                .map(move |i| format!("{}/{}", g.group, i.id))
        })
        .collect()
}

// ---- upload doğrulaması (her iki route ağacı da kullanır) ----

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
pub enum UploadReject {
    /// İçerik tipi hiçbir format kuralına uymadı (415).
    UnsupportedType(String),
    /// Eşleşen kuralın boyut sınırı aşıldı (413) — taşınan değer sınır (MB).
    TooLarge(f64),
}

/// Yüklenen dosyayı item'ın format kurallarına göre doğrular.
/// - `formats` boşsa: her tip/boyut kabul (yalnız boşluk kontrolü route'ta).
/// - Aksi halde: `content_type` bir kurala UYMALI; uyan kuralın `max_size_mb`'si uygulanır.
///   İçerik tipi verilmemişse (`None`) ve kural varsa → UnsupportedType.
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
