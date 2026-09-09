//! Paralel fork/join testlerinin ORTAK koşum takımı — in-memory `WfeStore`
//! (`ParStore`, `WfeAdapter` semantiğinin taklidi) + mock'lar + fixture varyantları.
//!
//! `fork_join.rs` ve `sim_valid_parity.rs` AYNI takımı kullanır: parite testi
//! "gerçek akış" tarafını ikinci kez yazsaydı, karşılaştırdığı iki yoldan biri
//! kendi taklidinin kopyası olurdu ve ayrışmayı göremezdi.
#![allow(dead_code)]

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
use wfe_core::types::wfd_v22::{AutoexecDef, JoinRule, Wfd};
use wfe_core::types::wfe::WfeStatus;
use wfe_core::v22::ports::{
    AutoexecRunner, BranchState, BranchStatus, CommitOutcome, ExecEnv, ExecFailure, NewWfe,
    TransitionCommit, WfdStore, WfeStore, Wfes,
};
use wfe_core::{ConflictKind, EngineError};

pub const PARALLEL_FIXTURE: &str = include_str!("../../../../docs/spec/examples/paralel-onay.json");

// ---- mock'lar (pipeline.rs kalıbı; authorize anchor = actor.orgu_id) ----------

pub struct MockOrg;

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

pub struct MockRunner;

#[async_trait]
impl AutoexecRunner for MockRunner {
    async fn run(&self, _def: &AutoexecDef, _env: &ExecEnv) -> Result<Value, ExecFailure> {
        Ok(json!({}))
    }
}

pub struct FixtureWfdStore(pub Wfd);

#[async_trait]
impl WfdStore for FixtureWfdStore {
    async fn fetch(&self, _wfd_id: Uuid, _version: i32) -> Result<Wfd, EngineError> {
        Ok(self.0.clone())
    }
}

// ---- paralel-farkında in-memory store (WfeAdapter semantiğinin taklidi) --------

#[derive(Default)]
pub struct ParStore {
    pub wfes: Mutex<HashMap<Uuid, Wfes>>,
    /// Test enjeksiyonu: >0 iken her `commit` çağrısı state'i DEĞİŞTİRMEDEN
    /// Conflict döner (adapter'ın FOR UPDATE/CAS uyumsuzluğunun karşılığı) —
    /// executor retry döngüsünü doğrulamak için.
    pub fail_commits: AtomicU32,
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
    pub commit_delays_ms: Mutex<std::collections::VecDeque<u64>>,
    /// 2026-08-13: son commit'in görünürlük projeksiyonu — `WfeExecutor::
    /// fill_view_grants`in kol başına yazdığı `branch_c_a` kaydı. Gerçek adapter
    /// bunu `wf.wfe_branch.c_a` kolonuna yazar; testte doğrulanabilmesi için
    /// mock yalnız KAYDEDER (kolon taklidi gereksiz karmaşa olurdu).
    pub last_branch_c_a: Mutex<Vec<(String, usize)>>,
    /// Aynı kaydın ROL kırılımı — "liste dolu mu" ile "genişlemiş küme yazıldı mı"
    /// ayrı sorulardır ve yalnız sayı bakan bir test ikincisini göremez.
    pub last_branch_c_a_roles: Mutex<Vec<(String, Vec<String>)>>,
    /// 2026-08-13 node listable: son commit'in kol başına `branch_view_c_a`
    /// kaydı (gerçek adapter `wf.wfe_branch.view_c_a` kolonuna yazar). `c_a`nın
    /// YANINDA ayrı tutulur — ikisinin aynı kol kümesini kapsadığı ancak ayrı
    /// kaydedilirse doğrulanabilir.
    pub last_branch_view_c_a: Mutex<Vec<(String, usize)>>,
}

