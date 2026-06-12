use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;
use wf_org::repo::user_role;
use wfe_core::{EngineError, OrgPort, types::actor::OrgUnit};

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
        expr:           &str,
        orgtnt_id:      Uuid,
    ) -> Result<Vec<OrgUnit>, EngineError> {
        let units = user_role::resolve_orgu(&self.pool, anchor_orgu_id, expr, orgtnt_id)
            .await
            .map_err(|e| EngineError::OrgPort(e.to_string()))?;

        Ok(units.into_iter().map(|u| OrgUnit {
            orgu_id:   u.orgu_id,
            orgu_type: u.orgu_type,
            path:      u.path,
        }).collect())
    }

    async fn check_user_role(
        &self,
        user_id:   Uuid,
        orgu_id:   Uuid,
        role_name: &str,
    ) -> Result<bool, EngineError> {
        user_role::check_user_role(&self.pool, user_id, orgu_id, role_name)
            .await
            .map_err(|e| EngineError::OrgPort(e.to_string()))
    }

    async fn orgtnt_for_orgu(
        &self,
        orgu_id: Uuid,
    ) -> Result<Uuid, EngineError> {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT orgtnt_id
             FROM org.orgt_orgu
             WHERE orgu_id = $1 AND is_active = true
             LIMIT 1"
        )
        .bind(orgu_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| EngineError::OrgPort(e.to_string()))?
        .ok_or_else(|| EngineError::OrgPort(format!("orgtnt not found for orgu {orgu_id}")))
    }
}
