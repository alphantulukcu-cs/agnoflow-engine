//! Madde 6: vekalet/delegasyon çalışma-zamanı tipi.
//!
//! `OrgPort::active_delegations_for` bir claimant için o an geçerli (aktif +
//! zaman penceresi içinde) vekaletleri döndürür. Matcher (`authorize_with_delegation`)
//! her aday için iki şeyi doğrular: (a) claimant `grantee`'ye uyuyor mu, (b)
//! delegator'ın koltuğu (`seat_orgu_id` + `seat_role`) node c_a'sına uyuyor mu.

use crate::types::wfd_v22::CandidateActor;
use uuid::Uuid;

/// Bir claimant'a verilmiş tek aktif vekalet kaydının matcher görünümü.
#[derive(Debug, Clone)]
pub struct DelegationGrant {
    pub delegation_id: Uuid,
    /// Şapka sahibi — sentetik aktörün user_id'si (c_u node'ları + audit).
    pub delegator_user_id: Uuid,
    /// Delege edilen koltuk: orgu.
    pub seat_orgu_id: Uuid,
    /// Delege edilen koltuk: rol adı (c_a.c_r ile aynı uzay).
    pub seat_role: String,
    /// Alıcı kuralı: kişi ({c_u}) veya havuz ({c_orgu, c_r}). Matcher bunu
    /// `authorize(grantee, claimant)` ile değerlendirir.
    pub grantee: CandidateActor,
}
