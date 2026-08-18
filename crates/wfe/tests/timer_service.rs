//! Event-driven SLA timer servisi entegrasyon testi (2026-07-17).
//!
//! Amaç: kısa (2 sn) deadline'lı bir WFE, 60 sn'lik güvenlik-ağı süpürme
//! aralığı BEKLENMEDEN ~deadline anında `terminated` olmalı. Servis aktif WFE
//! yokken 60 sn uykuya girer; `WfeExecutor::start` sinyaliyle uyanıp vadeyi
//! yeniden hesaplaması testin asıl doğruladığı şeydir — sinyal çalışmazsa
//! test 8 sn'lik tavana takılıp kırmızıya döner.
//!
//! Zaman: engine vade kontrolü `chrono::Utc::now()` (duvar saati) kullandığı
//! için tokio `start_paused` soyutlaması uygulanamaz — gerçek-zamanlı kısa
//! sürelerle koşan bir entegrasyon testidir (~3 sn).

use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;
use wf_wfe::timer::{run_timer_service, ActiveWfeSource};
use wf_wfe::WfeExecutor;
use wfe_core::ports::OrgPort;
use wfe_core::types::actor::{Actor, OrgUnit};
use wfe_core::types::dynctx::DynCtx;
use wfe_core::types::wfah::{Wfah, WfahEntry};
use wfe_core::types::wfd_v22::{AutoexecDef, AutoexecType, JoinRule, Wfd};
use wfe_core::types::wfe::WfeStatus;
use wfe_core::v22::ports::{
    AutoexecRunner, CommitOutcome, ExecEnv, ExecFailure, NewWfe, TransitionCommit, WfdStore,
    WfeStore, Wfes,
};
use wfe_core::EngineError;

const FIXTURE: &str =
    include_str!("../../wfe-core/tests/fixtures/kredi-basvuru.golden.json");

// ---- mock'lar (wfe-core/tests/pipeline.rs kalıbı) --------------------------

struct MockOrg;

#[async_trait]
impl OrgPort for MockOrg {
    async fn resolve_c_orgu(
        &self,
        anchor: Uuid,
        _expr: &str,
        _orgtnt: Uuid,
    ) -> Result<Vec<OrgUnit>, EngineError> {
        Ok(vec![OrgUnit {
            orgu_id: anchor,
            orgu_type: json!({"type": "branch"}),
            path: "1".into(),
        }])
    }
    async fn check_user_role(&self, _: Uuid, _: Uuid, _: &str) -> Result<bool, EngineError> {
        Ok(true)
    }
    async fn orgtnt_for_orgu(&self, _: Uuid) -> Result<Uuid, EngineError> {
        Ok(Uuid::nil())
    }
}

struct MockRunner;

#[async_trait]
impl AutoexecRunner for MockRunner {
    async fn run(&self, def: &AutoexecDef, _env: &ExecEnv) -> Result<Value, ExecFailure> {
        match def.kind {
            AutoexecType::Rest => Ok(json!({"score": 700, "grade": "A"})),
            AutoexecType::Calc => Ok(json!({"within_limit": true})),
            _ => Err(ExecFailure::failed("desteklenmeyen tip")),
        }
    }
}

struct FixtureWfdStore(Wfd);

#[async_trait]
impl WfdStore for FixtureWfdStore {
    async fn fetch(&self, _wfd_id: Uuid, _version: i32) -> Result<Wfd, EngineError> {
        Ok(self.0.clone())
    }
}

/// In-memory WfeStore — WfeAdapter'ın create/commit semantiğini birebir taklit
/// eder (status/current_node eşlemesi, node değişiminde assignment reset).
#[derive(Default)]
struct MemStore {
    wfes: Mutex<HashMap<Uuid, Wfes>>,
}

impl MemStore {
    fn snapshot(&self, wfe_id: Uuid) -> Option<Wfes> {
        self.wfes.lock().unwrap().get(&wfe_id).cloned()
    }
}

