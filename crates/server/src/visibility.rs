//! Görünürlük kuralının server tarafındaki yüzü.
//!
//! Kuralın KENDİSİ motor tarafında (`wf_wfe::visibility`): SQL parçasını hem
//! liste ucu (burada) hem de `WfeExecutor`ın detay kapısı (`VisibilityPort`)
//! kullanıyor. İkinci bir uygulama olmasın diye buradan yalnız yeniden ihraç
//! edilir — kural tek dosyada yaşar.
pub use wf_wfe::visibility::{sql, ViewerFilters};
