//! Bağlantı secret'ları ve `$env` secret değerleri için AES-256-GCM şifreleme.
//! Anahtar env `DB_CONN_SECRET` (base64, 32 byte). Format: nonce(12) || ciphertext.
//!
//! **Rotasyon:** `DB_CONN_SECRET` virgülle ayrılmış bir LİSTE olabilir. Şifreleme daima
//! listenin İLKİ ile yapılır, çözme listedeki tüm anahtarlar sırayla denenerek. Böylece yeni
//! anahtar başa eklenir, eski satırlar hâlâ çözülür, yeni yazılanlar yeni anahtarla şifrelenir.
//! Tek anahtarlı mevcut kurulumlar (tek elemanlı liste) hiç değişmeden çalışır.
//! GitLab'ın `db_key_base` dizisiyle aynı yaklaşım — tek fark, orada son eleman şifreler.
use aes_gcm::{
    aead::{rand_core::RngCore, Aead, KeyInit, OsRng},
    Aes256Gcm, Key, Nonce,
};
use base64::{engine::general_purpose::STANDARD, Engine};

#[derive(Debug)]
pub enum CryptoError {
    NoKey,
    BadKey,
    Encrypt,
    Decrypt,
}

impl std::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            CryptoError::NoKey => "DB_CONN_SECRET tanımlı değil",
            CryptoError::BadKey => "DB_CONN_SECRET geçersiz (base64 32 byte olmalı)",
            CryptoError::Encrypt => "şifreleme hatası",
            CryptoError::Decrypt => "çözme hatası (anahtar/veri uyumsuz)",
        };
        write!(f, "{s}")
    }
}
impl std::error::Error for CryptoError {}

fn key_from(b64: &str) -> Result<Key<Aes256Gcm>, CryptoError> {
    let raw = STANDARD
        .decode(b64.trim())
        .map_err(|_| CryptoError::BadKey)?;
    if raw.len() != 32 {
        return Err(CryptoError::BadKey);
    }
    Ok(*Key::<Aes256Gcm>::from_slice(&raw))
}

/// Verilen anahtarla şifreler → nonce||ciphertext byte'ları.
pub fn encrypt_with(key_b64: &str, plaintext: &str) -> Result<Vec<u8>, CryptoError> {
    let cipher = Aes256Gcm::new(&key_from(key_b64)?);
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), plaintext.as_bytes())
        .map_err(|_| CryptoError::Encrypt)?;
    let mut out = nonce.to_vec();
    out.extend_from_slice(&ct);
    Ok(out)
}

/// nonce||ciphertext → düz metin.
pub fn decrypt_with(key_b64: &str, data: &[u8]) -> Result<String, CryptoError> {
    if data.len() < 13 {
        return Err(CryptoError::Decrypt);
    }
    let cipher = Aes256Gcm::new(&key_from(key_b64)?);
    let (nonce, ct) = data.split_at(12);
    let pt = cipher
        .decrypt(Nonce::from_slice(nonce), ct)
        .map_err(|_| CryptoError::Decrypt)?;
    String::from_utf8(pt).map_err(|_| CryptoError::Decrypt)
}

/// `DB_CONN_SECRET`'ı virgülden ayırıp doğrulanmış anahtar listesi döner.
/// Listedeki HER anahtar baştan doğrulanır: rotasyon listesindeki bir tipo, çözme
/// denemelerinin arasında yutulup gizemli bir `Decrypt` hatası olarak değil, `BadKey`
/// olarak görünsün.
fn keys_from_env() -> Result<Vec<String>, CryptoError> {
    let raw = std::env::var("DB_CONN_SECRET").map_err(|_| CryptoError::NoKey)?;
    let keys: Vec<String> = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    if keys.is_empty() {
        return Err(CryptoError::NoKey);
    }
    for k in &keys {
        key_from(k)?;
    }
    Ok(keys)
}

/// Env'deki listenin İLK anahtarıyla şifreler.
pub fn encrypt(plaintext: &str) -> Result<Vec<u8>, CryptoError> {
    encrypt_with(&keys_from_env()?[0], plaintext)
}

