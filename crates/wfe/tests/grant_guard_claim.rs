//! `E02`/S2 + `Ç9` — **grant guard'ı false'a dönünce claim aynı transaction'da düşer.**
//!
//! Ç9 grant'ın `when`ini "her yetki sorgusunda değerlendirilir" diye tanımladı; E02/S2
//! bunun YAZMA tarafını verdi: WFAH satırı stage eden HER commit'in sonunda, claim
//! sahibinin yetkisi POST-APPEND defterle yeniden değerlendirilir. Düşerse claim aynı
//! transaction'da bırakılır ve deftere `claim_released:<node>` / `reason:
//! "grant_guard_false"` satırı yazılır.
//!
//! ## Neden ayrı dosya
//!
//! Kapı iki şeyi AYNI ANDA ister ve mevcut hiçbir harness ikisini birden vermiyor:
//!
//! 1. **Rol farkındalığı.** Claim'i alan kişi node'un KENDİ havuzunda olmamalı, yalnız
//!    grant'la yetkili olmalı — yoksa guard false dönse bile claim'in düşmemesi DOĞRU
//!    cevaptır ve test hiçbir şey ölçmez. Mevcut mock'lar `check_user_role`'dan daima
//!    `true` döndürüyor.
//! 2. **Gerçek adapter'ın claim davranışı.** Öteki in-memory store'lar `commit`in
//!    sonunda claim'i KOŞULSUZ düşürüyor (E02'nin "12. sessiz yer" dediği şey); böyle
//!    bir store'da bu test kuralı değil, store'un kör davranışını ölçerdi. Buradaki
//!    store `clears_claim()` sorar — gerçek adapter da öyle yapar.
//!
//! ⚠️ `sim`de SINANAMAZ (E02/S3): orada `claimed_at` daima NULL ve projeksiyon kolonu
//! yok.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
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
    AutoexecRunner, ClaimRecheck, ExecEnv, ExecFailure, NewWfe, TransitionCommit, WfdStore,
    WfeStore, Wfes,
};
use wfe_core::EngineError;

/// `mudur` node'unda İKİ kademe. Birincinin grant'ı, İKİNCİNİN ateşlenmemiş olmasına
/// bağlı — yani ikinci kademe ateşlendiği ANDA birincinin guard'ı false döner.
///
/// Guard'ın `$wfah`a bakması bilinçli: E02/S2'nin kapsamı "ctx yazan commit" DEĞİL,
/// "WFAH satırı stage eden HER yol"dur ve bu fixture tam olarak o farkı kurar —
/// ikinci kademenin commit'i ctx'e HİÇ dokunmaz, yalnız marker yazar.
fn wfd_json() -> Value {
    json!({
        "wfd_version": "2.3",
        "expression_language": "zen@1",
        "id": "grant-guard-claim",
        "name": "Grant guard'ı ve claim",
        "version": "1.0.0",
        "context": {"type": "object", "properties": {}},
        "wf_admin": [{
            "c_a": {"c_orgu": "self", "c_r": ["wf_admin"]},
            "allowed_global_actions": ["skip_escalation"]
        }],
        "nodes": {
            "memur": {"c_a": {"c_orgu": "self", "c_r": ["memur"]}},
            "mudur": {
                "c_a": {"c_orgu": "self", "c_r": ["mudur"]},
                // Madde 7: sahibi işi devredebilir — `reassign` testinin ön koşulu.
                "reassign": {"c_orgu": "self", "c_r": ["mudur"]},
                "escalation": [
                    {
                        "after": "PT1H",
                        "grant": {
                            "c_a": {"c_orgu": "self", "c_r": ["denetci"]},
                            "when": "count($wfah, #.action == \"escalate:mudur:1\") == 0"
                        }
                    },
                    {
                        "after": "PT2H",
                        "grant": {"c_a": {"c_orgu": "self", "c_r": ["arsiv"]}}
                    }
                ]
            }
        },
        "start": [{"id": "s1", "action": "basvur"}],
        "actions": {
            "basvur": {"input": {"required": [], "optional": []},
                       "from": "memur", "wft": {"node": "mudur"}},
            "onayla": {"input": {"required": [], "optional": []},
                       "from": "mudur", "wft": {"terminal": "bitti"}}
        },
        "terminals": [{"id": "bitti", "wfe_end_response": {}}]
    })
}

// ---- mock'lar ---------------------------------------------------------------

