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
use wfe_core::types::wfd_v22::{AutoexecDef, JoinRule, Wfd, WftTarget};
use wfe_core::types::wfe::WfeStatus;
use wfe_core::v22::ports::{
    AutoexecRunner, BranchState, BranchStatus, CommitOutcome, ExecEnv, ExecFailure, NewWfe,
    TransitionCommit, WfdStore, WfeStore, Wfes,
};
use wfe_core::{ConflictKind, EngineError};

const PARALLEL_FIXTURE: &str =
    include_str!("../../wfe-core/tests/fixtures/paralel-onay.json");

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
    /// WOR-62 yarış penceresi enjeksiyonu: SIRADAKİ `commit` çağrısı, state'e
    /// dokunmadan ÖNCE kuyruğun başındaki kadar (sanal) ms bekler; kuyruk
    /// boşsa beklemez.
    ///
    /// `start_paused` altında bu, iki eşzamanlı `apply`'ın load→commit
    /// pencerelerini deterministik olarak ÜST ÜSTE bindirir: ikisi de aynı
    /// (paralel-mod) snapshot'ı okur, sonra farklı uyanma anlarında sırayla
    /// commit ederler — yani gerçek yarışın tam olarak istediğimiz kesiti.
    /// Gerçek adapter'da bu pencereyi `SELECT ... FOR UPDATE` kapatır; burada
    /// aynı rolü mutex + paralel-mod kapısı üstlenir.
    commit_delays_ms: Mutex<std::collections::VecDeque<u64>>,
    /// 2026-08-13: son commit'in görünürlük projeksiyonu — `WfeExecutor::
    /// fill_view_grants`in kol başına yazdığı `branch_c_a` kaydı. Gerçek adapter
    /// bunu `wf.wfe_branch.c_a` kolonuna yazar; testte doğrulanabilmesi için
    /// mock yalnız KAYDEDER (kolon taklidi gereksiz karmaşa olurdu).
    last_branch_c_a: Mutex<Vec<(String, usize)>>,
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
    /// Bir kolun entered_at'ını geçmişe alır (SLA-2 escalation dwell'i buradan ölçülür).
    fn rewind_branch_entered(&self, wfe_id: Uuid, node: &str, at: chrono::DateTime<chrono::Utc>) {
        let mut m = self.wfes.lock().unwrap();
        let w = m.get_mut(&wfe_id).unwrap();
        for b in &mut w.branches {
            if b.branch_node == node {
                b.entered_at = at;
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

/// WOR-73: `WfeAdapter::JoinState::arrival_matches` taklidi — engine kararını hangi
/// varış kümesi üzerinde verdiyse, kilit (burada mutex) altındaki gerçek küme de o
/// olmalı. Sayı DEĞİL küme karşılaştırılır: ZEN join koşulu sayıyla ifade edilemez,
/// ama küme aynıysa saf engine'in kararı da aynıdır (adapter ZEN çalıştırmaz).
fn arrival_matches(w: &Wfes, acting_branch: &str, expected: &[String]) -> bool {
    let mut actual: Vec<String> = w
        .branches
        .iter()
        .filter(|b| b.status == BranchStatus::Arrived)
        .map(|b| b.entry_or_current().to_string())
        .collect();
    if let Some(acting) = w
        .branches
        .iter()
        .find(|b| b.status == BranchStatus::Active && b.branch_node == acting_branch)
    {
        actual.push(acting.entry_or_current().to_string());
    }
    actual.sort();
    actual.dedup();
    let mut expected = expected.to_vec();
    expected.sort();
    expected.dedup();
    actual == expected
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

/// `drop_branch_rows`: WOR-31 AND-join'de kol satırları silinir (audit WFAH'ta);
/// WOR-72 quorum join'de KALIR (iptal edilen kol `cancelled` olarak görünür).
fn apply_next(w: &mut Wfes, next: &CommitOutcome, drop_branch_rows: bool) {
    if drop_branch_rows {
        w.branches.clear();
    }
    match next {
        CommitOutcome::MoveTo { node } => {
            w.current_node = Some(node.clone());
            w.join_target = None;
            w.join_rule = JoinRule::All;
            w.assigned_to = None;
            w.claimed_at = None;
        }
        CommitOutcome::Terminal { end_response } => {
            w.status = WfeStatus::Terminal;
            w.current_node = None;
            w.end_response = Some(end_response.clone());
            w.join_target = None;
            w.join_rule = JoinRule::All;
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
            environment_id: None,
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
            join_rule: JoinRule::All,
            origin_orgu_id: None,
        };
        self.seed(w);
        Ok(())
    }

    async fn commit(&self, commit: &TransitionCommit) -> Result<(), EngineError> {
        *self.last_branch_c_a.lock().unwrap() = commit
            .branch_c_a
            .iter()
            .map(|(node, c_a)| (node.clone(), c_a.len()))
            .collect();
        if self.fail_commits.load(Ordering::SeqCst) > 0 {
            self.fail_commits.fetch_sub(1, Ordering::SeqCst);
            return Err(EngineError::Conflict(ConflictKind::BranchArrival));
        }
        // WOR-62: yarış penceresi (bkz. `commit_delays_ms`). Bekleme state
        // mutex'ini ALMADAN önce olur — bekleyen commit hiçbir şeyi tutmaz,
        // tıpkı henüz `FOR UPDATE` almamış bir tx gibi.
        let delay = self.commit_delays_ms.lock().unwrap().pop_front();
        if let Some(ms) = delay.filter(|ms| *ms > 0) {
            tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
        }
        let mut map = self.wfes.lock().unwrap();
        let w = map
            .get_mut(&commit.wfe_id)
            .ok_or_else(|| EngineError::WfePort("not found".into()))?;

        // WOR-62: `WfeAdapter::lock_wfe_parallel` taklidi — kol satırlarına
        // dokunan outcome'lar, mutex ALTINDA hâlâ paralel modda olmayı şart
        // koşar. Paralel mod bu arada bittiyse bir kardeş kazanmıştır →
        // `Conflict(Collapsed)` (retry-edilemez, doğrudan 409).
        let needs_parallel = matches!(
            &commit.outcome,
            CommitOutcome::BranchMoveTo { .. }
                | CommitOutcome::BranchArrived { .. }
                | CommitOutcome::JoinComplete { .. }
                | CommitOutcome::CollapseTo { .. }
        );
        if needs_parallel && w.join_target.is_none() {
            return Err(EngineError::Conflict(ConflictKind::Collapsed));
        }

        // WOR-65: `wf.wfah` (ve `wf.wfe_dynctx`) `UNIQUE (wfe_id, seq)` kısıtının
        // taklidi. Engine seq'i yüklediği snapshot'tan hesaplar; araya başka bir
        // commit girdiyse aynı seq ikinci kez yazılmak istenir. Gerçek adapter'da
        // Postgres 23505 döner ve `insert_err` bunu `StaleRevision`'a eşler —
        // burada aynı verdikt doğrudan üretilir. Bu, TEKİL (paralel-olmayan)
        // moddaki `MoveTo` yolunun tek yarış korumasıdır: orada ne FOR UPDATE
        // ne de CAS vardır.
        if let (Some(first), Some(last)) = (commit.wfah_entries.first(), w.wfah.entries().last()) {
            if first.seq <= last.seq {
                return Err(EngineError::Conflict(ConflictKind::StaleRevision));
            }
        }

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
                w.join_rule = JoinRule::All;
            }
            CommitOutcome::ForkTo {
                branches,
                join,
                join_rule,
            } => {
                w.current_node = None;
                w.assigned_to = None;
                w.claimed_at = None;
                w.join_target = Some(join.clone());
                w.join_rule = join_rule.clone();
                let now = chrono::Utc::now();
                w.branches = branches
                    .iter()
                    .map(|n| BranchState {
                        entry_node: n.clone(),
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
                    return Err(EngineError::Conflict(ConflictKind::BranchMoved));
                };
                b.branch_node = node.clone();
                b.claimed_by = None;
                b.claimed_at = None;
                b.entered_at = chrono::Utc::now();
            }
            CommitOutcome::BranchArrived {
                from_node,
                arrived_entries,
            } => {
                let matches = arrival_matches(w, from_node, arrived_entries);
                let Some(b) = w
                    .branches
                    .iter_mut()
                    .find(|b| b.status == BranchStatus::Active && &b.branch_node == from_node)
                else {
                    return Err(EngineError::Conflict(ConflictKind::BranchMoved));
                };
                b.status = BranchStatus::Arrived;
                b.claimed_by = None;
                b.claimed_at = None;
                if !matches {
                    return Err(EngineError::Conflict(ConflictKind::BranchArrival));
                }
            }
            CommitOutcome::JoinComplete {
                from_node,
                quorum_collapse,
                arrived_entries,
                next,
            } => {
                let matches = arrival_matches(w, from_node, arrived_entries);
                let leftover_active = active_count(w) as i64 - 1;
                let Some(b) = w
                    .branches
                    .iter_mut()
                    .find(|b| b.status == BranchStatus::Active && &b.branch_node == from_node)
                else {
                    return Err(EngineError::Conflict(ConflictKind::BranchMoved));
                };
                b.status = BranchStatus::Arrived;
                if !matches || *quorum_collapse != (leftover_active > 0) {
                    return Err(EngineError::Conflict(ConflictKind::BranchArrival));
                }
                // WOR-72: quorum join'de kalan aktif kollar iptal edilir; satırlar
                // (adapter'da olduğu gibi) SİLİNMEZ — `apply_next` yalnız AND
                // yolunda temizler.
                if *quorum_collapse {
                    cancel_active_branches(w);
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
                apply_next(w, next, !*quorum_collapse);
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
        marker: Option<&WfahEntry>,
    ) -> Result<bool, EngineError> {
        let mut map = self.wfes.lock().unwrap();
        let Some(w) = map.get_mut(&wfe_id) else {
            return Ok(false);
        };
        if w.status != WfeStatus::Active {
            return Ok(false);
        }
        let won = match branch {
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
                true
            }
            None => {
                if w.assigned_to.is_some() {
                    return Ok(false);
                }
                w.assigned_to = Some(user_id);
                w.claimed_at = Some(chrono::Utc::now());
                true
            }
        };
        if won {
            if let Some(entry) = marker {
                w.wfah.0.push(entry.clone());
            }
        }
        Ok(won)
    }

    async fn release_claim(
        &self,
        wfe_id: Uuid,
        _orgtnt_id: Uuid,
        wfah_entry: &WfahEntry,
        branch: Option<&str>,
        new_dynctx: Option<&serde_json::Value>,
    ) -> Result<(), EngineError> {
        let mut map = self.wfes.lock().unwrap();
        let w = map.get_mut(&wfe_id).unwrap();
        if let Some(ctx) = new_dynctx {
            w.dynctx = wfe_core::types::dynctx::DynCtx(ctx.clone());
        }
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

    async fn append_marker(
        &self,
        wfe_id: Uuid,
        _orgtnt_id: Uuid,
        wfah_entry: &WfahEntry,
    ) -> Result<(), EngineError> {
        let mut map = self.wfes.lock().unwrap();
        let w = map.get_mut(&wfe_id).unwrap();
        w.wfah.0.push(wfah_entry.clone());
        Ok(())
    }

    async fn reassign(
        &self,
        wfe_id: Uuid,
        _orgtnt_id: Uuid,
        target: Option<Uuid>,
        wfah_entry: &WfahEntry,
        branch: Option<&str>,
    ) -> Result<(), EngineError> {
        let mut map = self.wfes.lock().unwrap();
        let w = map.get_mut(&wfe_id).unwrap();
        match branch {
            Some(node) => {
                for b in &mut w.branches {
                    if b.status == BranchStatus::Active && b.branch_node == node {
                        b.claimed_by = target;
                        b.claimed_at = target.map(|_| chrono::Utc::now());
                    }
                }
            }
            None => {
                w.assigned_to = target;
                w.claimed_at = target.map(|_| chrono::Utc::now());
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
    assert_eq!(started.current_node.as_ref().map(|n| n.id.as_str()), Some("self__coordinator"));
    let coord = actor("coordinator");
    let c = exec.claim(wfe_id, &coord, None, None).await.unwrap();
    assert!(c.success, "coordinator claim");
    let res = exec
        .apply(wfe_id, &coord, "start_review", &json!({}), None, None, None)
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
        let c = exec.claim(wfe_id, &a, Some(node), None).await.unwrap();
        assert!(c.success, "{node} claim");
    }

    // ilk iki approve → BranchArrived (wfe hâlâ paralel, aktif)
    for (role, node) in &branches[..2] {
        let a = claim_owner(&store, wfe_id, node);
        let a = Actor {
            role: (*role).into(),
            ..a
        };
        let r = exec
            .apply(wfe_id, &a, "approve", &json!({}), Some(node), None, None)
            .await
            .unwrap();
        assert!(!r.terminal);
        assert_eq!(r.current_node, None);
    }
    assert_eq!(active_count(&store.snapshot(wfe_id)), 1);

    // son approve → JoinComplete → MoveTo join node
    let (role, node) = branches[2];
    let a = claim_owner(&store, wfe_id, node);
    let a = Actor {
        role: role.into(),
        ..a
    };
    let r = exec
        .apply(wfe_id, &a, "approve", &json!({}), Some(node), None, None)
        .await
        .unwrap();
    assert!(!r.terminal, "join node terminal değil");
    let w = store.snapshot(wfe_id);
    assert_eq!(w.current_node.as_deref(), Some("self__resultCoordinator"));
    assert!(w.join_target.is_none(), "paralel mod bitti");
    assert!(w.branches.is_empty(), "kollar temizlendi");
    assert!(
        w.wfah.entries().iter().any(|e| e.action == "_join"),
        "_join marker"
    );

    // finalize → terminal_approved
    let rc = actor("resultCoordinator");
    let c = exec.claim(wfe_id, &rc, None, None).await.unwrap();
    assert!(c.success, "resultCoordinator claim");
    let r = exec
        .apply(wfe_id, &rc, "finalize", &json!({}), None, None, None)
        .await
        .unwrap();
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
    exec.claim(wfe_id, &fin, Some("self__financeApprover"), None)
        .await
        .unwrap();
    let r = exec
        .apply(
            wfe_id,
            &fin,
            "reject",
            &json!({}),
            Some("self__financeApprover"),
            None,
            None,
        )
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
        assert!(
            exec.claim(wfe_id, &a, Some(node), None)
                .await
                .unwrap()
                .success,
            "{node} claim"
        );
    }
    let legal_owner = claim_owner(&store, wfe_id, "self__legalApprover").user_id;
    let hr_owner = claim_owner(&store, wfe_id, "self__hrApprover").user_id;

    let fin = claim_owner(&store, wfe_id, "self__financeApprover");
    let fin = Actor {
        role: "financeApprover".into(),
        ..fin
    };
    let r = exec
        .apply(
            wfe_id,
            &fin,
            "reject",
            &json!({}),
            Some("self__financeApprover"),
            None,
            None,
        )
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

    // WOR-67: collapse'ı TETİKLEYEN (acting = finance) kolun düşen claim'i de
    // audit'lenmeli — kardeş kollarda olduğu gibi. Ayrı marker yok; `_collapse`
    // manşetinde `trigger_claimed_by` + `trigger_claimed_at`.
    let summary = w
        .wfah
        .entries()
        .iter()
        .find(|e| e.action == "_collapse")
        .and_then(|e| e.input.as_ref())
        .expect("_collapse özeti");
    assert_eq!(summary["trigger_branch"], json!("self__financeApprover"));
    assert_eq!(
        summary["trigger_claimed_by"],
        json!(fin.user_id),
        "acting claim sahibi"
    );
    assert!(
        !summary["trigger_claimed_at"].is_null(),
        "acting kolun claimed_at'i manşette olmalı (hold süresi hesabı için)"
    );
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
        assert!(
            exec.claim(wfe_id, &a, Some(node), None)
                .await
                .unwrap()
                .success,
            "{node} claim"
        );
    }

    // legal onaylar → kol `arrived` (hâlâ 2 aktif kol var, join tamamlanmaz)
    let legal = claim_owner(&store, wfe_id, "self__legalApprover");
    let legal = Actor {
        role: "legalApprover".into(),
        ..legal
    };
    let r = exec
        .apply(
            wfe_id,
            &legal,
            "approve",
            &json!({}),
            Some("self__legalApprover"),
            None,
            None,
        )
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
    let fin = Actor {
        role: "financeApprover".into(),
        ..fin
    };
    exec.apply(
        wfe_id,
        &fin,
        "reject",
        &json!({}),
        Some("self__financeApprover"),
        None,
        None,
    )
    .await
    .unwrap();

    let w = store.snapshot(wfe_id);
    // kol satırı `arrived` KALIR (yeni statü yok — bkz. decisions.md)
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
    assert_eq!(
        superseded.len(),
        1,
        "yalnız arrived kol superseded marker'ı alır"
    );
    let input = superseded[0].input.as_ref().unwrap();
    assert_eq!(input["node"], json!("self__legalApprover"));
    assert_eq!(input["reason"], json!("sibling_terminal"));
    assert_eq!(input["approved_by"]["user_id"], json!(legal.user_id));
    assert_eq!(input["approved_by"]["role"], json!("legalApprover"));
    assert!(!input["approved_at"].is_null(), "onay zamanı taşınmalı");
    // WOR-63: geçersizleşmeyi TETİKLEYEN kol/aksiyon/actor
    assert_eq!(input["trigger_node"], json!("self__financeApprover"));
    assert_eq!(input["trigger_action"], json!("reject"));
    assert_eq!(input["trigger_actor"]["user_id"], json!(fin.user_id));
    // hr hâlâ aktifti → cancelled marker'ı; iki marker karışmaz
    let cancels: Vec<&WfahEntry> = w
        .wfah
        .entries()
        .iter()
        .filter(|e| e.action == "_branch_cancelled")
        .collect();
    assert_eq!(cancels.len(), 1);
    assert_eq!(
        cancels[0].input.as_ref().unwrap()["node"],
        json!("self__hrApprover")
    );

    // WOR-61: collapse başına TEK özet kaydı; detay marker'ları onun üstünde değil
    // altında kalır (ikisi de WFAH'ta).
    let summaries: Vec<&WfahEntry> = w
        .wfah
        .entries()
        .iter()
        .filter(|e| e.action == "_collapse")
        .collect();
    assert_eq!(summaries.len(), 1, "collapse başına tek özet");
    let s = summaries[0].input.as_ref().unwrap();
    assert_eq!(s["trigger_branch"], json!("self__financeApprover"));
    assert_eq!(s["trigger_action"], json!("reject"));
    assert_eq!(s["trigger_actor"]["user_id"], json!(fin.user_id));
    assert_eq!(s["kind"], json!("terminal"));
    assert_eq!(s["cancelled"], json!(["self__hrApprover"]));
    assert_eq!(s["superseded"], json!(["self__legalApprover"]));
    // WOR-67: acting (finance) kolun düşen claim'i manşette
    assert_eq!(
        s["trigger_claimed_by"],
        json!(fin.user_id),
        "acting claim sahibi"
    );
    assert!(
        !s["trigger_claimed_at"].is_null(),
        "acting kolun claimed_at'i manşette"
    );

    // WOR-68: arrived (legal) kolun `_branch_arrived` marker'ı claim BAŞLANGICINI
    // taşımalı — kol varışta claim'ini kaybettiği için hold süresi (approved_at −
    // claimed_at) yalnız buradan hesaplanabilir.
    let arrived_marker = w
        .wfah
        .entries()
        .iter()
        .find(|e| {
            e.action == "_branch_arrived"
                && e.input
                    .as_ref()
                    .map(|i| i["node"] == json!("self__legalApprover"))
                    == Some(true)
        })
        .and_then(|e| e.input.as_ref())
        .expect("legal için _branch_arrived marker'ı");
    assert!(
        !arrived_marker["claimed_at"].is_null(),
        "arrived marker claimed_at taşımalı (hold süresi hesabı için)"
    );
    assert!(!arrived_marker["approved_at"].is_null());
    // hold süresi hesaplanabilir: approved_at ≥ claimed_at
    let claimed = arrived_marker["claimed_at"].as_str().unwrap();
    let approved = arrived_marker["approved_at"].as_str().unwrap();
    assert!(
        approved >= claimed,
        "approved_at ({approved}) ≥ claimed_at ({claimed})"
    );
}

/// Executor retry döngüsü: adapter Conflict döndürdüğünde reload + engine
/// yeniden koşulur. İlk commit Conflict, ikinci başarı → apply başarılı.
#[tokio::test]
async fn apply_retries_on_conflict_then_succeeds() {
    let store = Arc::new(ParStore::default());
    let exec = executor(store.clone());
    let wfe_id = fork_setup(&exec).await;
    let fin = actor("financeApprover");
    exec.claim(wfe_id, &fin, Some("self__financeApprover"), None)
        .await
        .unwrap();

    // ilk commit çağrısı Conflict → executor reload edip yeniden dener
    store.fail_commits.store(1, Ordering::SeqCst);
    let r = exec
        .apply(
            wfe_id,
            &fin,
            "approve",
            &json!({}),
            Some("self__financeApprover"),
            None,
            None,
        )
        .await
        .unwrap();
    assert!(!r.terminal);
    assert_eq!(
        store.fail_commits.load(Ordering::SeqCst),
        0,
        "bir kez conflict tüketildi"
    );
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
    exec.claim(wfe_id, &fin, Some("self__financeApprover"), None)
        .await
        .unwrap();

    store.fail_commits.store(5, Ordering::SeqCst);
    let err = exec
        .apply(
            wfe_id,
            &fin,
            "approve",
            &json!({}),
            Some("self__financeApprover"),
            None,
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        EngineError::Conflict(ConflictKind::BranchArrival)
    ));
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
    assert!(
        exec.claim(wfe_id, &a1, Some("self__financeApprover"), None)
            .await
            .unwrap()
            .success
    );
    let second = exec
        .claim(wfe_id, &a2, Some("self__financeApprover"), None)
        .await
        .unwrap();
    assert!(!second.success);
    assert_eq!(second.reason.as_deref(), Some("already_claimed"));

    // farklı kol hâlâ claim edilebilir
    assert!(
        exec.claim(
            wfe_id,
            &actor("legalApprover"),
            Some("self__legalApprover"),
            None
        )
        .await
        .unwrap()
        .success
    );
}

// ---- WOR-62: CollapseTo yarış serileştirmesi -----------------------------------

/// TÜM `reject` transition'larını node hedefli collapse'a çeviren fixture varyantı
/// (WOR-56 `{"collapse": {"node": ...}}`) — `CommitOutcome::CollapseTo` üretir.
/// Fixture'ın kendi `reject`'i terminal hedeflidir; terminal yolu paralel modu
/// başka bir arm'dan bitirir, biz burada tam olarak CollapseTo'yu test ediyoruz.
fn paralel_with_collapse_to_node() -> Wfd {
    let mut v: Value = serde_json::from_str(PARALLEL_FIXTURE).unwrap();
    for t in v["transitions"].as_array_mut().unwrap() {
        if t["action"] == json!("reject") {
            t["wft"] = json!({"collapse": {"node": "self__coordinator"}});
        }
    }
    Wfd::from_value(v).unwrap()
}

fn collapse_executor(store: Arc<ParStore>) -> WfeExecutor {
    WfeExecutor::new(
        Arc::new(MockOrg),
        Arc::new(FixtureWfdStore(paralel_with_collapse_to_node())),
        store,
        Arc::new(MockRunner),
    )
}

/// Üç kolu da claim eder; her kolun (claim sahibi user_id'sini taşıyan) aktörünü
/// rolüyle birlikte döndürür.
async fn claim_all_branches(exec: &WfeExecutor, store: &ParStore, wfe_id: Uuid) -> Vec<Actor> {
    let mut out = Vec::new();
    for (role, node) in [
        ("financeApprover", "self__financeApprover"),
        ("legalApprover", "self__legalApprover"),
        ("hrApprover", "self__hrApprover"),
    ] {
        let a = actor(role);
        assert!(
            exec.claim(wfe_id, &a, Some(node), None)
                .await
                .unwrap()
                .success,
            "{node} claim"
        );
        let owner = claim_owner(store, wfe_id, node);
        out.push(Actor {
            role: role.into(),
            ..owner
        });
    }
    out
}

/// WOR-62 ANA KABUL: collapse ile eşzamanlı kardeş aksiyonu DETERMİNİSTİK
/// sonuçlanır. İki `apply` aynı (paralel-mod) snapshot'ı okur; collapse önce
/// commit eder ve KAZANIR, kaybeden kardeş `Conflict(Collapsed)` alır — sessizce
/// yutulmaz, tutarsız ara durum kalmaz.
///
/// `start_paused`: `commit_delays_ms` sanal zamanda ilerler, testte gerçek
/// bekleme yok.
#[tokio::test(start_paused = true)]
async fn collapse_wins_race_and_losing_sibling_gets_collapsed_conflict() {
    let store = Arc::new(ParStore::default());
    let exec = collapse_executor(store.clone());
    let wfe_id = fork_setup(&exec).await;
    let actors = claim_all_branches(&exec, &store, wfe_id).await;
    let (fin, legal) = (actors[0].clone(), actors[1].clone());

    // 1. commit (collapse) 100ms'de, 2. commit (kardeş varışı) 200ms'de uyanır.
    // İkisi de ÖNCE load eder → ikisi de paralel modu görür → sonra sırayla
    // commit'e girer. Yarışın kesiti tam olarak bu.
    store.commit_delays_ms.lock().unwrap().extend([100, 200]);

    // `join!` future'ları SIRAYLA poll eder: collapse önce commit'e girer.
    let input = json!({});
    let collapse = exec.apply(
        wfe_id,
        &fin,
        "reject",
        &input,
        Some("self__financeApprover"),
        None,
        None,
    );
    let sibling = exec.apply(
        wfe_id,
        &legal,
        "approve",
        &input,
        Some("self__legalApprover"),
        None,
        None,
    );
    let (collapse_res, sibling_res) = tokio::join!(collapse, sibling);

    // KAZANAN: collapse — WFE hedef node'a taşındı, paralel mod bitti.
    let r = collapse_res.expect("collapse kazanmalı");
    assert!(!r.terminal);
    assert_eq!(r.current_node.as_ref().map(|n| n.id.as_str()), Some("self__coordinator"));

    // KAYBEDEN: kardeş varışı — NET Conflict(Collapsed) (409 conflict.collapsed).
    let err = sibling_res.expect_err("kaybeden kardeş Conflict almalı");
    assert!(
        matches!(err, EngineError::Conflict(ConflictKind::Collapsed)),
        "kaybeden kardeş `Collapsed` conflict'i almalı, alınan: {err:?}"
    );
    assert_eq!(
        EngineError::Conflict(ConflictKind::Collapsed).to_string(),
        format!(
            "optimistic concurrency conflict [{}]: state changed under commit",
            ConflictKind::Collapsed.code()
        ),
    );

    // Kaybeden aksiyon HİÇBİR iz bırakmadı: legal kolu `arrived` OLMADI, WFE
    // collapse durumunda tutarlı (tek kaynak: kazanan commit).
    let w = store.snapshot(wfe_id);
    assert!(w.join_target.is_none(), "paralel mod bitti");
    assert_eq!(active_count(&w), 0, "collapse tüm aktif kolları düşürdü");
    let legal_row = w
        .branches
        .iter()
        .find(|b| b.branch_node == "self__legalApprover")
        .unwrap();
    assert_eq!(
        legal_row.status,
        BranchStatus::Cancelled,
        "kaybeden kol `arrived` değil, collapse ile `cancelled` olmalı"
    );
    assert!(
        !w.wfah
            .entries()
            .iter()
            .any(|e| e.action == "_branch_superseded"),
        "kaybeden varış commit edilmediği için superseded marker'ı da olmamalı"
    );
}

/// WOR-62 tersi yön: kardeş varışı ÖNCE commit ederse collapse yine de KAZANIR —
/// serileştirme collapse'ı gereksiz yere reddetmez ("otoriter" davranış korunur,
/// yalnız sıraya sokulur).
#[tokio::test(start_paused = true)]
async fn sibling_arrival_first_then_collapse_still_wins() {
    let store = Arc::new(ParStore::default());
    let exec = collapse_executor(store.clone());
    let wfe_id = fork_setup(&exec).await;
    let actors = claim_all_branches(&exec, &store, wfe_id).await;
    let (fin, legal) = (actors[0].clone(), actors[1].clone());

    // Bu kez ÖNCE kardeş varışı commit eder (100ms), collapse sonra (200ms).
    store.commit_delays_ms.lock().unwrap().extend([100, 200]);
    let input = json!({});
    let sibling = exec.apply(
        wfe_id,
        &legal,
        "approve",
        &input,
        Some("self__legalApprover"),
        None,
        None,
    );
    let collapse = exec.apply(
        wfe_id,
        &fin,
        "reject",
        &input,
        Some("self__financeApprover"),
        None,
        None,
    );
    let (sibling_res, collapse_res) = tokio::join!(sibling, collapse);

    sibling_res.expect("kardeş varışı önce commit etti, başarılı olmalı");
    let r = collapse_res.expect("collapse hâlâ paralel modda, kazanmalı");
    assert_eq!(r.current_node.as_ref().map(|n| n.id.as_str()), Some("self__coordinator"));

    let w = store.snapshot(wfe_id);
    assert!(w.join_target.is_none());
    assert_eq!(active_count(&w), 0);
    // Önce varan kol `arrived` KALIR (WOR-60: statü değişmez, marker eklenir).
    let legal_row = w
        .branches
        .iter()
        .find(|b| b.branch_node == "self__legalApprover")
        .unwrap();
    assert_eq!(legal_row.status, BranchStatus::Arrived);
}

/// İki kardeş AYNI ANDA collapse ederse tam olarak BİRİ kazanır; ikincisi
/// `Conflict(Collapsed)` alır. "İlk kilidi alan kazanır" kuralı collapse-collapse
/// yarışında da geçerlidir (aksi halde ikisi de yazıp WFE'yi iki kez taşırdı).
#[tokio::test(start_paused = true)]
async fn two_concurrent_collapses_exactly_one_wins() {
    let store = Arc::new(ParStore::default());
    let exec = collapse_executor(store.clone());
    let wfe_id = fork_setup(&exec).await;
    let actors = claim_all_branches(&exec, &store, wfe_id).await;
    let (fin, legal) = (actors[0].clone(), actors[1].clone());

    store.commit_delays_ms.lock().unwrap().extend([100, 200]);
    let input = json!({});
    let a = exec.apply(
        wfe_id,
        &fin,
        "reject",
        &input,
        Some("self__financeApprover"),
        None,
        None,
    );
    let b = exec.apply(
        wfe_id,
        &legal,
        "reject",
        &input,
        Some("self__legalApprover"),
        None,
        None,
    );
    let (a_res, b_res) = tokio::join!(a, b);

    a_res.expect("ilk collapse kazanmalı");
    let err = b_res.expect_err("ikinci collapse Conflict almalı");
    assert!(
        matches!(err, EngineError::Conflict(ConflictKind::Collapsed)),
        "beklenen Collapsed, alınan: {err:?}"
    );
    assert_eq!(
        store.snapshot(wfe_id).current_node.as_deref(),
        Some("self__coordinator")
    );
}

/// `Collapsed` retry EDİLMEZ: reload aynı verdikti üretir. Retry sayacı
/// (`fail_commits`) hiç tüketilmeden conflict aynen yukarı verilir — executor
/// 3 tur dönüp keyfi bir engine hatasına dönüşmez.
#[tokio::test(start_paused = true)]
async fn collapsed_conflict_is_not_retried() {
    let store = Arc::new(ParStore::default());
    let exec = collapse_executor(store.clone());
    let wfe_id = fork_setup(&exec).await;
    let actors = claim_all_branches(&exec, &store, wfe_id).await;
    let (fin, legal) = (actors[0].clone(), actors[1].clone());

    store.commit_delays_ms.lock().unwrap().extend([100, 200]);
    let input = json!({});
    let collapse = exec.apply(
        wfe_id,
        &fin,
        "reject",
        &input,
        Some("self__financeApprover"),
        None,
        None,
    );
    let sibling = exec.apply(
        wfe_id,
        &legal,
        "approve",
        &input,
        Some("self__legalApprover"),
        None,
        None,
    );
    let (_, sibling_res) = tokio::join!(collapse, sibling);

    assert!(matches!(
        sibling_res.unwrap_err(),
        EngineError::Conflict(ConflictKind::Collapsed)
    ));
    // Kuyrukta bekleyen gecikme kalmadıysa retry olmamıştır: retry her turda
    // yeni bir commit çağrısı yapardı ve kuyruk zaten boş olduğundan bunu
    // doğrudan `is_retryable` sözleşmesinden okuyoruz.
    assert!(
        !ConflictKind::Collapsed.is_retryable(),
        "Collapsed retry-edilemez olmalı"
    );
    assert!(
        ConflictKind::BranchArrival.is_retryable(),
        "kol-varış yarışı retry-edilebilir"
    );
    assert!(
        ConflictKind::BranchMoved.is_retryable(),
        "kol taşınması retry-edilebilir"
    );
}

// ---- timer kol iterasyonu -----------------------------------------------------

/// Bir kol node'una claim_timeout ekli varyant WFD.
fn paralel_with_branch_claim_timeout() -> Wfd {
    let mut v: Value = serde_json::from_str(PARALLEL_FIXTURE).unwrap();
    v["nodes"]["self__financeApprover"]["claim_timeout"] = json!({ "after": "PT10M" });
    Wfd::from_value(v).unwrap()
}

fn seed_parallel_state(store: &ParStore, wfd: &Wfd, claimed_branch: &str, claimant: Uuid) -> Uuid {
    let _ = wfd;
    let wfe_id = Uuid::new_v4();
    let now = chrono::Utc::now();
    let mk = |node: &str, claimed: Option<Uuid>| BranchState {
        entry_node: node.into(),
        branch_node: node.into(),
        status: BranchStatus::Active,
        claimed_by: claimed,
        claimed_at: claimed.map(|_| now),
        entered_at: now,
    };
    let branches = vec![
        mk(
            "self__financeApprover",
            if claimed_branch == "self__financeApprover" {
                Some(claimant)
            } else {
                None
            },
        ),
        mk("self__legalApprover", None),
        mk("self__hrApprover", None),
    ];
    store.seed(Wfes {
        wfe_id,
        orgtnt_id: Uuid::nil(),
        environment_id: None,
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
        join_target: Some(WftTarget::Node {
            node: "self__resultCoordinator".into(),
        }),
        join_rule: JoinRule::All,
        origin_orgu_id: None,
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
    assert!(
        (due - expected).num_seconds().abs() < 30,
        "due={due} expected≈{expected}"
    );
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
    store.rewind_branch_claim(
        wfe_id,
        "self__financeApprover",
        chrono::Utc::now() - chrono::Duration::minutes(11),
    );

    let fired = exec.tick_timers(wfe_id).await.unwrap();
    assert!(fired, "kol claim_timeout ateşlenmeli");

    let w = store.snapshot(wfe_id);
    // o kolun claim'i sıfırlandı, node aktif kaldı
    let fin = w
        .branches
        .iter()
        .find(|b| b.branch_node == "self__financeApprover")
        .unwrap();
    assert_eq!(fin.status, BranchStatus::Active);
    assert!(fin.claimed_by.is_none(), "claim sıfırlanmalı");
    assert!(w
        .wfah
        .entries()
        .iter()
        .any(|e| e.action == "claim_timeout:self__financeApprover"));
}

/// WOR-56/SLA-1 (2026-08-03): kol node'unda `collapses_parallel` + node hedefli
/// claim_timeout.
fn paralel_with_collapsing_branch_claim_timeout() -> Wfd {
    let mut v: Value = serde_json::from_str(PARALLEL_FIXTURE).unwrap();
    v["nodes"]["self__financeApprover"]["claim_timeout"] = json!({
        "after": "PT10M",
        "wft": "self__coordinator",
        "collapses_parallel": true,
    });
    Wfd::from_value(v).unwrap()
}

/// WOR-56/SLA-1 ANA KABUL: `collapses_parallel` işaretli claim_timeout kol
/// bağlamında dolunca yalnız kolu taşımaz — PARALELİ SONLANDIRIR: kardeş kollar
/// iptal, paralel mod kapanır, WFE hedef node'a gider. Aksiyon collapse'ıyla aynı
/// yol (`CommitOutcome::CollapseTo` + `_collapse` özeti), tetikleyicisi system.
#[tokio::test]
async fn tick_timers_branch_claim_timeout_collapses_parallel() {
    let wfd = paralel_with_collapsing_branch_claim_timeout();
    let store = Arc::new(ParStore::default());
    let exec = WfeExecutor::new(
        Arc::new(MockOrg),
        Arc::new(FixtureWfdStore(wfd.clone())),
        store.clone(),
        Arc::new(MockRunner),
    );
    let claimant = Uuid::new_v4();
    let wfe_id = seed_parallel_state(&store, &wfd, "self__financeApprover", claimant);
    store.rewind_branch_claim(
        wfe_id,
        "self__financeApprover",
        chrono::Utc::now() - chrono::Duration::minutes(11),
    );

    assert!(
        exec.tick_timers(wfe_id).await.unwrap(),
        "kol claim_timeout ateşlenmeli"
    );

    let w = store.snapshot(wfe_id);
    assert_eq!(
        w.current_node.as_deref(),
        Some("self__coordinator"),
        "collapse hedefine gidilmeli"
    );
    assert!(w.join_target.is_none(), "paralel mod kapanmalı");
    assert!(
        w.branches
            .iter()
            .all(|b| b.status != BranchStatus::Active),
        "kardeş kollar iptal edilmeli: {:?}",
        w.branches.iter().map(|b| b.status).collect::<Vec<_>>()
    );
    let actions: Vec<&str> = w.wfah.entries().iter().map(|e| e.action.as_str()).collect();
    assert!(
        actions.contains(&"claim_timeout:self__financeApprover"),
        "SLA-1 marker'ı: {actions:?}"
    );
    assert!(actions.contains(&"_collapse"), "collapse özeti: {actions:?}");
}

/// WOR-56/SLA-2 (2026-08-03): kol node'una node hedefli collapse escalation'ı.
fn paralel_with_collapsing_branch_escalation() -> Wfd {
    let mut v: Value = serde_json::from_str(PARALLEL_FIXTURE).unwrap();
    v["nodes"]["self__financeApprover"]["escalation"] = json!([{
        "after": "PT10M",
        "wft": { "collapse": { "node": "self__coordinator" } }
    }]);
    Wfd::from_value(v).unwrap()
}

/// WOR-56/SLA-2 ANA KABUL: kol bekleme süresi (escalation) dolunca `{collapse:{node}}`
/// hedefi paraleli SONLANDIRIR — kardeş kollar iptal, paralel mod kapanır, WFE hedefe
/// gider. SLA-1 collapse'ıyla aynı yol; tek fark sayacın claim değil GİRİŞ anından
/// (`entered_at`) ölçülmesi.
#[tokio::test]
async fn tick_timers_branch_escalation_collapses_parallel() {
    let wfd = paralel_with_collapsing_branch_escalation();
    let store = Arc::new(ParStore::default());
    let exec = WfeExecutor::new(
        Arc::new(MockOrg),
        Arc::new(FixtureWfdStore(wfd.clone())),
        store.clone(),
        Arc::new(MockRunner),
    );
    let wfe_id = seed_parallel_state(&store, &wfd, "none", Uuid::new_v4());
    store.rewind_branch_entered(
        wfe_id,
        "self__financeApprover",
        chrono::Utc::now() - chrono::Duration::minutes(11),
    );

    assert!(
        exec.tick_timers(wfe_id).await.unwrap(),
        "kol escalation'ı ateşlenmeli"
    );

    let w = store.snapshot(wfe_id);
    assert_eq!(
        w.current_node.as_deref(),
        Some("self__coordinator"),
        "collapse hedefine gidilmeli"
    );
    assert!(w.join_target.is_none(), "paralel mod kapanmalı");
    assert!(
        w.branches
            .iter()
            .all(|b| b.status != BranchStatus::Active),
        "kardeş kollar iptal edilmeli: {:?}",
        w.branches.iter().map(|b| b.status).collect::<Vec<_>>()
    );
    let actions: Vec<&str> = w.wfah.entries().iter().map(|e| e.action.as_str()).collect();
    assert!(
        actions.contains(&"escalate:self__financeApprover:0"),
        "SLA-2 marker'ı: {actions:?}"
    );
    assert!(actions.contains(&"_collapse"), "collapse özeti: {actions:?}");
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

// ---- WOR-65: revizyon token'ı + stale-write reddi -----------------------------

/// Revizyon token'ı = son WFAH `seq`'i (`Wfes::rev()`). Her transition en az bir
/// WFAH kaydı yazdığı için token her aksiyonda KESİN artar — yeni bir kolon
/// olmadan monotonik bir revizyon sayacı elde edilir.
#[tokio::test]
async fn rev_is_monotonic_across_transitions() {
    let store = Arc::new(ParStore::default());
    let exec = executor(store.clone());

    let requester = actor("requester");
    let started = exec
        .start(
            Uuid::new_v4(),
            1,
            &requester,
            None,
            &json!({"request": {"title": "t", "amount": 1}}),
            None,
        )
        .await
        .unwrap();
    let wfe_id = started.wfe_id;

    let after_start = store.snapshot(wfe_id).rev();
    assert!(after_start >= 1, "start en az bir WFAH kaydı yazar");

    let coord = actor("coordinator");
    exec.claim(wfe_id, &coord, None, None).await.unwrap();
    assert_eq!(
        store.snapshot(wfe_id).rev(),
        after_start,
        "claim WFAH'a yazmaz → revizyon ARTMAZ (bilinçli kapsam istisnası)"
    );

    exec.apply(wfe_id, &coord, "start_review", &json!({}), None, None, None)
        .await
        .unwrap();
    let after_fork = store.snapshot(wfe_id).rev();
    assert!(
        after_fork > after_start,
        "transition revizyonu artırmalı: {after_start} → {after_fork}"
    );
}

/// Token GÖNDERMEYEN istemci bugünkü davranışı aynen görür (geriye uyumluluk),
/// TAZE token gönderen istemcinin aksiyonu normal biçimde uygulanır.
#[tokio::test]
async fn omitted_and_fresh_rev_both_apply_normally() {
    let store = Arc::new(ParStore::default());
    let exec = executor(store.clone());
    let wfe_id = fork_setup(&exec).await;
    let actors = claim_all_branches(&exec, &store, wfe_id).await;

    // taze token ile: uygulanır
    let rev = store.snapshot(wfe_id).rev();
    exec.apply(
        wfe_id,
        &actors[0],
        "approve",
        &json!({}),
        Some("self__financeApprover"),
        None,
        Some(rev),
    )
    .await
    .expect("taze revizyonla apply geçmeli");

    // token'sız: uygulanır (eski istemci yolu)
    exec.apply(
        wfe_id,
        &actors[1],
        "approve",
        &json!({}),
        Some("self__legalApprover"),
        None,
        None,
    )
    .await
    .expect("revizyonsuz apply bugünkü gibi geçmeli");
}

/// ANA KABUL: eskimiş revizyonla gelen apply `Conflict(StaleRevision)` alır ve
/// HİÇBİR yan etki üretmez — durum aksiyondan önceki hâlinde kalır.
#[tokio::test]
async fn stale_rev_apply_is_rejected_without_side_effects() {
    let store = Arc::new(ParStore::default());
    let exec = executor(store.clone());
    let wfe_id = fork_setup(&exec).await;
    let actors = claim_all_branches(&exec, &store, wfe_id).await;

    // İstemci bu revizyonu okur…
    let stale_rev = store.snapshot(wfe_id).rev();
    // …sonra motor durumu "sessizce" ilerletir (başka bir kol onaylar).
    exec.apply(
        wfe_id,
        &actors[0],
        "approve",
        &json!({}),
        Some("self__financeApprover"),
        None,
        None,
    )
    .await
    .unwrap();

    let before = store.snapshot(wfe_id);
    assert!(before.rev() > stale_rev, "durum ilerlemiş olmalı");

    let err = exec
        .apply(
            wfe_id,
            &actors[1],
            "approve",
            &json!({}),
            Some("self__legalApprover"),
            None,
            Some(stale_rev),
        )
        .await
        .expect_err("eskimiş revizyon reddedilmeli");
    assert!(
        matches!(err, EngineError::Conflict(ConflictKind::StaleRevision)),
        "beklenen StaleRevision, alınan: {err:?}"
    );
    assert_eq!(
        EngineError::Conflict(ConflictKind::StaleRevision).to_string(),
        "optimistic concurrency conflict [conflict.stale_revision]: state changed under commit",
    );

    let after = store.snapshot(wfe_id);
    assert_eq!(
        after.rev(),
        before.rev(),
        "reddedilen apply WFAH'a yazmamalı"
    );
    assert_eq!(
        active_count(&after),
        active_count(&before),
        "reddedilen apply kol durumuna dokunmamalı"
    );
}

/// WOR-65'in asıl senaryosu: collapse WFE'yi kullanıcının altından aldı; portal
/// 4-10s boyunca eski revizyonu taşıyor. Token'lı apply artık ayırt edilebilir
/// `conflict.stale_revision` alır — token'sız yolda düşülen KEYFİ engine hatası
/// (TransitionNotFound/AmbiguousAction/PermissionDenied) yerine.
#[tokio::test]
async fn stale_rev_after_collapse_is_distinguishable() {
    let store = Arc::new(ParStore::default());
    let exec = collapse_executor(store.clone());
    let wfe_id = fork_setup(&exec).await;
    let actors = claim_all_branches(&exec, &store, wfe_id).await;

    // portal'ın elindeki revizyon
    let stale_rev = store.snapshot(wfe_id).rev();

    // finans kolu reddeder → collapse; kardeş kollar cancelled, WFE hedefe gider
    exec.apply(
        wfe_id,
        &actors[0],
        "reject",
        &json!({}),
        Some("self__financeApprover"),
        None,
        None,
    )
    .await
    .unwrap();
    assert!(
        store.snapshot(wfe_id).join_target.is_none(),
        "collapse oldu"
    );

    let err = exec
        .apply(
            wfe_id,
            &actors[1],
            "approve",
            &json!({}),
            Some("self__legalApprover"),
            None,
            Some(stale_rev),
        )
        .await
        .expect_err("collapse sonrası eski revizyon reddedilmeli");
    assert!(
        matches!(err, EngineError::Conflict(ConflictKind::StaleRevision)),
        "beklenen StaleRevision, alınan: {err:?}"
    );
}

/// Claim de opsiyonel token kabul eder. Token YOKSA akış hiç değişmez
/// (`ClaimOutcome`, HTTP 200); token VARSA ve eskimişse `Conflict(StaleRevision)`
/// — yanıltıcı `already_claimed` / `not_eligible` gerekçesi yerine.
#[tokio::test]
async fn stale_rev_claim_is_rejected_but_untokened_claim_is_untouched() {
    let store = Arc::new(ParStore::default());
    let exec = collapse_executor(store.clone());
    let wfe_id = fork_setup(&exec).await;
    let actors = claim_all_branches(&exec, &store, wfe_id).await;

    let stale_rev = store.snapshot(wfe_id).rev();
    exec.apply(
        wfe_id,
        &actors[0],
        "reject",
        &json!({}),
        Some("self__financeApprover"),
        None,
        None,
    )
    .await
    .unwrap();

    // token'lı: NET conflict
    let latecomer = actor("legalApprover");
    let err = exec
        .claim(
            wfe_id,
            &latecomer,
            Some("self__legalApprover"),
            Some(stale_rev),
        )
        .await
        .expect_err("eskimiş revizyonlu claim reddedilmeli");
    assert!(
        matches!(err, EngineError::Conflict(ConflictKind::StaleRevision)),
        "beklenen StaleRevision, alınan: {err:?}"
    );

    // token'sız: bugünkü yol — hata DEĞİL, `success:false` + gerekçe
    let outcome = exec
        .claim(wfe_id, &latecomer, Some("self__legalApprover"), None)
        .await
        .expect("token'sız claim hata döndürmemeli (geriye uyumluluk)");
    assert!(!outcome.success, "iptal edilmiş kol claim edilemez");
    assert!(outcome.reason.is_some(), "gerekçe taşımalı");
}

/// Örtük koruma: token GÖNDERMEYEN istemciler için bile `UNIQUE (wfe_id, seq)`
/// bir optimistic lock'tur. Tekil (paralel-olmayan) modda `MoveTo` yolunda başka
/// hiçbir CAS/kilit yoktur — iki eşzamanlı apply aynı snapshot'tan aynı seq'i
/// hesaplar; kaybeden 500 yerine `Conflict(StaleRevision)` alır.
///
/// İki taraf da AYNI `expected_rev`'i taşıdığı için sonuç deterministiktir:
/// tam olarak biri kazanır, diğeri (ister commit'teki seq çakışmasından ister
/// retry turundaki revizyon kapısından) `StaleRevision` görür.
#[tokio::test(start_paused = true)]
async fn concurrent_single_mode_applies_exactly_one_wins() {
    let store = Arc::new(ParStore::default());
    let exec = executor(store.clone());

    let requester = actor("requester");
    let started = exec
        .start(
            Uuid::new_v4(),
            1,
            &requester,
            None,
            &json!({"request": {"title": "t", "amount": 1}}),
            None,
        )
        .await
        .unwrap();
    let wfe_id = started.wfe_id;
    let coord = actor("coordinator");
    exec.claim(wfe_id, &coord, None, None).await.unwrap();

    let rev = store.snapshot(wfe_id).rev();
    store.commit_delays_ms.lock().unwrap().extend([100, 200]);
    let input = json!({});
    let a = exec.apply(wfe_id, &coord, "start_review", &input, None, None, Some(rev));
    let b = exec.apply(wfe_id, &coord, "start_review", &input, None, None, Some(rev));
    let (a_res, b_res) = tokio::join!(a, b);

    let (winner, loser) = match (a_res, b_res) {
        (Ok(_), Err(e)) => (1, e),
        (Err(e), Ok(_)) => (2, e),
        (Ok(_), Ok(_)) => panic!("iki apply de kazanamaz — lost update"),
        (Err(x), Err(y)) => panic!("en az biri kazanmalı: {x:?} / {y:?}"),
    };
    assert!(
        matches!(loser, EngineError::Conflict(ConflictKind::StaleRevision)),
        "kaybeden StaleRevision almalı (kazanan #{winner}), alınan: {loser:?}"
    );
    assert_eq!(
        active_count(&store.snapshot(wfe_id)),
        3,
        "fork TEK kez uygulandı"
    );
}

// ---- WOR-72: OR-join (K-of-N quorum) ------------------------------------------

/// Fixture'ın fork'unu quorum join'e çevirir: 3 kol, `join_mode: or`, eşik `k`.
/// (Eşik None verilirse alan yazılmaz → saf OR = 1-of-N.)
fn paralel_with_quorum(k: Option<u32>) -> Wfd {
    let mut v: Value = serde_json::from_str(PARALLEL_FIXTURE).unwrap();
    for t in v["transitions"].as_array_mut().unwrap() {
        if t["wft"].get("parallel").is_some() {
            t["wft"]["parallel"]["join_mode"] = json!("or");
            if let Some(k) = k {
                t["wft"]["parallel"]["join_threshold"] = json!(k);
            }
        }
    }
    Wfd::from_value(v).unwrap()
}

fn quorum_executor(store: Arc<ParStore>, k: Option<u32>) -> WfeExecutor {
    WfeExecutor::new(
        Arc::new(MockOrg),
        Arc::new(FixtureWfdStore(paralel_with_quorum(k))),
        store,
        Arc::new(MockRunner),
    )
}

fn marker<'w>(w: &'w Wfes, action: &str) -> Option<&'w Value> {
    w.wfah
        .entries()
        .iter()
        .find(|e| e.action == action)
        .and_then(|e| e.input.as_ref())
}

fn branch_status(w: &Wfes, node: &str) -> Option<BranchStatus> {
    w.branches
        .iter()
        .find(|b| b.branch_node == node)
        .map(|b| b.status)
}

/// Saf OR (1-of-N): İLK varış join'i tamamlar; kalan iki kol `cancelled`,
/// WFE join node'una geçer. `_fork` marker'ı eşiği taşır.
#[tokio::test]
async fn or_join_first_arrival_completes_and_cancels_siblings() {
    let store = Arc::new(ParStore::default());
    let exec = quorum_executor(store.clone(), None);
    let wfe_id = fork_setup(&exec).await;

    assert_eq!(
        store.snapshot(wfe_id).join_rule,
        JoinRule::Quorum(1),
        "kural persist"
    );
    assert_eq!(
        marker(&store.snapshot(wfe_id), "_fork").and_then(|i| i.get("join_threshold").cloned()),
        Some(json!(1)),
        "_fork marker'ı eşiği taşır"
    );

    let actors = claim_all_branches(&exec, &store, wfe_id).await;
    let r = exec
        .apply(
            wfe_id,
            &actors[0],
            "approve",
            &json!({}),
            Some("self__financeApprover"),
            None,
            None,
        )
        .await
        .unwrap();
    assert!(!r.terminal);

    let w = store.snapshot(wfe_id);
    assert_eq!(w.current_node.as_deref(), Some("self__resultCoordinator"));
    assert!(w.join_target.is_none(), "paralel mod bitti");
    assert_eq!(w.join_rule, JoinRule::All, "kural temizlendi");
    assert!(
        w.wfah.entries().iter().any(|e| e.action == "_join"),
        "_join marker"
    );
    // Kol satırları quorum yolunda KALIR: hangi kol düştü görünür.
    assert_eq!(
        branch_status(&w, "self__financeApprover"),
        Some(BranchStatus::Arrived)
    );
    assert_eq!(
        branch_status(&w, "self__legalApprover"),
        Some(BranchStatus::Cancelled)
    );
    assert_eq!(
        branch_status(&w, "self__hrApprover"),
        Some(BranchStatus::Cancelled)
    );
    let collapse = marker(&w, "_collapse").expect("_collapse özeti");
    assert_eq!(collapse["kind"], json!("join_quorum"));
    assert_eq!(collapse["reason"], json!("join_quorum"));
    assert_eq!(collapse["target"], json!("self__resultCoordinator"));
    let cancelled: Vec<&str> = w
        .wfah
        .entries()
        .iter()
        .filter(|e| e.action == "_branch_cancelled")
        .filter_map(|e| e.input.as_ref()?.get("node")?.as_str())
        .collect();
    assert_eq!(cancelled.len(), 2, "iki kardeş kol iptal marker'ı: {cancelled:?}");
    assert!(
        !w.wfah
            .entries()
            .iter()
            .any(|e| e.action == "_branch_superseded"),
        "quorum'da superseded YOK"
    );
}

/// 2-of-3 quorum: birinci varış BranchArrived (paralel mod sürer), ikinci varış
/// join'i tamamlar; üçüncü kol `cancelled`. Varmış kardeş `superseded` OLMAZ —
/// onayı quorum'un parçasıdır.
#[tokio::test]
async fn quorum_2_of_3_completes_on_second_arrival() {
    let store = Arc::new(ParStore::default());
    let exec = quorum_executor(store.clone(), Some(2));
    let wfe_id = fork_setup(&exec).await;
    assert_eq!(store.snapshot(wfe_id).join_rule, JoinRule::Quorum(2));

    let actors = claim_all_branches(&exec, &store, wfe_id).await;

    // 1. varış: eşik dolmadı → paralel mod sürer.
    let r = exec
        .apply(
            wfe_id,
            &actors[0],
            "approve",
            &json!({}),
            Some("self__financeApprover"),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(r.current_node, None, "eşik dolmadı, hâlâ paralel");
    let w = store.snapshot(wfe_id);
    assert!(w.join_target.is_some());
    assert_eq!(active_count(&w), 2);

    // 2. varış: eşik doldu → join.
    let r = exec
        .apply(
            wfe_id,
            &actors[1],
            "approve",
            &json!({}),
            Some("self__legalApprover"),
            None,
            None,
        )
        .await
        .unwrap();
    assert!(!r.terminal);
    let w = store.snapshot(wfe_id);
    assert_eq!(w.current_node.as_deref(), Some("self__resultCoordinator"));
    assert!(w.join_target.is_none());
    assert_eq!(
        branch_status(&w, "self__financeApprover"),
        Some(BranchStatus::Arrived),
        "quorum üyesi varmış kol arrived KALIR"
    );
    assert_eq!(
        branch_status(&w, "self__hrApprover"),
        Some(BranchStatus::Cancelled),
        "eşik dışında kalan kol iptal"
    );
    assert!(
        !w.wfah
            .entries()
            .iter()
            .any(|e| e.action == "_branch_superseded"),
        "quorum üyesinin onayı geçersizleşmez"
    );
}

/// Quorum dolduktan sonra iptal edilen kol ARTIK aksiyon alamaz: paralel mod
/// bitmiştir, o kolun claim'i düşmüştür. "Geç kalan onay" sessizce uygulanıp
/// akışı ikinci kez ilerletemez.
#[tokio::test]
async fn cancelled_branch_cannot_act_after_quorum() {
    let store = Arc::new(ParStore::default());
    let exec = quorum_executor(store.clone(), Some(2));
    let wfe_id = fork_setup(&exec).await;
    let actors = claim_all_branches(&exec, &store, wfe_id).await;
    for (a, node) in [
        (&actors[0], "self__financeApprover"),
        (&actors[1], "self__legalApprover"),
    ] {
        exec.apply(wfe_id, a, "approve", &json!({}), Some(node), None, None)
            .await
            .unwrap();
    }
    // Üçüncü kol iptal edildiği için aksiyon alamaz — paralel mod da bitti.
    let err = exec
        .apply(
            wfe_id,
            &actors[2],
            "approve",
            &json!({}),
            Some("self__hrApprover"),
            None,
            None,
        )
        .await
        .expect_err("iptal edilmiş kol aksiyon alamaz");
    assert!(
        matches!(err, EngineError::InvalidInput(_) | EngineError::TransitionNotFound(_)),
        "beklenmeyen hata: {err:?}"
    );
}

/// Quorum join'de EŞZAMANLI iki varış: eşik 2 iken ikisi de "ben ikinciyim"
/// diyemez — biri BranchArrived olur, diğeri join'i tamamlar. Kaybeden taraf
/// Conflict alıp executor retry ile doğru outcome'a düşer, lost update olmaz.
#[tokio::test(start_paused = true)]
async fn concurrent_arrivals_in_quorum_resolve_to_single_join() {
    let store = Arc::new(ParStore::default());
    let exec = quorum_executor(store.clone(), Some(2));
    let wfe_id = fork_setup(&exec).await;
    let actors = claim_all_branches(&exec, &store, wfe_id).await;

    store.commit_delays_ms.lock().unwrap().extend([100, 200]);
    let input = json!({});
    let a = exec.apply(
        wfe_id,
        &actors[0],
        "approve",
        &input,
        Some("self__financeApprover"),
        None,
        None,
    );
    let b = exec.apply(
        wfe_id,
        &actors[1],
        "approve",
        &input,
        Some("self__legalApprover"),
        None,
        None,
    );
    let (ra, rb) = tokio::join!(a, b);
    assert!(ra.is_ok() && rb.is_ok(), "ikisi de uygulanmalı: {ra:?} / {rb:?}");

    let w = store.snapshot(wfe_id);
    assert_eq!(
        w.current_node.as_deref(),
        Some("self__resultCoordinator"),
        "iki varıştan sonra eşik dolmuş olmalı"
    );
    assert!(w.join_target.is_none());
    assert_eq!(
        w.wfah.entries().iter().filter(|e| e.action == "_join").count(),
        1,
        "_join TEK kez"
    );
}

// ---- WOR-73: ZEN join koşulu (uçtan uca orkestrasyon) --------------------------

/// Fixture'ın fork'unu ZEN join koşuluna çevirir: "(finans VE hukuk) YA DA İK".
fn paralel_with_join_expr() -> Wfd {
    let mut v: Value = serde_json::from_str(PARALLEL_FIXTURE).unwrap();
    for t in v["transitions"].as_array_mut().unwrap() {
        if t["wft"].get("parallel").is_some() {
            t["wft"]["parallel"]["join_mode"] = json!("expr");
            t["wft"]["parallel"]["join_when"] = json!(
                "($branches.self__financeApprover and $branches.self__legalApprover) or $branches.self__hrApprover"
            );
        }
    }
    Wfd::from_value(v).unwrap()
}

fn expr_executor(store: Arc<ParStore>) -> WfeExecutor {
    WfeExecutor::new(
        Arc::new(MockOrg),
        Arc::new(FixtureWfdStore(paralel_with_join_expr())),
        store,
        Arc::new(MockRunner),
    )
}

/// `or` tarafı: İK tek başına onaylayınca join dolar, iki kardeş kol iptal olur.
/// Eşikle ifade edilemez — finans tek başına YETMEZ (aşağıdaki teste bak).
#[tokio::test]
async fn expr_join_hr_alone_completes_and_cancels_siblings() {
    let store = Arc::new(ParStore::default());
    let exec = expr_executor(store.clone());
    let wfe_id = fork_setup(&exec).await;
    assert!(
        matches!(store.snapshot(wfe_id).join_rule, JoinRule::Expr(_)),
        "ZEN kuralı persist"
    );

    let actors = claim_all_branches(&exec, &store, wfe_id).await;
    exec.apply(
        wfe_id,
        &actors[2],
        "approve",
        &json!({}),
        Some("self__hrApprover"),
        None,
        None,
    )
    .await
    .unwrap();

    let w = store.snapshot(wfe_id);
    assert_eq!(w.current_node.as_deref(), Some("self__resultCoordinator"));
    assert_eq!(w.join_rule, JoinRule::All, "paralel mod bitti, kural temizlendi");
    assert_eq!(
        branch_status(&w, "self__financeApprover"),
        Some(BranchStatus::Cancelled)
    );
    assert_eq!(
        branch_status(&w, "self__legalApprover"),
        Some(BranchStatus::Cancelled)
    );
    let collapse = marker(&w, "_collapse").expect("_collapse özeti");
    assert_eq!(collapse["kind"], json!("join_quorum"));
}

/// `and` tarafı: finans TEK BAŞINA yetmez (ara varış), hukuk da onaylayınca dolar.
#[tokio::test]
async fn expr_join_finance_alone_waits_then_legal_completes() {
    let store = Arc::new(ParStore::default());
    let exec = expr_executor(store.clone());
    let wfe_id = fork_setup(&exec).await;
    let actors = claim_all_branches(&exec, &store, wfe_id).await;

    let r = exec
        .apply(
            wfe_id,
            &actors[0],
            "approve",
            &json!({}),
            Some("self__financeApprover"),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(r.current_node, None, "finans tek başına yetmez");
    assert!(store.snapshot(wfe_id).join_target.is_some());

    exec.apply(
        wfe_id,
        &actors[1],
        "approve",
        &json!({}),
        Some("self__legalApprover"),
        None,
        None,
    )
    .await
    .unwrap();

    let w = store.snapshot(wfe_id);
    assert_eq!(w.current_node.as_deref(), Some("self__resultCoordinator"));
    assert_eq!(
        branch_status(&w, "self__hrApprover"),
        Some(BranchStatus::Cancelled),
        "kural dolduğu için İK kolu iptal"
    );
    assert_eq!(
        branch_status(&w, "self__financeApprover"),
        Some(BranchStatus::Arrived),
        "kuralın üyesi kol arrived kalır"
    );
}

/// `_fork` marker'ı ZEN kuralını taşır — "neden İK tek başına yetti" audit'ten okunur.
#[tokio::test]
async fn fork_marker_records_join_expression() {
    let store = Arc::new(ParStore::default());
    let exec = expr_executor(store.clone());
    let wfe_id = fork_setup(&exec).await;
    let w = store.snapshot(wfe_id);
    let fork = marker(&w, "_fork").expect("_fork marker");
    assert_eq!(fork["join_mode"], json!("expr"));
    assert!(fork["join_when"]
        .as_str()
        .expect("join_when")
        .contains("$branches.self__hrApprover"));
    assert_eq!(fork["join_threshold"], Value::Null);
}

// ======================= 2026-08-13: kol c_a projeksiyonu (fill_view_grants)

/// Fork commit'i HER kol için aday listesi taşır.
///
/// Kol c_a'sı `wf.wfe_branch.c_a` kolonuna yazılır ve görünürlük SQL'i (aktif kol
/// kanalı) onu okur. Fork'ta yazılmazsa paralel moda giren iş, kolların
/// havuzunda GÖRÜNMEZ — kol satırı var, adayı yok. Bu testin işi o boşluğu
/// kapatmaktır.
#[tokio::test(start_paused = true)]
async fn fork_commit_carries_candidate_list_per_branch() {
    let store = Arc::new(ParStore::default());
    let exec = executor(store.clone());
    let wfe_id = fork_setup(&exec).await;

    let recorded = store.last_branch_c_a.lock().unwrap().clone();
    let w = store.snapshot(wfe_id);
    let active: Vec<&str> = w
        .branches
        .iter()
        .filter(|b| b.status == BranchStatus::Active)
        .map(|b| b.branch_node.as_str())
        .collect();

    assert!(!active.is_empty(), "fork sonrası aktif kol beklenir");
    for node in &active {
        let entry = recorded.iter().find(|(n, _)| n == node);
        let (_, len) = entry.unwrap_or_else(|| panic!("kol '{node}' için c_a yazılmamış: {recorded:?}"));
        assert!(*len > 0, "kol '{node}' aday listesi BOŞ yazılmış");
    }
}
