//! Kapı B ve Kapı C — çalışma anı tip denetiminin EXECUTOR seviyesindeki davranışı.
//!
//! Kapı B (`pipeline::guard_written_ctx`) BU GEÇİŞİN YAZDIĞINA bakar: yeni bozulma
//! ctx'e girmesin. Kapı C (`executor::guard_stored_ctx`) ZATEN VAR OLAN duruma bakar:
//! bozuk veriyle iş yapılmasın — ama GÖRÜNTÜLEME serbesttir ve ihlaller
//! `WfeView.ctx_violations` ile BİLDİRİLİR (kayıt görünmez olursa düzeltilemez).
//!
//! Mock kalıbı `view_grants_wfah_anchor.rs`ten alınmıştır (in-memory store, sahte org).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{json, Value};
use uuid::Uuid;
use wf_wfe::WfeExecutor;
use wfe_core::ports::OrgPort;
use wfe_core::types::actor::{Actor, CandidateActor as ResolvedCandidate, OrgUnit};
use wfe_core::types::dynctx::DynCtx;
use wfe_core::types::wfah::{Wfah, WfahEntry};
use wfe_core::types::wfd_v22::{AutoexecDef, JoinRule, Wfd};
use wfe_core::types::wfe::WfeStatus;
use wfe_core::v22::ports::{
    AutoexecRunner, CommitOutcome, ExecEnv, ExecFailure, NewWfe, TransitionCommit, WfdStore,
    WfeStore, Wfes,
};
use wfe_core::EngineError;

/// Tek node + tek aksiyon; `tutar` NUMBER olarak bildirilmiş.
fn wfd_json() -> Value {
    json!({
        "wfd_version": "2.2",
        "id": "tip-kapisi",
        "name": "Tip Kapısı",
        "version": "1.0.0",
        "context": { "type": "object", "properties": {
            "tutar": { "type": "number" },
            "not": { "type": "string" }
        }},
        "nodes": {
            "self__memur": { "c_a": { "c_orgu": "self", "c_r": ["memur"] } },
            "self__mudur": { "c_a": { "c_orgu": "self", "c_r": ["mudur"] } }
        },
        "start": [{ "id": "s1", "from": "self__memur", "action": "basvur",
                    "wfes_effects": { "set": { "tutar": "$action.input.tutar" } },
                    "wft": { "node": "self__mudur" } }],
        "actions": {
            "basvur": { "input": { "required": ["tutar"], "optional": [] } },
            "onayla": { "input": { "required": ["not"], "optional": [] } }
        },
        "transitions": [{ "id": "t1", "from": "self__mudur", "action": "onayla",
                          "wfes_effects": { "set": { "not": "$action.input.not" } },
                          "wft": { "terminal": "bitti" } }],
        "terminals": [{ "id": "bitti", "wfe_end_response": {} }]
    })
}

/// `c_orgu: "self"` çapası WFE'nin BAŞLATILDIĞI birimdir (`origin_orgu_id`), soranın
/// birimi değil — bu yüzden testteki memur ve müdür AYNI şubede olmalı, yoksa müdür
/// kendi node'unu bile göremez (doğru davranış, bkz. `matcher::authorize_anchored`).
fn actor_in(orgu_id: Uuid, role: &str) -> Actor {
    Actor {
        orgu_id,
        user_id: Uuid::new_v4(),
        role: role.into(),
    }
}

