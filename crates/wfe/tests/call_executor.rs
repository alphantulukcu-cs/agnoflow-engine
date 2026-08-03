//! WFC executor entegrasyon testleri — engine'in ÜRETTİĞİ outbox satırının
//! executor tarafından nasıl işlendiği.
//!
//! `wfe-core/tests/call_runtime.rs` outbox satırının DOĞRULUĞUNU sınar; buradaki
//! testler onun ötesindeki executor davranışlarını kapsar:
//!   • uçtan uca `wait`: çağrı başlatılır → çağrılan biter → çağıran ilerler
//!   • ardıl (`terminal`): çağıran `completed` kalır, ardıl ayrı bir WFE olarak başlar
//!   • **Handoff Isolation**: ardıl başlatılamasa bile çağıran `completed` kalır
//!   • derinlik frenleri (`depth` / `next_depth` + `max_next`) → satır `skipped`
//!   • **WFC-CASCADE**: çağıran sonlanınca alt akışlar iptal, ardıl KAPSAM DIŞI
//!   • `start_as` aktör seçimi (actor = son ACT, system = akışı başlatan)
//!
//! In-memory store `WfeAdapter`'ın WFC semantiğini taklit eder: outbox satırları,
//! `mark_callee_finished`'ın mode'a göre `returned`/`consumed` ayrımı ve
//! `cancel_subcalls_of`'un ardılı atlaması.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;
use wf_wfe::WfeExecutor;
use wfe_core::ports::OrgPort;
use wfe_core::types::actor::{Actor, OrgUnit};
use wfe_core::types::dynctx::DynCtx;
use wfe_core::types::wfah::{Wfah, WfahEntry};
use wfe_core::types::wfd_v22::{AutoexecDef, CallMode, JoinRule, StartAs, Wfd};
use wfe_core::types::wfe::WfeStatus;
use wfe_core::v22::ports::{
    AutoexecRunner, CallSite, CallView, CommitOutcome, ExecEnv, ExecFailure, NewWfe, PendingCall,
    StagedCall, TransitionCommit, WfdStore, WfeStore, Wfes,
};
use wfe_core::EngineError;

const CALLER: &str = include_str!("../../wfe-core/tests/fixtures/akis-cagrisi.json");
const SKOR: &str = include_str!("../../wfe-core/tests/fixtures/kredi-skor.json");
const KULLANDIRIM: &str = include_str!("../../wfe-core/tests/fixtures/kredi-kullandirim.json");

// ---- mock'lar --------------------------------------------------------------

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

struct NoRunner;

#[async_trait]
impl AutoexecRunner for NoRunner {
    async fn run(&self, _def: &AutoexecDef, _env: &ExecEnv) -> Result<Value, ExecFailure> {
        Err(ExecFailure::failed("bu fixture'da autoexec yok"))
    }
}

/// Doküman `id`'sine göre çözen WFD deposu — `WfdAdapter`'ın `resolve_doc`'unun
/// bellekteki karşılığı. `wfd_id` (uuid) her doküman için deterministik üretilir.
struct DocStore {
    docs: Vec<Wfd>,
    /// Belirli bir doküman `id`'si için çözümü kapatır (yayınlanmamış senaryosu).
    unresolvable: Option<String>,
}

impl DocStore {
    fn new() -> Self {
        Self {
            docs: [CALLER, SKOR, KULLANDIRIM]
                .iter()
                .map(|s| Wfd::from_json(s).unwrap())
                .collect(),
            unresolvable: None,
        }
    }

    fn uuid_for(&self, index: usize) -> Uuid {
        Uuid::from_u128(0xC0FFEE00 + index as u128)
    }
}

#[async_trait]
impl WfdStore for DocStore {
    async fn fetch(&self, wfd_id: Uuid, _version: i32) -> Result<Wfd, EngineError> {
        self.docs
            .iter()
            .enumerate()
            .find(|(i, _)| self.uuid_for(*i) == wfd_id)
            .map(|(_, w)| w.clone())
            .ok_or_else(|| EngineError::WfdPort(format!("bilinmeyen wfd_id {wfd_id}")))
    }

