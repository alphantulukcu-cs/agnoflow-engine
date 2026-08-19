//! Not defteri KURALLARI — saf. Not motorun ne context'ine (`$ctx`) ne defterine
//! (`$wfah`) yazılır (K1); burada yalnız "gövde ne kadar uzun olabilir",
//! "ad-hoc dosya limitleri ne", "hangi MIME yasak" ve "notu kim görür" (audience)
//! soruları yaşar.
//!
//! NEDEN BU CRATE: `wf_server::notes` (gerçek akış, DB) ile `wf_wfe::sim`
//! (simülasyon/senaryo, in-memory) AYNI limitleri uygulamalı — yoksa senaryoda
//! geçen bir not portalda 413'e düşerdi. `wf_server::notes` bu sabitleri
//! `pub use` ile yeniden ihraç eder; oradaki çağrı yerleri değişmedi.
//! `wfe-core`'a KONMADI: motor not katmanından habersiz kalır (K1) — burası
//! adapter crate'i, `sim`in de yaşadığı yer.
//!
//! Burada OLMAYAN: DB satırları, draft/publish yaşam döngüsü, okundu takibi,
//! `Content-Disposition`/başlık kodlaması (`wf_server::notes::decode_filename`).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Not gövdesi uzunluk sınırı (karakter). Aşımı `400`.
pub const MAX_BODY_LEN: usize = 10_000;

/// Ad-hoc dosya başı boyut sınırı (bayt) — aşımı `413 note.too_large`.
pub const MAX_FILE_BYTES: i64 = 25 * 1024 * 1024;

/// Not başına dosya sayısı sınırı — aşımı `422`.
pub const MAX_FILES_PER_NOTE: i64 = 10;

/// WFE başına TÜM notların dosyalarının toplam boyutu — aşımı `422`.
pub const MAX_WFE_QUOTA_BYTES: i64 = 200 * 1024 * 1024;

/// Dosya adı uzunluk sınırı (karakter).
pub const MAX_FILENAME_LEN: usize = 255;

/// Çalıştırılabilir/tehlikeli MIME blocklist — ad-hoc not dosyasında YASAK
/// (`415 note.unsupported_type`). Katalogun `formats` allowlist'inin aksine burada
/// allowlist yok (herhangi bir belge/görsel/ofis dosyası serbest); yalnız bilinen
/// çalıştırılabilir tipler reddedilir.
pub const MIME_BLOCKLIST: &[&str] = &[
    "application/x-msdownload",
    "application/x-executable",
    "application/x-sh",
    "application/x-bat",
    "application/x-msdos-program",
    "application/vnd.microsoft.portable-executable",
    "application/java-archive",
    "application/x-elf",
    "application/vnd.apple.installer+xml",
];

/// MIME blocklist kontrolü — `Content-Type`'ın parametre kısmı (`; charset=...`)
/// yok sayılır, karşılaştırma büyük/küçük harf duyarsızdır.
pub fn is_blocked_mime(mime: &str) -> bool {
    let m = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    MIME_BLOCKLIST.iter().any(|b| *b == m)
}

/// Not hedefleme (K9). `{"kind":"all"}` (varsayılan) → WFE'yi görebilen herkes;
/// `{"kind":"users","ids":[...]}` → yalnız listelenen `user_id`'ler (+ notun yazarı).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Audience {
    /// Varsayılan — WFE'yi görebilen herkes.
    #[default]
    All,
    Users { ids: Vec<Uuid> },
}

/// Ad-hoc bir not dosyasının kural açısından TAŞIDIĞI her şey — baytlar değil.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NoteFileSpec {
    pub filename: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default)]
    pub size_bytes: i64,
}

/// Not doğrulama reddi — route katmanı HTTP statüsüne çevirir.
#[derive(Debug, PartialEq)]
pub enum NoteReject {
    /// Gövde boş (yalnız boşluk) — not içeriksiz olamaz.
    EmptyBody,
    /// Gövde `MAX_BODY_LEN` karakteri aştı (400).
    BodyTooLong { len: usize, max: usize },
    /// Not başına dosya sayısı aşıldı (422).
    TooManyFiles { count: usize, max: i64 },
    /// Tek dosya `MAX_FILE_BYTES`'ı aştı (413).
    FileTooLarge { filename: String, max_bytes: i64 },
    /// WFE kotası aşıldı (422).
    QuotaExceeded { total: i64, max: i64 },
    /// Çalıştırılabilir MIME (415).
    BlockedMime { filename: String, mime: String },
}

