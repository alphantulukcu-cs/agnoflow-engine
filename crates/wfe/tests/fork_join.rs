//! WOR-31 T3 — paralel fork/join persistence + executor orkestrasyon testleri.
//!
//! DB'siz: `WfeAdapter`'ın paralel commit semantiğini (ForkTo/BranchMoveTo/
//! BranchArrived/JoinComplete/terminal-cancel + kol CAS + Conflict) birebir taklit
//! eden in-memory bir `WfeStore` (`ParStore`) üzerinden `WfeExecutor` sürülür.
//! Engine saf mantığı `wfe-core/tests/pipeline.rs`'te; burada ORKESTRASYON test
//! edilir: fork→claim→varış→join→finalize akışı, red yolu, Conflict retry döngüsü,
//! kol-bazlı timer next-due.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use uuid::Uuid;
use wf_wfe::WfeExecutor;
use wfe_core::ports::OrgPort;
use wfe_core::types::actor::{Actor, OrgUnit};
use wfe_core::types::dynctx::DynCtx;
use wfe_core::types::wfah::{Wfah, WfahEntry};
use wfe_core::types::wfd_v22::{AutoexecDef, Wfd, WftTarget};
use wfe_core::types::wfe::WfeStatus;
use wfe_core::v22::ports::{
    AutoexecRunner, BranchState, BranchStatus, CommitOutcome, ExecEnv, ExecFailure, NewWfe,
    TransitionCommit, WfdStore, WfeStore, Wfes,
};
use wfe_core::EngineError;

const PARALLEL_FIXTURE: &str =
    include_str!("../../wfe-core/tests/fixtures/example-wfd_paralel-onay_v2_2.json");

// ---- mock'lar (pipeline.rs kalıbı; authorize anchor = actor.orgu_id) ----------

struct MockOrg;

