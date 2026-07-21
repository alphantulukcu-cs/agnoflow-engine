use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;
use wf_org::repo::{delegation, user_role};
use wfe_core::types::delegation::DelegationGrant;
use wfe_core::types::wfd_v22::CandidateActor;
use wfe_core::{types::actor::OrgUnit, EngineError, OrgPort};

pub struct OrgAdapter {
    pub pool: PgPool,
}

impl OrgAdapter {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl OrgPort for OrgAdapter {
    async fn resolve_c_orgu(
        &self,
        anchor_orgu_id: Uuid,
        expr: &str,
        orgtnt_id: Uuid,
    ) -> Result<Vec<OrgUnit>, EngineError> {
        let units = user_role::resolve_orgu(&self.pool, anchor_orgu_id, expr, orgtnt_id)
            .await
            .map_err(|e| EngineError::OrgPort(e.to_string()))?;

        Ok(units
            .into_iter()
            .map(|u| OrgUnit {
                orgu_id: u.orgu_id,
                orgu_type: u.orgu_type,
                path: u.path,
            })
            .collect())
    }

    async fn check_user_role(
        &self,
        user_id: Uuid,
        orgu_id: Uuid,
        role_name: &str,
    ) -> Result<bool, EngineError> {
        user_role::check_user_role(&self.pool, user_id, orgu_id, role_name)
            .await
            .map_err(|e| EngineError::OrgPort(e.to_string()))
    }

    async fn orgtnt_for_orgu(&self, orgu_id: Uuid) -> Result<Uuid, EngineError> {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT orgtnt_id
             FROM org.orgt_orgu
             WHERE orgu_id = $1 AND is_active = true
             LIMIT 1",
        )
        .bind(orgu_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| EngineError::OrgPort(e.to_string()))?
        .ok_or_else(|| EngineError::OrgPort(format!("orgtnt not found for orgu {orgu_id}")))
    }

    async fn user_ident(&self, user_id: Uuid) -> Result<Option<String>, EngineError> {
        sqlx::query_scalar::<_, String>(
            "SELECT username FROM org.u WHERE u_id = $1 AND is_active = true",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| EngineError::OrgPort(e.to_string()))
    }

    async fn active_delegations_for(
        &self,
        claimant_user_id: Uuid,
        orgtnt_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<Vec<DelegationGrant>, EngineError> {
        let rows = delegation::active_candidates(&self.pool, claimant_user_id, orgtnt_id, now)
            .await
            .map_err(|e| EngineError::OrgPort(e.to_string()))?;

        rows.into_iter()
            .map(|d| {
                // grantee JSONB → wfe-core CandidateActor (kişi veya havuz kuralı).
                let grantee: CandidateActor = serde_json::from_value(d.grantee).map_err(|e| {
                    EngineError::OrgPort(format!(
                        "delegation {} grantee parse: {e}",
                        d.delegation_id
                    ))
                })?;
                Ok(DelegationGrant {
                    delegation_id: d.delegation_id,
                    delegator_user_id: d.delegator_user_id,
                    seat_orgu_id: d.seat_orgu_id,
                    seat_role: d.seat_role,
                    grantee,
                })
            })
            .collect()
    }
}
