//! TEK İSTEKLİ BAŞLATMADA ÇİFT WFE KORUMASI (2026-08-11, K6).
//!
//! Sorun: tek istekli akışta gövde (ek-belgeler dahil) büyük olabilir, istek uzun
//! sürer; süre uzadıkça timeout/bağlantı kopması ihtimali artar. En kötü senaryo:
//! WFE commit oldu ama cevap istemciye ulaşmadı, kullanıcı "Başlat"a tekrar bastı →
//! aynı başvuru İKİNCİ kez oluşur. Standart çözüm istemcinin `Idempotency-Key`
//! üretmesidir (Stripe/PayPal deseni) — bu tasarımda ALINMADI: "UI dosyaları ve
//! girdileri toplar, tek istek atar, başka hiçbir şey bilmez" sözüne aykırı düşer;
//! anahtar üretmeyen her istemci korumasız kalır ve bunu fark etmez.
//!
//! Bunun yerine anahtar **isteğin kendisinden türetilir** (`fingerprint`) —
//! istemciden hiçbir header gelmez. Aynı parmak izi `window_secs` (varsayılan 60 sn,
//! `WFE_START_DEDUPE_WINDOW_SECS`) içinde tekrar gelirse iş tekrar KOŞMAZ: ya ilk
//! sonuç aynen döner (`Claim::Replay`) ya da hâlâ koşuyorsa `409` verilir
//! (`Claim::InProgress`). Parmak izi YALNIZ `payload`tan türetilir (dosya
//! baytlarından değil) — karar dosyalar okunmadan/aktarılmadan önce verilebilsin diye.
//!
//! Defter `wf.wfe_start_dedupe` (bkz. `migrations/wf/20260811000001_wfe_start_dedupe.sql`).
//! Fiziksel temizlik ayrı bir TTL'dir (1 saat) ve mevcut saatlik süpürücüde yapılır
//! (`reservation::sweep`'in çağırdığı `sweep_expired`) — `window_secs` yalnız "bu
//! parmak izi hâlâ taze mi" sorusuna cevap verir, satırı fiziksel olarak silmez.

use crate::error::AppError;
use axum::http::StatusCode;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::collections::BTreeMap;
use uuid::Uuid;

/// Fiziksel süpürme eşiği. `window_secs`ten (tazelik penceresi, tipik 60 sn) AYRI
/// bir kavramdır: bir satır "bayat" sayılıp üzerine yazılabilir olsa da (window_secs
/// geçmiş) defterde bir süre daha durur — süpürücü turu gelene kadar. 1 saat, pencere
/// süresinin kat kat üstünde olduğu için canlı bir dedupe kaydını asla silmez.
const SWEEP_TTL_HOURS: i64 = 1;

/// JSON'u kanonik forma getirir: obje anahtarları SIRALANIR, dizi sırası KORUNUR.
///
/// Bu repo `serde_json`'ı `preserve_order` özelliğiyle kullanır (utoipa'nın şema
/// üretimi anahtar sırasını korumak ister) — yani `serde_json::Value::Object` normalde
/// eklenme sırasını taşır. Aynı `input`/`attachments` payload'ı farklı anahtar
/// sırasıyla (ör. farklı bir istemci kütüphanesi, farklı bir JSON serileştiricisi)
/// gelirse `serde_json::to_vec` farklı bayt dizisi üretir ve aynı isteğin özeti
/// FARKLI çıkar — dedupe sessizce delinir. `BTreeMap`e normalize edip yeniden
/// `Value`'ya çevirmek anahtarları sözlük sırasına sabitler; diziler (sırası
/// ANLAMLI olabilir, ör. `attachments` listesi) olduğu gibi bırakılır, yalnız
/// elemanları özyinelemeli olarak kanonikleştirilir.
fn canonicalize(v: &serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(map) => {
            let sorted: BTreeMap<String, serde_json::Value> = map
                .iter()
                .map(|(k, v)| (k.clone(), canonicalize(v)))
                .collect();
            // BTreeMap::into_iter() anahtar sırasına göre (sözlük sırası) yürür;
            // hedef `serde_json::Map` `preserve_order` altında bile bu EKLENME
            // sırasını aynen tutar — sonuç daima sıralı çıkar.
            serde_json::Value::Object(sorted.into_iter().collect())
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(canonicalize).collect())
        }
        other => other.clone(),
    }
}

