//! Tenant kapsamlı SALT OKUMA API anahtarı (`/ext` ağacı).
//!
//! Neden küresel `ADMIN_API_KEY` değil: o anahtar TÜM tenant'larda tam org YAZMA
//! yetkisi verir ve staging'de tanımsız olduğu için uç tamamen açık olurdu. Bu
//! anahtar tek tenant'a bağlıdır ve yalnız okuma rotalarında geçerlidir — yetki
//! sınırı YAPI gereğidir (ayrı router), bayrak değil.
//!
//! Biçim: `agp_<prefix:8>_<secret:32>`. `prefix` DB'de düz durur (lookup anahtarı),
//! `secret` yalnız SHA-256 özeti olarak saklanır ve düz metin sadece yaratılışta
//! bir kez döner.

use crate::{error::AppError, state::AppState};
use axum::{
    async_trait,
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const SCHEME: &str = "agp";
const PREFIX_LEN: usize = 8;
const SECRET_LEN: usize = 32;

/// Yeni üretilmiş anahtar. `plaintext` çağırana BİR KEZ döner, saklanmaz.
#[derive(Debug, Clone)]
pub struct GeneratedKey {
    pub plaintext: String,
    pub prefix: String,
    pub key_hash: String,
}

/// Ayrıştırılmış istek başlığı.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedKey {
    pub prefix: String,
    pub secret: String,
}

pub fn generate() -> GeneratedKey {
    let prefix = random_token(PREFIX_LEN);
    let secret = random_token(SECRET_LEN);
    GeneratedKey {
        plaintext: format!("{SCHEME}_{prefix}_{secret}"),
        key_hash: hash_secret(&secret),
        prefix,
    }
}

/// Başlığı ayrıştırır. Biçim/uzunluk tutmuyorsa `None` — kısaltılmış bir secret
/// ile DB'ye gitmek hem boşa iş hem gevşek bir kapı.
pub fn parse(raw: &str) -> Option<ParsedKey> {
    let mut parts = raw.trim().split('_');
    let scheme = parts.next()?;
    let prefix = parts.next()?;
    let secret = parts.next()?;
    if parts.next().is_some()
        || scheme != SCHEME
        || prefix.len() != PREFIX_LEN
        || secret.len() != SECRET_LEN
        || !prefix.bytes().all(|b| b.is_ascii_alphanumeric())
        || !secret.bytes().all(|b| b.is_ascii_alphanumeric())
    {
        return None;
    }
    Some(ParsedKey {
        prefix: prefix.to_string(),
        secret: secret.to_string(),
    })
}