    async fn resolve_doc(
        &self,
        _orgtnt_id: Uuid,
        doc_id: &str,
        _doc_version: Option<&str>,
    ) -> Result<Option<(Uuid, i32)>, EngineError> {
        if self.unresolvable.as_deref() == Some(doc_id) {
            return Ok(None);
        }
        Ok(self
            .docs
            .iter()
            .position(|w| w.id == doc_id)
            .map(|i| (self.uuid_for(i), 1)))
    }
}

/// Bellekte tutulan bir WFC satırı — `wf.wfe_call` şemasının aynası.
#[derive(Clone)]
struct CallRow {
    id: Uuid,
    orgtnt_id: Uuid,
    caller_wfe_id: Uuid,
    site: CallSite,
    call_key: String,
    mode: CallMode,
    input: Value,
    deadline: Option<DateTime<Utc>>,
    start_as: StartAs,
    max_next: Option<u32>,
    depth: i32,
    next_depth: i32,
    status: String,
    callee_wfe_id: Option<Uuid>,
    end_response: Option<Value>,
    call_status: Option<String>,
}

impl CallRow {
    fn to_pending(&self) -> PendingCall {
        PendingCall {
            id: self.id,
            orgtnt_id: self.orgtnt_id,
            caller_wfe_id: self.caller_wfe_id,
            site: self.site.clone(),
            call_key: self.call_key.clone(),
            mode: self.mode,
            input: self.input.clone(),
            deadline: self.deadline,
            start_as: self.start_as,
            max_next: self.max_next,
            depth: self.depth,
            next_depth: self.next_depth,
            callee_wfe_id: self.callee_wfe_id,
            end_response: self.end_response.clone(),
            call_status: self.call_status.clone(),
        }
    }
}

#[derive(Default)]
struct MemStore {
    wfes: Mutex<HashMap<Uuid, Wfes>>,
    calls: Mutex<Vec<CallRow>>,
}

impl MemStore {
    fn snapshot(&self, wfe_id: Uuid) -> Option<Wfes> {
        self.wfes.lock().unwrap().get(&wfe_id).cloned()
    }
    fn rows(&self) -> Vec<CallRow> {
        self.calls.lock().unwrap().clone()
    }
    fn row_for(&self, call_key: &str) -> CallRow {
        self.rows()
            .into_iter()
            .find(|r| r.call_key == call_key)
            .unwrap_or_else(|| panic!("'{call_key}' çağrı satırı yok"))
    }

    /// `WfeAdapter::commit`'in outbox yazımı: derinlik sayaçları ÇAĞIRANIN satırından
    /// taşınır ve moda göre biri +1'lenir.
    fn stage(&self, orgtnt_id: Uuid, caller: Uuid, staged: &[StagedCall]) {
        let (depth, next_depth) = self
            .rows()
            .into_iter()
            .find(|r| r.callee_wfe_id == Some(caller))
            .map(|r| (r.depth, r.next_depth))
            .unwrap_or((0, 0));
        let mut calls = self.calls.lock().unwrap();
        for c in staged {
            // UNIQUE (caller, site_kind, site_key) — çift start koruması.
            if calls
                .iter()
                .any(|r| r.caller_wfe_id == caller && r.site == c.site)
            {
                continue;
            }
            let (d, nd) = match c.mode {
                CallMode::Terminal => (depth, next_depth + 1),
                _ => (depth + 1, next_depth),
            };
            calls.push(CallRow {
                id: Uuid::new_v4(),
                orgtnt_id,
                caller_wfe_id: caller,
                site: c.site.clone(),
                call_key: c.call_key.clone(),
                mode: c.mode,
                input: c.input.clone(),
                deadline: c.deadline,
                start_as: c.start_as,
                max_next: c.max_next,
                depth: d,
                next_depth: nd,
                status: "queued".into(),
                callee_wfe_id: None,
                end_response: None,
                call_status: None,
            });
        }
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
        self.wfes.lock().unwrap().insert(
            new.wfe_id,
            Wfes {
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
                created_at: Utc::now(),
                branches: vec![],
                join_target: None,
            join_rule: JoinRule::All,
            },
        );
        self.stage(new.orgtnt_id, new.wfe_id, &new.staged_calls);
        Ok(())
    }

