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
        let Some((project_id, wfd_name)) = sqlx::query_as::<_, (Option<Uuid>, String)>(
            "SELECT project_id, name FROM wf.wfd_meta WHERE wfd_id = $1",
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

        // Secret'lar taslak/yayınlanmış AYRIMI OLMADAN çözülür.
        //
        // Başlangıçta GitLab'ın "protected variable" kuralı alınmıştı (secret yalnız
        // published koşumda). 2026-08-04'te kullanıcı kararıyla KALDIRILDI: kural yanlış
        // eksende koruyordu. Tasarımcı akışı kurarken autoexec'ini ve simülasyonunu
        // gerçek uçlara karşı denemek zorunda; taslakta secret'ı kesmek, anahtar isteyen
        // her entegrasyonu editörde denenemez yapıyordu. Erişim kontrolü ağ katmanının
        // işidir (FW kuralları / ortam erişilebilirliği): prod'a ulaşamayan bir makineden
        // prod anahtarı zaten bir işe yaramaz.
        //
        // Koruma tamamen kalkmadı — secret DEĞERLERİ hâlâ hiçbir ekrana dönmez:
        // `resolved_config()` ve hata metinleri `[MASKED]` uygular, ZEN/effects secret'ı
        // hiç görmez (tip düzeyinde). Yani kullanılabilir ama okunamaz.
        //
        // `include_secrets` parametresi bilerek DURUYOR: korumayı ortam bazına (örn.
        // `wf.environment.is_protected`) geri getirmek istenirse bağlanacak seam odur.
        repo::env::load_run_env(&self.pool, project_id, &wfd_name, env_id, true)
            .await
            .map_err(db_err)
    }
}
