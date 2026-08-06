//! Ortam konfigürasyonu (`$env`) — tasarım: `docs/superpowers/specs/2026-08-04-env-config-design.md`.
//!
//! Bir WFD bir kez tasarlanır ve şirketin farklı ortamlarında koşar. Ortama göre değişen
//! değerler (`$env.AUTH_API`, `$env.MONGO_HOST`, API anahtarları) WFD dokümanının DIŞINDA
//! durur; koşum ortamı WFE başlatılırken sabitlenir ve örnek ömrü boyunca değişmez.
//!
//! Bu modülün iki işi var: (1) çözülmüş değişken kümesini taşımak, (2) `$env.KEY`
//! referanslarını string'lerde çözmek.
//!
//! # `$env`, ara-değer çözülen TEK namespace'tir
//!
//! `"$env.AUTH_API/v1/users"` çalışır; `"$ctx.foo/bar"` çalışmaz. Bu asimetri bilinçlidir:
//!
//! 1. Anahtar karakter kümesi `[A-Z][A-Z0-9_]*` olduğu için token sınırı tartışmasızdır —
//!    ilk küçük harf / `/` / `:` karakterinde biter, ayrıştırma belirsizliği yoktur.
//! 2. `$env` değerleri her zaman skalerdir (string/number/boolean), `$ctx` gibi obje ya da
//!    dizi olamaz — "tam eşleşme mi enterpolasyon mu" tip çelişkisi doğmaz.
//!
//! # Eksik anahtar HATADIR, `null` değil
//!
//! `$ctx.X` eksikse null okur (motorun yerleşik kuralı). `$env` bunun istisnasıdır: null bir
//! domain `https://null/v1/users` üretir ya da daha kötüsü yanlış bir hosta gider. Validator
//! eksik anahtarı publish anında yakalar; runtime hatası son savunma hattıdır.
//!
//! # Secret'lar tip düzeyinde ayrılır
//!
//! [`EnvSet`] secret'lar DAHİL her şeyi taşır ve YALNIZ autoexec config çözümünde kullanılır.
//! ZEN ifadeleri ve `wfes_effects` yalnız [`PublicEnv`] görür — [`EnvSet::public`] ile
//! türetilen, secret'ları hiç içermeyen kopya. Secret bir değer ctx'e yazılamıyorsa portalda
//! görünemez ve `$exec` üzerinden sızamaz; maskeleme böylece çalışma zamanı kontrolüyle değil
//! **inşa yoluyla** sağlanır.

use crate::error::EngineError;
use serde_json::Value;
use std::collections::BTreeMap;

/// Tek bir ortam değişkeni.
#[derive(Debug, Clone, PartialEq)]
pub struct EnvValue {
    /// Daima skaler: `String`, `Number` ya da `Bool`.
    pub value: Value,
    pub is_secret: bool,
}

impl EnvValue {
    pub fn public(value: Value) -> Self {
        Self {
            value,
            is_secret: false,
        }
    }
    pub fn secret(value: Value) -> Self {
        Self {
            value,
            is_secret: true,
        }
    }
}

/// Bir koşum ortamının çözülmüş değişkenleri — **secret'lar dahil**.
///
/// Yalnız autoexec config çözüm yolunda (`AutoexecDef.config`, `db_connection` alanları)
/// kullanılır. ZEN ve effects için [`EnvSet::public`] ile daraltılmış görünüm verilir.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EnvSet {
    vars: BTreeMap<String, EnvValue>,
}

/// Secret İÇERMEYEN değişken görünümü. `EvalEnv` ve `EffectEnv` yalnız bunu kabul eder;
/// secret'ın ctx'e sızması bu tip ayrımıyla imkânsız kılınır.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PublicEnv(EnvSet);

impl EnvSet {
    pub fn new(vars: BTreeMap<String, EnvValue>) -> Self {
        Self { vars }
    }

    pub fn is_empty(&self) -> bool {
        self.vars.is_empty()
    }

    pub fn get(&self, key: &str) -> Option<&EnvValue> {
        self.vars.get(key)
    }

    /// Secret'ları ELEYEN görünüm — ZEN ve effects buna bakar.
    pub fn public(&self) -> PublicEnv {
        PublicEnv(Self {
            vars: self
                .vars
                .iter()
                .filter(|(_, v)| !v.is_secret)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        })
    }