/// Rol farkındalıklı org portu: kim hangi rolü taşıyor, testin kendisi söyler.
/// `check_user_role`dan daima `true` dönen bir mock bu testi ANLAMSIZ kılardı —
/// denetçi zaten node havuzunda sayılır, claim'in grant'a bağlı olduğu iddiası
/// hiç kurulamazdı.
struct RoleOrg {
    roles: HashMap<Uuid, Vec<&'static str>>,
}

#[async_trait]
impl OrgPort for RoleOrg {
    async fn resolve_c_orgu(
        &self,
        anchor: Uuid,
        _expr: &str,
        _orgtnt: Uuid,
    ) -> Result<Vec<OrgUnit>, EngineError> {
        Ok(vec![OrgUnit {
            orgu_id: anchor,
            orgu_type: json!({"type": "sube"}),
            path: "1".into(),
        }])
    }
    async fn check_user_role(
        &self,
        user_id: Uuid,
        _orgu_id: Uuid,
        role: &str,
    ) -> Result<bool, EngineError> {
        Ok(self
            .roles
            .get(&user_id)
            .is_some_and(|rs| rs.contains(&role)))
    }
    async fn orgtnt_for_orgu(&self, _: Uuid) -> Result<Uuid, EngineError> {
        Ok(Uuid::nil())
    }
}

struct NoRunner;

#[async_trait]
impl AutoexecRunner for NoRunner {
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

/// In-memory store — claim'i GERÇEK adapter gibi ele alır.
#[derive(Default)]
struct MemStore {
    wfes: Mutex<HashMap<Uuid, Wfes>>,
    /// Son commit'in havuz projeksiyonu (`wf.wfe.current_c_a`), rol kırılımıyla.
    /// `E02`/S2'nin atlama yolu kanıtı buradan okunur: satır yazmak yetmez, havuz
    /// kolonunun da genişlemesi gerekir.
    last_pool: Mutex<Vec<String>>,
}

impl MemStore {
    fn snapshot(&self, wfe_id: Uuid) -> Wfes {
        self.wfes.lock().unwrap().get(&wfe_id).cloned().unwrap()
    }

    /// Defterdeki her satırın `applied_at`ini geriye alır — SLA sayaçlarının tabanı
    /// (`R02`: `to_node != null` olan son satır) böylece geçmişe kayar. Testte
    /// gerçek zaman beklemek yerine defteri yaşlandırıyoruz.
    fn age_wfah(&self, wfe_id: Uuid, by: chrono::Duration) {
        let mut g = self.wfes.lock().unwrap();
        let w = g.get_mut(&wfe_id).unwrap();
        for e in &mut w.wfah.0 {
            e.applied_at -= by;
        }
        if let Some(at) = w.claimed_at {
            w.claimed_at = Some(at - by);
        }
    }
}

#[async_trait]
impl WfeStore for MemStore {
    async fn load(&self, wfe_id: Uuid) -> Result<Wfes, EngineError> {
        self.wfes
            .lock()
            .unwrap()
            .get(&wfe_id)
            .cloned()
            .ok_or_else(|| EngineError::WfePort(format!("not found: {wfe_id}")))
    }

    async fn create(&self, new: &NewWfe) -> Result<(), EngineError> {
        let (status, current_node, end_response) = new.outcome.resolution();
        let wfes = Wfes {
            wfe_id: new.wfe_id,
            orgtnt_id: new.orgtnt_id,
            environment_id: None,
            wfd_id: new.wfd_id,
            wfd_version: new.wfd_version,
            dynctx: DynCtx(new.initial_dynctx.clone()),
            visited_nodes: vec![],
            wfah: Wfah(new.wfah_entries.clone()),
            status,
            current_node: current_node.map(str::to_string),
            end_terminal: new.end_terminal.clone(),
            assigned_to: None,
            end_response: end_response.cloned(),
            deadline: new.deadline,
            claimed_at: None,
            created_at: chrono::Utc::now(),
            branches: vec![],
            join_target: None,
            join_rule: JoinRule::All,
            origin_orgu_id: Some(new.origin_orgu_id),
        };
        self.wfes.lock().unwrap().insert(new.wfe_id, wfes);
        Ok(())
    }