    async fn commit(&self, commit: &TransitionCommit) -> Result<(), EngineError> {
        {
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
            wfes.assigned_to = None;
            wfes.claimed_at = None;
        }
        self.stage(commit.orgtnt_id, commit.wfe_id, &commit.staged_calls);
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
        let Some(w) = map.get_mut(&wfe_id) else {
            return Ok(false);
        };
        if w.assigned_to.is_some() {
            return Ok(false);
        }
        w.assigned_to = Some(user_id);
        w.claimed_at = Some(Utc::now());
        Ok(true)
    }

    async fn release_claim(
        &self,
        _wfe_id: Uuid,
        _orgtnt_id: Uuid,
        _wfah_entry: &WfahEntry,
        _branch: Option<&str>,
        _new_dynctx: Option<&Value>,
    ) -> Result<(), EngineError> {
        Ok(())
    }

    async fn reassign(
        &self,
        _wfe_id: Uuid,
        _orgtnt_id: Uuid,
        _target: Option<Uuid>,
        _wfah_entry: &WfahEntry,
        _branch: Option<&str>,
    ) -> Result<(), EngineError> {
        Ok(())
    }

    // ---- WFC ----

    async fn pending_call_starts(&self, _limit: i64) -> Result<Vec<PendingCall>, EngineError> {
        Ok(self
            .rows()
            .iter()
            .filter(|r| r.status == "queued")
            .map(CallRow::to_pending)
            .collect())
    }

    async fn pending_call_returns(&self, _limit: i64) -> Result<Vec<PendingCall>, EngineError> {
        Ok(self
            .rows()
            .iter()
            .filter(|r| r.status == "returned")
            .map(CallRow::to_pending)
            .collect())
    }

    async fn overdue_calls(
        &self,
        now: DateTime<Utc>,
        _limit: i64,
    ) -> Result<Vec<PendingCall>, EngineError> {
        Ok(self
            .rows()
            .iter()
            .filter(|r| {
                matches!(r.status.as_str(), "queued" | "running")
                    && r.mode == CallMode::Wait
                    && r.deadline.is_some_and(|d| d <= now)
            })
            .map(CallRow::to_pending)
            .collect())
    }

    async fn set_call_status(
        &self,
        call_row_id: Uuid,
        status: &str,
        callee_wfe_id: Option<Uuid>,
    ) -> Result<(), EngineError> {
        let mut calls = self.calls.lock().unwrap();
        if let Some(r) = calls.iter_mut().find(|r| r.id == call_row_id) {
            r.status = status.to_string();
            if callee_wfe_id.is_some() {
                r.callee_wfe_id = callee_wfe_id;
            }
        }
        Ok(())
    }

    async fn cancel_subcalls_of(&self, caller_wfe_id: Uuid) -> Result<Vec<Uuid>, EngineError> {
        let mut calls = self.calls.lock().unwrap();
        let mut out = Vec::new();
        for r in calls.iter_mut() {
            // Ardıl KAPSAM DIŞI — çağıranın ömrüne bağlı değil.
            if r.caller_wfe_id == caller_wfe_id
                && r.mode != CallMode::Terminal
                && matches!(r.status.as_str(), "queued" | "running" | "returned")
            {
                r.status = "cancelled".into();
                if let Some(c) = r.callee_wfe_id {
                    out.push(c);
                }
            }
        }
        Ok(out)
    }

