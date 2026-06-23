pub mod inline_wfd_port;
pub mod in_memory_wfe_port;

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use wfe_core::{
    ports::WFES,
    types::{
        actor::CandidateActor,
        dynctx::DynCtx,
        wfah::{Wfah, WfahEntry},
        wfe::WfeStatus,
    },
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimState {
    pub wfe_id:       Uuid,
    pub orgtnt_id:    Uuid,
    pub dynctx:       serde_json::Value,
    pub wfah:         Vec<WfahEntry>,
    pub current_c_a:  Vec<CandidateActor>,
    pub status:       WfeStatus,
    pub end_response: Option<serde_json::Value>,
}

impl SimState {
    pub fn from_wfes(wfes: &WFES) -> Self {
        Self {
            wfe_id:       wfes.wfe_id,
            orgtnt_id:    wfes.orgtnt_id,
            dynctx:       wfes.dynctx.as_value().clone(),
            wfah:         wfes.wfah.entries().to_vec(),
            current_c_a:  wfes.current_c_a.clone(),
            status:       wfes.status.clone(),
            end_response: wfes.end_response.clone(),
        }
    }

    pub fn into_wfes(self) -> WFES {
        WFES {
            wfe_id:      self.wfe_id,
            dynctx:      DynCtx(self.dynctx),
            wfah:        Wfah(self.wfah),
            status:      self.status,
            orgtnt_id:   self.orgtnt_id,
            wfd_id:      Uuid::nil(),
            wfd_version: 0,
            current_c_a: self.current_c_a,
            end_response: self.end_response,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_wfes(wfe_id: Uuid, orgtnt_id: Uuid) -> WFES {
        WFES {
            wfe_id,
            dynctx: DynCtx::empty(),
            wfah: Wfah::empty(),
            status: WfeStatus::Active,
            orgtnt_id,
            wfd_id: Uuid::nil(),
            wfd_version: 0,
            current_c_a: vec![],
            end_response: None,
        }
    }

    #[test]
    fn from_wfes_preserves_ids() {
        let wfe_id    = Uuid::new_v4();
        let orgtnt_id = Uuid::new_v4();
        let wfes      = sample_wfes(wfe_id, orgtnt_id);
        let sim       = SimState::from_wfes(&wfes);
        assert_eq!(sim.wfe_id,    wfe_id);
        assert_eq!(sim.orgtnt_id, orgtnt_id);
        assert_eq!(sim.status,    WfeStatus::Active);
    }

    #[test]
    fn into_wfes_round_trips() {
        let wfe_id    = Uuid::new_v4();
        let orgtnt_id = Uuid::new_v4();
        let wfes      = sample_wfes(wfe_id, orgtnt_id);
        let sim       = SimState::from_wfes(&wfes);
        let back      = sim.into_wfes();
        assert_eq!(back.wfe_id,    wfe_id);
        assert_eq!(back.orgtnt_id, orgtnt_id);
        assert_eq!(back.status,    WfeStatus::Active);
    }
}