fn outcome_parts(outcome: &CommitOutcome) -> (WfeStatus, Option<String>, Option<Value>) {
    match outcome {
        CommitOutcome::MoveTo { node } => (WfeStatus::Active, Some(node.clone()), None),
        CommitOutcome::Terminal { end_response } => {
            (WfeStatus::Terminal, None, Some(end_response.clone()))
        }
        CommitOutcome::Failed { end_response } => {
            (WfeStatus::Error, None, Some(end_response.clone()))
        }
        CommitOutcome::Terminated { end_response } => {
            (WfeStatus::Terminated, None, Some(end_response.clone()))
        }
        // WOR-31: paralel outcome'lar aktiftir; kol durumu bu mock'ta izlenmez.
        CommitOutcome::ForkTo { .. }
        | CommitOutcome::BranchMoveTo { .. }
        | CommitOutcome::BranchArrived { .. } => (WfeStatus::Active, None, None),
        CommitOutcome::JoinComplete { next, .. } => outcome_parts(next),
        CommitOutcome::CollapseTo { node, .. } => (WfeStatus::Active, Some(node.clone()), None),
    }
}

#[async_trait]
impl WfeStore for MemStore {
    async fn load(&self, wfe_id: Uuid) -> Result<Wfes, EngineError> {
        self.snapshot(wfe_id)
            .ok_or_else(|| EngineError::WfePort(format!("not found: {wfe_id}")))
    }

    async fn create(&self, new: &NewWfe) -> Result<(), EngineError> {
        let (status, current_node, end_response) = outcome_parts(&new.outcome);
        let wfes = Wfes {
            wfe_id: new.wfe_id,
            orgtnt_id: new.orgtnt_id,
            environment_id: None,
            wfd_id: new.wfd_id,
            wfd_version: new.wfd_version,
            dynctx: DynCtx(new.initial_dynctx.clone()),
            wfah: Wfah(new.wfah_entries.clone()),
            status,
            current_node,
            end_terminal: new.end_terminal.clone(),
            assigned_to: None,
            end_response,
            deadline: new.deadline,
            claimed_at: None,
            created_at: chrono::Utc::now(),
            branches: vec![],
            join_target: None,
            join_rule: JoinRule::All,
            origin_orgu_id: None,
        };
        self.wfes.lock().unwrap().insert(new.wfe_id, wfes);
        Ok(())
    }

    async fn commit(&self, commit: &TransitionCommit) -> Result<(), EngineError> {
        let mut map = self.wfes.lock().unwrap();
        let wfes = map
            .get_mut(&commit.wfe_id)
            .ok_or_else(|| EngineError::WfePort(format!("not found: {}", commit.wfe_id)))?;
        let (status, current_node, end_response) = outcome_parts(&commit.outcome);
        wfes.dynctx = DynCtx(commit.new_dynctx.clone());
        wfes.wfah.0.extend(commit.wfah_entries.iter().cloned());
        wfes.status = status;
        wfes.current_node = current_node;
        if end_response.is_some() {
            wfes.end_response = end_response;
        }
        if commit.end_terminal.is_some() {
            wfes.end_terminal = commit.end_terminal.clone();
        }
        // M8: her node değişimi / sonlanma assignment'ı temizler
        wfes.assigned_to = None;
        wfes.claimed_at = None;
        Ok(())
    }

    async fn claim(
        &self,
        wfe_id: Uuid,
        _orgtnt_id: Uuid,
        user_id: Uuid,
        _branch: Option<&str>,
        _marker: Option<&WfahEntry>,
    ) -> Result<bool, EngineError> {
        let mut map = self.wfes.lock().unwrap();
        let Some(wfes) = map.get_mut(&wfe_id) else {
            return Ok(false);
        };
        if wfes.status != WfeStatus::Active || wfes.assigned_to.is_some() {
            return Ok(false);
        }
        wfes.assigned_to = Some(user_id);
        wfes.claimed_at = Some(chrono::Utc::now());
        Ok(true)
    }

    async fn release_claim(
        &self,
        wfe_id: Uuid,
        _orgtnt_id: Uuid,
        wfah_entry: &WfahEntry,
        _branch: Option<&str>,
        new_dynctx: Option<&serde_json::Value>,
    ) -> Result<(), EngineError> {
        let mut map = self.wfes.lock().unwrap();
        let wfes = map
            .get_mut(&wfe_id)
            .ok_or_else(|| EngineError::WfePort(format!("not found: {wfe_id}")))?;
        wfes.assigned_to = None;
        wfes.claimed_at = None;
        if let Some(ctx) = new_dynctx {
            wfes.dynctx = wfe_core::types::dynctx::DynCtx(ctx.clone());
        }
        wfes.wfah.0.push(wfah_entry.clone());
        Ok(())
    }