    async fn commit(&self, commit: &TransitionCommit) -> Result<(), EngineError> {
        let mut map = self.wfes.lock().unwrap();
        let wfes = map
            .get_mut(&commit.wfe_id)
            .ok_or_else(|| EngineError::WfePort(format!("not found: {}", commit.wfe_id)))?;
        let (status, current_node, end_response) = commit.outcome.resolution();
        wfes.dynctx = DynCtx(commit.new_dynctx.clone());
        wfes.wfah.0.extend(commit.wfah_entries.iter().cloned());
        wfes.status = status;
        wfes.current_node = current_node.map(str::to_string);
        if let Some(end) = end_response {
            wfes.end_response = Some(end.clone());
        }
        if commit.end_terminal.is_some() {
            wfes.end_terminal = commit.end_terminal.clone();
        }
        *self.last_pool.lock().unwrap() =
            commit.resolved_c_a.iter().map(|c| c.role.clone()).collect();
        // E02/S1-EK: claim'i KOŞULSUZ düşüren store, `StayAt`i olduğu gibi yutar ve
        // guard-false mantığını hiç sınamaz. Soru tek yerde cevaplanır.
        if commit.outcome.clears_claim() {
            wfes.assigned_to = None;
            wfes.claimed_at = None;
        }
        // E02/S2: yetki yeniden değerlendirmesinin cevabı AYNI transaction'da uygulanır.
        // Gerçek adapter da böyle yapar — iki iş ayrı tx'e bölünseydi arada "claim'i
        // yok ama deftere göre sahibi var" penceresi kalırdı.
        match &commit.claim_recheck {
            ClaimRecheck::Released { entry } => {
                wfes.assigned_to = None;
                wfes.claimed_at = None;
                wfes.wfah.0.push(entry.clone());
            }
            ClaimRecheck::Kept | ClaimRecheck::NotApplicable => {}
        }
        Ok(())
    }

    async fn claim(
        &self,
        wfe_id: Uuid,
        _orgtnt_id: Uuid,
        user_id: Uuid,
        _branch: Option<&str>,
        marker: &WfahEntry,
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
        wfes.wfah.0.push(marker.clone());
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
        let Some(wfes) = map.get_mut(&wfe_id) else {
            return Ok(());
        };
        wfes.assigned_to = None;
        wfes.claimed_at = None;
        wfes.wfah.0.push(wfah_entry.clone());
        if let Some(ctx) = new_dynctx {
            wfes.dynctx = DynCtx(ctx.clone());
        }
        Ok(())
    }