impl std::fmt::Display for NoteReject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyBody => write!(f, "not gövdesi boş olamaz"),
            Self::BodyTooLong { len, max } => {
                write!(f, "not gövdesi çok uzun ({len} karakter, sınır {max})")
            }
            Self::TooManyFiles { count, max } => {
                write!(f, "not başına en çok {max} dosya eklenebilir ({count} verildi)")
            }
            Self::FileTooLarge {
                filename,
                max_bytes,
            } => write!(
                f,
                "\"{filename}\" çok büyük (sınır {} MB)",
                max_bytes / 1024 / 1024
            ),
            Self::QuotaExceeded { total, max } => write!(
                f,
                "WFE not dosyası kotası aşıldı ({} MB, sınır {} MB)",
                total / 1024 / 1024,
                max / 1024 / 1024
            ),
            Self::BlockedMime { filename, mime } => {
                write!(f, "\"{filename}\": çalıştırılabilir dosya tipi yasak ({mime})")
            }
        }
    }
}

/// Bir notun (gövde + ad-hoc dosyaları) limitlere uygunluğunu denetler.
/// `already_used_bytes`: aynı WFE'de ÖNCEDEN yüklenmiş not dosyalarının toplamı —
/// kota bu notun dosyalarıyla birlikte sorgulanır.
pub fn check_note(
    body: &str,
    files: &[NoteFileSpec],
    already_used_bytes: i64,
) -> Result<(), NoteReject> {
    if body.trim().is_empty() {
        return Err(NoteReject::EmptyBody);
    }
    let len = body.chars().count();
    if len > MAX_BODY_LEN {
        return Err(NoteReject::BodyTooLong {
            len,
            max: MAX_BODY_LEN,
        });
    }
    if files.len() as i64 > MAX_FILES_PER_NOTE {
        return Err(NoteReject::TooManyFiles {
            count: files.len(),
            max: MAX_FILES_PER_NOTE,
        });
    }
    for f in files {
        if f.size_bytes > MAX_FILE_BYTES {
            return Err(NoteReject::FileTooLarge {
                filename: f.filename.clone(),
                max_bytes: MAX_FILE_BYTES,
            });
        }
        if let Some(ct) = &f.content_type {
            if is_blocked_mime(ct) {
                return Err(NoteReject::BlockedMime {
                    filename: f.filename.clone(),
                    mime: ct.clone(),
                });
            }
        }
    }
    let total = already_used_bytes + files.iter().map(|f| f.size_bytes).sum::<i64>();
    if total > MAX_WFE_QUOTA_BYTES {
        return Err(NoteReject::QuotaExceeded {
            total,
            max: MAX_WFE_QUOTA_BYTES,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, size: i64) -> NoteFileSpec {
        NoteFileSpec {
            filename: name.into(),
            content_type: Some("application/pdf".into()),
            size_bytes: size,
        }
    }

    #[test]
    fn plain_note_passes() {
        assert!(check_note("gözden geçirildi", &[], 0).is_ok());
    }

    #[test]
    fn empty_body_is_rejected() {
        assert_eq!(check_note("   ", &[], 0), Err(NoteReject::EmptyBody));
    }

    #[test]
    fn body_length_is_counted_in_chars_not_bytes() {
        // "ş" iki bayt — sınır KARAKTER cinsindendir, aksi halde Türkçe not erken kesilirdi.
        let body: String = std::iter::repeat_n('ş', MAX_BODY_LEN).collect();
        assert!(check_note(&body, &[], 0).is_ok());
        let too_long: String = std::iter::repeat_n('ş', MAX_BODY_LEN + 1).collect();
        assert!(matches!(
            check_note(&too_long, &[], 0),
            Err(NoteReject::BodyTooLong { .. })
        ));
    }

    #[test]
    fn file_count_and_size_limits() {
        let many: Vec<_> = (0..(MAX_FILES_PER_NOTE + 1))
            .map(|i| file(&format!("f{i}.pdf"), 10))
            .collect();
        assert!(matches!(
            check_note("x", &many, 0),
            Err(NoteReject::TooManyFiles { .. })
        ));
        assert!(matches!(
            check_note("x", &[file("big.pdf", MAX_FILE_BYTES + 1)], 0),
            Err(NoteReject::FileTooLarge { .. })
        ));
    }

    #[test]
    fn quota_counts_previously_used_bytes() {
        let f = file("a.pdf", 10 * 1024 * 1024);
        assert!(check_note("x", &[f.clone()], 0).is_ok());
        assert!(matches!(
            check_note("x", &[f], MAX_WFE_QUOTA_BYTES),
            Err(NoteReject::QuotaExceeded { .. })
        ));
    }

    #[test]
    fn executable_mime_is_blocked_with_parameters_ignored() {
        let f = NoteFileSpec {
            filename: "kur.sh".into(),
            content_type: Some("application/x-sh; charset=utf-8".into()),
            size_bytes: 10,
        };
        assert!(matches!(
            check_note("x", &[f], 0),
            Err(NoteReject::BlockedMime { .. })
        ));
    }

    #[test]
    fn audience_defaults_to_all() {
        assert_eq!(Audience::default(), Audience::All);
        let parsed: Audience = serde_json::from_str(r#"{"kind":"all"}"#).unwrap();
        assert_eq!(parsed, Audience::All);
    }
}
