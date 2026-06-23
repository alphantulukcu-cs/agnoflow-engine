use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use async_trait::async_trait;
use uuid::Uuid;
use serde_json::Value;
use wfe_core::{
    EngineError, WfePort, WFES,
    types::{
        actor::CandidateActor,
        dynctx::DynCtx,
        wfah::{Wfah, WfahEntry},
        wfe::WfeStatus,
    },
};

pub struct InMemoryWfePort {
    store: Arc<Mutex<HashMap<Uuid, WFES>>>,
}

impl InMemoryWfePort {
    pub fn new() -> Self {
        Self { store: Arc::new(Mutex::new(HashMap::new())) }
    }

    pub fn seeded(wfes: WFES) -> Self {
        let mut map = HashMap::new();
        map.insert(wfes.wfe_id, wfes);
        Self { store: Arc::new(Mutex::new(map)) }
    }

    pub fn get(&self, wfe_id: Uuid) -> Option<WFES> {
        self.store.lock().unwrap().get(&wfe_id).cloned()
    }
}

#[async_trait]
impl WfePort for InMemoryWfePort {
    async fn create_wfe(
        &self,
        orgtnt_id:   Uuid,
        wfd_id:      Uuid,
        wfd_version: u32,
        initial_ctx: &DynCtx,
        initial_c_a: &[CandidateActor],
    ) -> Result<Uuid, EngineError> {
        let wfe_id = Uuid::new_v4();
        let wfes = WFES {
            wfe_id,
            dynctx:      initial_ctx.clone(),
            wfah:        Wfah::empty(),
            status:      WfeStatus::Active,
            orgtnt_id,
            wfd_id,
            wfd_version,
            current_c_a: initial_c_a.to_vec(),
            end_response: None,
        };
        self.store.lock().unwrap().insert(wfe_id, wfes);
        Ok(wfe_id)
    }

    async fn load_wfes(&self, wfe_id: Uuid) -> Result<WFES, EngineError> {
        self.store
            .lock()
            .unwrap()
            .get(&wfe_id)
            .cloned()
            .ok_or_else(|| EngineError::WfePort(format!("sim wfe not found: {wfe_id}")))
    }

    async fn persist_new_dynctx(
        &self,
        wfe_id: Uuid,
        ctx:    &DynCtx,
        _seq:   u32,
    ) -> Result<(), EngineError> {
        if let Some(w) = self.store.lock().unwrap().get_mut(&wfe_id) {
            w.dynctx = ctx.clone();
        }
        Ok(())
    }

    async fn append_wfah(&self, wfe_id: Uuid, entry: &WfahEntry) -> Result<(), EngineError> {
        if let Some(w) = self.store.lock().unwrap().get_mut(&wfe_id) {
            let mut entries = w.wfah.entries().to_vec();
            entries.push(entry.clone());
            w.wfah = Wfah(entries);
        }
        Ok(())
    }

    async fn update_c_a(&self, wfe_id: Uuid, c_a: &[CandidateActor]) -> Result<(), EngineError> {
        if let Some(w) = self.store.lock().unwrap().get_mut(&wfe_id) {
            w.current_c_a = c_a.to_vec();
        }
        Ok(())
    }

    async fn set_terminal(&self, wfe_id: Uuid, end_response: &Value) -> Result<(), EngineError> {
        if let Some(w) = self.store.lock().unwrap().get_mut(&wfe_id) {
            w.status       = WfeStatus::Terminal;
            w.end_response = Some(end_response.clone());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_wfes(wfe_id: Uuid) -> WFES {
        WFES {
            wfe_id,
            dynctx:      DynCtx::empty(),
            wfah:        Wfah::empty(),
            status:      WfeStatus::Active,
            orgtnt_id:   Uuid::nil(),
            wfd_id:      Uuid::nil(),
            wfd_version: 0,
            current_c_a: vec![],
            end_response: None,
        }
    }

    #[tokio::test]
    async fn create_and_load() {
        let port   = InMemoryWfePort::new();
        let wfe_id = port.create_wfe(Uuid::nil(), Uuid::nil(), 0, &DynCtx::empty(), &[]).await.unwrap();
        let wfes   = port.load_wfes(wfe_id).await.unwrap();
        assert_eq!(wfes.wfe_id, wfe_id);
        assert_eq!(wfes.status, WfeStatus::Active);
    }

    #[tokio::test]
    async fn seeded_loads_correctly() {
        let wfe_id = Uuid::new_v4();
        let port   = InMemoryWfePort::seeded(sample_wfes(wfe_id));
        let loaded = port.load_wfes(wfe_id).await.unwrap();
        assert_eq!(loaded.wfe_id, wfe_id);
    }

    #[tokio::test]
    async fn load_missing_errors() {
        let port = InMemoryWfePort::new();
        let res  = port.load_wfes(Uuid::new_v4()).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn set_terminal_updates_status_and_response() {
        let wfe_id = Uuid::new_v4();
        let port   = InMemoryWfePort::seeded(sample_wfes(wfe_id));
        port.set_terminal(wfe_id, &serde_json::json!({"ok": true})).await.unwrap();
        let w = port.load_wfes(wfe_id).await.unwrap();
        assert_eq!(w.status, WfeStatus::Terminal);
        assert_eq!(w.end_response, Some(serde_json::json!({"ok": true})));
    }

    #[tokio::test]
    async fn update_c_a_replaces_candidates() {
        let wfe_id = Uuid::new_v4();
        let port   = InMemoryWfePort::seeded(sample_wfes(wfe_id));
        let c_a    = vec![CandidateActor { orgu_id: Uuid::new_v4(), role: "clerk".into() }];
        port.update_c_a(wfe_id, &c_a).await.unwrap();
        let w = port.load_wfes(wfe_id).await.unwrap();
        assert_eq!(w.current_c_a.len(), 1);
        assert_eq!(w.current_c_a[0].role, "clerk");
    }
}
