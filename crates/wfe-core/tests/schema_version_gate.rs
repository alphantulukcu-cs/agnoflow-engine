//! Sürüm çapası kapısı: gömülü şemanın `wfd_version.const`i motorun sabitiyle EŞİT olmalı.
//!
//! ## Neden bu dosya var
//!
//! v2.3'ün wire değişikliği üç depoda birden yaşıyor ve `D05`ten sonra "aynı commit"
//! disiplini YOK: spec kendi deposunda (`agnoflow-spec`), tüketiciler onu submodule
//! olarak PİNLİYOR. Depolar arası git atomikliği olmadığı için motorun `"2.3"` derken
//! pinlediği şemanın `"2.2"` demesi mümkündür ve bu ayrışma GÖRÜNMEZ.
//!
//! Sözleşmenin sürümü sözleşmenin kendi içinde yazılıdır (`R06`/S2): çapa
//! `docs/spec/schema.json` → `properties.wfd_version.const`. Ayrı bir `VERSION` dosyası
//! BİLEREK açılmadı — ikinci bir yer ikinci bir drift kaynağıdır. Bu test o çapayı
//! motorun sabitine perçinler.
//!
//! Ek okuma YOK: şema `include_str!` ile derleme anında gömülüdür (`schema.rs`), yani
//! test dosya sistemine değil BINARY'nin içindekine bakıyor — pinli SHA neyse o.
//!
//! Kapı TEK DEĞERLİDİR (eşitlik): aralık, liste ya da "en az" mantığı yoktur
//! (`R07`/S1). `check_version` de aynı eşitliği koşuyor (`wfd_v22.rs`); bu test o
//! eşitliğin İKİ TARAFININ aynı sürümü söylediğini garanti eder.

use serde_json::Value;
use wfe_core::schema::SCHEMA_JSON;
use wfe_core::types::wfd_v22::SUPPORTED_WFD_VERSION;

#[test]
fn embedded_schema_version_const_matches_supported_version() {
    let schema: Value =
        serde_json::from_str(SCHEMA_JSON).expect("gömülü schema.json parse edilemedi");

    let anchor = schema
        .pointer("/properties/wfd_version/const")
        .and_then(Value::as_str)
        .expect(
            "sürüm çapası KAYIP: docs/spec/schema.json içinde \
             `properties.wfd_version.const` bir dize olmalı. Yol kayarsa bu kapı da, \
             editördeki eşi de sessizce anlamsızlaşır (agnoflow-spec kapısı bunu \
             kendi deposunda da doğruluyor)",
        );

    assert_eq!(
        anchor, SUPPORTED_WFD_VERSION,
        "sürüm ayrışması: pinli şema \"{anchor}\" diyor, motor \
         SUPPORTED_WFD_VERSION = \"{SUPPORTED_WFD_VERSION}\" diyor. Ya submodule SHA'sı \
         (docs/spec) eski, ya sabit güncellenmedi — ikisi AYNI MR'da hareket eder \
         (R06 / MR zinciri, adım 2.2)"
    );
}