    async fn mark_callee_finished(
        &self,
        callee_wfe_id: Uuid,
        status: &str,
        end_response: Option<&Value>,
    ) -> Result<(), EngineError> {
        let mut calls = self.calls.lock().unwrap();
        for r in calls.iter_mut() {
            if r.callee_wfe_id == Some(callee_wfe_id)
                && matches!(r.status.as_str(), "running" | "queued")
            {
                // `wait` dönüş bekler; diğer modlarda dönüş YOKTUR.
                r.status = if r.mode == CallMode::Wait {
                    "returned".into()
                } else {
                    "consumed".into()
                };
                r.call_status = Some(status.to_string());
                r.end_response = end_response.cloned();
            }
        }
        Ok(())
    }

    async fn calls_of_caller(&self, caller_wfe_id: Uuid) -> Result<Vec<CallView>, EngineError> {
        Ok(self
            .rows()
            .iter()
            .filter(|r| r.caller_wfe_id == caller_wfe_id)
            .map(|r| CallView {
                site_kind: r.site.kind().into(),
                site_key: r.site.key().into(),
                call_key: r.call_key.clone(),
                mode: r.mode.as_str().into(),
                status: r.status.clone(),
                wfe_id: r.callee_wfe_id,
                call_status: r.call_status.clone(),
            })
            .collect())
    }
}

// ---- kurulum --------------------------------------------------------------

struct Harness {
    exec: WfeExecutor,
    store: Arc<MemStore>,
    wfd: Arc<DocStore>,
}

fn harness_with(docs: DocStore) -> Harness {
    let store = Arc::new(MemStore::default());
    let wfd = Arc::new(docs);
    let exec = WfeExecutor::new(
        Arc::new(MockOrg),
        wfd.clone(),
        store.clone(),
        Arc::new(NoRunner),
    );
    Harness { exec, store, wfd }
}

fn harness() -> Harness {
    harness_with(DocStore::new())
}

fn actor(role: &str) -> Actor {
    Actor {
        orgu_id: Uuid::new_v4(),
        user_id: Uuid::new_v4(),
        role: role.into(),
    }
}

/// Çağıran akışı başlatır → `self__creditAnalyst` çağrı node'unda bekler.
async fn start_caller(h: &Harness, clerk: &Actor) -> Uuid {
    let caller_wfd_id = h.wfd.uuid_for(0);
    let res = h
        .exec
        .start(
            caller_wfd_id,
            1,
            clerk,
            Some("basvuru_olustur"),
            &json!({ "basvuru": { "musteri_no": "M-42", "tutar": 50000 } }),
            None,
        )
        .await
        .expect("çağıran başlamalı");
    assert_eq!(res.current_node.as_deref(), Some("self__creditAnalyst"));
    res.wfe_id
}

// ================================================================ wait: uçtan uca

