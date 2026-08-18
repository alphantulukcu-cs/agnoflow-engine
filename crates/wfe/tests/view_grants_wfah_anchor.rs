//! `listable[]`in WFAH ÇAPALI kuralı, çapaladığı aksiyon uygulandığı COMMIT'te
//! projeksiyona girmeli — bir sonrakinde değil.
//!
//! Regresyon: `WfeExecutor::fill_view_grants` projeksiyonu `wfes.wfah` ile
//! çözüyordu; o defter, o an uygulanan aksiyonun kaydını HENÜZ içermiyor (kayıt
//! `commit.wfah_entries`te staged duruyor). Sonuç: `{from: {wfah: "onayla"}}`
//! çapalı bir kural "onayla" anında `view_c_a`ya yazılmıyor, liste/havuz ucu
//! (saf projeksiyon) o pencerede kimseye göstermiyordu — oysa referans okuma
//! `can_view` canlı defteri gördüğü için görünür diyordu.
//!
//! DB'siz: `timer_service.rs` kalıbında in-memory `WfeStore`; farkı, `commit`in
//! yazdığı `view_c_a`yı da saklaması (asıl iddia onun üstünde).

use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
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

// ---- WFD: memur başlatır → müdür onaylar; GM'i müdürün biriminden çapala ----

fn wfd_json() -> Value {
    json!({
        "wfd_version": "2.2",
        "expression_language": "zen@1",
        "id": "wfah-anchor-listable",
        "name": "WFAH çapalı listable",
        "version": "1.0.0",
        "context": {"type": "object", "properties": {}},
        "nodes": {
            "memur": {
                "label": "Şube Memuru",
                "c_a": {"c_orgu": "*:[type:sube]", "c_r": ["memur"]}
            },
            "mudur": {
                "label": "Şube Müdürü",
                "c_a": {"c_orgu": "*:[type:sube]", "c_r": ["mudur"]}
            }
        },
        "start": [{
            "id": "start__memur",
            "from": "memur",
            "action": "basvur",
            "wft": {"node": "mudur"}
        }],
        "actions": {
            "basvur": {"label": "Başvur", "input": {"required": [], "optional": []}},
            "onayla": {"label": "Onayla", "input": {"required": [], "optional": []}}
        },
        "transitions": [{
            "id": "t_onayla",
            "from": "mudur",
            "action": "onayla",
            "wft": {"terminal": "bitti"}
        }],
        "terminals": [{"id": "bitti", "wfe_end_response": {"status": "ok"}}],
        // "onayla'yı yapanın biriminin parent'ındaki genel müdür" — kural ancak
        // "onayla" WFAH'a girdikten SONRA bir birim çözebilir.
        "listable": [{
            "c_a": {
                "c_orgu": {
                    "from": {"wfah": "onayla", "field": "actor.orgu"},
                    "traverse": "self.parent"
                },
                "c_r": ["genel_mudur"]
            }
        }]
    })
}

// ---- mock'lar --------------------------------------------------------------

/// Üç ifadeyi ayırt eden minimal org: `*:[…]` = tenant'taki iki şube, `self` =
/// çapanın kendisi, `self.parent` = çapanın DETERMİNİSTİK parent'ı (`parent_of`).
///
/// Ayırt edicilik testin çekirdeği: `self.parent` çapaya göre farklı bir birim
/// verdiği için "çapa WFAH aktörü mü, WFE'nin origin'i mi" sorusu iddiada
/// görünür hâle gelir. Tek birim döndüren bir mock ikisini aynı gösterirdi.
struct MockOrg {
    units: Vec<Uuid>,
}

/// Birimin parent'ı — gerçek ltree yerine tersinir bir eşleme.
fn parent_of(orgu: Uuid) -> Uuid {
    Uuid::from_u128(orgu.as_u128() ^ 1)
}

fn unit(orgu_id: Uuid) -> OrgUnit {
    OrgUnit {
        orgu_id,
        orgu_type: json!({"type": "sube"}),
        path: "1".into(),
    }
}

#[async_trait]
impl OrgPort for MockOrg {
    async fn resolve_c_orgu(
        &self,
        anchor: Uuid,
        expr: &str,
        _orgtnt: Uuid,
    ) -> Result<Vec<OrgUnit>, EngineError> {
        Ok(match expr {
            e if e.starts_with("*:") => self.units.iter().copied().map(unit).collect(),
            "self.parent" => vec![unit(parent_of(anchor))],
            _ => vec![unit(anchor)],
        })
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

// ---- test ------------------------------------------------------------------

fn actor(role: &str) -> Actor {
    Actor {
        orgu_id: Uuid::new_v4(),
        user_id: Uuid::new_v4(),
        role: role.into(),
    }
}

#[tokio::test]
async fn wfah_anchored_listable_lands_in_projection_on_the_same_commit() {
    let memur = actor("memur");
    let mudur = actor("mudur");

    let wfd = Wfd::from_value(wfd_json()).unwrap();
    let store = Arc::new(MemStore::default());
    let executor = WfeExecutor::new(
        // Memur ile müdür AYRI şubelerde: kural WFAH çapasını kullanmazsa
        // (ya da bir commit geç yazarsa) müdürün parent'ı projeksiyona girmez.
        Arc::new(MockOrg {
            units: vec![memur.orgu_id, mudur.orgu_id],
        }),
        Arc::new(FixtureWfdStore(wfd)),
        store.clone(),
        Arc::new(MockRunner),
    );

    let started = executor
        .start(Uuid::new_v4(), 1, &memur, Some("basvur"), &json!({}), None)
        .await
        .unwrap();
    let wfe_id = started.wfe_id;

    // Start'ta "onayla" henüz WFAH'ta yok → çapa çözülmez → grant YOK. Bu doğru
    // davranıştır (bkz. `resolver::resolve_c_orgu`: çözülemeyen çapa BOŞ küme).
    assert!(
        store.view_grants(wfe_id).is_empty(),
        "onayla yapılmadan grant yazılmış"
    );

    assert!(executor
        .claim(wfe_id, &mudur, None, None)
        .await
        .unwrap()
        .success);
    executor
        .apply(wfe_id, &mudur, "onayla", &json!({}), None, None, None)
        .await
        .unwrap();

    // Regresyonun düştüğü yer: "onayla" AYNI commit'te WFAH'a giriyor, dolayısıyla
    // çapa da AYNI commit'te çözülmeli. Eski davranışta bu liste boştu.
    let grants = store.view_grants(wfe_id);
    assert!(
        grants
            .iter()
            .any(|c| c.role == "genel_mudur" && c.orgu_id == Some(parent_of(mudur.orgu_id))),
        "WFAH çapalı listable projeksiyona girmedi \
         (çapa ONAYLAYANIN birimi olmalı, WFE'nin origin'i değil): {grants:?}"
    );

    // WFE terminal oldu ama grant KALICI: `view_c_a` silinmez.
    assert_eq!(store.snapshot(wfe_id).unwrap().status, WfeStatus::Terminal);
}
