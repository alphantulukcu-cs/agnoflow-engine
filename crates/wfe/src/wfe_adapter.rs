//! v2.2 WfeStore implementasyonu — TransitionCommit TEK PostgreSQL
//! transaction'ında uygulanır (M8 / WOR-43; WOR-7 fix).

use async_trait::async_trait;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;
use wfe_core::types::{
    actor::Actor,
    dynctx::DynCtx,
    wfah::{Wfah, WfahEntry},
    wfe::WfeStatus,
};
use wfe_core::v22::ports::{CommitOutcome, NewWfe, TransitionCommit, WfeStore, Wfes};
use wfe_core::EngineError;

use crate::repo;

pub struct WfeAdapter {
    pub pool: PgPool,
}

impl WfeAdapter {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn db_err(e: impl std::fmt::Display) -> EngineError {
    EngineError::WfePort(e.to_string())
}

async fn insert_wfah_entries(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    wfe_id: Uuid,
    entries: &[WfahEntry],
) -> Result<(), EngineError> {
    for entry in entries {
        let actor_json = serde_json::to_value(&entry.actor).map_err(db_err)?;
        sqlx::query(
            "INSERT INTO wf.wfah (wfe_id, seq, action, actor, input, applied_at)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(wfe_id)
        .bind(entry.seq as i32)
        .bind(&entry.action)
        .bind(&actor_json)
        .bind(entry.input.as_ref())
        .bind(entry.applied_at)
        .execute(&mut **tx)
        .await
        .map_err(db_err)?;
    }
    Ok(())
}

#[async_trait]
impl WfeStore for WfeAdapter {
    async fn load(&self, wfe_id: Uuid) -> Result<Wfes, EngineError> {
        let row = repo::wfe::get(&self.pool, wfe_id).await.map_err(db_err)?;
        let ctx = repo::dynctx::load_latest(&self.pool, wfe_id)
            .await
            .map_err(db_err)?;
        let wfah_rows = repo::wfah::load_all(&self.pool, wfe_id)
            .await
            .map_err(db_err)?;

        let entries: Vec<WfahEntry> = wfah_rows
            .into_iter()
            .map(|r| {
                let actor: Actor = serde_json::from_value(r.actor).unwrap_or_else(|e| {
                    // WOR-19: bozuk kayıt sessiz kalmasın — audit izi log'a düşer
                    tracing::warn!(
                        "wfe {wfe_id} wfah seq {} actor parse edilemedi: {e}",
                        r.seq
                    );
                    Actor {
                        orgu_id: Uuid::nil(),
                        user_id: Uuid::nil(),
                        role: "unknown".into(),
                    }
                });
                WfahEntry {
                    seq: r.seq as u32,
                    action: r.action,
                    actor,
                    input: r.input,
                    applied_at: r.applied_at,
                }
            })
            .collect();

        let status = match row.status.as_str() {
            "terminal" => WfeStatus::Terminal,
            "error" => WfeStatus::Error,
            _ => WfeStatus::Active,
        };

        let assigned_to = row
            .claimed_by
            .as_ref()
            .and_then(|cb| cb.get("user_id"))
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok());

        Ok(Wfes {
            wfe_id,
            orgtnt_id: row.orgtnt_id,
            wfd_id: row.wfd_id,
            wfd_version: row.wfd_version,
            dynctx: DynCtx(ctx),
            wfah: Wfah(entries),
            status,
            current_node: row.current_node,
            assigned_to,
            end_response: row.end_response,
        })
    }

    async fn create(&self, new: &NewWfe) -> Result<(), EngineError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;

        let (status, current_node, end_response) = match &new.outcome {
            CommitOutcome::MoveTo { node } => ("active", Some(node.as_str()), None),
            CommitOutcome::Terminal { end_response } => ("terminal", None, Some(end_response)),
        };
        let c_a_json = serde_json::to_value(&new.resolved_c_a).map_err(db_err)?;

        sqlx::query(
            "INSERT INTO wf.wfe
               (wfe_id, orgtnt_id, wfd_id, wfd_version, status, current_node, current_c_a, end_response)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(new.wfe_id)
        .bind(new.orgtnt_id)
        .bind(new.wfd_id)
        .bind(new.wfd_version)
        .bind(status)
        .bind(current_node)
        .bind(&c_a_json)
        .bind(end_response)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;

        sqlx::query("INSERT INTO wf.wfe_dynctx (wfe_id, seq, ctx) VALUES ($1, 1, $2)")
            .bind(new.wfe_id)
            .bind(&new.initial_dynctx)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;

        insert_wfah_entries(&mut tx, new.wfe_id, &new.wfah_entries).await?;

        tx.commit().await.map_err(db_err)
    }

    async fn commit(&self, commit: &TransitionCommit) -> Result<(), EngineError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;

        let dynctx_seq = commit
            .wfah_entries
            .last()
            .map(|e| e.seq as i32)
            .unwrap_or(1);
        sqlx::query("INSERT INTO wf.wfe_dynctx (wfe_id, seq, ctx) VALUES ($1, $2, $3)")
            .bind(commit.wfe_id)
            .bind(dynctx_seq)
            .bind(&commit.new_dynctx)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;

        insert_wfah_entries(&mut tx, commit.wfe_id, &commit.wfah_entries).await?;

        match &commit.outcome {
            CommitOutcome::MoveTo { node } => {
                let c_a_json = serde_json::to_value(&commit.resolved_c_a).map_err(db_err)?;
                // M8: yeni node'a UNASSIGNED giriş — claimed_by temizlenir
                sqlx::query(
                    "UPDATE wf.wfe
                     SET current_node = $1, current_c_a = $2, claimed_by = NULL, updated_at = now()
                     WHERE wfe_id = $3 AND orgtnt_id = $4",
                )
                .bind(node)
                .bind(&c_a_json)
                .bind(commit.wfe_id)
                .bind(commit.orgtnt_id)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
            }
            CommitOutcome::Terminal { end_response } => {
                sqlx::query(
                    "UPDATE wf.wfe
                     SET status = 'terminal', current_node = NULL, current_c_a = '[]'::jsonb,
                         claimed_by = NULL, end_response = $1, updated_at = now()
                     WHERE wfe_id = $2 AND orgtnt_id = $3",
                )
                .bind(end_response)
                .bind(commit.wfe_id)
                .bind(commit.orgtnt_id)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
            }
        }

        tx.commit().await.map_err(db_err)
    }

    async fn claim(
        &self,
        wfe_id: Uuid,
        orgtnt_id: Uuid,
        user_id: Uuid,
    ) -> Result<bool, EngineError> {
        // CAS: yalnızca unassigned aktif WFE claim edilebilir — eşzamanlı
        // claim'lerden yalnızca biri satırı günceller (V1 stateless claim'in kalıcı çözümü)
        let claimed_by = json!({ "user_id": user_id.to_string() });
        let result = sqlx::query(
            "UPDATE wf.wfe
             SET claimed_by = $1, updated_at = now()
             WHERE wfe_id = $2 AND orgtnt_id = $3 AND status = 'active' AND claimed_by IS NULL",
        )
        .bind(&claimed_by)
        .bind(wfe_id)
        .bind(orgtnt_id)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(result.rows_affected() == 1)
    }
}