#[tokio::test]
async fn wait_call_starts_callee_and_resumes_caller_on_completion() {
    let h = harness();
    let clerk = actor("branchClerk");
    let caller_id = start_caller(&h, &clerk).await;

    // Outbox: çağrı `queued` olarak yazıldı.
    let row = h.store.row_for("kredi_skor_sorgusu");
    assert_eq!(row.status, "queued");
    assert_eq!(
        row.depth, 1,
        "alt akış çağrısı yuvalanma derinliğini 1 yapar"
    );
    assert_eq!(row.next_depth, 0);

    // Tarama çağrılanı başlatır.
    assert_eq!(h.exec.run_pending_calls(16).await.unwrap(), 1);
    let row = h.store.row_for("kredi_skor_sorgusu");
    assert_eq!(row.status, "running");
    let callee_id = row.callee_wfe_id.expect("çağrılan id'si yazılmalı");

    // WFC-IN: çağıranın ctx'inden çözülmüş girdi çağrılanın ctx'ine geçti.
    let callee = h.store.snapshot(callee_id).unwrap();
    assert_eq!(callee.dynctx.as_value()["musteri_no"], json!("M-42"));
    assert_eq!(callee.dynctx.as_value()["talep_tutari"], json!(50000));

    // Çağıran hâlâ çağrı node'unda bekliyor.
    assert_eq!(
        h.store.snapshot(caller_id).unwrap().current_node.as_deref(),
        Some("self__creditAnalyst")
    );

    // Çağrılanda skor girilir → çağrılan biter.
    let risk = actor("riskAnalyst");
    assert!(
        h.exec
            .claim(callee_id, &risk, None, None)
            .await
            .unwrap()
            .success
    );
    let applied = h
        .exec
        .apply(
            callee_id,
            &risk,
            "skor_gir",
            &json!({ "skor": 780 }),
            None,
            None,
        )
        .await
        .expect("skor girilmeli");
    assert!(applied.terminal, "çağrılan bitmeli");

    // `after_wfe_settled` satırı `returned`'e çekti — dönüş taraması işler.
    assert_eq!(h.store.row_for("kredi_skor_sorgusu").status, "returned");
    assert_eq!(h.exec.run_call_returns(16).await.unwrap(), 1);
    assert_eq!(h.store.row_for("kredi_skor_sorgusu").status, "consumed");

    // Çağıran skoru ctx'e yazdı ve müdür havuzuna ilerledi.
    let caller = h.store.snapshot(caller_id).unwrap();
    assert_eq!(caller.dynctx.as_value()["skor"], json!(780));
    assert_eq!(caller.dynctx.as_value()["skor_durumu"], json!("uygun"));
    assert_eq!(caller.current_node.as_deref(), Some("self__branchManager"));
}

// ================================================================ ardıl

/// Ardıl akış: çağıran `completed` KALIR ve ardıl ayrı bir WFE olarak başlar.
#[tokio::test]
async fn successor_starts_after_caller_completes_and_caller_stays_completed() {
    let h = harness();
    let clerk = actor("branchClerk");
    let caller_id = start_caller(&h, &clerk).await;

    // Alt akışı hızlıca bitirip müdür havuzuna gel.
    h.exec.run_pending_calls(16).await.unwrap();
    let callee_id = h.store.row_for("kredi_skor_sorgusu").callee_wfe_id.unwrap();
    let risk = actor("riskAnalyst");
    h.exec.claim(callee_id, &risk, None, None).await.unwrap();
    h.exec
        .apply(
            callee_id,
            &risk,
            "skor_gir",
            &json!({ "skor": 800 }),
            None,
            None,
        )
        .await
        .unwrap();
    h.exec.run_call_returns(16).await.unwrap();

    // Müdür onaylar → çağıran biter + ardıl stage edilir.
    let boss = actor("branchManager");
    h.exec.claim(caller_id, &boss, None, None).await.unwrap();
    let applied = h
        .exec
        .apply(
            caller_id,
            &boss,
            "manager_decide",
            &json!({ "kullandirim_tutari": 25000 }),
            None,
            None,
        )
        .await
        .unwrap();
    assert!(applied.terminal);

    let next_row = h.store.row_for("kredi_kullandirim");
    assert_eq!(next_row.status, "queued");
    assert_eq!(next_row.mode, CallMode::Terminal);
    assert_eq!(next_row.next_depth, 1, "ardıl zinciri 1 olur");
    assert_eq!(next_row.depth, 0, "ardıl yuvalanma derinliğini ARTIRMAZ");

    assert_eq!(h.exec.run_pending_calls(16).await.unwrap(), 1);
    let next_row = h.store.row_for("kredi_kullandirim");
    let successor_id = next_row.callee_wfe_id.expect("ardıl WFE yaratılmalı");
    assert_ne!(successor_id, caller_id, "ardıl AYRI bir WFE'dir");

    // Ardıl başladı ama henüz bitmedi → satır `running`. Ardıl bittiğinde `consumed`
    // olur, `returned` ASLA olmaz: `returned` yalnız `wait` modunun dönüş kuyruğudur.
    assert_eq!(next_row.status, "running");

    // Çağıran hâlâ `completed`.
    assert_eq!(
        h.store.snapshot(caller_id).unwrap().status,
        WfeStatus::Terminal
    );
    // Ardıl kendi ctx'ini WFC-IN'den aldı.
    let succ = h.store.snapshot(successor_id).unwrap();
    assert_eq!(succ.dynctx.as_value()["musteri_no"], json!("M-42"));
    assert_eq!(succ.dynctx.as_value()["tutar"], json!(25000));
}

