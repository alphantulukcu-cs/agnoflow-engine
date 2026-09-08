//! Görünürlük kuralının server tarafındaki yüzü.
//!
//! Kuralın KENDİSİ motor tarafında (`wf_wfe::visibility`): SQL parçasını liste
//! ucu (burada), `WfeExecutor`ın detay kapısı (`VisibilityPort`) ve portal havuzu
//! (`routes::portal::pool`) kullanıyor. İkinci bir uygulama olmasın diye buradan
//! yalnız yeniden ihraç edilir — kural tek dosyada yaşar.
//!
//! `PARAM_COUNT` YALNIZ testte ihraç edilir: üretim yolunda sorgular parçanın
//! metnini gömüyor, parametre sayısını elle taşımıyor — sabit yalnız havuzun
//! offset regresyon assert'inde (`routes::portal::pool`, `$1` = tenant) gerekli.
pub use wf_wfe::visibility::{sql, ViewerFilters};
#[cfg(test)]
pub use wf_wfe::visibility::PARAM_COUNT;
