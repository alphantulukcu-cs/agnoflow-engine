use crate::{
    error::EngineError,
    types::{actor::OrgUnit, delegation::DelegationGrant},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Organizasyon katmanı portu — ORGTRVLANG çözümleme + rol/kimlik doğrulama.
/// v2.2 runtime port'ları için bkz. `crate::v22::ports`.
#[async_trait]
pub trait OrgPort: Send + Sync {
    /// Resolves an ORGTRVLANG expression from an anchor ORGU.
    async fn resolve_c_orgu(
        &self,
        anchor_orgu_id: Uuid,
        expr: &str,
        orgtnt_id: Uuid,
    ) -> Result<Vec<OrgUnit>, EngineError>;

    async fn check_user_role(
        &self,
        user_id: Uuid,
        orgu_id: Uuid,
        role_name: &str,
    ) -> Result<bool, EngineError>;

    async fn orgtnt_for_orgu(&self, orgu_id: Uuid) -> Result<Uuid, EngineError>;

    /// Kullanıcının c_u eşleşmesinde kullanılacak kimliği (username).
    /// v2.2 c_u listeleri username veya UUID string içerebilir.
    async fn user_ident(&self, _user_id: Uuid) -> Result<Option<String>, EngineError> {
        Ok(None)
    }

    /// Madde 6: `claimant`'a o an (now) geçerli — aktif + zaman penceresi içinde —
    /// vekaletlerin aday listesi. Alıcı (grantee) eşleşmesi matcher'da
    /// (`authorize_with_delegation`) yapılır; bu yüzden dönen liste bir üst kümedir
    /// (adapter tenant + zaman + kaba grantee-c_u/havuz filtresiyle daraltabilir).
    /// Default `Ok(vec![])` — vekalet desteklemeyen port'lar (sim/mock) için geriye uyum.
    async fn active_delegations_for(
        &self,
        _claimant_user_id: Uuid,
        _orgtnt_id: Uuid,
        _now: DateTime<Utc>,
    ) -> Result<Vec<DelegationGrant>, EngineError> {
        Ok(vec![])
    }
}