/// İstekten türetilen parmak izi. İstemciden GELMEZ — `actor_user_id` sunucunun
/// doğruladığı aktörden, geri kalanı `POST /wfe` gövdesinden okunur. `action`
/// `Option`'dır: tek-node'lu akışlarda gövdede hiç geçmeyebilir.
pub fn fingerprint(
    actor_user_id: Uuid,
    wfd_id: Uuid,
    version: i32,
    action: Option<&str>,
    input: &serde_json::Value,
    attachments: &serde_json::Value,
) -> String {
    let canonical = serde_json::json!({
        "actor_user_id": actor_user_id,
        "wfd_id": wfd_id,
        "version": version,
        "action": action,
        "input": canonicalize(input),
        "attachments": canonicalize(attachments),
    });
    // Üstteki obje de `json!` ile eklenme sırasıyla kuruldu (sabit alan adları) ama
    // yine de `canonicalize` ile geçiriyoruz: tutarlılık ilkesi tek nokta — kanonik
    // olmayan bir yol bırakmayalım diye üst seviye de aynı fonksiyondan geçer.
    let bytes = serde_json::to_vec(&canonicalize(&canonical)).unwrap_or_default();
    sha256_hex(&bytes)
}

/// `claim` sonucu.
pub enum Claim {
    /// Satır bize ait: iş koşulabilir. Çağıran işi bitirince `complete`, hata
    /// alırsa `release` çağırmalı (aksi halde satır `InProgress` gibi takılı kalır).
    Fresh,
    /// Aynı istek `window_secs` içinde tamamlanmış: bu `wfe_id` aynen dönülmeli,
    /// iş TEKRAR KOŞULMAMALI.
    Replay(Uuid),
    /// Aynı istek şu an başka bir çağrı tarafından işleniyor → `409`.
    InProgress,
}

/// Parmak izini SAHİPLENMEYE çalışır.
///
/// Tek SQL ifadesiyle (`INSERT ... ON CONFLICT DO UPDATE ... WHERE`) atomik iddia:
/// fingerprint hiç yoksa satır eklenir. Varsa ve `created_at` yaşı `window_secs`i
/// GEÇMİŞSE `WHERE` koşulu sağlanır, satır üzerine yazılır (yeniden sahiplenme) —
/// bu durumda da `RETURNING` bir satır verir, ikisi de `Fresh` sayılır. Satır TAZEYSE
/// (pencere içindeyse) `WHERE` koşulu sağlanmadığı için Postgres `DO UPDATE`'i hiç
/// uygulamaz ve `RETURNING` BOŞ döner — bu, iki eşzamanlı isteğin de update'i
/// "kazanamayacağı", dolayısıyla yarışa GİRMEDİĞİ anlamına gelir: ikisi de aşağıdaki
/// salt-okuma dalına düşer, hiçbiri satırı değiştirmez.
///
/// Boş dönüşte mevcut satır ayrıca okunur (yarışsız, çünkü artık yazma yok):
/// `wfe_id` doluysa `Replay`, boşsa `InProgress`. Okuma ile bu fonksiyonun kendi
/// `INSERT` denemesi arasında satır silinmişse (ör. `release` az önce koştu) `None`
/// döner — bu son derece nadir bir yarış penceresidir, aynı deneme bir kez daha
/// yapılır (satır artık gerçekten yoksa bu sefer `INSERT` başarıyla `Fresh` sahiplenir).
pub async fn claim(
    pool: &PgPool,
    fingerprint: &str,
    actor_user_id: Uuid,
    window_secs: u64,
) -> Result<Claim, AppError> {
    loop {
        let claimed = sqlx::query(
            "INSERT INTO wf.wfe_start_dedupe (fingerprint, actor_user_id, wfe_id, created_at) \
               VALUES ($1, $2, NULL, now()) \
             ON CONFLICT (fingerprint) DO UPDATE \
               SET actor_user_id = EXCLUDED.actor_user_id, wfe_id = NULL, created_at = now() \
               WHERE wf.wfe_start_dedupe.created_at < now() - ($3 || ' seconds')::interval \
             RETURNING fingerprint",
        )
        .bind(fingerprint)
        .bind(actor_user_id)
        .bind(window_secs.to_string())
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

        if claimed.is_some() {
            return Ok(Claim::Fresh);
        }

        let existing: Option<Option<Uuid>> = sqlx::query_scalar(
            "SELECT wfe_id FROM wf.wfe_start_dedupe WHERE fingerprint = $1",
        )
        .bind(fingerprint)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

        match existing {
            Some(Some(wfe_id)) => return Ok(Claim::Replay(wfe_id)),
            Some(None) => return Ok(Claim::InProgress),
            // Satır aradaki anda silindi (bkz. doc yorumu) — tekrar dene.
            None => continue,
        }
    }
}