/// **Handoff Isolation** — ardıl başlatılamasa bile çağıran `completed` kalır.
#[tokio::test]
async fn successor_failure_leaves_the_caller_completed() {
    let mut docs = DocStore::new();
    docs.unresolvable = Some("kredi-kullandirim".into()); // yayınlanmamış ardıl
    let h = harness_with(docs);
    let clerk = actor("branchClerk");
    let caller_id = start_caller(&h, &clerk).await;

    h.exec.run_pending_calls(16).await.unwrap();
    let callee_id = h.store.row_for("kredi_skor_sorgusu").callee_wfe_id.unwrap();
    let risk = actor("riskAnalyst");
    h.exec.claim(callee_id, &risk, None, None).await.unwrap();
    h.exec
        .apply(
            callee_id,
            &risk,
            "skor_gir",
            &json!({ "skor": 800 }),
            None,
            None,
        )
        .await
        .unwrap();
    h.exec.run_call_returns(16).await.unwrap();

    let boss = actor("branchManager");
    h.exec.claim(caller_id, &boss, None, None).await.unwrap();
    h.exec
        .apply(
            caller_id,
            &boss,
            "manager_decide",
            &json!({ "kullandirim_tutari": 10 }),
            None,
            None,
        )
        .await
        .unwrap();

    // Ardıl başlatma başarısız — hata YALNIZ satırda.
    h.exec.run_pending_calls(16).await.unwrap();
    assert_eq!(h.store.row_for("kredi_kullandirim").status, "failed");
    assert_eq!(
        h.store.snapshot(caller_id).unwrap().status,
        WfeStatus::Terminal,
        "ardıl hatası çağıranın sonucunu DEĞİŞTİRMEZ (Handoff Isolation)"
    );
}

/// `max_next` yerel sınırı aşılırsa ardıl HİÇ başlatılmaz, satır `skipped` olur.
#[tokio::test]
async fn next_depth_cap_skips_the_successor() {
    let h = harness();
    let clerk = actor("branchClerk");
    let caller_id = start_caller(&h, &clerk).await;

    // Satırı elle "zaten 5 tur zincirlenmiş" hale getir; fixture'da `max_next` yok →
    // global cap (16) geçerli, o yüzden `max_next: 1` ile yerel sınırı test ediyoruz.
    {
        let mut calls = h.store.calls.lock().unwrap();
        for r in calls.iter_mut() {
            r.next_depth = 5;
            r.max_next = Some(1);
        }
    }
    // Bu satır alt akış (`wait`) — ardıl sınırı ona uygulanmaz, normal başlar.
    assert_eq!(h.exec.run_pending_calls(16).await.unwrap(), 1);

    // Şimdi gerçek bir ardıl satırı üret ve sınırı aş.
    let callee_id = h.store.row_for("kredi_skor_sorgusu").callee_wfe_id.unwrap();
    let risk = actor("riskAnalyst");
    h.exec.claim(callee_id, &risk, None, None).await.unwrap();
    h.exec
        .apply(
            callee_id,
            &risk,
            "skor_gir",
            &json!({ "skor": 800 }),
            None,
            None,
        )
        .await
        .unwrap();
    h.exec.run_call_returns(16).await.unwrap();
    let boss = actor("branchManager");
    h.exec.claim(caller_id, &boss, None, None).await.unwrap();
    h.exec
        .apply(
            caller_id,
            &boss,
            "manager_decide",
            &json!({ "kullandirim_tutari": 10 }),
            None,
            None,
        )
        .await
        .unwrap();
    {
        let mut calls = h.store.calls.lock().unwrap();
        for r in calls.iter_mut().filter(|r| r.mode == CallMode::Terminal) {
            r.next_depth = 4;
            r.max_next = Some(2); // 4 > 2 → sınır aşıldı
        }
    }
    h.exec.run_pending_calls(16).await.unwrap();
    assert_eq!(
        h.store.row_for("kredi_kullandirim").status,
        "skipped",
        "sınır aşılınca ardıl başlatılmamalı"
    );
    assert_eq!(
        h.store.snapshot(caller_id).unwrap().status,
        WfeStatus::Terminal,
        "atlanan ardıl çağıranın sonucunu değiştirmez"
    );
}