    /// Maskeleme için: çözülmüş secret değerlerin string biçimleri. `/autoexec/test`
    /// yanıtında, `resolved_config()`'te ve hata metinlerinde `[MASKED]` ile değiştirilir.
    pub fn secret_strings(&self) -> Vec<String> {
        self.vars
            .values()
            .filter(|v| v.is_secret)
            .map(|v| scalar_to_string(&v.value))
            .filter(|s| !s.is_empty())
            .collect()
    }
}

/// Boş ortam — `$env` kullanmayan bağlamların ödünç alabileceği sabit.
/// `EffectEnv` env'i referansla tutar; her kurulum noktasında geçici bir `Default`
/// üretmek ödünç ömrü hatası verirdi. `const` DEĞİL `static`: `const` her kullanımda
/// geçici bir değer olarak açılır ve referansı fonksiyondan dışarı taşınamaz.
pub static EMPTY_PUBLIC_ENV: PublicEnv = PublicEnv(EnvSet {
    vars: BTreeMap::new(),
});

impl PublicEnv {
    pub fn get(&self, key: &str) -> Option<&EnvValue> {
        self.0.get(key)
    }

    /// ZEN context'ine konacak `$env` objesi.
    pub fn to_json(&self) -> Value {
        Value::Object(
            self.0
                .vars
                .iter()
                .map(|(k, v)| (k.clone(), v.value.clone()))
                .collect(),
        )
    }

    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.0.vars.keys()
    }
}

/// Bir koşumun ortam bağlamı: tam küme + secret'sız görünüm.
///
/// `Engine` bunu SAHİPLİ tutar ve `Default` boş bir ortamdır — `$env` kullanmayan WFD'ler
/// (bugünkü tüm fixture'lar dahil) hiçbir şey vermek zorunda kalmasın. Public görünüm
/// kurulumda BİR KEZ türetilir, kullanım başına değil.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunEnv {
    full: EnvSet,
    public: PublicEnv,
}

impl RunEnv {
    pub fn new(full: EnvSet) -> Self {
        let public = full.public();
        Self { full, public }
    }

    /// Secret'lar DAHİL — yalnız autoexec config / db_connection alan çözümü.
    pub fn full(&self) -> &EnvSet {
        &self.full
    }

    /// Secret'sız görünüm — ZEN ifadeleri ve `wfes_effects`.
    pub fn public(&self) -> &PublicEnv {
        &self.public
    }
}

/// `$env.KEY` referanslarını çözebilen kaynak. `EnvSet` ve `PublicEnv` ortak yüzeyi.
pub trait EnvLookup {
    fn lookup(&self, key: &str) -> Option<&EnvValue>;
}

impl EnvLookup for EnvSet {
    fn lookup(&self, key: &str) -> Option<&EnvValue> {
        self.get(key)
    }
}
impl EnvLookup for PublicEnv {
    fn lookup(&self, key: &str) -> Option<&EnvValue> {
        self.get(key)
    }
}

const PREFIX: &str = "$env.";

/// Bir string'de `$env.` geçiyor mu? Çözüm yollarının ucuz ön elemesi.
pub fn contains_ref(s: &str) -> bool {
    s.contains(PREFIX)
}

/// `$env.` önekinden SONRAKİ anahtarı okur. Anahtar `[A-Z][A-Z0-9_]*`; ilk uymayan
/// karakterde biter. Geçerli bir anahtar yoksa `None` (çağıran bunu hata yapar).
fn read_key(rest: &str) -> Option<&str> {
    let end = rest
        .find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
        .unwrap_or(rest.len());
    let key = &rest[..end];
    match key.chars().next() {
        Some(c) if c.is_ascii_uppercase() => Some(key),
        _ => None,
    }
}

/// Bir string'deki tüm `$env.KEY` referanslarını toplar (validator ve eval ön-kontrolü için).
/// Bozuk referans (`$env.foo`, `$env.` gibi) `Err` döner — sessizce atlanmaz, çünkü bu
/// neredeyse her zaman bir tipodur ve runtime'da düz metin olarak dışarı sızardı.
pub fn references(s: &str) -> Result<Vec<String>, EngineError> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(at) = rest.find(PREFIX) {
        let after = &rest[at + PREFIX.len()..];
        match read_key(after) {
            Some(key) => {
                out.push(key.to_string());
                rest = &after[key.len()..];
            }
            None => {
                let snippet: String = after.chars().take(20).collect();
                return Err(EngineError::InvalidExpression(format!(
                    "geçersiz $env referansı '$env.{snippet}' — anahtar [A-Z][A-Z0-9_]* olmalı"
                )));
            }
        }
    }
    Ok(out)
}