struct MockOrg;
#[async_trait]
impl OrgPort for MockOrg {
    async fn resolve_c_orgu(
        &self,
        anchor: Uuid,
        _e: &str,
        _t: Uuid,
    ) -> Result<Vec<OrgUnit>, EngineError> {
        Ok(vec![OrgUnit {
            orgu_id: anchor,
            orgu_type: json!({ "type": "branch" }),
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
    async fn run(&self, _d: &AutoexecDef, _e: &ExecEnv) -> Result<Value, ExecFailure> {
        Ok(json!({}))
    }
}

struct FixtureWfdStore(Wfd);
#[async_trait]
impl WfdStore for FixtureWfdStore {
    async fn fetch(&self, _wfd_id: Uuid, _version: i32) -> Result<Wfd, EngineError> {
        Ok(self.0.clone())
    }
}

/// In-memory store — `view_c_a` kolonunu da tutar (adapter onu aynı tx'te yazar).
#[derive(Default)]
struct MemStore {
    wfes: Mutex<HashMap<Uuid, Wfes>>,
    view_c_a: Mutex<HashMap<Uuid, Vec<ResolvedCandidate>>>,
}

impl MemStore {
    fn snapshot(&self, wfe_id: Uuid) -> Option<Wfes> {
        self.wfes.lock().unwrap().get(&wfe_id).cloned()
    }
    fn view_grants(&self, wfe_id: Uuid) -> Vec<ResolvedCandidate> {
        self.view_c_a
            .lock()
            .unwrap()
            .get(&wfe_id)
            .cloned()
            .unwrap_or_default()
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
            // Projeksiyon çapası: WFE'nin kendi birimi (start'ta donar).
            origin_orgu_id: Some(new.origin_orgu_id),
        };
        self.wfes.lock().unwrap().insert(new.wfe_id, wfes);
        self.view_c_a
            .lock()
            .unwrap()
            .insert(new.wfe_id, new.view_c_a.clone());
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
        wfes.assigned_to = None;
        wfes.claimed_at = None;
        // `view_c_a` KALICI grant kolonudur — terminalde de yazılır (silinmez).
        self.view_c_a
            .lock()
            .unwrap()
            .insert(commit.wfe_id, commit.view_c_a.clone());
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
        new_dynctx: Option<&Value>,
    ) -> Result<(), EngineError> {
        let mut map = self.wfes.lock().unwrap();
        let wfes = map
            .get_mut(&wfe_id)
            .ok_or_else(|| EngineError::WfePort(format!("not found: {wfe_id}")))?;
        wfes.assigned_to = None;
        wfes.claimed_at = None;
        if let Some(ctx) = new_dynctx {
            wfes.dynctx = DynCtx(ctx.clone());
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


impl MemStore {
    /// Testin ctx'i BOZMASI için: enforcement öncesi yazılmış bir değeri taklit eder.
    fn corrupt(&self, wfe_id: Uuid, field: &str, value: Value) {
        let mut map = self.wfes.lock().unwrap();
        let wfes = map.get_mut(&wfe_id).expect("wfe yok");
        let mut ctx = wfes.dynctx.as_value().clone();
        ctx[field] = value;
        wfes.dynctx = DynCtx(ctx);
    }
}

fn harness() -> (WfeExecutor, Arc<MemStore>) {
    let store = Arc::new(MemStore::default());
    let executor = WfeExecutor::new(
        Arc::new(MockOrg),
        Arc::new(FixtureWfdStore(Wfd::from_value(wfd_json()).unwrap())),
        store.clone(),
        Arc::new(MockRunner),
    );
    (executor, store)
}

/// Kapı B — start'ta yanlış tipli girdi ctx'e GİRMEZ (kapı A da aynı değeri reddeder;
/// bu test zincirin sonuçta hiçbir şey YAZMADIĞINI kanıtlar).
#[tokio::test]
async fn wrong_typed_start_writes_nothing() {
    let (executor, store) = harness();
    let memur = actor_in(Uuid::new_v4(), "memur");
    let err = executor
        .start(Uuid::new_v4(), 1, &memur, Some("basvur"), &json!({ "tutar": "yüz" }), None)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::InputTypeMismatch(v) if v[0].path == "tutar"),
        "{err}"
    );
    assert!(store.wfes.lock().unwrap().is_empty(), "reddedilen start WFE yaratmamalı");
}

/// Kapı C — ctx'i ELLE bozulmuş bir WFE'de aksiyon REDDEDİLİR.
#[tokio::test]
async fn action_on_corrupt_ctx_is_rejected() {
    let (executor, store) = harness();
    let sube = Uuid::new_v4();
    let memur = actor_in(sube, "memur");
    let mudur = actor_in(sube, "mudur");
    let started = executor
        .start(Uuid::new_v4(), 1, &memur, Some("basvur"), &json!({ "tutar": 100 }), None)
        .await
        .unwrap();

    // Enforcement öncesinden kalmış bozuk değeri taklit et: `tutar` şemada number.
    store.corrupt(started.wfe_id, "tutar", json!("yüz bin"));

    let err = executor
        .apply(started.wfe_id, &mudur, "onayla", &json!({ "not": "ok" }), None, None, None)
        .await
        .unwrap_err();
    match &err {
        EngineError::CtxTypeMismatch(v) => assert_eq!(v[0].path, "tutar"),
        other => panic!("ctx tip ihlali beklendi: {other}"),
    }
}

/// Kapı C — claim de bir EYLEMDİR: kişi işi üstlenip ilk aksiyonda 422 almasın.
#[tokio::test]
async fn claim_on_corrupt_ctx_is_rejected() {
    let (executor, store) = harness();
    let sube = Uuid::new_v4();
    let memur = actor_in(sube, "memur");
    let mudur = actor_in(sube, "mudur");
    let started = executor
        .start(Uuid::new_v4(), 1, &memur, Some("basvur"), &json!({ "tutar": 100 }), None)
        .await
        .unwrap();
    store.corrupt(started.wfe_id, "tutar", json!("yüz bin"));

    let err = executor.claim(started.wfe_id, &mudur, None, None).await.unwrap_err();
    assert!(matches!(&err, EngineError::CtxTypeMismatch(_)), "{err}");
}

/// Kapı C — GÖRÜNTÜLEME serbest: `query` 200 döner ve ihlalleri BİLDİRİR. Kayıt
/// görünmez olsaydı kullanıcı neyin bozuk olduğunu göremez ve düzeltemezdi.
#[tokio::test]
async fn reading_a_corrupt_wfe_succeeds_and_reports_violations() {
    let (executor, store) = harness();
    let sube = Uuid::new_v4();
    let memur = actor_in(sube, "memur");
    let mudur = actor_in(sube, "mudur");
    let started = executor
        .start(Uuid::new_v4(), 1, &memur, Some("basvur"), &json!({ "tutar": 100 }), None)
        .await
        .unwrap();
    store.corrupt(started.wfe_id, "tutar", json!("yüz bin"));

    // Görünürlük bu testin konusu değil: müdür işi ÜSTLENİR (sahiplik kriteri (a)),
    // sonra ctx bozulur. Sıra önemli — claim de kapı C'den geçiyor.
    let view = executor
        .query(started.wfe_id, &mudur)
        .await
        .expect("bozuk ctx GÖRÜNTÜLENEBİLİR olmalı");
    assert_eq!(view.ctx_violations.len(), 1, "{:?}", view.ctx_violations);
    assert_eq!(view.ctx_violations[0].path, "tutar");
    assert!(view.ctx_violations[0].expected.contains("number"));
}

/// Temiz bir WFE'de ihlal listesi BOŞ — alan hiç serileşmez.
#[tokio::test]
async fn healthy_wfe_reports_no_violations() {
    let (executor, store) = harness();
    let sube = Uuid::new_v4();
    let memur = actor_in(sube, "memur");
    let mudur = actor_in(sube, "mudur");
    let started = executor
        .start(Uuid::new_v4(), 1, &memur, Some("basvur"), &json!({ "tutar": 100 }), None)
        .await
        .unwrap();
    let view = executor.query(started.wfe_id, &mudur).await.unwrap();
    assert!(view.ctx_violations.is_empty(), "{:?}", view.ctx_violations);
}
