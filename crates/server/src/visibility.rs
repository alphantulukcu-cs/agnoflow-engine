//! Görünürlük kuralının server tarafındaki yüzü.
//!
//! Kuralın KENDİSİ motor tarafında (`wf_wfe::visibility`): SQL parçasını liste
//! ucu (burada), `WfeExecutor`ın detay kapısı (`VisibilityPort`) ve portal havuzu
//! (`routes::portal::pool`) kullanıyor. İkinci bir uygulama olmasın diye buradan
//! yalnız yeniden ihraç edilir — kural tek dosyada yaşar.
//!
//! `PARAM_COUNT` de ihraç edilir: kendi parametresi olan çağıranlar (havuz:
//! `$1` = tenant) offset'i ondan hesaplar, elle sayı yazmaz.
pub use wf_wfe::visibility::{sql, ViewerFilters, PARAM_COUNT};
