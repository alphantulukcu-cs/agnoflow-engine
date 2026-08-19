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
//!
//! **Ad-hoc not dosyaları AYRI bir anahtar ailesindedir:** `notes/{wfe_id}/{file_id}`
//! (`docs/superpowers/specs/2026-08-10-wfe-not-ve-adhoc-belge-design.md`, "Storage
//! anahtarı"). Bu dosyaların katalogda (grup/item, format kuralı) karşılığı yoktur;
//! aynı ağaca (`attachments/...`) karıştırmak `status_for_node` ve gate mantığını
//! yanıltırdı — bu yüzden kökten ayrı bir prefiks (`notes/...`) kullanılır.

use opendal::Operator;
use serde::Serialize;
use uuid::Uuid;
use wfe_core::types::wfd_v22::{AttachmentFormatRule, Wfd};
use wfe_core::v22::attachments::GateSlot;

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

    /// Ad-hoc not dosyası anahtarı — katalog anahtarından (`key`) kasıtlı olarak ayrı
    /// bir kök (`notes/`) kullanır; bkz. modül başlığı.
    fn note_key(wfe_id: Uuid, file_id: Uuid) -> String {
        format!("notes/{wfe_id}/{file_id}")
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

    /// Ham opendal `Operator` — YALNIZ bu store'un bilmediği anahtar kökleri için
    /// (bugün: `staging/{upload_id}`, bkz. `crate::staging`). Katalog ve not anahtarları
    /// için buradaki tipli metotlar kullanılır; anahtar biçimi bu dosyanın dışına sızmasın.
    ///
    /// Neden erişimci: staging nesnesi nihai anahtarla AYNI depoda olmak zorunda (taşıma
    /// server-side copy olsun diye), o depo da WFD başına `$env` ile çözülüyor. İkinci bir
    /// Operator kurmak `$env` çözümünü ikiye bölerdi.
    pub fn operator(&self) -> &Operator {
        &self.op
    }

    /// AKIŞ halinde yükleme (tek istekli başlatma yolu, 2026-08-11).
    ///
    /// `write` tüm gövdeyi `Vec<u8>` olarak ister; multipart yolunda bu, isteğe konan
    /// TÜM dosyaların aynı anda bellekte olması demekti. Writer chunk chunk yazar →
    /// bellek kullanımı dosya sayısından ve boyutundan BAĞIMSIZ kalır.
    ///
    /// Çağıran `close()` etmeli; etmezse nesne tamamlanmaz (S3'te multipart upload
    /// abort edilir) — yarıda kalan yükleme yarım nesne bırakmaz.
    pub async fn writer(
        &self,
        wfe_id: Uuid,
        group: &str,
        item: &str,
    ) -> Result<opendal::Writer, opendal::Error> {
        self.op.writer(&Self::key(wfe_id, group, item)).await
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

    /// Bir WFE'nin (ya da başlatılmamış rezervasyonun) TÜM dosyalarını siler — hem
    /// katalog attachment'larını (`attachments/{wfe_id}/`) hem ad-hoc not dosyalarını
    /// (`notes/{wfe_id}/`). Süpürücü kullanır: süresi geçen rezervasyonun dosyaları
    /// sahipsiz kalır; WFE silinince iki ağaç da birlikte temizlenir.
    pub async fn remove_all(&self, wfe_id: Uuid) -> Result<(), opendal::Error> {
        self.op.remove_all(&format!("attachments/{wfe_id}/")).await?;
        self.op.remove_all(&format!("notes/{wfe_id}/")).await
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

    // ---- ad-hoc not dosyaları (`notes/{wfe_id}/{file_id}`) ----

    /// Not dosyası slotu yüklenmiş mi?
    pub async fn note_exists(&self, wfe_id: Uuid, file_id: Uuid) -> Result<bool, opendal::Error> {
        self.op.exists(&Self::note_key(wfe_id, file_id)).await
    }

    /// Not dosyası yükleme.
    pub async fn note_write(
        &self,
        wfe_id: Uuid,
        file_id: Uuid,
        bytes: Vec<u8>,
    ) -> Result<(), opendal::Error> {
        self.op
            .write(&Self::note_key(wfe_id, file_id), bytes)
            .await
            .map(|_| ())
    }

    /// Not dosyası indirme.
    pub async fn note_read(&self, wfe_id: Uuid, file_id: Uuid) -> Result<Vec<u8>, opendal::Error> {
        Ok(self.op.read(&Self::note_key(wfe_id, file_id)).await?.to_vec())
    }

    /// Not dosyası silme (idempotent — yoksa da hata vermez opendal semantiğinde).
    pub async fn note_delete(&self, wfe_id: Uuid, file_id: Uuid) -> Result<(), opendal::Error> {
        self.op.delete(&Self::note_key(wfe_id, file_id)).await
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
    /// Aşağıdaki alanlar `wf.wfe_attachment` DB metadata'sından gelir (bkz.
    /// `enrich_with_meta`) — depo (`AttachmentStore`) yalnız "var mı" bilir, "hangi ad,
    /// ne boyut, ne zaman, kim yükledi" sorusunun cevabı burada. Metadata satırı yoksa
    /// (eski yükleme yolu ya da tablo eklenmeden ÖNCEki her şey) hepsi `None` kalır;
    /// bu durum `uploaded: true` ile ÇELİŞMEZ — yalnız ek bilgi eksik demektir.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uploaded_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<i32>,
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
                // DB metadata'sı burada YOK — bu fonksiyon depo dışına (pool) bağımlı
                // olmasın diye kasıtlı olarak sızdırılmadı (bkz. `enrich_with_meta`).
                filename: None,
                content_type: None,
                size_bytes: None,
                sha256: None,
                uploaded_at: None,
                version: None,
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

/// Depodan üretilmiş durum listesini (`status_for_node`) DB metadata'sıyla
/// (`wf.wfe_attachment`, bkz. `crate::wfe_attachment`) zenginleştirir: dosya adı, MIME
/// tipi, boyut, sha256, yükleyen zaman ve sürüm numarası eklenir.
///
/// Kapı mantığı (`satisfied`/`missing_required`) bunu ÇAĞIRMAZ ve `uploaded` alanı
/// gerçeğin kaynağı olarak DAİMA DEPODA kalır — bu fonksiyon `uploaded`'a HİÇ
/// dokunmaz. NEDEN: `wf.wfe_attachment` tablosu 2026-08-11'de eklendi; ondan önce
/// yüklenmiş (ve o tarihten sonra bile eski koddan geçmiş) hiçbir dosyanın metadata
/// satırı yoktur. Metadata'yı gerçeğin kaynağı yapsaydık, storage'da fiilen duran bu
/// dosyalar "yüklenmemiş" görünürdü — sırf DB'de audit kaydı yok diye zaten teslim
/// edilmiş bir belgeyi kaybettirmiş olurduk. Bu yüzden metadata yalnız EK bilgi
/// (ad/tip/boyut/tarih/sürüm) taşır; eşleşen satır yoksa item'ın ek alanları `None`
/// kalır ama `uploaded` depodan geldiği gibi `true` olmaya devam eder.
pub fn enrich_with_meta(
    groups: &mut [AttachmentGroupStatus],
    metas: &[crate::wfe_attachment::AttachmentMeta],
) {
    for group in groups.iter_mut() {
        for item in group.items.iter_mut() {
            let Some(meta) = metas
                .iter()
                .find(|m| m.grp == group.group && m.item == item.id)
            else {
                continue;
            };
            item.filename = meta.filename.clone();
            item.content_type = Some(meta.content_type.clone());
            item.size_bytes = Some(meta.size_bytes);
            item.sha256 = Some(meta.sha256.clone());
            item.uploaded_at = Some(meta.uploaded_at);
            item.version = Some(meta.version);
        }
    }
}

/// Gate koşulu: sorulan aksiyonu KAPAYAN grupların tüm `required` dosyaları yüklü mü?
/// Kapamayan gruplar (aksiyon kapsamı dışı) eksik olsa da engellemez.
///
/// Bu, `satisfied_with_pending`in boş `pending` ile özel hâlidir — davranış DEĞİŞMEDİ,
/// yalnız ortak mantık tek yerde toplandı.
pub fn satisfied(groups: &[AttachmentGroupStatus]) -> bool {
    satisfied_with_pending(groups, &[])
}

/// Eksik zorunlu item id'leri ("grup/item" biçiminde) — 422 mesajı için.
///
/// Bu, `missing_required_with_pending`in boş `pending` ile özel hâlidir — davranış
/// DEĞİŞMEDİ, yalnız ortak mantık tek yerde toplandı.
pub fn missing_required(groups: &[AttachmentGroupStatus]) -> Vec<String> {
    missing_required_with_pending(groups, &[])
}

/// `missing_required`in genelleştirilmiş hâli: bu istekte STAGING'e yazılmış ama henüz
/// nihai anahtara taşınmamış slotları da YÜKLENMİŞ sayarak kapıyı sorar. `pending`:
/// `(grup, item)` çiftleri.
///
/// Neden gerekli: Faz 4'te (`POST /wfe/{id}/actions`) dosyalar aksiyonla AYNI istekte
/// gönderiliyor; aksiyon uygulanamazsa mevcut dosya DEĞİŞMEMELİ diye dosyalar önce
/// staging'e yazılıyor, nihai anahtara ancak aksiyon başarılı olunca taşınıyor. Ama kapı
/// (`status_for_node` → depodan `exists`) aksiyondan ÖNCE koşar ve staging'i hiç GÖREMEZ:
/// birleştirilmezse kullanıcı eksik belgeyi bu istekte gönderse bile kapı "eksik" der,
/// aksiyon reddedilir, dosya hiç yerine konmaz — kilitlenme. `pending` bu iki bilgiyi
/// (depodaki gerçek + bu istekte staging'e konan) kapı için birleştirir.
///
/// Yalnız `gates: true` gruplar sayılır (mevcut `missing_required` ile aynı ayrım).
/// `AttachmentItemStatus.uploaded` alanına HİÇ DOKUNMAZ: o alan deponun gerçeğidir,
/// istemciye "şu an depoda ne var" bilgisini verir; `pending` yalnız KAPIYI gevşetir,
/// depo/uploaded görünümü aksiyon başarılı olup taşıma gerçekleşene kadar eskisi gibi kalır.
pub fn missing_required_with_pending(
    groups: &[AttachmentGroupStatus],
    pending: &[(String, String)],
) -> Vec<String> {
    // Kural (hangi slot kapıdır, hangisi eksiktir) `wfe_core::v22::attachments`'ta —
    // simülasyon/senaryo koşucusu (`wf_wfe::sim`) AYNI fonksiyonu çağırır. Burada
    // yalnız bu katmanın "yüklenmiş" tanımı verilir: depo ∪ bu isteğin staging'i.
    let slots: Vec<GateSlot> = groups
        .iter()
        .flat_map(|g| {
            g.items.iter().map(move |i| GateSlot {
                group: g.group.clone(),
                item: i.id.clone(),
                label: i.label.clone(),
                required: i.required,
                gates: g.gates,
                formats: i.formats.clone(),
            })
        })
        .collect();
    let uploaded = |group: &str, item: &str| {
        groups.iter().any(|g| {
            g.group == group
                && g.items
                    .iter()
                    .any(|i| i.id == item && i.uploaded)
        }) || pending.iter().any(|(pg, pi)| pg == group && pi == item)
    };
    wfe_core::v22::attachments::missing_required(&slots, uploaded)
}

/// `satisfied`in genelleştirilmiş hâli — bkz. `missing_required_with_pending` NEDEN
/// açıklaması için. Boşluk kontrolüyle tanımlanır: ikisi aynı kuralı iki farklı biçimde
/// (bool / liste) sormasın diye ayrı yeniden yazılmaz.
pub fn satisfied_with_pending(groups: &[AttachmentGroupStatus], pending: &[(String, String)]) -> bool {
    missing_required_with_pending(groups, pending).is_empty()
}

// ---- upload doğrulaması (her iki route ağacı da kullanır) ----

// ---- format/boyut kuralı: wfe-core'da ----
//
// `mime_matches` / `UploadReject` / `check_upload` / `all_accept_patterns`
// 2026-08-19'da `wfe_core::v22::attachments`'a taşındı: AYNI kuralı iki tüketici
// uygular — bu katman (gerçek akış, `uploaded` bilgisi depodan) ve
// `wf_wfe::sim` (simülasyon/senaryo, `uploaded` bilgisi `SimState`ten). İki
// kopya olsaydı simülasyonda geçen bir senaryo portalda 415/413 alabilirdi.
// Buradaki `pub use` çağrı yerlerini (`crate::attachments::check_upload`, …)
// olduğu gibi bırakır. Baytlara bakan kısım (`sniff_content_type`/
// `detect_mismatch`) TAŞINMADI — o route katmanının işi.
pub use wfe_core::v22::attachments::{
    all_accept_patterns, check_upload, mime_matches, UploadReject,
};

// ---- magic-byte sniff (beyan edilen Content-Type'a körü körüne güvenmemek için) ----

/// `PK\x03\x04` (ve boş/parçalı arşiv varyantları `PK\x05\x06`/`PK\x07\x08`) yalnız
/// gerçek `.zip`'e değil, aynı konteyneri kullanan Office Open XML (docx/xlsx/pptx) ve
/// OpenDocument/jar/apk ailesine de aittir — baytlardan bu ayrım YAPILAMAZ. Bu yüzden
/// `detect_mismatch` `application/zip` tespitini bu ailenin herhangi bir üyesiyle beyan
/// edilmiş olmaya izin verir; aksi halde meşru bir `.docx` yüklemesi yanlışlıkla "tip
/// uyuşmazlığı" sayılırdı.
const ZIP_FAMILY: &[&str] = &[
    "application/zip",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    "application/vnd.oasis.opendocument.text",
    "application/vnd.oasis.opendocument.spreadsheet",
    "application/vnd.oasis.opendocument.presentation",
    "application/java-archive",
    "application/vnd.android.package-archive",
];

/// İçeriğin ilk baytlarından (`head`) gerçek tipi tespit eder. Bilinmeyen imza → `None`
/// (reddetme sebebi DEĞİL; tanımadığımız her formatı yasaklamak katalogdaki serbest
/// tipleri (`formats` boş item'lar) kullanılamaz yapardı).
pub fn sniff_content_type(head: &[u8]) -> Option<&'static str> {
    if head.starts_with(b"%PDF") {
        return Some("application/pdf");
    }
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if head.starts_with(b"\xFF\xD8\xFF") {
        return Some("image/jpeg");
    }
    if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if head.len() >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06") || head.starts_with(b"PK\x07\x08")
    {
        return Some("application/zip");
    }
    if head.starts_with(b"Rar!\x1a\x07\x00") || head.starts_with(b"Rar!\x1a\x07\x01\x00") {
        return Some("application/vnd.rar");
    }
    if head.starts_with(b"7z\xbc\xaf\x27\x1c") {
        return Some("application/x-7z-compressed");
    }
    if head.starts_with(b"\x1f\x8b") {
        return Some("application/gzip");
    }
    // ELF/PE/Mach-O: çalıştırılabilir dosyanın belge diye geçmesi bu sniff'in kapatmak
    // istediği asıl senaryo — bunlar `ZIP_FAMILY` gibi bir eşdeğerlik grubuna ASLA
    // eklenmez, `detect_mismatch`'te yalnız kendileriyle (tam eşit) uyuşurlar.
    if head.starts_with(b"\x7fELF") {
        return Some("application/x-elf");
    }
    if head.starts_with(b"MZ") {
        return Some("application/x-msdownload");
    }
    if head.len() >= 4 {
        const MACHO_MAGICS: [[u8; 4]; 6] = [
            [0xFE, 0xED, 0xFA, 0xCE], // 32-bit, big-endian
            [0xFE, 0xED, 0xFA, 0xCF], // 64-bit, big-endian
            [0xCE, 0xFA, 0xED, 0xFE], // 32-bit, little-endian
            [0xCF, 0xFA, 0xED, 0xFE], // 64-bit, little-endian
            [0xCA, 0xFE, 0xBA, 0xBE], // fat/universal binary, big-endian
            [0xBE, 0xBA, 0xFE, 0xCA], // fat/universal binary, little-endian
        ];
        let magic = [head[0], head[1], head[2], head[3]];
        if MACHO_MAGICS.contains(&magic) {
            return Some("application/x-mach-binary");
        }
    }
    None
}

/// Beyan edilen tip ile içerikten tespit edilen tip çelişiyor mu?
/// Tespit edilemiyorsa (`None`) ÇELİŞKİ YOK sayılır — bilinmeyen imzalı serbest formatları
/// bu fonksiyon da (`sniff_content_type` gibi) yasaklamaz. Beyan hiç verilmemişse de
/// (`declared: None`, ya da boş string) kıyaslanacak bir şey yoktur → çelişki yok.
pub fn detect_mismatch(declared: Option<&str>, head: &[u8]) -> Option<UploadReject> {
    let detected = sniff_content_type(head)?;
    let declared = declared?.trim();
    if declared.is_empty() {
        return None;
    }
    // check_upload'ın accept eşleştirmesiyle AYNI joker kuralı (`image/*` vb.) — bu
    // mantık `mime_matches`'te zaten var, burada kopyalanmadan yeniden kullanılıyor.
    // İki yönde de denenir: hangi tarafın joker/kalıp olduğu çağırana göre değişebilir.
    if mime_matches(declared, detected) || mime_matches(detected, declared) {
        return None;
    }
    if detected == "application/zip" && ZIP_FAMILY.contains(&declared) {
        return None;
    }
    // Ret tipi `UploadReject`: yükleme reddinin TEK tipi olsun, çağıran iki ayrı
    // hata biçimini birleştirmek zorunda kalmasın.
    Some(UploadReject::TypeMismatch {
        declared: declared.to_string(),
        detected: detected.to_string(),
    })
}

// ---- akış hâlinde SHA-256 (dosya tümüyle bellekte tutulmadan özetlenmesi için) ----

/// Chunk chunk beslenen SHA-256 özeti. Yeni yükleme yolu dosyayı stream'le opendal'a
/// yazacağı için tüm gövdenin bir kerede bellekte durup `Sha256::digest(&bytes)` ile
/// özetlenmesi İSTENMİYOR — bu tip her `update` çağrısında parçayı içerdeki hasher'a
/// besler, sonunda `finish` hex özeti üretir.
pub struct Sha256Stream(sha2::Sha256);

impl Sha256Stream {
    pub fn new() -> Self {
        use sha2::Digest;
        Self(sha2::Sha256::new())
    }

    pub fn update(&mut self, chunk: &[u8]) {
        use sha2::Digest;
        self.0.update(chunk);
    }

    /// Hex kodlu özet (64 karakter, küçük harf).
    pub fn finish(self) -> String {
        use sha2::Digest;
        let digest = self.0.finalize();
        let mut hex = String::with_capacity(digest.len() * 2);
        for b in digest {
            hex.push_str(&format!("{b:02x}"));
        }
        hex
    }
}

impl Default for Sha256Stream {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest;

    #[test]
    fn sniff_detects_pdf() {
        assert_eq!(sniff_content_type(b"%PDF-1.7\n..."), Some("application/pdf"));
    }

    #[test]
    fn sniff_detects_png() {
        let mut head = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        head.extend_from_slice(&[0, 0, 0, 0]); // gerisi önemsiz
        assert_eq!(sniff_content_type(&head), Some("image/png"));
    }

    #[test]
    fn sniff_detects_zip() {
        let head = [b'P', b'K', 0x03, 0x04, 0, 0, 0, 0];
        assert_eq!(sniff_content_type(&head), Some("application/zip"));
    }

    #[test]
    fn sniff_detects_elf() {
        let head = [0x7f, b'E', b'L', b'F', 2, 1, 1, 0];
        assert_eq!(sniff_content_type(&head), Some("application/x-elf"));
    }

    #[test]
    fn sniff_unknown_signature_is_none() {
        assert_eq!(sniff_content_type(b"bilinmeyen-format-baytlari"), None);
    }

    #[test]
    fn mismatch_none_when_sniff_unknown() {
        // Tespit edilemeyen imza → çelişki yok, ne beyan edilirse edilsin.
        assert_eq!(detect_mismatch(Some("application/pdf"), b"???"), None);
    }

    #[test]
    fn mismatch_none_when_declared_matches_detected() {
        assert_eq!(detect_mismatch(Some("application/pdf"), b"%PDF-1.4"), None);
    }

    #[test]
    fn mismatch_none_with_wildcard_declared() {
        let mut head = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        head.extend_from_slice(&[0, 0, 0, 0]);
        assert_eq!(detect_mismatch(Some("image/*"), &head), None);
    }

    #[test]
    fn mismatch_none_for_docx_zip_family() {
        let head = [b'P', b'K', 0x03, 0x04, 0, 0, 0, 0];
        let docx = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
        assert_eq!(detect_mismatch(Some(docx), &head), None);
    }

    #[test]
    fn mismatch_none_when_declared_absent() {
        let head = [0x7f, b'E', b'L', b'F', 2, 1, 1, 0];
        assert_eq!(detect_mismatch(None, &head), None);
    }

    #[test]
    fn mismatch_detects_executable_disguised_as_pdf() {
        let head = [0x7f, b'E', b'L', b'F', 2, 1, 1, 0];
        let got = detect_mismatch(Some("application/pdf"), &head);
        assert_eq!(
            got,
            Some(UploadReject::TypeMismatch {
                declared: "application/pdf".to_string(),
                detected: "application/x-elf".to_string(),
            })
        );
    }

    #[test]
    fn mismatch_detects_pe_disguised_as_pdf() {
        let head = [b'M', b'Z', 0x90, 0x00, 0x03, 0x00, 0x00, 0x00];
        let got = detect_mismatch(Some("application/pdf"), &head);
        assert_eq!(
            got,
            Some(UploadReject::TypeMismatch {
                declared: "application/pdf".to_string(),
                detected: "application/x-msdownload".to_string(),
            })
        );
    }

    #[test]
    fn sha256_stream_matches_one_shot_hash() {
        let data = b"Agnoflow attachment stream sha256 testi - biraz daha uzun bir icerik.";

        let mut stream = Sha256Stream::new();
        for chunk in data.chunks(7) {
            stream.update(chunk);
        }
        let streamed = stream.finish();

        let expected = {
            let digest = sha2::Sha256::digest(data);
            let mut hex = String::with_capacity(digest.len() * 2);
            for b in digest {
                hex.push_str(&format!("{b:02x}"));
            }
            hex
        };

        assert_eq!(streamed, expected);
    }

    // ---- enrich_with_meta ----

    fn test_item(id: &str, uploaded: bool) -> AttachmentItemStatus {
        AttachmentItemStatus {
            id: id.to_string(),
            label: None,
            required: true,
            formats: vec![],
            uploaded,
            filename: None,
            content_type: None,
            size_bytes: None,
            sha256: None,
            uploaded_at: None,
            version: None,
        }
    }

    fn test_group(group: &str, items: Vec<AttachmentItemStatus>) -> AttachmentGroupStatus {
        AttachmentGroupStatus {
            group: group.to_string(),
            label: None,
            items,
            gates: true,
            actions: None,
        }
    }

    fn test_meta(grp: &str, item: &str, filename: &str) -> crate::wfe_attachment::AttachmentMeta {
        crate::wfe_attachment::AttachmentMeta {
            grp: grp.to_string(),
            item: item.to_string(),
            version: 3,
            filename: Some(filename.to_string()),
            content_type: "application/pdf".to_string(),
            size_bytes: 1234,
            sha256: "deadbeef".to_string(),
            uploaded_by: Uuid::nil(),
            uploaded_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn enrich_with_meta_zenginlestirir_eslesen_slotu() {
        let mut groups = vec![test_group("belgeler", vec![test_item("kimlik", true)])];
        let metas = vec![test_meta("belgeler", "kimlik", "kimlik.pdf")];

        enrich_with_meta(&mut groups, &metas);

        let item = &groups[0].items[0];
        assert!(item.uploaded);
        assert_eq!(item.filename.as_deref(), Some("kimlik.pdf"));
        assert_eq!(item.content_type.as_deref(), Some("application/pdf"));
        assert_eq!(item.size_bytes, Some(1234));
        assert_eq!(item.sha256.as_deref(), Some("deadbeef"));
        assert_eq!(item.version, Some(3));
        assert!(item.uploaded_at.is_some());
    }

    #[test]
    fn enrich_with_meta_metadatasiz_slot_uploaded_true_kalir() {
        // Tabloya kayıt düşmeden önce (ya da eski yükleme yolundan) depoya yazılmış
        // dosya: metadata satırı yok ama `exists` true dönmüştü → `uploaded: true`
        // bu fonksiyondan geçtikten sonra da DEĞİŞMEMELİ, yalnız ek alanlar boş kalır.
        let mut groups = vec![test_group("belgeler", vec![test_item("kimlik", true)])];
        let metas: Vec<crate::wfe_attachment::AttachmentMeta> = vec![];

        enrich_with_meta(&mut groups, &metas);

        let item = &groups[0].items[0];
        assert!(item.uploaded);
        assert!(item.filename.is_none());
        assert!(item.content_type.is_none());
        assert!(item.size_bytes.is_none());
        assert!(item.sha256.is_none());
        assert!(item.uploaded_at.is_none());
        assert!(item.version.is_none());
    }

    // ---- missing_required_with_pending / satisfied_with_pending ----

    fn test_group_gates(group: &str, items: Vec<AttachmentItemStatus>, gates: bool) -> AttachmentGroupStatus {
        AttachmentGroupStatus {
            group: group.to_string(),
            label: None,
            items,
            gates,
            actions: None,
        }
    }

    #[test]
    fn pending_eksik_zorunlu_slotu_kapiyi_acar() {
        // "belgeler/kimlik" depoda yok ama bu istekte staging'e yazıldı (pending) —
        // kapı bunu YÜKLENMİŞ saymalı.
        let groups = vec![test_group("belgeler", vec![test_item("kimlik", false)])];
        let pending = vec![("belgeler".to_string(), "kimlik".to_string())];

        assert!(missing_required_with_pending(&groups, &pending).is_empty());
        assert!(satisfied_with_pending(&groups, &pending));
    }

    #[test]
    fn pendingte_olmayan_eksik_zorunlu_slot_kapiyi_kapali_tutar() {
        let groups = vec![test_group("belgeler", vec![test_item("kimlik", false)])];
        // Farklı bir slot pending'de — "belgeler/kimlik" hâlâ eksik.
        let pending = vec![("belgeler".to_string(), "baska-item".to_string())];

        assert_eq!(
            missing_required_with_pending(&groups, &pending),
            vec!["belgeler/kimlik".to_string()]
        );
        assert!(!satisfied_with_pending(&groups, &pending));
    }

    #[test]
    fn pendingteki_alakasiz_cift_hicbir_seyi_degistirmez() {
        let groups = vec![test_group("belgeler", vec![test_item("kimlik", true)])];
        // Zaten yüklü bir slot için alakasız pending girdileri — sonucu değiştirmemeli.
        let pending = vec![
            ("baska-grup".to_string(), "kimlik".to_string()),
            ("belgeler".to_string(), "baska-item".to_string()),
        ];

        assert!(missing_required_with_pending(&groups, &pending).is_empty());
        assert!(satisfied_with_pending(&groups, &pending));
    }

    #[test]
    fn gates_false_grup_pending_ile_de_kapiyi_etkilemez() {
        // gates:false grup aksiyon kapsamı dışı — eksik zorunlu slot pending'de olsun ya
        // da olmasın, kapıyı hiç saymaz.
        let groups_without_pending = vec![test_group_gates(
            "belgeler",
            vec![test_item("kimlik", false)],
            false,
        )];
        assert!(missing_required_with_pending(&groups_without_pending, &[]).is_empty());
        assert!(satisfied_with_pending(&groups_without_pending, &[]));

        let pending = vec![("belgeler".to_string(), "kimlik".to_string())];
        assert!(missing_required_with_pending(&groups_without_pending, &pending).is_empty());
        assert!(satisfied_with_pending(&groups_without_pending, &pending));
    }

    #[test]
    fn enrich_with_meta_eslesmeyen_metadata_hicbir_seyi_bozmaz() {
        let mut groups = vec![test_group("belgeler", vec![test_item("kimlik", false)])];
        // Farklı grup + farklı item — hiçbiri "belgeler/kimlik" ile eşleşmiyor.
        let metas = vec![
            test_meta("baska-grup", "kimlik", "yanlis-grup.pdf"),
            test_meta("belgeler", "baska-item", "yanlis-item.pdf"),
        ];

        enrich_with_meta(&mut groups, &metas);

        let item = &groups[0].items[0];
        assert!(!item.uploaded); // depodan gelen değer aynen korunur
        assert!(item.filename.is_none());
        assert!(item.content_type.is_none());
        assert!(item.size_bytes.is_none());
        assert!(item.sha256.is_none());
        assert!(item.uploaded_at.is_none());
        assert!(item.version.is_none());
    }
}