#[async_trait]
impl OrgPort for MockOrg {
    async fn resolve_c_orgu(
        &self,
        anchor: Uuid,
        _expr: &str,
        _orgtnt: Uuid,
    ) -> Result<Vec<OrgUnit>, EngineError> {
        // "self" anchor'u authorize'da actor.orgu_id ile çağrılır → daima eşleşir.
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
    async fn run(&self, _def: &AutoexecDef, _env: &ExecEnv) -> Result<Value, ExecFailure> {
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

// ---- paralel-farkında in-memory store (WfeAdapter semantiğinin taklidi) --------

#[derive(Default)]
struct ParStore {
    wfes: Mutex<HashMap<Uuid, Wfes>>,
    /// Test enjeksiyonu: >0 iken her `commit` çağrısı state'i DEĞİŞTİRMEDEN
    /// Conflict döner (adapter'ın FOR UPDATE/CAS uyumsuzluğunun karşılığı) —
    /// executor retry döngüsünü doğrulamak için.
    fail_commits: AtomicU32,
}

impl ParStore {
    fn snapshot(&self, wfe_id: Uuid) -> Wfes {
        self.wfes.lock().unwrap().get(&wfe_id).cloned().unwrap()
    }
    fn seed(&self, wfes: Wfes) {
        self.wfes.lock().unwrap().insert(wfes.wfe_id, wfes);
    }
    /// Bir kolun claimed_at'ını geçmişe alır (claim_timeout'u tetiklemek için).
    fn rewind_branch_claim(&self, wfe_id: Uuid, node: &str, at: chrono::DateTime<chrono::Utc>) {
        let mut m = self.wfes.lock().unwrap();
        let w = m.get_mut(&wfe_id).unwrap();
        for b in &mut w.branches {
            if b.branch_node == node {
                b.claimed_at = Some(at);
            }
        }
    }
}

fn active_count(w: &Wfes) -> usize {
    w.branches
        .iter()
        .filter(|b| b.status == BranchStatus::Active)
        .count()
}

/// `WfeAdapter::cancel_active_branches` taklidi — WOR-59: statü ile BİRLİKTE
/// claim de düşer (aksi halde iptal edilmiş kol "hâlâ birine atanmış" görünür).
fn cancel_active_branches(w: &mut Wfes) {
    for b in &mut w.branches {
        if b.status == BranchStatus::Active {
            b.status = BranchStatus::Cancelled;
            b.claimed_by = None;
            b.claimed_at = None;
        }
    }
}

fn apply_next(w: &mut Wfes, next: &CommitOutcome) {
    match next {
        CommitOutcome::MoveTo { node } => {
            w.current_node = Some(node.clone());
            w.join_target = None;
            w.branches.clear();
            w.assigned_to = None;
            w.claimed_at = None;
        }
        CommitOutcome::Terminal { end_response } => {
            w.status = WfeStatus::Terminal;
            w.current_node = None;
            w.end_response = Some(end_response.clone());
            w.join_target = None;
            w.branches.clear();
        }
        other => panic!("JoinComplete.next beklenmeyen: {other:?}"),
    }
}

#[async_trait]
impl WfeStore for ParStore {
    async fn load(&self, wfe_id: Uuid) -> Result<Wfes, EngineError> {
        self.wfes
            .lock()
            .unwrap()
            .get(&wfe_id)
            .cloned()
            .ok_or_else(|| EngineError::WfePort(format!("not found: {wfe_id}")))
    }

    async fn create(&self, new: &NewWfe) -> Result<(), EngineError> {
        let (status, current_node, end_response) = match &new.outcome {
            CommitOutcome::MoveTo { node } => (WfeStatus::Active, Some(node.clone()), None),
            CommitOutcome::Terminal { end_response } => {
                (WfeStatus::Terminal, None, Some(end_response.clone()))
            }
            other => panic!("create paralel outcome almamalı: {other:?}"),
        };
        let w = Wfes {
            wfe_id: new.wfe_id,
            orgtnt_id: new.orgtnt_id,
            wfd_id: new.wfd_id,
            wfd_version: new.wfd_version,
            dynctx: DynCtx(new.initial_dynctx.clone()),
            wfah: Wfah(new.wfah_entries.clone()),
            status,
            current_node,
            assigned_to: None,
            end_response,
            deadline: new.deadline,
            claimed_at: None,
            created_at: chrono::Utc::now(),
            branches: vec![],
            join_target: None,
        };
        self.seed(w);
        Ok(())
    }

    async fn commit(&self, commit: &TransitionCommit) -> Result<(), EngineError> {
        if self.fail_commits.load(Ordering::SeqCst) > 0 {
            self.fail_commits.fetch_sub(1, Ordering::SeqCst);
            return Err(EngineError::Conflict);
        }
        let mut map = self.wfes.lock().unwrap();
        let w = map
            .get_mut(&commit.wfe_id)
            .ok_or_else(|| EngineError::WfePort("not found".into()))?;
        w.dynctx = DynCtx(commit.new_dynctx.clone());
        w.wfah.0.extend(commit.wfah_entries.iter().cloned());

        match &commit.outcome {
            CommitOutcome::MoveTo { node } => {
                w.current_node = Some(node.clone());
                w.assigned_to = None;
                w.claimed_at = None;
            }
            CommitOutcome::Terminal { end_response }
            | CommitOutcome::Failed { end_response }
            | CommitOutcome::Terminated { end_response } => {
                w.status = match &commit.outcome {
                    CommitOutcome::Terminal { .. } => WfeStatus::Terminal,
                    CommitOutcome::Failed { .. } => WfeStatus::Error,
                    _ => WfeStatus::Terminated,
                };
                w.current_node = None;
                w.end_response = Some(end_response.clone());
                w.assigned_to = None;
                w.claimed_at = None;
                // paralel modda aktif kolları iptal et + join_target temizle
                cancel_active_branches(w);
                w.join_target = None;
            }
            CommitOutcome::ForkTo { branches, join } => {
                w.current_node = None;
                w.assigned_to = None;
                w.claimed_at = None;
                w.join_target = Some(join.clone());
                let now = chrono::Utc::now();
                w.branches = branches
                    .iter()
                    .map(|n| BranchState {
                        branch_node: n.clone(),
                        status: BranchStatus::Active,
                        claimed_by: None,
                        claimed_at: None,
                        entered_at: now,
                    })
                    .collect();
            }
            CommitOutcome::BranchMoveTo { from_node, node } => {
                let Some(b) = w
                    .branches
                    .iter_mut()
                    .find(|b| b.status == BranchStatus::Active && &b.branch_node == from_node)
                else {
                    return Err(EngineError::Conflict);
                };
                b.branch_node = node.clone();
                b.claimed_by = None;
                b.claimed_at = None;
                b.entered_at = chrono::Utc::now();
            }
            CommitOutcome::BranchArrived { from_node } => {
                let remaining = active_count(w);
                let Some(b) = w
                    .branches
                    .iter_mut()
                    .find(|b| b.status == BranchStatus::Active && &b.branch_node == from_node)
                else {
                    return Err(EngineError::Conflict);
                };
                b.status = BranchStatus::Arrived;
                b.claimed_by = None;
                b.claimed_at = None;
                if remaining <= 1 {
                    return Err(EngineError::Conflict);
                }
            }
            CommitOutcome::JoinComplete { from_node, next } => {
                let remaining = active_count(w);
                let Some(b) = w
                    .branches
                    .iter_mut()
                    .find(|b| b.status == BranchStatus::Active && &b.branch_node == from_node)
                else {
                    return Err(EngineError::Conflict);
                };
                b.status = BranchStatus::Arrived;
                if remaining != 1 {
                    return Err(EngineError::Conflict);
                }
                // `_join` marker (adapter istisnası)
                let seq = commit.wfah_entries.last().map(|e| e.seq + 1).unwrap_or(1);
                w.wfah.0.push(WfahEntry {
                    seq,
                    action: "_join".into(),
                    actor: Actor {
                        orgu_id: Uuid::nil(),
                        user_id: Uuid::nil(),
                        role: "system".into(),
                    },
                    input: None,
                    applied_at: chrono::Utc::now(),
                });
                apply_next(w, next);
            }
            CommitOutcome::CollapseTo { node, .. } => {
                // WOR-56: paralel mod biter, WFE `node`'a; aktif kollar iptal.
                cancel_active_branches(w);
                w.join_target = None;
                w.current_node = Some(node.clone());
                w.assigned_to = None;
                w.claimed_at = None;
            }
        }
        Ok(())
    }

    async fn claim(
        &self,
        wfe_id: Uuid,
        _orgtnt_id: Uuid,
        user_id: Uuid,
        branch: Option<&str>,
    ) -> Result<bool, EngineError> {
        let mut map = self.wfes.lock().unwrap();
        let Some(w) = map.get_mut(&wfe_id) else {
            return Ok(false);
        };
        if w.status != WfeStatus::Active {
            return Ok(false);
        }
        match branch {
            Some(node) => {
                let Some(b) = w
                    .branches
                    .iter_mut()
                    .find(|b| b.status == BranchStatus::Active && b.branch_node == node)
                else {
                    return Ok(false);
                };
                if b.claimed_by.is_some() {
                    return Ok(false);
                }
                b.claimed_by = Some(user_id);
                b.claimed_at = Some(chrono::Utc::now());
                Ok(true)
            }
            None => {
                if w.assigned_to.is_some() {
                    return Ok(false);
                }
                w.assigned_to = Some(user_id);
                w.claimed_at = Some(chrono::Utc::now());
                Ok(true)
            }
        }
    }

    async fn release_claim(
        &self,
        wfe_id: Uuid,
        _orgtnt_id: Uuid,
        wfah_entry: &WfahEntry,
        branch: Option<&str>,
    ) -> Result<(), EngineError> {
        let mut map = self.wfes.lock().unwrap();
        let w = map.get_mut(&wfe_id).unwrap();
        match branch {
            Some(node) => {
                for b in &mut w.branches {
                    if b.status == BranchStatus::Active && b.branch_node == node {
                        b.claimed_by = None;
                        b.claimed_at = None;
                    }
                }
            }
            None => {
                w.assigned_to = None;
                w.claimed_at = None;
            }
        }
        w.wfah.0.push(wfah_entry.clone());
        Ok(())
    }
}

// ---- yardımcılar --------------------------------------------------------------

fn actor(role: &str) -> Actor {
    Actor {
        orgu_id: Uuid::new_v4(),
        user_id: Uuid::new_v4(),
        role: role.into(),
    }
}

fn executor(store: Arc<ParStore>) -> WfeExecutor {
    let wfd = Wfd::from_json(PARALLEL_FIXTURE).unwrap();
    WfeExecutor::new(
        Arc::new(MockOrg),
        Arc::new(FixtureWfdStore(wfd)),
        store,
        Arc::new(MockRunner),
    )
}

/// start → coordinator; claim coordinator; start_review → fork. wfe_id döner.
async fn fork_setup(exec: &WfeExecutor) -> Uuid {
    // start kuralı: self__requester (rol=requester) → wft self__coordinator.
    let requester = actor("requester");
    let start_input = json!({"request": {"title": "Sunucu alımı", "amount": 150000}});
    let started = exec
        .start(Uuid::new_v4(), 1, &requester, None, &start_input, None)
        .await
        .unwrap();
    let wfe_id = started.wfe_id;
    assert_eq!(started.current_node.as_deref(), Some("self__coordinator"));
    let coord = actor("coordinator");
    let c = exec.claim(wfe_id, &coord, None).await.unwrap();
    assert!(c.success, "coordinator claim");
    let res = exec
        .apply(wfe_id, &coord, "start_review", &json!({}), None)
        .await
        .unwrap();
    assert!(!res.terminal);
    assert_eq!(res.current_node, None, "fork sonrası wfe-seviyesi node yok");
    wfe_id
}

// ---- testler ------------------------------------------------------------------

/// Uçtan uca mutlu yol: fork → 3 kol claim → 3 approve (2 varış + son join) →
/// join node'a promote → finalize → terminal_approved.
#[tokio::test]
async fn happy_path_fork_join_finalize() {
    let store = Arc::new(ParStore::default());
    let exec = executor(store.clone());
    let wfe_id = fork_setup(&exec).await;

    assert_eq!(active_count(&store.snapshot(wfe_id)), 3);
    assert!(store.snapshot(wfe_id).join_target.is_some());

    let branches = [
        ("financeApprover", "self__financeApprover"),
        ("legalApprover", "self__legalApprover"),
        ("hrApprover", "self__hrApprover"),
    ];
    // her kolu claim et
    for (role, node) in branches {
        let a = actor(role);
        let c = exec.claim(wfe_id, &a, Some(node)).await.unwrap();
        assert!(c.success, "{node} claim");
    }

    // ilk iki approve → BranchArrived (wfe hâlâ paralel, aktif)
    for (role, node) in &branches[..2] {
        let a = claim_owner(&store, wfe_id, node);
        let a = Actor { role: (*role).into(), ..a };
        let r = exec.apply(wfe_id, &a, "approve", &json!({}), Some(node)).await.unwrap();
        assert!(!r.terminal);
        assert_eq!(r.current_node, None);
    }
    assert_eq!(active_count(&store.snapshot(wfe_id)), 1);

    // son approve → JoinComplete → MoveTo join node
    let (role, node) = branches[2];
    let a = claim_owner(&store, wfe_id, node);
    let a = Actor { role: role.into(), ..a };
    let r = exec.apply(wfe_id, &a, "approve", &json!({}), Some(node)).await.unwrap();
    assert!(!r.terminal, "join node terminal değil");
    let w = store.snapshot(wfe_id);
    assert_eq!(w.current_node.as_deref(), Some("self__resultCoordinator"));
    assert!(w.join_target.is_none(), "paralel mod bitti");
    assert!(w.branches.is_empty(), "kollar temizlendi");
    assert!(w.wfah.entries().iter().any(|e| e.action == "_join"), "_join marker");

    // finalize → terminal_approved
    let rc = actor("resultCoordinator");
    let c = exec.claim(wfe_id, &rc, None).await.unwrap();
    assert!(c.success, "resultCoordinator claim");
    let r = exec.apply(wfe_id, &rc, "finalize", &json!({}), None).await.unwrap();
    assert!(r.terminal, "finalize terminal olmalı");
    assert_eq!(store.snapshot(wfe_id).status, WfeStatus::Terminal);
}

/// Bir kolun reddi TÜM WFE'yi terminal_rejected'a alır; kardeş kollar iptal
/// (`_branch_cancelled` marker'ları engine tarafından staged).
#[tokio::test]
async fn branch_reject_terminates_and_cancels_siblings() {
    let store = Arc::new(ParStore::default());
    let exec = executor(store.clone());
    let wfe_id = fork_setup(&exec).await;

    let fin = actor("financeApprover");
    exec.claim(wfe_id, &fin, Some("self__financeApprover")).await.unwrap();
    let r = exec
        .apply(wfe_id, &fin, "reject", &json!({}), Some("self__financeApprover"))
        .await
        .unwrap();
    assert!(r.terminal, "red WFE-terminal");

    let w = store.snapshot(wfe_id);
    assert_eq!(w.status, WfeStatus::Terminal);
    assert!(w.join_target.is_none());
    // aktif kol kalmamalı — reddeden arrived/cancel, kardeşler cancelled
    assert_eq!(active_count(&w), 0);
    let cancels = w
        .wfah
        .entries()
        .iter()
        .filter(|e| e.action == "_branch_cancelled")
        .count();
    assert_eq!(cancels, 2, "iki kardeş kol iptal marker'ı");
}

/// WOR-59: collapse (burada branch-reject → WFE-terminal) kardeş kolların
/// claim'ini DB'de düşürür ve düşen claim `_branch_cancelled` marker'ına yazılır.
#[tokio::test]
async fn collapse_drops_sibling_claims_and_records_them() {
    let store = Arc::new(ParStore::default());
    let exec = executor(store.clone());
    let wfe_id = fork_setup(&exec).await;

    // üç kolun da claim'i alınır — reject edilince ikisi iptal olacak
    for (role, node) in [
        ("financeApprover", "self__financeApprover"),
        ("legalApprover", "self__legalApprover"),
        ("hrApprover", "self__hrApprover"),
    ] {
        let a = actor(role);
        assert!(exec.claim(wfe_id, &a, Some(node)).await.unwrap().success, "{node} claim");
    }
    let legal_owner = claim_owner(&store, wfe_id, "self__legalApprover").user_id;
    let hr_owner = claim_owner(&store, wfe_id, "self__hrApprover").user_id;

    let fin = claim_owner(&store, wfe_id, "self__financeApprover");
    let fin = Actor { role: "financeApprover".into(), ..fin };
    let r = exec
        .apply(wfe_id, &fin, "reject", &json!({}), Some("self__financeApprover"))
        .await
        .unwrap();
    assert!(r.terminal);

    let w = store.snapshot(wfe_id);
    // KABUL 1: cancel sonrası hiçbir kolda asılı claim kalmaz
    for b in &w.branches {
        assert!(
            b.claimed_by.is_none() && b.claimed_at.is_none(),
            "kol {} claim'i düşmemiş: {:?}",
            b.branch_node,
            b.claimed_by
        );
    }

    // KABUL 2: marker düşen claim'in sahibini + claimed_at'ini taşır
    let cancels: Vec<&WfahEntry> = w
        .wfah
        .entries()
        .iter()
        .filter(|e| e.action == "_branch_cancelled")
        .collect();
    assert_eq!(cancels.len(), 2, "iki kardeş kol iptal marker'ı");
    for m in cancels {
        let input = m.input.as_ref().unwrap();
        let expected = match input["node"].as_str().unwrap() {
            "self__legalApprover" => legal_owner,
            "self__hrApprover" => hr_owner,
            other => panic!("beklenmeyen kol: {other}"),
        };
        assert_eq!(input["claimed_by"], json!(expected), "marker claim sahibi");
        assert!(!input["claimed_at"].is_null(), "marker claimed_at taşımalı");
    }
}

/// WOR-60: onaylanıp `arrived` olmuş kol, kardeşten gelen collapse ile
/// geçersizleşir → `_branch_superseded` marker'ı (onaylayan + onay zamanı ile).
#[tokio::test]
async fn arrived_branch_gets_superseded_marker_on_collapse() {
    let store = Arc::new(ParStore::default());
    let exec = executor(store.clone());
    let wfe_id = fork_setup(&exec).await;

    for (role, node) in [
        ("financeApprover", "self__financeApprover"),
        ("legalApprover", "self__legalApprover"),
        ("hrApprover", "self__hrApprover"),
    ] {
        let a = actor(role);
        assert!(exec.claim(wfe_id, &a, Some(node)).await.unwrap().success, "{node} claim");
    }

    // legal onaylar → kol `arrived` (hâlâ 2 aktif kol var, join tamamlanmaz)
    let legal = claim_owner(&store, wfe_id, "self__legalApprover");
    let legal = Actor { role: "legalApprover".into(), ..legal };
    let r = exec
        .apply(wfe_id, &legal, "approve", &json!({}), Some("self__legalApprover"))
        .await
        .unwrap();
    assert!(!r.terminal);
    let arrived = store
        .snapshot(wfe_id)
        .branches
        .iter()
        .find(|b| b.branch_node == "self__legalApprover")
        .map(|b| b.status)
        .unwrap();
    assert_eq!(arrived, BranchStatus::Arrived);

    // finance reddeder → WFE-terminal; legal'in ONAYI boşa gitti
    let fin = claim_owner(&store, wfe_id, "self__financeApprover");
    let fin = Actor { role: "financeApprover".into(), ..fin };
    exec.apply(wfe_id, &fin, "reject", &json!({}), Some("self__financeApprover"))
        .await
        .unwrap();

    let w = store.snapshot(wfe_id);
    // kol satırı `arrived` KALIR (yeni statü yok — bkz. DECISIONS_v2_2.md)
    let legal_row = w
        .branches
        .iter()
        .find(|b| b.branch_node == "self__legalApprover")
        .unwrap();
    assert_eq!(legal_row.status, BranchStatus::Arrived);

    let superseded: Vec<&WfahEntry> = w
        .wfah
        .entries()
        .iter()
        .filter(|e| e.action == "_branch_superseded")
        .collect();
    assert_eq!(superseded.len(), 1, "yalnız arrived kol superseded marker'ı alır");
    let input = superseded[0].input.as_ref().unwrap();
    assert_eq!(input["node"], json!("self__legalApprover"));
    assert_eq!(input["reason"], json!("sibling_terminal"));
    assert_eq!(input["approved_by"]["user_id"], json!(legal.user_id));
    assert_eq!(input["approved_by"]["role"], json!("legalApprover"));
    assert!(!input["approved_at"].is_null(), "onay zamanı taşınmalı");
    // hr hâlâ aktifti → cancelled marker'ı; iki marker karışmaz
    let cancels: Vec<&WfahEntry> = w
        .wfah
        .entries()
        .iter()
        .filter(|e| e.action == "_branch_cancelled")
        .collect();
    assert_eq!(cancels.len(), 1);
    assert_eq!(cancels[0].input.as_ref().unwrap()["node"], json!("self__hrApprover"));
}

/// Executor retry döngüsü: adapter Conflict döndürdüğünde reload + engine
/// yeniden koşulur. İlk commit Conflict, ikinci başarı → apply başarılı.
#[tokio::test]
async fn apply_retries_on_conflict_then_succeeds() {
    let store = Arc::new(ParStore::default());
    let exec = executor(store.clone());
    let wfe_id = fork_setup(&exec).await;
    let fin = actor("financeApprover");
    exec.claim(wfe_id, &fin, Some("self__financeApprover")).await.unwrap();

    // ilk commit çağrısı Conflict → executor reload edip yeniden dener
    store.fail_commits.store(1, Ordering::SeqCst);
    let r = exec
        .apply(wfe_id, &fin, "approve", &json!({}), Some("self__financeApprover"))
        .await
        .unwrap();
    assert!(!r.terminal);
    assert_eq!(store.fail_commits.load(Ordering::SeqCst), 0, "bir kez conflict tüketildi");
    // varış işlendi: bu kol artık aktif değil
    assert_eq!(active_count(&store.snapshot(wfe_id)), 2);
}

/// 3 denemenin hepsi Conflict → apply Conflict ile döner (sonsuz döngü yok).
#[tokio::test]
async fn apply_gives_up_after_three_conflicts() {
    let store = Arc::new(ParStore::default());
    let exec = executor(store.clone());
    let wfe_id = fork_setup(&exec).await;
    let fin = actor("financeApprover");
    exec.claim(wfe_id, &fin, Some("self__financeApprover")).await.unwrap();

    store.fail_commits.store(5, Ordering::SeqCst);
    let err = exec
        .apply(wfe_id, &fin, "approve", &json!({}), Some("self__financeApprover"))
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Conflict));
    // tam 3 deneme tüketildi (5 - 3 = 2 kaldı)
    assert_eq!(store.fail_commits.load(Ordering::SeqCst), 2);
}

/// Kol-bazlı claim CAS: ikinci claim aynı kolu alamaz (already_claimed).
#[tokio::test]
async fn branch_claim_is_exclusive_per_branch() {
    let store = Arc::new(ParStore::default());
    let exec = executor(store.clone());
    let wfe_id = fork_setup(&exec).await;

    let a1 = actor("financeApprover");
    let a2 = actor("financeApprover");
    assert!(exec.claim(wfe_id, &a1, Some("self__financeApprover")).await.unwrap().success);
    let second = exec.claim(wfe_id, &a2, Some("self__financeApprover")).await.unwrap();
    assert!(!second.success);
    assert_eq!(second.reason.as_deref(), Some("already_claimed"));

    // farklı kol hâlâ claim edilebilir
    assert!(exec.claim(wfe_id, &actor("legalApprover"), Some("self__legalApprover")).await.unwrap().success);
}

// ---- timer kol iterasyonu -----------------------------------------------------

/// Bir kol node'una claim_timeout ekli varyant WFD.
fn paralel_with_branch_claim_timeout() -> Wfd {
    let mut v: Value = serde_json::from_str(PARALLEL_FIXTURE).unwrap();
    v["nodes"]["self__financeApprover"]["claim_timeout"] =
        json!({ "after": "PT10M" });
    Wfd::from_value(v).unwrap()
}

fn seed_parallel_state(store: &ParStore, wfd: &Wfd, claimed_branch: &str, claimant: Uuid) -> Uuid {
    let _ = wfd;
    let wfe_id = Uuid::new_v4();
    let now = chrono::Utc::now();
    let mk = |node: &str, claimed: Option<Uuid>| BranchState {
        branch_node: node.into(),
        status: BranchStatus::Active,
        claimed_by: claimed,
        claimed_at: claimed.map(|_| now),
        entered_at: now,
    };
    let branches = vec![
        mk("self__financeApprover", if claimed_branch == "self__financeApprover" { Some(claimant) } else { None }),
        mk("self__legalApprover", None),
        mk("self__hrApprover", None),
    ];
    store.seed(Wfes {
        wfe_id,
        orgtnt_id: Uuid::nil(),
        wfd_id: Uuid::new_v4(),
        wfd_version: 1,
        dynctx: DynCtx(json!({})),
        wfah: Wfah(vec![]),
        status: WfeStatus::Active,
        current_node: None,
        assigned_to: None,
        end_response: None,
        deadline: None,
        claimed_at: None,
        created_at: now,
        branches,
        join_target: Some(WftTarget::Node { node: "self__resultCoordinator".into() }),
    });
    wfe_id
}

/// next_timer_due paralel modda AKTİF KOLLAR üzerinden döner: claim_timeout'lu
/// kolun claim deadline'ı (claimed_at + PT10M) döndürülmeli.
#[tokio::test]
async fn next_timer_due_iterates_active_branches() {
    let wfd = paralel_with_branch_claim_timeout();
    let store = Arc::new(ParStore::default());
    let exec = WfeExecutor::new(
        Arc::new(MockOrg),
        Arc::new(FixtureWfdStore(wfd.clone())),
        store.clone(),
        Arc::new(MockRunner),
    );
    let claimant = Uuid::new_v4();
    let wfe_id = seed_parallel_state(&store, &wfd, "self__financeApprover", claimant);

    let now = chrono::Utc::now();
    let due = exec.next_timer_due(wfe_id, now).await.unwrap();
    let due = due.expect("kol claim_timeout vadesi bekleniyordu");
    // ~ claimed_at + 10dk (±30 sn tolerans)
    let expected = now + chrono::Duration::minutes(10);
    assert!((due - expected).num_seconds().abs() < 30, "due={due} expected≈{expected}");
}

/// tick_timers paralel modda kolun claim_timeout'unu (wft'siz → Release) o kol
/// üzerinden ateşler: sadece o kolun claim'i sıfırlanır, node değişmez.
#[tokio::test]
async fn tick_timers_fires_branch_claim_timeout_release() {
    let wfd = paralel_with_branch_claim_timeout();
    let store = Arc::new(ParStore::default());
    let exec = WfeExecutor::new(
        Arc::new(MockOrg),
        Arc::new(FixtureWfdStore(wfd.clone())),
        store.clone(),
        Arc::new(MockRunner),
    );
    let claimant = Uuid::new_v4();
    let wfe_id = seed_parallel_state(&store, &wfd, "self__financeApprover", claimant);
    // claim'i 11dk geçmişe al → PT10M claim_timeout vadesi doldu
    store.rewind_branch_claim(wfe_id, "self__financeApprover", chrono::Utc::now() - chrono::Duration::minutes(11));

    let fired = exec.tick_timers(wfe_id).await.unwrap();
    assert!(fired, "kol claim_timeout ateşlenmeli");

    let w = store.snapshot(wfe_id);
    // o kolun claim'i sıfırlandı, node aktif kaldı
    let fin = w.branches.iter().find(|b| b.branch_node == "self__financeApprover").unwrap();
    assert_eq!(fin.status, BranchStatus::Active);
    assert!(fin.claimed_by.is_none(), "claim sıfırlanmalı");
    assert!(w.wfah.entries().iter().any(|e| e.action == "claim_timeout:self__financeApprover"));
}

/// Belirli bir kolun mevcut claimant'ını Actor olarak döndürür (approve için).
fn claim_owner(store: &ParStore, wfe_id: Uuid, node: &str) -> Actor {
    let w = store.snapshot(wfe_id);
    let b = w.branches.iter().find(|b| b.branch_node == node).unwrap();
    Actor {
        orgu_id: Uuid::new_v4(),
        user_id: b.claimed_by.expect("kol claim'li olmalı"),
        role: "placeholder".into(),
    }
}