/// Bir string'i çözer.
///
/// * String'in TAMAMI `$env.KEY` ise → değişkenin **tipli** değeri (`number`/`boolean` korunur).
/// * İçinde geçiyorsa → tüm referanslar string'e çevrilip yerine konur, sonuç `Value::String`.
/// * Hiç geçmiyorsa → `Ok(None)`; çağıran string'i kendi kurallarıyla işlemeye devam eder.
///
/// Tanımsız anahtar `Err`'dir (modül başlığındaki gerekçe).
pub fn resolve_string(s: &str, env: &impl EnvLookup) -> Result<Option<Value>, EngineError> {
    if !contains_ref(s) {
        return Ok(None);
    }

    // Tam eşleşme: tipi koru. "$env.TIMEOUT_MS" sayı döner, "$env.TIMEOUT_MSX" değil —
    // read_key'in okuduğu anahtar string'in geri kalanını tamamen tüketmeli.
    if let Some(after) = s.strip_prefix(PREFIX) {
        if let Some(key) = read_key(after) {
            if key.len() == after.len() {
                return get(env, key).map(|v| Some(v.value.clone()));
            }
        }
    }

    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find(PREFIX) {
        out.push_str(&rest[..at]);
        let after = &rest[at + PREFIX.len()..];
        let Some(key) = read_key(after) else {
            let snippet: String = after.chars().take(20).collect();
            return Err(EngineError::InvalidExpression(format!(
                "geçersiz $env referansı '$env.{snippet}' — anahtar [A-Z][A-Z0-9_]* olmalı"
            )));
        };
        out.push_str(&scalar_to_string(&get(env, key)?.value));
        rest = &after[key.len()..];
    }
    out.push_str(rest);
    Ok(Some(Value::String(out)))
}

/// İfade değerlendirmesi öncesi ön-kontrol. ZEN'de eksik alan `null` okur; `$env` için bunu
/// kabul edemeyiz (modül başlığı), ama ZEN'in attribute erişimine giremeyiz — o yüzden
/// ifadenin METNİ taranır ve tanımsız anahtar değerlendirmeden ÖNCE hata verir.
pub fn assert_refs_defined(expr: &str, env: &impl EnvLookup) -> Result<(), EngineError> {
    for key in references(expr)? {
        get(env, &key)?;
    }
    Ok(())
}

fn get<'a>(env: &'a impl EnvLookup, key: &str) -> Result<&'a EnvValue, EngineError> {
    env.lookup(key).ok_or_else(|| {
        EngineError::InvalidExpression(format!(
            "$env.{key} bu ortamda tanımlı değil (secret ise: draft/simulate koşumunda \
             secret'lar çözülmez)"
        ))
    })
}