pub fn hash_secret(secret: &str) -> String {
    let digest = Sha256::digest(secret.as_bytes());
    digest.iter().fold(String::with_capacity(64), |mut acc, b| {
        use std::fmt::Write;
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

/// Sabit zamanlı karşılaştırma: erken `return` bir zamanlama kanalı açar.
/// (Karşılaştırılan şey bir özet olduğu için sömürülmesi zor, ama üç satırlık
/// katlama bedelsiz.)
pub fn verify_hash(secret: &str, expected_hash: &str) -> bool {
    let actual = hash_secret(secret);
    if actual.len() != expected_hash.len() {
        return false;
    }
    actual
        .bytes()
        .zip(expected_hash.bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

/// `_` AYIRICIDIR, jetonun içinde geçemez — alfanümerik alfabe bunu garanti eder.
fn random_token(len: usize) -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
        .collect()
}

/// `/ext` rotalarının aktörü: hangi tenant adına okuma yapıldığı.
///
/// Anahtarın kendi id'sini TAŞIMAZ: rotaların hiçbiri onu kullanmıyor ve kullanılmayan
/// alan "bir gün lazım olur" borcudur. Denetim izi gerekirse `last_used_at` ve
/// `touch_api_key` zaten hangi anahtarın çalıştığını biliyor.
#[derive(Debug, Clone)]
pub struct ApiKeyActor {
    pub orgtnt_id: Uuid,
}

#[async_trait]
impl FromRequestParts<AppState> for ApiKeyActor {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let raw = parts
            .headers
            .get("X-Api-Key")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| invalid())?;
        let parsed = parse(raw).ok_or_else(invalid)?;

        let row = wf_org::repo::permission::api_key_by_prefix(&state.pool, &parsed.prefix)
            .await
            .map_err(|_| invalid())?
            .ok_or_else(invalid)?;

        // Hash tutmuyorsa, kapalıysa ya da süresi geçmişse AYNI cevap: hangi
        // koşulun düştüğü sızmaz.
        if !row.is_active
            || row.expires_at.is_some_and(|e| e <= chrono::Utc::now())
            || !verify_hash(&parsed.secret, &row.key_hash)
        {
            return Err(invalid());
        }

        // Best-effort: `check` ucu okuma yolundadır, yazma hatası isteği düşürmez.
        if let Err(e) = wf_org::repo::permission::touch_api_key(&state.pool, row.key_id).await {
            tracing::warn!("api key last_used_at güncellenemedi: {e}");
        }

        Ok(ApiKeyActor {
            orgtnt_id: row.orgtnt_id,
        })
    }
}

fn invalid() -> AppError {
    AppError {
        message: "X-Api-Key geçersiz".into(),
        status: StatusCode::UNAUTHORIZED,
        code: Some("api_key.invalid"),
        items: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Üretilen anahtar kendi önekine ayrışır ve kendi özetini doğrular.
    #[test]
    fn generated_key_parses_and_verifies() {
        let key = generate();
        let parsed = parse(&key.plaintext).expect("üretilen anahtar ayrışmalı");
        assert_eq!(parsed.prefix, key.prefix);
        assert!(verify_hash(&parsed.secret, &key.key_hash));
    }

    /// Düz metin DB'ye yazılmaz: özet düz metinden farklıdır.
    #[test]
    fn hash_is_not_the_plaintext() {
        let key = generate();
        assert!(!key.plaintext.contains(&key.key_hash));
        assert_eq!(key.key_hash.len(), 64, "SHA-256 hex");
    }

    /// Yanlış secret doğrulanmaz.
    #[test]
    fn wrong_secret_fails_verification() {
        let key = generate();
        assert!(!verify_hash("x".repeat(SECRET_LEN).as_str(), &key.key_hash));
    }

    /// İki üretim aynı anahtarı vermez (önek de secret de rastgele).
    #[test]
    fn two_generated_keys_differ() {
        let a = generate();
        let b = generate();
        assert_ne!(a.prefix, b.prefix);
        assert_ne!(a.plaintext, b.plaintext);
        assert_ne!(a.key_hash, b.key_hash);
    }

    /// Aynı secret aynı özeti verir (lookup çalışsın).
    #[test]
    fn hash_is_deterministic() {
        assert_eq!(hash_secret("abc"), hash_secret("abc"));
        assert_ne!(hash_secret("abc"), hash_secret("abd"));
    }

    /// Şema öneki olmayan başlık reddedilir — başka bir sistemin anahtarı
    /// kazayla kabul edilmesin.
    #[test]
    fn parse_rejects_foreign_scheme() {
        let key = generate();
        let foreign = key.plaintext.replacen(SCHEME, "xyz", 1);
        assert!(parse(&foreign).is_none());
    }

    /// Eksik/fazla bölüm reddedilir.
    #[test]
    fn parse_rejects_malformed_segments() {
        assert!(parse("agp_short").is_none());
        assert!(parse("agp__").is_none());
        assert!(parse("").is_none());
        assert!(parse("agp_aaaaaaaa_bbb_ccc").is_none());
    }

    /// Yanlış uzunlukta bölüm reddedilir: kısaltılmış bir secret ile DB'ye
    /// gitmek anlamsız iş ve gevşek bir kapıdır.
    #[test]
    fn parse_rejects_wrong_lengths() {
        let short_prefix = format!("agp_{}_{}", "a".repeat(4), "b".repeat(SECRET_LEN));
        let short_secret = format!("agp_{}_{}", "a".repeat(PREFIX_LEN), "b".repeat(8));
        assert!(parse(&short_prefix).is_none());
        assert!(parse(&short_secret).is_none());
    }

    /// Ayrıştırma boşlukları kırpar — kopyala/yapıştır kaynaklı baştaki/sondaki
    /// boşluk yüzünden geçerli anahtar reddedilmesin.
    #[test]
    fn parse_trims_surrounding_whitespace() {
        let key = generate();
        let padded = format!("  {}\t", key.plaintext);
        assert_eq!(parse(&padded).map(|p| p.prefix), Some(key.prefix));
    }
}
