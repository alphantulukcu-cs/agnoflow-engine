use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;
use serde_json::Value;
use wfe_core::{
    EngineError, WfePort,
    ports::WFES,
    types::{
        actor::{Actor, CandidateActor},
        dynctx::DynCtx,
        wfah::{Wfah, WfahEntry},
        wfe::WfeStatus,
    },
};
use crate::repo;

pub struct WfeAdapter {
    pub pool: PgPool,
}

impl WfeAdapter {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl WfePort for WfeAdapter {
    async fn load_wfes(&self, wfe_id: Uuid) -> Result<WFES, EngineError> {
        let row = repo::wfe::get(&self.pool, wfe_id)
            .await
            .map_err(|e| EngineError::WfePort(e.to_string()))?;

        let ctx_val = repo::dynctx::load_latest(&self.pool, wfe_id)
            .await
            .map_err(|e| EngineError::WfePort(e.to_string()))?;

        let wfah_rows = repo::wfah::load_all(&self.pool, wfe_id)
            .await
            .map_err(|e| EngineError::WfePort(e.to_string()))?;

        let entries: Vec<WfahEntry> = wfah_rows
            .into_iter()
            .map(|r| {
                let actor: Actor = serde_json::from_value(r.actor).unwrap_or(Actor {
                    orgu_id: Uuid::nil(),
                    user_id: Uuid::nil(),
                    role:    "unknown".into(),
                });
                WfahEntry {
                    seq:        r.seq as u32,
                    action:     r.action,
                    actor,
                    input:      r.input,
                    applied_at: r.applied_at,
                }
            })
            .collect();

        let status = match row.status.as_str() {
            "terminal" => WfeStatus::Terminal,
            "error"    => WfeStatus::Error,
            _          => WfeStatus::Active,
        };

        let current_c_a: Vec<CandidateActor> = serde_json::from_value(row.current_c_a.clone())
            .unwrap_or_default();

        Ok(WFES {
            wfe_id,
            dynctx:       DynCtx(ctx_val),
            wfah:         Wfah(entries),
            status,
            orgtnt_id:    row.orgtnt_id,
            wfd_id:       row.wfd_id,
            wfd_version:  row.wfd_version as u32,
            current_c_a,
            end_response: row.end_response,
        })
    }

    async fn persist_new_dynctx(
        &self,
        wfe_id: Uuid,
        ctx:    &DynCtx,
        seq:    u32,
    ) -> Result<(), EngineError> {
        repo::dynctx::insert(&self.pool, wfe_id, seq as i32, ctx.as_value())
            .await
            .map_err(|e| EngineError::WfePort(e.to_string()))
    }

    async fn append_wfah(
        &self,
        wfe_id: Uuid,
        entry:  &WfahEntry,
    ) -> Result<(), EngineError> {
        let actor_json = serde_json::to_value(&entry.actor)
            .map_err(|e| EngineError::WfePort(e.to_string()))?;
        repo::wfah::append(
            &self.pool, wfe_id, entry.seq as i32,
            &entry.action, &actor_json, entry.input.as_ref(),
        )
        .await
        .map_err(|e| EngineError::WfePort(e.to_string()))
    }

    async fn update_c_a(
        &self,
        wfe_id: Uuid,
        c_a:    &[CandidateActor],
    ) -> Result<(), EngineError> {
        let c_a_json = serde_json::to_value(c_a)
            .map_err(|e| EngineError::WfePort(e.to_string()))?;
        repo::wfe::update_c_a(&self.pool, wfe_id, &c_a_json)
            .await
            .map_err(|e| EngineError::WfePort(e.to_string()))
    }

    async fn set_terminal(
        &self,
        wfe_id:       Uuid,
        end_response: &Value,
    ) -> Result<(), EngineError> {
        repo::wfe::set_terminal(&self.pool, wfe_id, end_response)
            .await
            .map_err(|e| EngineError::WfePort(e.to_string()))
    }

    async fn create_wfe(
        &self,
        orgtnt_id:   Uuid,
        wfd_id:      Uuid,
        wfd_version: u32,
        initial_ctx: &DynCtx,
        initial_c_a: &[CandidateActor],
    ) -> Result<Uuid, EngineError> {
        let c_a_json = serde_json::to_value(initial_c_a)
            .map_err(|e| EngineError::WfePort(e.to_string()))?;
        let wfe_id = repo::wfe::create(
            &self.pool, orgtnt_id, wfd_id, wfd_version as i32, &c_a_json
        )
        .await
        .map_err(|e| EngineError::WfePort(e.to_string()))?;

        // Persist initial DynCtx as seq=1
        repo::dynctx::insert(&self.pool, wfe_id, 1, initial_ctx.as_value())
            .await
            .map_err(|e| EngineError::WfePort(e.to_string()))?;

        Ok(wfe_id)
    }
}
