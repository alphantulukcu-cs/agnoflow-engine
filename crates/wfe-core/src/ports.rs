use async_trait::async_trait;
use uuid::Uuid;
use crate::{
    error::EngineError,
    types::{actor::{OrgUnit, CandidateActor}, dynctx::DynCtx, wfah::WfahEntry, wfd::WFD, wfe::WfeStatus},
};
use serde_json::Value;

/// WFES — the complete execution state passed around the engine.
#[derive(Debug, Clone)]
pub struct WFES {
    pub wfe_id:       Uuid,
    pub dynctx:       DynCtx,
    pub wfah:         crate::types::wfah::Wfah,
    pub status:       WfeStatus,
    pub orgtnt_id:    Uuid,
    pub wfd_id:       Uuid,
    pub wfd_version:  u32,
    pub current_c_a:  Vec<crate::types::actor::CandidateActor>,
    pub end_response: Option<serde_json::Value>,
}

#[async_trait]
pub trait OrgPort: Send + Sync {
    /// Resolves an ORGTRVLANG expression from an anchor ORGU.
    async fn resolve_c_orgu(
        &self,
        anchor_orgu_id: Uuid,
        expr:           &str,
        orgtnt_id:      Uuid,
    ) -> Result<Vec<OrgUnit>, EngineError>;

    async fn check_user_role(
        &self,
        user_id:   Uuid,
        orgu_id:   Uuid,
        role_name: &str,
    ) -> Result<bool, EngineError>;
}

#[async_trait]
pub trait WfdPort: Send + Sync {
    async fn fetch(&self, wfd_id: Uuid, version: u32) -> Result<WFD, EngineError>;
}

#[async_trait]
pub trait WfePort: Send + Sync {
    async fn load_wfes(&self, wfe_id: Uuid) -> Result<WFES, EngineError>;

    /// Insert a new DynCtx snapshot (insert-only, never update).
    async fn persist_new_dynctx(
        &self,
        wfe_id: Uuid,
        ctx:    &DynCtx,
        seq:    u32,
    ) -> Result<(), EngineError>;

    async fn append_wfah(
        &self,
        wfe_id: Uuid,
        entry:  &WfahEntry,
    ) -> Result<(), EngineError>;

    async fn update_c_a(
        &self,
        wfe_id: Uuid,
        c_a:    &[CandidateActor],
    ) -> Result<(), EngineError>;

    async fn set_terminal(
        &self,
        wfe_id:       Uuid,
        end_response: &Value,
    ) -> Result<(), EngineError>;

    async fn create_wfe(
        &self,
        orgtnt_id:   Uuid,
        wfd_id:      Uuid,
        wfd_version: u32,
        initial_ctx: &DynCtx,
        initial_c_a: &[CandidateActor],
    ) -> Result<Uuid, EngineError>;
}