/// Env'deki anahtarları sırayla deneyerek çözer; hiçbiri tutmazsa `Decrypt`.
pub fn decrypt(data: &[u8]) -> Result<String, CryptoError> {
    let keys = keys_from_env()?;
    for k in &keys {
        if let Ok(pt) = decrypt_with(k, data) {
            return Ok(pt);
        }
    }
    Err(CryptoError::Decrypt)
}

#[cfg(test)]
mod tests {
    use super::*;
    // 32 byte base64 (deterministik test anahtarı)
    const KEY: &str = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=";

    #[test]
    fn round_trip() {
        let ct = encrypt_with(KEY, "s3cret-pass").unwrap();
        assert_ne!(ct, b"s3cret-pass");
        assert_eq!(decrypt_with(KEY, &ct).unwrap(), "s3cret-pass");
    }

    #[test]
    fn nonce_randomizes_ciphertext() {
        let a = encrypt_with(KEY, "x").unwrap();
        let b = encrypt_with(KEY, "x").unwrap();
        assert_ne!(a, b); // aynı düz metin farklı ciphertext (rastgele nonce)
    }

    #[test]
    fn bad_key_rejected() {
        assert!(matches!(
            encrypt_with("short", "x"),
            Err(CryptoError::BadKey)
        ));
    }

    #[test]
    fn wrong_key_fails_decrypt() {
        let ct = encrypt_with(KEY, "x").unwrap();
        let other = "ZmVkY2JhOTg3NjU0MzIxMGZlZGNiYTk4NzY1NDMyMTA=";
        assert!(decrypt_with(other, &ct).is_err());
    }

    const NEW_KEY: &str = "ZmVkY2JhOTg3NjU0MzIxMGZlZGNiYTk4NzY1NDMyMTA=";

    /// Rotasyonun tamamı TEK testte: `DB_CONN_SECRET` süreç geneli bir değişken,
    /// ayrı testlere bölmek paralel koşumda yarış yaratırdı.
    #[test]
    fn env_key_rotation() {
        // Tek anahtar — mevcut kurulumların davranışı değişmez.
        std::env::set_var("DB_CONN_SECRET", KEY);
        let old_ct = encrypt("s3cret-pass").unwrap();
        assert_eq!(decrypt(&old_ct).unwrap(), "s3cret-pass");

        // Rotasyon: yeni anahtar başa eklenir. ESKİ anahtarla şifrelenmiş satır hâlâ çözülür.
        std::env::set_var("DB_CONN_SECRET", format!("{NEW_KEY},{KEY}"));
        assert_eq!(decrypt(&old_ct).unwrap(), "s3cret-pass");

        // ...ve yeni yazılanlar LİSTENİN İLKİYLE şifrelenir: eski anahtar tek başına çözemez.
        let new_ct = encrypt("taze").unwrap();
        assert_eq!(decrypt_with(NEW_KEY, &new_ct).unwrap(), "taze");
        assert!(decrypt_with(KEY, &new_ct).is_err());

        // Boşluklu liste kabul edilir (SOPS/YAML'den gelen değerler böyle olur).
        std::env::set_var("DB_CONN_SECRET", format!("{NEW_KEY} , {KEY}"));
        assert_eq!(decrypt(&old_ct).unwrap(), "s3cret-pass");

        // Eski anahtar listeden düşünce o satır artık çözülemez — Decrypt, BadKey değil.
        std::env::set_var("DB_CONN_SECRET", NEW_KEY);
        assert!(matches!(decrypt(&old_ct), Err(CryptoError::Decrypt)));

        // Listedeki tipo yutulmaz: BadKey olarak görünür, "çözemedim" diye değil.
        std::env::set_var("DB_CONN_SECRET", format!("{KEY},bozuk"));
        assert!(matches!(decrypt(&old_ct), Err(CryptoError::BadKey)));
        assert!(matches!(encrypt("x"), Err(CryptoError::BadKey)));

        // Tanımsız / yalnız ayraç → NoKey.
        std::env::set_var("DB_CONN_SECRET", " , ");
        assert!(matches!(encrypt("x"), Err(CryptoError::NoKey)));
        std::env::remove_var("DB_CONN_SECRET");
        assert!(matches!(encrypt("x"), Err(CryptoError::NoKey)));
    }
}