impl ParStore {
    pub fn snapshot(&self, wfe_id: Uuid) -> Wfes {
        self.wfes.lock().unwrap().get(&wfe_id).cloned().unwrap()
    }
    pub fn seed(&self, wfes: Wfes) {
        self.wfes.lock().unwrap().insert(wfes.wfe_id, wfes);
    }
    /// Bir kolun claimed_at'ını geçmişe alır (claim_timeout'u tetiklemek için).
    pub fn rewind_branch_claim(&self, wfe_id: Uuid, node: &str, at: chrono::DateTime<chrono::Utc>) {
        let mut m = self.wfes.lock().unwrap();
        let w = m.get_mut(&wfe_id).unwrap();
        for b in &mut w.branches {
            if b.branch_node == node {
                b.claimed_at = Some(at);
            }
        }
    }
    /// Bir kolun entered_at'ını geçmişe alır (SLA-2 escalation dwell'i buradan ölçülür).
    /// WFE'nin kendi birimi (`origin_orgu_id`). `fill_view_grants` çapa yoksa
    /// projeksiyonu HİÇ yazmaz — sistem yollarında (timer) aktör de olmadığı için
    /// çapayı seed'den sonra kurmak gerekir.
    pub fn set_origin_orgu(&self, wfe_id: Uuid, orgu_id: Uuid) {
        let mut g = self.wfes.lock().unwrap();
        g.get_mut(&wfe_id).unwrap().origin_orgu_id = Some(orgu_id);
    }

    pub fn rewind_branch_entered(&self, wfe_id: Uuid, node: &str, at: chrono::DateTime<chrono::Utc>) {
        let mut m = self.wfes.lock().unwrap();
        let w = m.get_mut(&wfe_id).unwrap();
        for b in &mut w.branches {
            if b.branch_node == node {
                b.entered_at = at;
            }
        }
    }
}

pub fn active_count(w: &Wfes) -> usize {
    w.branches
        .iter()
        .filter(|b| b.status == BranchStatus::Active)
        .count()
}