/// İş başarıyla bitince satırı sonuç `wfe_id` ile işaretler (`Replay`in okuyacağı
/// alan budur). Satırı SİLMEZ — `window_secs` içinde gelecek tekrar isteği bu satırı
/// bulup `Replay` dönebilsin diye.
pub async fn complete(pool: &PgPool, fingerprint: &str, wfe_id: Uuid) -> Result<(), AppError> {
    sqlx::query("UPDATE wf.wfe_start_dedupe SET wfe_id = $2 WHERE fingerprint = $1")
        .bind(fingerprint)
        .bind(wfe_id)
        .execute(pool)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    Ok(())
}

/// Hata yolunda satırı SİLER — aynı payload'la tekrar denenebilsin diye
/// (`reservation::release`in dedupe karşılığı; K4 rollback'inin parçası).
pub async fn release(pool: &PgPool, fingerprint: &str) -> Result<(), AppError> {
    sqlx::query("DELETE FROM wf.wfe_start_dedupe WHERE fingerprint = $1")
        .bind(fingerprint)
        .execute(pool)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    Ok(())
}

/// 1 saatten eski satırları temizler (`reservation::sweep`'in çağırdığı saatlik
/// süpürücü turu — bkz. `SWEEP_TTL_HOURS`). `window_secs` tazelik sorusuna cevap
/// verir, bu fonksiyon fiziksel disk/defter temizliğine cevap verir; ikisi ayrı
/// eksenlerdir (bir satır bayatlayıp üzerine yazılabilir olsa da süpürülene kadar
/// defterde durur).
pub async fn sweep_expired(pool: &PgPool) -> Result<u64, AppError> {
    let result = sqlx::query(
        "DELETE FROM wf.wfe_start_dedupe WHERE created_at < now() - ($1 || ' hours')::interval",
    )
    .bind(SWEEP_TTL_HOURS.to_string())
    .execute(pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    Ok(result.rows_affected())
}

/// `sha2` bu crate'te zaten bağımlılık (`attachments.rs`'in dosya bütünlüğü
/// denetiminde de aynı crate kullanılıyor, bkz. `Sha256Stream`) — burada da aynı
/// deseni sürdürüyoruz, elle SHA-256 yazmaya gerek yok.
fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    format!("{digest:x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sha256_matches_known_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn fingerprint_is_stable_across_key_order() {
        let actor = Uuid::nil();
        let wfd = Uuid::nil();
        let a = json!({"applicant": {"name": "a", "age": 30}});
        let b = json!({"age": 30, "applicant": {"age": 30, "name": "a"}});
        // Bilerek "farklı" iki obje değil — aynı anlamı taşıyan, anahtar sırası
        // değişik iki payload karşılaştırılıyor: `applicant` içeriği aynı, dış
        // seviyedeki anahtar sırası ters. Kanonikleştirme sonrası eşit olmalı.
        let a2 = json!({"applicant": {"age": 30, "name": "a"}});
        assert_eq!(
            fingerprint(actor, wfd, 1, Some("start"), &a, &json!([])),
            fingerprint(actor, wfd, 1, Some("start"), &a2, &json!([]))
        );
        // Kontrol: gerçekten farklı payload farklı özet üretmeli.
        assert_ne!(
            fingerprint(actor, wfd, 1, Some("start"), &a, &json!([])),
            fingerprint(actor, wfd, 1, Some("start"), &b, &json!([]))
        );
    }

    #[test]
    fn canonicalize_sorts_object_keys_but_keeps_array_order() {
        let v = json!({"b": 1, "a": [3, 2, 1]});
        let c = canonicalize(&v);
        let s = serde_json::to_string(&c).unwrap();
        assert_eq!(s, r#"{"a":[3,2,1],"b":1}"#);
    }
}
