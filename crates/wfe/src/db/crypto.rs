//! Bağlantı secret'ları için AES-256-GCM şifreleme.
//! Anahtar env `DB_CONN_SECRET` (base64, 32 byte). Format: nonce(12) || ciphertext.
use aes_gcm::{aead::{Aead, KeyInit, OsRng, rand_core::RngCore}, Aes256Gcm, Key, Nonce};
use base64::{engine::general_purpose::STANDARD, Engine};

#[derive(Debug)]
pub enum CryptoError { NoKey, BadKey, Encrypt, Decrypt }

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
    let raw = STANDARD.decode(b64.trim()).map_err(|_| CryptoError::BadKey)?;
    if raw.len() != 32 { return Err(CryptoError::BadKey); }
    Ok(*Key::<Aes256Gcm>::from_slice(&raw))
}

/// Verilen anahtarla şifreler → nonce||ciphertext byte'ları.
pub fn encrypt_with(key_b64: &str, plaintext: &str) -> Result<Vec<u8>, CryptoError> {
    let cipher = Aes256Gcm::new(&key_from(key_b64)?);
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let ct = cipher.encrypt(Nonce::from_slice(&nonce), plaintext.as_bytes())
        .map_err(|_| CryptoError::Encrypt)?;
    let mut out = nonce.to_vec();
    out.extend_from_slice(&ct);
    Ok(out)
}

/// nonce||ciphertext → düz metin.
pub fn decrypt_with(key_b64: &str, data: &[u8]) -> Result<String, CryptoError> {
    if data.len() < 13 { return Err(CryptoError::Decrypt); }
    let cipher = Aes256Gcm::new(&key_from(key_b64)?);
    let (nonce, ct) = data.split_at(12);
    let pt = cipher.decrypt(Nonce::from_slice(nonce), ct).map_err(|_| CryptoError::Decrypt)?;
    String::from_utf8(pt).map_err(|_| CryptoError::Decrypt)
}

/// Env'den anahtar okuyan sarmalayıcılar.
pub fn encrypt(plaintext: &str) -> Result<Vec<u8>, CryptoError> {
    let k = std::env::var("DB_CONN_SECRET").map_err(|_| CryptoError::NoKey)?;
    encrypt_with(&k, plaintext)
}
pub fn decrypt(data: &[u8]) -> Result<String, CryptoError> {
    let k = std::env::var("DB_CONN_SECRET").map_err(|_| CryptoError::NoKey)?;
    decrypt_with(&k, data)
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
        assert!(matches!(encrypt_with("short", "x"), Err(CryptoError::BadKey)));
    }

    #[test]
    fn wrong_key_fails_decrypt() {
        let ct = encrypt_with(KEY, "x").unwrap();
        let other = "ZmVkY2JhOTg3NjU0MzIxMGZlZGNiYTk4NzY1NDMyMTA=";
        assert!(decrypt_with(other, &ct).is_err());
    }
}