    async fn append_marker(
        &self,
        wfe_id: Uuid,
        _orgtnt_id: Uuid,
        wfah_entry: &WfahEntry,
    ) -> Result<(), EngineError> {
        let mut map = self.wfes.lock().unwrap();
        let wfes = map
            .get_mut(&wfe_id)
            .ok_or_else(|| EngineError::WfePort(format!("not found: {wfe_id}")))?;
        wfes.wfah.0.push(wfah_entry.clone());
        Ok(())
    }

    async fn reassign(
        &self,
        wfe_id: Uuid,
        _orgtnt_id: Uuid,
        target: Option<Uuid>,
        wfah_entry: &WfahEntry,
        _branch: Option<&str>,
    ) -> Result<(), EngineError> {
        let mut map = self.wfes.lock().unwrap();
        let wfes = map
            .get_mut(&wfe_id)
            .ok_or_else(|| EngineError::WfePort(format!("not found: {wfe_id}")))?;
        wfes.assigned_to = target;
        wfes.claimed_at = target.map(|_| chrono::Utc::now());
        wfes.wfah.0.push(wfah_entry.clone());
        Ok(())
    }
}

#[async_trait]
impl ActiveWfeSource for MemStore {
    async fn active_ids(&self) -> Result<Vec<Uuid>, EngineError> {
        Ok(self
            .wfes
            .lock()
            .unwrap()
            .values()
            .filter(|w| w.status == WfeStatus::Active)
            .map(|w| w.wfe_id)
            .collect())
    }
}

// ---- yardımcılar ------------------------------------------------------------

fn clerk() -> Actor {
    Actor {
        orgu_id: Uuid::new_v4(),
        user_id: Uuid::new_v4(),
        role: "branchClerk".into(),
    }
}

fn start_input() -> Value {
    json!({
        "applicant": {"name": "Ayşe Yılmaz", "tckid": "12345678901", "income": 30000},
        "credit_info": {"amount_requested": 30000, "purpose": "ev tadilatı"}
    })
}

// ---- test --------------------------------------------------------------------

/// 2 sn'lik deadline (SLA-3) 60 sn'lik güvenlik-ağı süpürmesi beklenmeden
/// ~2 sn'de `terminated` olmalı. Sinyal zinciri uçtan uca: start → Notify →
/// next-due hesabı → sleep(~2s) → tick_timers → fire_deadline_timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn short_deadline_fires_without_waiting_fallback_sweep() {
    let wfd = Wfd::from_json(FIXTURE).unwrap();
    let store = Arc::new(MemStore::default());
    let executor = Arc::new(WfeExecutor::new(
        Arc::new(MockOrg),
        Arc::new(FixtureWfdStore(wfd)),
        store.clone(),
        Arc::new(MockRunner),
    ));

    // Servis aktif WFE yokken başlar → FALLBACK_SWEEP (60 sn) uykusuna girer.
    // Onu 60 sn dolmadan uyandıracak TEK şey executor.start'ın sinyalidir.
    tokio::spawn(run_timer_service(executor.clone(), store.clone()));
    tokio::time::sleep(Duration::from_millis(300)).await;

    let t0 = std::time::Instant::now();
    let started = executor
        .start(
            Uuid::new_v4(),
            1,
            &clerk(),
            None,
            &start_input(),
            Some("PT2S"),
        )
        .await
        .unwrap();
    assert!(!started.terminal, "start terminal olmamalı");
    let wfe_id = started.wfe_id;

    // Deadline'dan önce hâlâ aktif olmalı — erken ateşleme yok.
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(
        store.snapshot(wfe_id).unwrap().status,
        WfeStatus::Active,
        "deadline dolmadan terminate edildi"
    );

    // 2 sn'lik SLA en geç 8 sn duvar-saati içinde ateşlenmeli (poll aralığı 60 sn'ydi).
    let cap = Duration::from_secs(8);
    loop {
        if store.snapshot(wfe_id).unwrap().status == WfeStatus::Terminated {
            break;
        }
        assert!(
            t0.elapsed() < cap,
            "2 sn'lik deadline {cap:?} içinde ateşlenmedi — event-driven timer sinyali çalışmıyor"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let elapsed = t0.elapsed();
    assert!(
        elapsed >= Duration::from_millis(1900),
        "deadline erken ateşlendi: {elapsed:?}"
    );

    let final_state = store.snapshot(wfe_id).unwrap();
    assert_eq!(
        final_state.end_response.unwrap()["reason"],
        json!("SLA.Deadline")
    );
    assert_eq!(final_state.current_node, None);
}