/// Enterpolasyonda kullanılan string biçimi. `Value::String` tırnaksız gider — aksi hâlde
/// URL'ye tırnak karışırdı; sayı/boolean JSON biçimini alır.
fn scalar_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn env() -> EnvSet {
        EnvSet::new(BTreeMap::from([
            (
                "AUTH_API".to_string(),
                EnvValue::public(json!("https://auth.test.cs.com.tr")),
            ),
            ("TIMEOUT_MS".to_string(), EnvValue::public(json!(5000))),
            ("DEBUG".to_string(), EnvValue::public(json!(true))),
            (
                "MONGO_PW".to_string(),
                EnvValue::secret(json!("s3cret-pass")),
            ),
        ]))
    }

    #[test]
    fn whole_string_preserves_type() {
        let e = env();
        assert_eq!(
            resolve_string("$env.TIMEOUT_MS", &e).unwrap(),
            Some(json!(5000))
        );
        assert_eq!(resolve_string("$env.DEBUG", &e).unwrap(), Some(json!(true)));
        assert_eq!(
            resolve_string("$env.AUTH_API", &e).unwrap(),
            Some(json!("https://auth.test.cs.com.tr"))
        );
    }

    /// Asıl vaka: URL öneki. Tam eşleşme olmayan her şey string'e düşer.
    #[test]
    fn interpolation() {
        let e = env();
        assert_eq!(
            resolve_string("$env.AUTH_API/v1/users", &e).unwrap(),
            Some(json!("https://auth.test.cs.com.tr/v1/users"))
        );
        assert_eq!(
            resolve_string("$env.AUTH_API/t/$env.TIMEOUT_MS", &e).unwrap(),
            Some(json!("https://auth.test.cs.com.tr/t/5000"))
        );
        // Sayı enterpolasyonu tırnaksız; string de tırnaksız.
        assert_eq!(
            resolve_string("timeout=$env.TIMEOUT_MS&d=$env.DEBUG", &e).unwrap(),
            Some(json!("timeout=5000&d=true"))
        );
    }

    /// Anahtar ilk küçük harf / `/` / `:` / `-` karakterinde BİTER; kalan metin olarak
    /// eklenir. Enterpolasyonun çalışması için zorunlu: `$env.HOST/v1`'de anahtar `/`'de
    /// bitmezse hiçbir URL kurulamaz. Aynı kural küçük harf için de geçerlidir.
    #[test]
    fn key_boundary() {
        let e = env();
        assert_eq!(
            resolve_string("$env.AUTH_APIx", &e).unwrap(),
            Some(json!("https://auth.test.cs.com.trx")),
            "anahtar AUTH_API'de biter, 'x' metne eklenir"
        );
        assert_eq!(references("$env.AUTH_APIx").unwrap(), vec!["AUTH_API"]);
        assert_eq!(
            resolve_string("x:$env.TIMEOUT_MS:y", &e).unwrap(),
            Some(json!("x:5000:y"))
        );
        assert_eq!(
            resolve_string("$env.AUTH_API-suffix", &e).unwrap(),
            Some(json!("https://auth.test.cs.com.tr-suffix"))
        );
    }

    #[test]
    fn no_reference_returns_none() {
        assert_eq!(resolve_string("https://sabit/v1", &env()).unwrap(), None);
        assert_eq!(resolve_string("$ctx.foo", &env()).unwrap(), None);
    }

    /// Eksik anahtar `null` DEĞİL, hata. Bu, `$ctx`'in aksine bilinçli bir istisna.
    #[test]
    fn missing_key_is_error() {
        let e = env();
        assert!(resolve_string("$env.YOK", &e).is_err());
        assert!(resolve_string("https://$env.YOK/v1", &e).is_err());
    }

    /// Bozuk referans sessizce düz metin kalmaz — tipo runtime'da dışarı sızardı.
    #[test]
    fn malformed_reference_is_error() {
        let e = env();
        assert!(resolve_string("$env.auth_api", &e).is_err());
        assert!(resolve_string("$env.", &e).is_err());
        assert!(resolve_string("https://$env./v1", &e).is_err());
        assert!(references("$env.9BAD").is_err());
    }

    #[test]
    fn references_collects_all() {
        assert_eq!(
            references("$env.A/$env.B?x=$env.A").unwrap(),
            vec!["A", "B", "A"]
        );
        assert!(references("hiç yok").unwrap().is_empty());
    }

    /// Secret'lar `PublicEnv`'de YOKTUR — ZEN ve effects onları göremez.
    #[test]
    fn public_view_drops_secrets() {
        let e = env();
        let p = e.public();
        assert!(p.get("AUTH_API").is_some());
        assert!(p.get("MONGO_PW").is_none());
        assert!(resolve_string("$env.MONGO_PW", &p).is_err());
        // Aynı referans TAM env'de çözülür (autoexec config yolu).
        assert_eq!(
            resolve_string("$env.MONGO_PW", &e).unwrap(),
            Some(json!("s3cret-pass"))
        );
        assert_eq!(p.to_json().get("MONGO_PW"), None);
    }

    #[test]
    fn secret_strings_for_masking() {
        assert_eq!(env().secret_strings(), vec!["s3cret-pass".to_string()]);
    }

    /// ZEN ön-kontrolü: ifade metnindeki tanımsız anahtar değerlendirmeden önce patlar.
    #[test]
    fn assert_refs_catches_undefined_before_eval() {
        let p = env().public();
        assert!(assert_refs_defined("$env.AUTH_API == 'x'", &p).is_ok());
        assert!(assert_refs_defined("$env.YOK == 'x'", &p).is_err());
        // Secret bir anahtar ZEN'de tanımsız SAYILIR — public görünümde yok.
        assert!(assert_refs_defined("$env.MONGO_PW == 'x'", &p).is_err());
    }
}
