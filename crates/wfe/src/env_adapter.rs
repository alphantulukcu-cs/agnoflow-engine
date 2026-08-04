//! `EnvPort` implementasyonu — ortam konfigürasyonunu (`$env`) DB'den çözer.
//!
//! Tasarım: `docs/superpowers/specs/2026-08-04-env-config-design.md`.

use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;
use wfe_core::v22::env::RunEnv;
use wfe_core::v22::ports::EnvPort;
use wfe_core::EngineError;

use crate::repo;

pub struct EnvAdapter {
    pub pool: PgPool,
}

impl EnvAdapter {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn db_err(e: impl std::fmt::Display) -> EngineError {
    EngineError::WfePort(e.to_string())
}

#[async_trait]
impl EnvPort for EnvAdapter {
    async fn load_run_env(
        &self,
        orgtnt_id: Uuid,
        wfd_id: Uuid,
        environment_id: Option<Uuid>,
    ) -> Result<RunEnv, EngineError> {
        // Değerlerin sahibi MANTIKSAL WFD'dir: `(project_id, name)`. `wfd_id` her versiyon
        // için ayrı bir satırdır, ona bağlamak conf'u yeni versiyonda koparırdı.
        let Some((project_id, wfd_name, status)) =
            sqlx::query_as::<_, (Option<Uuid>, String, String)>(
                "SELECT project_id, name, status FROM wf.wfd_meta WHERE wfd_id = $1",
            )
            .bind(wfd_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?
        else {
            return Ok(RunEnv::default());
        };
        // Projesiz (eski) WFD satırlarının conf'u olamaz — sahiplik anahtarı eksik.
        let Some(project_id) = project_id else {
            return Ok(RunEnv::default());
        };

        let env_id = match environment_id {
            Some(id) => id,
            None => {
                repo::env::resolve_environment(&self.pool, orgtnt_id, None)
                    .await
                    .map_err(db_err)?
                    .id
            }
        };

        // GitLab'ın "protected variable" kuralı: secret'lar YALNIZ published WFD
        // koşumunda çözülür. Taslak denemesi prod kimlik bilgisiyle dış sisteme istek
        // atamaz; eksik secret, kullanan autoexec'i açık bir hatayla düşürür.
        let include_secrets = status == "published";

        repo::env::load_run_env(&self.pool, project_id, &wfd_name, env_id, include_secrets)
            .await
            .map_err(db_err)
    }
}