/// WOR-73: `WfeAdapter::JoinState::arrival_matches` taklidi — engine kararını hangi
/// varış kümesi üzerinde verdiyse, kilit (burada mutex) altındaki gerçek küme de o
/// olmalı. Sayı DEĞİL küme karşılaştırılır: ZEN join koşulu sayıyla ifade edilemez,
/// ama küme aynıysa saf engine'in kararı da aynıdır (adapter ZEN çalıştırmaz).
pub fn arrival_matches(w: &Wfes, acting_branch: &str, expected: &[String]) -> bool {
    let mut actual: Vec<String> = w
        .branches
        .iter()
        .filter(|b| b.status == BranchStatus::Arrived)
        .map(|b| b.entry_node.clone())
        .collect();
    if let Some(acting) = w
        .branches
        .iter()
        .find(|b| b.status == BranchStatus::Active && b.branch_node == acting_branch)
    {
        actual.push(acting.entry_node.clone());
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
pub fn cancel_active_branches(w: &mut Wfes) {
    for b in &mut w.branches {
        if b.status == BranchStatus::Active {
            b.status = BranchStatus::Cancelled;
            b.claimed_by = None;
            b.claimed_at = None;
        }
    }
}

/// `WfeAdapter::drop_branch_rows` taklidi — E14/S2: paralel modu bitiren HER yol kol
/// satırlarını siler (`wf.wfe_branch` yalnız YAŞAYAN turu taşır). WOR-72'nin "quorum
/// join'de satırlar KALIR" davranışı DEĞİŞTİ; kol geçmişi WFAH'tan okunur.
pub fn drop_branch_rows(w: &mut Wfes) {
    w.branches.clear();
}

pub fn apply_next(w: &mut Wfes, next: &CommitOutcome) {
    drop_branch_rows(w);
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
        let mut w = self
            .wfes
            .lock()
            .unwrap()
            .get(&wfe_id)
            .cloned()
            .ok_or_else(|| EngineError::WfePort(format!("not found: {wfe_id}")))?;
        // K-2 taklidi: `WfeAdapter::build_wfes` `visited_nodes`i SAKLAMAZ, her
        // yüklemede `wf.wfah.from_node`/`to_node` kolonlarından türetir. Mimic
        // bunu yapmasaydı geri gönderme menüsü (`targets ∩ visited_nodes`) testte
        // DAİMA boş kalır ve hedef her zaman `TargetInvalid` alırdı.
        // Kaydedilmiş değer KORUNUR: `seed_*` yardımcıları hareket satırı yazmadan
        // durum kurar, oradaki node'lar defterden türetilemez.
        for e in w.wfah.entries() {
            for n in [e.from_node.as_deref(), e.to_node.as_deref()]
                .into_iter()
                .flatten()
            {
                if !w.visited_nodes.iter().any(|v| v == n) {
                    w.visited_nodes.push(n.to_string());
                }
            }
        }
        Ok(w)
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
            visited_nodes: vec![],
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
        self.seed(w);
        Ok(())
    }

    async fn commit(&self, commit: &TransitionCommit) -> Result<(), EngineError> {
        *self.last_branch_c_a_roles.lock().unwrap() = commit
            .branch_c_a
            .iter()
            .map(|(node, c_a)| {
                (
                    node.clone(),
                    c_a.iter().map(|c| c.role.clone()).collect::<Vec<_>>(),
                )
            })
            .collect();
        *self.last_branch_c_a.lock().unwrap() = commit
            .branch_c_a
            .iter()
            .map(|(node, c_a)| (node.clone(), c_a.len()))
            .collect();
        *self.last_branch_view_c_a.lock().unwrap() = commit
            .branch_view_c_a
            .iter()
            .map(|(node, view)| (node.clone(), view.len()))
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
            // v2.3 (`E02`): `StayAt` kol durumunu DEĞİŞTİRMEZ (escalation iş taşımaz).
            CommitOutcome::StayAt { .. } => {}
            CommitOutcome::MoveTo { node } => {
                w.current_node = Some(node.clone());
                w.assigned_to = None;
                w.claimed_at = None;
            }
            CommitOutcome::Terminal { end_response }
            | CommitOutcome::Failed { end_response }
            | CommitOutcome::Terminated { end_response } => {
                // E02/S1-EK: JOKER SİLİNDİ. Bu bir DAVRANIŞ değil VERİ sorusu
                // (`outcome → statü`) ve cevabı `resolution()`ta tek yerde duruyor.
                w.status = commit.outcome.resolution().0;
                w.current_node = None;
                w.end_response = Some(end_response.clone());
                w.assigned_to = None;
                w.claimed_at = None;
                // paralel modda aktif kolları iptal et + join_target temizle
                cancel_active_branches(w);
                // E14/S2: paralel mod bitti → kol satırları düşer.
                drop_branch_rows(w);
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
                // E14/S2 + Ç4-EK: `UNIQUE (wfe_id, branch_node)` TAM kısıt olarak
                // duruyor (kısmi indeks YAZILMADI). Önceki turdan kalan bir satır
                // ikinci fork girişinde INSERT'i patlatırdı; kısıtı burada da taklit
                // ediyoruz — yoksa mimic, adapter'ın yakaladığı motor hatasını
                // sessizce yutar.
                assert!(
                    w.branches.is_empty(),
                    "UNIQUE (wfe_id, branch_node): fork öncesi kol satırı kalmış: {:?}",
                    w.branches.iter().map(|b| &b.branch_node).collect::<Vec<_>>()
                );
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
                // WOR-72: quorum join'de kalan aktif kollar iptal edilir; E14/S2 ile
                // satırlar İKİ modda da `apply_next` içinde silinir.
                if *quorum_collapse {
                    cancel_active_branches(w);
                }
                // `_join` marker (adapter istisnası). Satır ÜÇÜNCÜ kez elle
                // kurulmaz: `WfeAdapter` de, `sim` de aynı yapıcıyı çağırıyor
                // (`wfah_payload::join_row`) — burada elle kurulsaydı taklit,
                // ayrışmayı yakalaması beklenen testin kendi körlüğü olurdu.
                let seq = commit.wfah_entries.last().map(|e| e.seq + 1).unwrap_or(1);
                w.wfah
                    .0
                    .push(wfe_core::v22::wfah_payload::join_row(seq, chrono::Utc::now()));
                apply_next(w, next);
            }
            CommitOutcome::CollapseTo { node, .. } => {
                // WOR-56: paralel mod biter, WFE `node`'a; aktif kollar iptal.
                cancel_active_branches(w);
                // E14/S2: collapse paralel modu KAPATIR → kol satırları düşer.
                drop_branch_rows(w);
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
        marker: &WfahEntry,
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
            w.wfah.0.push(marker.clone());
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

    async fn reassign(
        &self,
        wfe_id: Uuid,
        _orgtnt_id: Uuid,
        target: Option<Uuid>,
        wfah_entries: &[WfahEntry],
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
        w.wfah.0.extend_from_slice(wfah_entries);
        Ok(())
    }
}

// ---- yardımcılar --------------------------------------------------------------

pub fn actor(role: &str) -> Actor {
    Actor {
        orgu_id: Uuid::new_v4(),
        user_id: Uuid::new_v4(),
        role: role.into(),
    }
}

pub fn executor(store: Arc<ParStore>) -> WfeExecutor {
    let wfd = Wfd::from_json(PARALLEL_FIXTURE).unwrap();
    WfeExecutor::new(
        Arc::new(MockOrg),
        Arc::new(FixtureWfdStore(wfd)),
        store,
        Arc::new(MockRunner),
    )
}

/// start → coordinator; claim coordinator; start_review → fork. wfe_id döner.
pub async fn fork_setup(exec: &WfeExecutor) -> Uuid {
    // start kuralı: self__requester (rol=requester) → wft self__coordinator.
    let requester = actor("requester");
    let start_input = json!({"request": {"title": "Sunucu alımı", "amount": 150000}});
    let started = exec
        .start(Uuid::new_v4(), 1, &requester, None, &start_input, None)
        .await
        .unwrap();
    let wfe_id = started.wfe_id;
    assert_eq!(
        started.current_node.as_ref().map(|n| n.id.as_str()),
        Some("self__coordinator")
    );
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
/// TÜM `reject` transition'larını node hedefli collapse'a çeviren fixture varyantı
/// (WOR-56 `{"collapse": {"node": ...}}`) — `CommitOutcome::CollapseTo` üretir.
/// Fixture'ın kendi `reject`'i terminal hedeflidir; terminal yolu paralel modu
/// başka bir arm'dan bitirir, biz burada tam olarak CollapseTo'yu test ediyoruz.
/// v2.3 (`Ç11`): onay aksiyonunun adı kol BAŞINA ayrıdır — bir aksiyonu yalnız bir
/// node kullanır, dolayısıyla üç kol `approve` adını paylaşamaz.
pub fn branch_approve(node: &str) -> &'static str {
    match node {
        "self__financeApprover" => "finans_onay",
        "self__legalApprover" => "hukuk_onay",
        "self__hrApprover" => "ik_onay",
        other => panic!("bilinmeyen kol node'u: {other}"),
    }
}

pub fn paralel_with_collapse_to_node() -> Wfd {
    let mut v: Value = serde_json::from_str(PARALLEL_FIXTURE).unwrap();
    for a in ["finans_ret", "hukuk_ret", "ik_ret"] {
        v["actions"][a]["wft"] = json!({"collapse": {"node": "self__coordinator"}});
    }
    Wfd::from_value(v).unwrap()
}

pub fn collapse_executor(store: Arc<ParStore>) -> WfeExecutor {
    WfeExecutor::new(
        Arc::new(MockOrg),
        Arc::new(FixtureWfdStore(paralel_with_collapse_to_node())),
        store,
        Arc::new(MockRunner),
    )
}

/// Üç kolu da claim eder; her kolun (claim sahibi user_id'sini taşıyan) aktörünü
/// rolüyle birlikte döndürür.
pub async fn claim_all_branches(exec: &WfeExecutor, store: &ParStore, wfe_id: Uuid) -> Vec<Actor> {
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

/// Belirli bir kolun mevcut claimant'ını Actor olarak döndürür (approve için).
pub fn claim_owner(store: &ParStore, wfe_id: Uuid, node: &str) -> Actor {
    let w = store.snapshot(wfe_id);
    let b = w.branches.iter().find(|b| b.branch_node == node).unwrap();
    Actor {
        orgu_id: Uuid::new_v4(),
        user_id: b.claimed_by.expect("kol claim'li olmalı"),
        role: "placeholder".into(),
    }
}

/// Finance koluna fork ÖNCESİNE (`self__coordinator`) geri gönderme menüsü takar.
pub fn paralel_with_send_back_before_fork() -> Wfd {
    let mut v: Value = serde_json::from_str(PARALLEL_FIXTURE).unwrap();
    // v2.3 (`Ç5`): yönlendirme kuralı aksiyonun KENDİ kaydında; finans kolunun ret
    // aksiyonu `finans_ret`tir (`from` TEK node olduğu için ad kola özgüdür).
    v["actions"]["finans_ret"]["wft"] = json!({"targets": [{"node": "self__coordinator"}]});
    Wfd::from_value(v).unwrap()
}