/// Alt akış yuvalanma sınırı (cap 8) aşılırsa çağrı `skipped` olur.
#[tokio::test]
async fn nesting_depth_cap_skips_the_subcall() {
    let h = harness();
    let clerk = actor("branchClerk");
    start_caller(&h, &clerk).await;
    {
        let mut calls = h.store.calls.lock().unwrap();
        for r in calls.iter_mut() {
            r.depth = 9; // > MAX_CALL_DEPTH (8)
        }
    }
    assert_eq!(h.exec.run_pending_calls(16).await.unwrap(), 0);
    assert_eq!(h.store.row_for("kredi_skor_sorgusu").status, "skipped");
}

// ================================================================ cascade

/// **WFC-CASCADE**: çağıran sonlandığında koşan ALT AKIŞ iptal edilir.
#[tokio::test]
async fn cascade_cancels_running_subcall_when_caller_terminates() {
    let h = harness();
    let clerk = actor("branchClerk");
    let caller_id = start_caller(&h, &clerk).await;
    h.exec.run_pending_calls(16).await.unwrap();
    let callee_id = h.store.row_for("kredi_skor_sorgusu").callee_wfe_id.unwrap();
    assert_eq!(
        h.store.snapshot(callee_id).unwrap().status,
        WfeStatus::Active
    );

    // Çağıranın kök süresi dolar → `terminated`.
    {
        let mut map = h.store.wfes.lock().unwrap();
        let w = map.get_mut(&caller_id).unwrap();
        w.deadline = Some(Utc::now() - chrono::Duration::seconds(1));
    }
    assert!(h.exec.tick_timers(caller_id).await.unwrap());

    assert_eq!(
        h.store.snapshot(caller_id).unwrap().status,
        WfeStatus::Terminated
    );
    assert_eq!(
        h.store.row_for("kredi_skor_sorgusu").status,
        "cancelled",
        "koşan alt akış iptal edilmeli"
    );
    assert_ne!(
        h.store.snapshot(callee_id).unwrap().status,
        WfeStatus::Active,
        "iptal edilen alt akışın WFE'si de sonlandırılmalı"
    );
}

// ================================================================ start_as