    async fn reassign(
        &self,
        wfe_id: Uuid,
        _orgtnt_id: Uuid,
        target: Option<Uuid>,
        wfah_entries: &[WfahEntry],
        _branch: Option<&str>,
    ) -> Result<(), EngineError> {
        let mut map = self.wfes.lock().unwrap();
        let Some(wfes) = map.get_mut(&wfe_id) else {
            return Ok(());
        };
        wfes.assigned_to = target;
        wfes.claimed_at = target.map(|_| chrono::Utc::now());
        wfes.wfah.0.extend(wfah_entries.iter().cloned());
        Ok(())
    }
}

// ---- kapı --------------------------------------------------------------------

fn actor(user: Uuid, orgu: Uuid, role: &str) -> Actor {
    Actor {
        orgu_id: orgu,
        user_id: user,
        role: role.into(),
    }
}

/// **E02/S2'nin ANA KABULÜ.**
///
/// 1. İş `mudur`da; denetçi node havuzunda DEĞİL (`c_r: ["mudur"]`).
/// 2. 1. kademe ateşlenir → grant açılır, denetçi claim ALABİLİR (E04: `c_a ∪ grant`).
/// 3. 2. kademe ateşlenir. O commit ctx'e HİÇ dokunmaz, yalnız
///    `escalate:mudur:1` satırını yazar — ama 1. kademenin guard'ı tam olarak o
///    satırın YOKLUĞUNA bakıyordu. POST-APPEND defterle guard false döner.
/// 4. Denetçinin yetkisi düştü → claim AYNI transaction'da bırakılır ve deftere
///    `claim_released:mudur` / `reason: "grant_guard_false"` yazılır.
///
/// Eski davranış: claim ayakta kalırdı. Denetçi, yetkisi kalmadığı hâlde işi
/// tutmaya devam eder; havuzdaki kimse alamaz. Hata sessizdir — log da yoktur.
#[tokio::test]
async fn claim_taken_through_a_grant_drops_when_its_guard_turns_false() {
    let wfd = Wfd::from_value(wfd_json()).expect("fixture geçerli olmalı");
    let sube = Uuid::new_v4();
    let (memur, mudur, denetci) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let org = RoleOrg {
        roles: HashMap::from([
            (memur, vec!["memur"]),
            (mudur, vec!["mudur"]),
            (denetci, vec!["denetci"]),
        ]),
    };
    let store = Arc::new(MemStore::default());
    let exec = WfeExecutor::new(
        Arc::new(org),
        Arc::new(FixtureWfdStore(wfd)),
        store.clone(),
        Arc::new(NoRunner),
    );

    let started = exec
        .start(
            Uuid::new_v4(),
            1,
            &actor(memur, sube, "memur"),
            Some("basvur"),
            &json!({}),
            None,
        )
        .await
        .expect("başlatma");
    let wfe_id = started.wfe_id;

    // Denetçi HENÜZ alamaz: node havuzunda yok, grant da açılmadı.
    assert!(
        !exec
            .claim(wfe_id, &actor(denetci, sube, "denetci"), None, None)
            .await
            .expect("claim sorgusu")
            .success,
        "ön koşul: grant açılmadan denetçi claim ALAMAMALI"
    );

    // 1. kademe vadesi (PT1H) dolsun.
    store.age_wfah(wfe_id, chrono::Duration::minutes(61));
    assert!(
        exec.tick_timers(wfe_id).await.expect("timer"),
        "1. kademe ateşlenmeli"
    );

    // Grant açıldı → denetçi artık alabilir (E04: node.c_a ∪ açılmış grantlar).
    assert!(
        exec.claim(wfe_id, &actor(denetci, sube, "denetci"), None, None)
            .await
            .expect("claim")
            .success,
        "grant açıldıktan sonra denetçi claim ALABİLMELİ"
    );
    assert_eq!(store.snapshot(wfe_id).assigned_to, Some(denetci));

    // 2. kademe vadesi (PT2H) dolsun; ateşlenince 1. kademenin guard'ı false döner.
    store.age_wfah(wfe_id, chrono::Duration::minutes(70));
    assert!(
        exec.tick_timers(wfe_id).await.expect("timer"),
        "2. kademe ateşlenmeli"
    );

    let w = store.snapshot(wfe_id);
    let actions: Vec<&str> = w.wfah.entries().iter().map(|e| e.action.as_str()).collect();
    assert!(
        actions.contains(&"escalate:mudur:1"),
        "ön koşul: 2. kademe marker'ı yazıldı: {actions:?}"
    );

    assert_eq!(
        w.assigned_to, None,
        "guard false döndü — denetçinin claim'i DÜŞMELİ, iş havuza dönmeli"
    );
    let released = w
        .wfah
        .entries()
        .iter()
        .find(|e| e.action == "claim_released:mudur")
        .expect("bırakma satırı deftere yazılmalı — sessiz düşürme YOK");
    assert_eq!(
        released.input.as_ref().expect("payload")["reason"],
        json!("grant_guard_false"),
        "sebep ADda değil payload'da taşınır (Ç1-EK)"
    );
}

/// `E02`/S2 — **`reassign` hedefi de `c_a ∪ grant` üzerinden sorulur.**
///
/// Kapı bugüne dek düz `node.c_a`ya bakıyordu: escalation grant'ıyla havuza giren
/// kişi listede GÖRÜNÜYOR ama admin işi ona DEVREDEMİYORDU (`TargetNotEligible`).
/// "Görüyorum ama veremiyorum" hâli, `E04`ün kapatmayı hedeflediği ayrışmanın devir
/// yolundaki karşılığı.
///
/// Karar `reassign` için ayrıca RET diyor (assign-then-release YAPILMAZ, ek satır
/// yazılmaz) — bu test o retlerin YANLIŞ hedefe uygulanmadığını çiviler: yetkili
/// hedef reddedilmemeli.
#[tokio::test]
async fn reassign_accepts_a_target_authorised_only_by_an_open_grant() {
    let wfd = Wfd::from_value(wfd_json()).expect("fixture geçerli olmalı");
    let sube = Uuid::new_v4();
    let (memur, mudur, denetci) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let org = RoleOrg {
        roles: HashMap::from([
            (memur, vec!["memur"]),
            (mudur, vec!["mudur"]),
            (denetci, vec!["denetci"]),
        ]),
    };
    let store = Arc::new(MemStore::default());
    let exec = WfeExecutor::new(
        Arc::new(org),
        Arc::new(FixtureWfdStore(wfd)),
        store.clone(),
        Arc::new(NoRunner),
    );

    let started = exec
        .start(
            Uuid::new_v4(),
            1,
            &actor(memur, sube, "memur"),
            Some("basvur"),
            &json!({}),
            None,
        )
        .await
        .expect("başlatma");
    let wfe_id = started.wfe_id;

    // Müdür işi üstlenir (node'un KENDİ havuzu).
    assert!(
        exec.claim(wfe_id, &actor(mudur, sube, "mudur"), None, None)
            .await
            .expect("claim")
            .success
    );

    // 1. kademe ateşlenir → denetçi havuza GRANT'la girer.
    store.age_wfah(wfe_id, chrono::Duration::minutes(61));
    assert!(
        exec.tick_timers(wfe_id).await.expect("timer"),
        "ön koşul: kademe ateşlenmeli"
    );

    // Müdür işi denetçiye devreder. Hedefin yetkisi YALNIZ grant'tan geliyor.
    exec.reassign(
        wfe_id,
        &actor(mudur, sube, "mudur"),
        Some(&actor(denetci, sube, "denetci")),
        None,
    )
    .await
    .expect("grant'la yetkili hedef reddedilmemeli");

    assert_eq!(
        store.snapshot(wfe_id).assigned_to,
        Some(denetci),
        "devir hedefe oturmalı"
    );
}

/// `E02`/S2 — **atlama yolu `append_marker`dan `StayAt` commit'ine geçti.**
///
/// `escalate:<node>:<idx>:skipped` satırı kademeyi ATEŞLENMİŞ sayar (`E13`: sayaç
/// kaymaz), dolayısıyla o kademenin grant'ı AÇILIR. Ama `append_marker` YALNIZ WFAH
/// yazıyordu: havuz kolonu (`current_c_a`) eski hâlinde kalıyor, genişleyen yetki
/// hiçbir listeye inmiyordu. Satır deftere düşer, kimse görmez.
///
/// `append_marker`ın imzası claim düşürmeyi de İFADE EDEMİYORDU (`Ç9`un guard-false
/// kuralı) — kararın (b) seçeneği: atlama yolu tam commit'e geçer, WFAH yazıp claim'i
/// ayakta bırakan İKİNCİ yol kalmaz.
#[tokio::test]
async fn skipping_a_step_opens_its_grant_in_the_pool_projection() {
    let wfd = Wfd::from_value(wfd_json()).expect("fixture geçerli olmalı");
    let sube = Uuid::new_v4();
    let (memur, admin, denetci) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let org = RoleOrg {
        roles: HashMap::from([
            (memur, vec!["memur"]),
            (admin, vec!["wf_admin"]),
            (denetci, vec!["denetci"]),
        ]),
    };
    let store = Arc::new(MemStore::default());
    let exec = WfeExecutor::new(
        Arc::new(org),
        Arc::new(FixtureWfdStore(wfd)),
        store.clone(),
        Arc::new(NoRunner),
    );

    let started = exec
        .start(
            Uuid::new_v4(),
            1,
            &actor(memur, sube, "memur"),
            Some("basvur"),
            &json!({}),
            None,
        )
        .await
        .expect("başlatma");
    let wfe_id = started.wfe_id;

    // WF Admin sıradaki kademeyi ATLAR — vade beklemeden.
    exec.skip_escalation(wfe_id, &actor(admin, sube, "wf_admin"), None)
        .await
        .expect("atlama");

    let w = store.snapshot(wfe_id);
    let actions: Vec<&str> = w.wfah.entries().iter().map(|e| e.action.as_str()).collect();
    assert!(
        actions.contains(&"escalate:mudur:0:skipped"),
        "ön koşul: atlama satırı yazıldı: {actions:?}"
    );

    let pool = store.last_pool.lock().unwrap().clone();
    assert!(
        pool.iter().any(|r| r == "denetci"),
        "atlanan kademenin grant'ı havuz kolonuna inmeli — satır yazmak YETMEZ: {pool:?}"
    );
    // İş KIPIRDAMADI: atlama bir geçiş değildir.
    assert_eq!(w.current_node.as_deref(), Some("mudur"));
    assert_eq!(w.status, WfeStatus::Active);
}