/// `start_as: "system"` akışın BAŞLATICISI ile başlatır (nil bir sistem aktörü
/// hiçbir `c_a` ile eşleşmediği için ardıl asla başlayamazdı — bkz. plan §9.1).
#[tokio::test]
async fn start_as_system_uses_the_flow_initiator() {
    let h = harness();
    let clerk = actor("branchClerk");
    let caller_id = start_caller(&h, &clerk).await;

    h.exec.run_pending_calls(16).await.unwrap();
    let callee_id = h.store.row_for("kredi_skor_sorgusu").callee_wfe_id.unwrap();
    let risk = actor("riskAnalyst");
    h.exec.claim(callee_id, &risk, None, None).await.unwrap();
    h.exec
        .apply(
            callee_id,
            &risk,
            "skor_gir",
            &json!({ "skor": 800 }),
            None,
            None,
        )
        .await
        .unwrap();
    h.exec.run_call_returns(16).await.unwrap();

    let boss = actor("branchManager");
    h.exec.claim(caller_id, &boss, None, None).await.unwrap();
    h.exec
        .apply(
            caller_id,
            &boss,
            "manager_decide",
            &json!({ "kullandirim_tutari": 25000 }),
            None,
            None,
        )
        .await
        .unwrap();
    h.exec.run_pending_calls(16).await.unwrap();

    let successor_id = h
        .store
        .row_for("kredi_kullandirim")
        .callee_wfe_id
        .expect("ardıl başlamalı");
    let succ = h.store.snapshot(successor_id).unwrap();
    let initiator = &succ.wfah.entries()[0].actor;
    assert_eq!(
        initiator.user_id, clerk.user_id,
        "start_as: system → akışı BAŞLATAN aktör kullanılmalı (müdür değil)"
    );
    assert_ne!(initiator.user_id, boss.user_id);
}

// ================================================================ simülasyon

/// WFC node'u simülasyonda **çıkmaz sokak değildir**: çağrı "bekliyor" olarak durur,
/// kullanıcı sonucu elle girer ve akış oradan devam eder.
///
/// Simülasyonda gerçek bir çağrılan WFE yaratılmaz — kendi aktörleri/SLA'sı/org çözümü
/// olurdu, bu simülasyonun kapsamı değil.
#[tokio::test]
async fn simulated_call_node_is_a_resolvable_stop_not_a_dead_end() {
    use wf_wfe::sim::SimState;
    use wfe_core::v22::pipeline::Engine;

    let wfd = Wfd::from_json(CALLER).unwrap();
    let org = MockOrg;
    let runner = NoRunner;
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let clerk = actor("branchClerk");

    let new = engine
        .start(
            &wfd,
            &clerk,
            Uuid::nil(),
            Some("basvuru_olustur"),
            &json!({ "basvuru": { "musteri_no": "M-7", "tutar": 1000 } }),
            Uuid::new_v4(),
            None,
        )
        .await
        .unwrap();
    let mut sim = SimState::from_new_wfe(&new);

    // Çağrı node'unda duruyor ve bekleyen çağrı GÖRÜNÜR — editör "burada şu akış
    // çağrılacak, sonucunu gir" diyebilir.
    assert_eq!(sim.current_node.as_deref(), Some("self__creditAnalyst"));
    let awaited = sim.awaited_call().expect("bekleyen çağrı görünmeli");
    assert_eq!(awaited.call_key, "kredi_skor_sorgusu");
    assert_eq!(awaited.mode, "wait");
    // WFC-IN çözülmüş olarak sunulur (kullanıcı çağrılana ne gittiğini görür).
    assert_eq!(awaited.input["musteri_no"], json!("M-7"));

    // Kullanıcı sonucu elle girer → akış ilerler.
    let commit = engine
        .fire_call_return(
            &wfd,
            &sim.to_wfes(None),
            "completed",
            None,
            Some(&json!({ "skor": 900, "karar": "uygun" })),
            // Simülasyonda gerçek bir çağrılan WFE yok — işlenecek geçmiş de yok.
            &[],
            Utc::now(),
        )
        .await
        .expect("elle girilen sonuçla dönüş uygulanmalı");
    sim.apply_commit(&commit);
    sim.clear_awaited_call("self__creditAnalyst");

    assert_eq!(sim.current_node.as_deref(), Some("self__branchManager"));
    assert_eq!(sim.dynctx["skor"], json!(900));
    assert!(
        sim.awaited_call().is_none(),
        "çözülen çağrı bekleyen listeden düşmeli (dönüş bir kez işlenir)"
    );
}
