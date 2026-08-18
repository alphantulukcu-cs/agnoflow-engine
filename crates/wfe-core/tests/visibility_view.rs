//! WFE-level VIEW authorization gate tests (`can_view`, spec Terminology
//! VISIBILITY/V + LISTABLE/L). Mirrors the golden-fixture MockOrg/Wfes setup
//! from `tests/pipeline.rs`.
//!
//! Covers:
//! (a) owner can always view
//! (b) a WFAH participant retains view — including on a TERMINAL WFE where
//!     both assignment and current_node are cleared by the commit
//! (c) actor authorized on the current node's c_a can view
//! (d) an unrelated actor (wrong role, not owner, not participant, not
//!     listable) CANNOT view — active or terminal
//! (e) a listable-matching actor can view; the golden fixture's second
//!     listable entry carries a `when` guard — tested true and false.
//!
//! 2026-08-13 kararları (bkz. `docs/spec/decisions.md` "Görünürlük"):
//! * kriter (b) KALDIRILDI — "işe dokunmuş olmak" yetki üretmez,
//! * bitmiş işi YALNIZ `listable`/`wf_admin` gösterir,
//! * ORGTRVLANG çapası WFE'nin birimidir (`origin_orgu_id`), soran kişinin DEĞİL.
//! Bu üçünün bekçisi dosyanın sonundaki `anchor_*` / `terminal_*` testleridir.

use async_trait::async_trait;
use serde_json::{json, Value};
use uuid::Uuid;
use wfe_core::error::EngineError;
use wfe_core::ports::OrgPort;
use wfe_core::types::actor::{Actor, OrgUnit};
use wfe_core::types::dynctx::DynCtx;
use wfe_core::types::wfah::Wfah;
use wfe_core::types::wfd_v22::{AutoexecDef, CaGrantRule, COrgu, CandidateActor, JoinRule};
use wfe_core::types::wfd_v22::Wfd;
use wfe_core::types::wfd_v22::WftTarget;
use wfe_core::types::wfe::WfeStatus;
use wfe_core::v22::pipeline::{ClaimCheck, Engine};
use wfe_core::v22::ports::{AutoexecRunner, BranchState, BranchStatus, ExecEnv, ExecFailure, Wfes};
use wfe_core::v22::visibility::can_view;

const FIXTURE: &str = include_str!("fixtures/kredi-basvuru.golden.json");
const PARALLEL_FIXTURE: &str = include_str!("fixtures/paralel-onay.json");

fn golden() -> Wfd {
    Wfd::from_json(FIXTURE).unwrap()
}

// ---- mock org: her ifade actor'ün anchor'ına çözülür (pipeline.rs testleriyle aynı) ----

struct MockOrg {
    role_assigned: bool,
}

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
        Ok(self.role_assigned)
    }
    async fn orgtnt_for_orgu(&self, _: Uuid) -> Result<Uuid, EngineError> {
        Ok(Uuid::nil())
    }
}

fn clerk(orgu: Uuid) -> Actor {
    Actor {
        orgu_id: orgu,
        user_id: Uuid::new_v4(),
        role: "branchClerk".into(),
    }
}

fn analyst(orgu: Uuid) -> Actor {
    Actor {
        orgu_id: orgu,
        user_id: Uuid::new_v4(),
        role: "creditAnalyst".into(),
    }
}

fn manager(orgu: Uuid) -> Actor {
    Actor {
        orgu_id: orgu,
        user_id: Uuid::new_v4(),
        role: "branchManager".into(),
    }
}

fn credit_dept_manager(orgu: Uuid) -> Actor {
    Actor {
        orgu_id: orgu,
        user_id: Uuid::new_v4(),
        role: "creditDeptManager".into(),
    }
}

fn start_input(amount_requested: i64) -> Value {
    json!({
        "applicant": {"name": "Ayşe Yılmaz", "tckid": "12345678901", "income": 30000},
        "credit_info": {"amount_requested": amount_requested, "purpose": "ev tadilatı"}
    })
}

fn wfes_at(node: &str, assigned: Option<Uuid>, ctx: Value) -> Wfes {
    let system = Actor {
        orgu_id: Uuid::nil(),
        user_id: Uuid::nil(),
        role: "system".into(),
    };
    let wfah = Wfah::empty().push("start".into(), system, None);
    let created_at = wfah.entries()[0].applied_at;
    Wfes {
        wfe_id: Uuid::new_v4(),
        orgtnt_id: Uuid::nil(),
        environment_id: None,
        wfd_id: Uuid::new_v4(),
        wfd_version: 1,
        dynctx: DynCtx(ctx),
        wfah,
        status: WfeStatus::Active,
        current_node: Some(node.into()),
        end_terminal: None,
        assigned_to: assigned,
        end_response: None,
        deadline: None,
        claimed_at: None,
        created_at,
        branches: vec![],
        join_target: None,
        join_rule: JoinRule::All,
        origin_orgu_id: None,
    }
}

// ================================================================ (a) owner

#[tokio::test]
async fn owner_can_view_regardless_of_role() {
    let org = MockOrg {
        role_assigned: false,
    };
    let owner_id = Uuid::new_v4();
    // owner has no relation to node c_a nor listable — only assignment matters
    let owner = Actor {
        orgu_id: Uuid::new_v4(),
        user_id: owner_id,
        role: "branchClerk".into(),
    };
    let wfes = wfes_at("self__creditAnalyst", Some(owner_id), start_input(30_000));

    assert!(can_view(&golden(), &wfes, &owner, &org).await.unwrap());
}

// ================================================ (b) katılımcı — KALDIRILDI

/// 2026-08-13 ürün kararı: "bu işe dokunmuş olmak" görünürlük ÜRETMEZ.
///
/// Eskiden bu senaryo `true` dönerdi (kriter (b)). Artık iş bittiğinde geriye
/// yalnız kalıcı grant'lar (`listable`/`wf_admin`) kalır; golden fixture'ın
/// `listable`'ı bu aktörü kapsamadığı için katılımcı da göremez. Takip
/// edilmesi istenen akış `listable[]` ile AÇIKÇA işaretlenir — yetki belgeden
/// okunur, geçmişten türetilmez. Ölçüm: `visibility_report` (57 çift).
#[tokio::test]
async fn wfah_participant_cannot_view_terminal_wfe() {
    let org = MockOrg {
        role_assigned: false,
    };
    let orgu = Uuid::new_v4();
    let participant = clerk(orgu);

    let mut wfes = wfes_at("self__creditAnalyst", None, start_input(30_000));
    wfes.wfah = wfes.wfah.push(
        "reject".into(),
        participant.clone(),
        Some(json!({"red_sebebi": "x"})),
    );
    wfes.status = WfeStatus::Terminal;
    wfes.current_node = None;

    assert!(!can_view(&golden(), &wfes, &participant, &org)
        .await
        .unwrap());
}

#[tokio::test]
async fn non_participant_cannot_view_terminal_wfe() {
    let org = MockOrg {
        role_assigned: true,
    };
    let orgu = Uuid::new_v4();
    // clerk: WFAH'ta yalnız system + başka bir kullanıcı var; listable rolleri
    // (branchManager/creditDeptManager) ile de eşleşmiyor
    let outsider = clerk(orgu);

    let mut wfes = wfes_at("self__creditAnalyst", None, start_input(30_000));
    wfes.wfah = wfes.wfah.push("reject".into(), clerk(Uuid::new_v4()), None);
    wfes.status = WfeStatus::Terminal;
    wfes.current_node = None;

    assert!(!can_view(&golden(), &wfes, &outsider, &org).await.unwrap());
}

/// System aktörünün (nil uuid) WFAH kayıtları katılımcı grant'i üretmez.
#[tokio::test]
async fn system_wfah_entries_do_not_grant_view() {
    let org = MockOrg {
        role_assigned: false,
    };
    let nil_viewer = Actor {
        orgu_id: Uuid::new_v4(),
        user_id: Uuid::nil(),
        role: "x".into(),
    };

    let mut wfes = wfes_at("self__creditAnalyst", None, start_input(30_000));
    wfes.status = WfeStatus::Terminal;
    wfes.current_node = None;

    // wfah'ta yalnızca system (nil) kaydı var — nil user_id'li viewer bile geçemez
    assert!(!can_view(&golden(), &wfes, &nil_viewer, &org).await.unwrap());
}

// ================================================================ (c) node c_a

#[tokio::test]
async fn actor_authorized_on_current_node_c_a_can_view() {
    let org = MockOrg {
        role_assigned: true,
    };
    let orgu = Uuid::new_v4();
    let a = analyst(orgu); // self__creditAnalyst node c_a = {c_orgu: self, c_r: [creditAnalyst]}
    let wfes = wfes_at("self__creditAnalyst", None, start_input(30_000));

    assert!(can_view(&golden(), &wfes, &a, &org).await.unwrap());
}

// ================================================================ (c) unrelated actor

#[tokio::test]
async fn unrelated_actor_cannot_view() {
    let org = MockOrg {
        role_assigned: true,
    };
    let orgu = Uuid::new_v4();
    // clerk: not owner, not node c_a (creditAnalyst), not in either listable rule
    // (branchManager / creditDeptManager)
    let c = clerk(orgu);
    let wfes = wfes_at("self__creditAnalyst", None, start_input(30_000));

    assert!(!can_view(&golden(), &wfes, &c, &org).await.unwrap());
}

// ================================================================ (d) listable

#[tokio::test]
async fn listable_actor_without_when_can_view() {
    let org = MockOrg {
        role_assigned: true,
    };
    let orgu = Uuid::new_v4();
    // listable[0]: {c_a: {c_orgu: self, c_r: [branchManager]}} — no `when`
    let m = manager(orgu);
    let wfes = wfes_at("self__creditAnalyst", None, start_input(30_000));

    assert!(can_view(&golden(), &wfes, &m, &org).await.unwrap());
}

#[tokio::test]
async fn listable_actor_with_when_true_can_view() {
    let org = MockOrg {
        role_assigned: true,
    };
    let orgu = Uuid::new_v4();
    // listable[1]: {when: "$ctx.credit_info.amount_requested >= 100000",
    //               c_a: {c_orgu: parent, c_r: [creditDeptManager]}}
    let d = credit_dept_manager(orgu);
    let wfes = wfes_at("self__creditAnalyst", None, start_input(150_000));

    assert!(can_view(&golden(), &wfes, &d, &org).await.unwrap());
}

#[tokio::test]
async fn listable_actor_with_when_false_cannot_view() {
    let org = MockOrg {
        role_assigned: true,
    };
    let orgu = Uuid::new_v4();
    let d = credit_dept_manager(orgu);
    // amount_requested below the listable[1] `when` threshold
    let wfes = wfes_at("self__creditAnalyst", None, start_input(30_000));

    assert!(!can_view(&golden(), &wfes, &d, &org).await.unwrap());
}

// ================================================== (c) WOR-31 parallel mode

fn paralel() -> Wfd {
    Wfd::from_json(PARALLEL_FIXTURE).unwrap()
}

fn role_actor(orgu: Uuid, role: &str) -> Actor {
    Actor {
        orgu_id: orgu,
        user_id: Uuid::new_v4(),
        role: role.into(),
    }
}

fn branch(node: &str, status: BranchStatus, claimed_by: Option<Uuid>) -> BranchState {
    let now = chrono::Utc::now();
    BranchState {
        entry_node: String::new(),
        branch_node: node.into(),
        status,
        claimed_by,
        claimed_at: claimed_by.map(|_| now),
        entered_at: now,
    }
}

/// Fork SONRASI paralel mod: current_node NULL, join_target persist,
/// wfe-seviyesi assignment temiz — görünürlük kolların c_a'sından türemeli.
fn parallel_wfes(branches: Vec<BranchState>) -> Wfes {
    let mut wfes = wfes_at(
        "self__coordinator",
        None,
        json!({"request": {"title": "Sunucu alımı", "amount": 150000}}),
    );
    wfes.current_node = None;
    wfes.branches = branches;
    wfes.join_target = Some(WftTarget::Node {
        node: "self__resultCoordinator".into(),
    });
    wfes
}

#[tokio::test]
async fn parallel_active_branch_c_a_actor_can_view() {
    let org = MockOrg {
        role_assigned: true,
    };
    let fin = role_actor(Uuid::new_v4(), "financeApprover");
    let wfes = parallel_wfes(vec![
        branch("self__financeApprover", BranchStatus::Active, None),
        branch("self__legalApprover", BranchStatus::Active, None),
    ]);

    assert!(can_view(&paralel(), &wfes, &fin, &org).await.unwrap());
}

/// Kolu claim eden kullanıcı — rol ataması doğrulanamasa bile (ör. $ctx
/// sonradan değişip c_a eşleşmez olsa da) kol sahibi olarak görmeye devam eder.
#[tokio::test]
async fn parallel_branch_claimer_can_view_without_role() {
    let org = MockOrg {
        role_assigned: false,
    };
    let claimer = role_actor(Uuid::new_v4(), "financeApprover");
    let wfes = parallel_wfes(vec![
        branch(
            "self__financeApprover",
            BranchStatus::Active,
            Some(claimer.user_id),
        ),
        branch("self__legalApprover", BranchStatus::Active, None),
    ]);

    assert!(can_view(&paralel(), &wfes, &claimer, &org).await.unwrap());
}

/// `arrived` kol artık aktif node değildir — o kolun c_a'sı VIEW grant'i üretmez.
#[tokio::test]
async fn parallel_arrived_branch_c_a_does_not_grant_view() {
    let org = MockOrg {
        role_assigned: true,
    };
    let fin = role_actor(Uuid::new_v4(), "financeApprover");
    let wfes = parallel_wfes(vec![
        branch("self__financeApprover", BranchStatus::Arrived, None),
        branch("self__legalApprover", BranchStatus::Active, None),
    ]);

    assert!(!can_view(&paralel(), &wfes, &fin, &org).await.unwrap());
}

#[tokio::test]
async fn parallel_unrelated_actor_cannot_view() {
    let org = MockOrg {
        role_assigned: true,
    };
    let outsider = role_actor(Uuid::new_v4(), "requester");
    let wfes = parallel_wfes(vec![
        branch("self__financeApprover", BranchStatus::Active, None),
        branch("self__legalApprover", BranchStatus::Active, None),
    ]);

    assert!(!can_view(&paralel(), &wfes, &outsider, &org).await.unwrap());
}

// ================================================================ (e) wf_admin
// T‑A5: akış yöneticisi yönettiği akışı GÖRÜR — yoksa yönetemez.

fn golden_with_wf_admin(when: Option<&str>) -> Wfd {
    let mut wfd = golden();
    wfd.listable.clear(); // (d) yolunu kapat: (e) tek başına test edilsin
    wfd.wf_admin = vec![CaGrantRule {
        c_a: CandidateActor {
            c_orgu: Some(COrgu::Selector("self".into())),
            c_r: Some(vec!["creditDeptManager".into()]),
            c_u: None,
        },
        when: when.map(String::from),
    }];
    wfd
}

#[tokio::test]
async fn wf_admin_can_view() {
    let org = MockOrg {
        role_assigned: true,
    };
    let orgu = Uuid::new_v4();
    let admin = credit_dept_manager(orgu);
    // Sahip DEĞİL, WFAH katılımcısı DEĞİL, aktif node'un c_a'sı creditAnalyst.
    let wfes = wfes_at("self__creditAnalyst", Some(Uuid::new_v4()), start_input(30000));

    assert!(
        can_view(&golden_with_wf_admin(None), &wfes, &admin, &org)
            .await
            .unwrap(),
        "wf_admin kuralına uyan aktör WFE'yi görmeli"
    );
}

#[tokio::test]
async fn non_matching_actor_still_cannot_view() {
    let org = MockOrg {
        role_assigned: true,
    };
    let orgu = Uuid::new_v4();
    let outsider = clerk(orgu); // wf_admin kuralı creditDeptManager ister
    let wfes = wfes_at("self__creditAnalyst", Some(Uuid::new_v4()), start_input(30000));

    assert!(
        !can_view(&golden_with_wf_admin(None), &wfes, &outsider, &org)
            .await
            .unwrap(),
        "wf_admin VARLIĞI görme hakkı vermez — kural eşleşmeli"
    );
}

#[tokio::test]
async fn wf_admin_when_guard_gates_visibility() {
    let org = MockOrg {
        role_assigned: true,
    };
    let orgu = Uuid::new_v4();
    let admin = credit_dept_manager(orgu);
    let wfes = wfes_at("self__creditAnalyst", Some(Uuid::new_v4()), start_input(30000));

    let gated = golden_with_wf_admin(Some("$ctx.credit_info.amount_requested > 100000"));
    assert!(
        !can_view(&gated, &wfes, &admin, &org).await.unwrap(),
        "when false iken görünmemeli (30.000 < 100.000)"
    );

    let big = wfes_at("self__creditAnalyst", Some(Uuid::new_v4()), start_input(250000));
    assert!(
        can_view(&gated, &big, &admin, &org).await.unwrap(),
        "when true iken görünmeli"
    );
}

// ============================================ 2026-08-13: çapa + terminal kuralı
//
// MockOrg her ifadeyi ANCHOR'a çözer (`resolve_c_orgu` → `[anchor]`), yani
// golden fixture'ın `listable[0]` kuralı (`{c_orgu:"self", c_r:["branchManager"]}`)
// "çapa biriminin branchManager'ı" demektir. Çapa `origin_orgu_id`den geldiği
// için bu, "işin ait olduğu şubenin müdürü" olur.

/// Çapa WFE'nin birimi: İŞİN şubesindeki müdür bitmiş işi görür.
#[tokio::test]
async fn listable_grant_anchors_at_wfe_origin_unit() {
    let org = MockOrg {
        role_assigned: true,
    };
    let origin = Uuid::new_v4();

    let mut wfes = wfes_at("self__creditAnalyst", None, start_input(30_000));
    wfes.origin_orgu_id = Some(origin);
    wfes.status = WfeStatus::Terminal;
    wfes.current_node = None;

    // Aynı birimin müdürü → görür (listable[0]).
    assert!(can_view(&golden(), &wfes, &manager(origin), &org)
        .await
        .unwrap());
}

/// Aynı kural, BAŞKA birimin müdürü: görmez.
///
/// Bu testin varlık sebebi tam olarak eski bug'dır: çapa SORAN KİŞİ olduğunda
/// `self` birim karşılaştırmasını kendisiyle yapıp daima geçiyordu, yani kural
/// sessizce "tenant'taki her branchManager" anlamına geliyordu. Çapa WFE'ye
/// bağlandığı için artık yalnız işin şubesi geçer — regresyon burada patlar.
#[tokio::test]
async fn listable_grant_does_not_leak_to_other_units() {
    let org = MockOrg {
        role_assigned: true,
    };
    let origin = Uuid::new_v4();
    let elsewhere = Uuid::new_v4();

    let mut wfes = wfes_at("self__creditAnalyst", None, start_input(30_000));
    wfes.origin_orgu_id = Some(origin);
    wfes.status = WfeStatus::Terminal;
    wfes.current_node = None;

    assert!(!can_view(&golden(), &wfes, &manager(elsewhere), &org)
        .await
        .unwrap());
}

/// `origin_orgu_id` NULL (backfill bekleyen eski satır) → çapa aktöre düşer,
/// yani ESKİ davranış korunur. Geçiş sırasında hiçbir akış görünürlük
/// kaybetmesin diye bilinçli olarak böyle.
#[tokio::test]
async fn missing_origin_falls_back_to_actor_anchor() {
    let org = MockOrg {
        role_assigned: true,
    };
    let mut wfes = wfes_at("self__creditAnalyst", None, start_input(30_000));
    wfes.origin_orgu_id = None;
    wfes.status = WfeStatus::Terminal;
    wfes.current_node = None;

    // Çapa yok → aktörün kendi birimi çapa olur → her birimin müdürü geçer.
    assert!(can_view(&golden(), &wfes, &manager(Uuid::new_v4()), &org)
        .await
        .unwrap());
}

/// Bitmiş işte node c_a'sı yetki ÜRETMEZ — aktif node zaten yok, ama kural
/// açıkça sınanır: aynı aktör AKTİF WFE'de görür, TERMINAL'de görmez.
#[tokio::test]
async fn node_c_a_grants_view_only_while_active() {
    let org = MockOrg {
        role_assigned: true,
    };
    let origin = Uuid::new_v4();
    let viewer = analyst(origin);

    let mut active = wfes_at("self__creditAnalyst", None, start_input(30_000));
    active.origin_orgu_id = Some(origin);
    assert!(can_view(&golden(), &active, &viewer, &org).await.unwrap());

    // Terminal: current_node temizlenir, geriye yalnız kalıcı grant'lar kalır.
    // Bu aktör branchManager DEĞİL → listable[0] onu kapsamaz.
    let mut done = active.clone();
    done.status = WfeStatus::Terminal;
    done.current_node = None;
    assert!(!can_view(&golden(), &done, &viewer, &org).await.unwrap());
}

/// Sahiplik de yalnız AKTİF işte yetki üretir: bitmiş işte `claimed_by` artık
/// "iş kimin havuzundaydı" sorusunun cevabıdır, görünürlük kuralı değil.
#[tokio::test]
async fn ownership_grants_view_only_while_active() {
    let org = MockOrg {
        role_assigned: false,
    };
    let owner_id = Uuid::new_v4();
    let owner = Actor {
        orgu_id: Uuid::new_v4(),
        user_id: owner_id,
        role: "branchClerk".into(),
    };
    let mut wfes = wfes_at("self__creditAnalyst", Some(owner_id), start_input(30_000));
    wfes.origin_orgu_id = Some(Uuid::new_v4());
    assert!(can_view(&golden(), &wfes, &owner, &org).await.unwrap());

    wfes.status = WfeStatus::Terminal;
    wfes.current_node = None;
    assert!(!can_view(&golden(), &wfes, &owner, &org).await.unwrap());
}

/// `listable[1]`in `when` guard'ı (`amount_requested >= 100000`) çapadan
/// BAĞIMSIZ olarak işler: guard false ise çapa doğru olsa da grant yok.
#[tokio::test]
async fn listable_when_guard_still_gates_after_anchor_change() {
    let org = MockOrg {
        role_assigned: true,
    };
    let origin = Uuid::new_v4();
    let dept = credit_dept_manager(origin);

    // 30k < 100k → guard false → görmez.
    let mut low = wfes_at("self__creditAnalyst", None, start_input(30_000));
    low.origin_orgu_id = Some(origin);
    low.status = WfeStatus::Terminal;
    low.current_node = None;
    assert!(!can_view(&golden(), &low, &dept, &org).await.unwrap());

    // 150k ≥ 100k → guard true → görür (çapa `parent`, MockOrg anchor'a çözer).
    let mut high = wfes_at("self__creditAnalyst", None, start_input(150_000));
    high.origin_orgu_id = Some(origin);
    high.status = WfeStatus::Terminal;
    high.current_node = None;
    assert!(can_view(&golden(), &high, &dept, &org).await.unwrap());
}

// ============================================ (f) node-seviyesi `listable[]`
//
// node-listable-design.md: kök `listable[]`in DURUMA BAĞLI karşılığı — WFE bu
// node'da İKEN kurallardan birine uyan aktör görür, node'dan çıkınca (b) gibi
// görünürlük de biter. ACT/claim VERMEZ.

/// `self__creditAnalyst` node'una `listable[]` ekler, kök `listable`'ı KAPATIR
/// (yalnız (f) tek başına sınansın) — `golden_with_wf_admin` ile aynı desen.
fn golden_with_node_listable(when: Option<&str>) -> Wfd {
    let mut wfd = golden();
    wfd.listable.clear();
    wfd.nodes.get_mut("self__creditAnalyst").unwrap().listable = vec![CaGrantRule {
        c_a: CandidateActor {
            c_orgu: Some(COrgu::Selector("self".into())),
            c_r: Some(vec!["branchManager".into()]),
            c_u: None,
        },
        when: when.map(String::from),
    }];
    wfd
}

/// (a) WFE `self__creditAnalyst` node'undayken, node `listable[]` kuralına uyan
/// aktör (branchManager, node c_a'sı olan creditAnalyst DEĞİL) WFE'yi görür.
#[tokio::test]
async fn node_listable_actor_can_view_while_wfe_is_at_that_node() {
    let org = MockOrg {
        role_assigned: true,
    };
    let orgu = Uuid::new_v4();
    let m = manager(orgu);
    let mut wfes = wfes_at("self__creditAnalyst", None, start_input(30_000));
    wfes.origin_orgu_id = Some(orgu);

    assert!(can_view(&golden_with_node_listable(None), &wfes, &m, &org)
        .await
        .unwrap());
}

/// (b) Aynı aktör, AYNI WFD, ama WFE başka bir node'a geçmiş: `nodes.<key>.listable`
/// DURUMA BAĞLIDIR — node değişince görünürlük de biter (kök `listable` KALICI
/// olsaydı bu senaryoda da görünür kalırdı; bu test tam o farkı sınar).
/// `parent__creditDeptManager` seçildi çünkü kendi `c_a`'sı da branchManager'ı
/// KAPSAMAZ (creditDeptManager ister) — kriterin (c) değil (f)'in düştüğünü ölçer.
#[tokio::test]
async fn node_listable_actor_cannot_view_once_wfe_leaves_that_node() {
    let org = MockOrg {
        role_assigned: true,
    };
    let orgu = Uuid::new_v4();
    let m = manager(orgu);
    let mut wfes = wfes_at("parent__creditDeptManager", None, start_input(30_000));
    wfes.origin_orgu_id = Some(orgu);

    assert!(!can_view(&golden_with_node_listable(None), &wfes, &m, &org)
        .await
        .unwrap());
}

/// (d) `when` guard'ı false ise — node c_a eşleşse de — görünmez.
#[tokio::test]
async fn node_listable_when_false_cannot_view() {
    let org = MockOrg {
        role_assigned: true,
    };
    let orgu = Uuid::new_v4();
    let m = manager(orgu);
    let gated = golden_with_node_listable(Some(
        "$ctx.credit_info.amount_requested >= 100000",
    ));

    let mut low = wfes_at("self__creditAnalyst", None, start_input(30_000));
    low.origin_orgu_id = Some(orgu);
    assert!(!can_view(&gated, &low, &m, &org).await.unwrap());

    let mut high = wfes_at("self__creditAnalyst", None, start_input(150_000));
    high.origin_orgu_id = Some(orgu);
    assert!(can_view(&gated, &high, &m, &org).await.unwrap());
}

/// `can_claim` autoexec KOŞTURMAZ — runner'a hiç dokunulmaz.
struct UnusedRunner;
#[async_trait]
impl AutoexecRunner for UnusedRunner {
    async fn run(&self, _def: &AutoexecDef, _env: &ExecEnv) -> Result<Value, ExecFailure> {
        unreachable!("can_claim autoexec çalıştırmaz")
    }
}

/// Claim kararının SAF çekirdeği — `WfeExecutor::can_claim_loaded` (dolayısıyla
/// `can_claim`/`claim` uçları VE havuzun `PoolTask.can_claim` alanı) bunu çağırır.
async fn claim_check(
    wfd: &Wfd,
    wfes: &Wfes,
    actor: &Actor,
    branch: Option<&str>,
    org: &MockOrg,
) -> ClaimCheck {
    let runner = UnusedRunner;
    let engine = Engine {
        org,
        exec: &runner,
        env: Default::default(),
    };
    engine.can_claim(wfd, wfes, actor, branch).await.unwrap()
}

/// (c) node `listable[]`e uyan aktör WFE'yi GÖRÜR ama `can_claim` node `c_a`'sını
/// (creditAnalyst) sorar — branchManager o kurala uymadığı için ACT/claim ALAMAZ.
/// node listable "görme"dir, "yapma" değil.
#[tokio::test]
async fn node_listable_actor_can_view_but_cannot_claim() {
    let org = MockOrg {
        role_assigned: true,
    };
    let orgu = Uuid::new_v4();
    let m = manager(orgu);
    let wfd = golden_with_node_listable(None);
    let mut wfes = wfes_at("self__creditAnalyst", None, start_input(30_000));
    wfes.origin_orgu_id = Some(orgu);

    // (f) görme hakkı VAR.
    assert!(can_view(&wfd, &wfes, &m, &org).await.unwrap());

    // ama claim YOK — node c_a'sı creditAnalyst, branchManager ona uymuyor.
    assert_eq!(
        claim_check(&wfd, &wfes, &m, None, &org).await,
        ClaimCheck::NotEligible
    );
}

// ================================ havuz cevabının taşıdığı karar (2026-08-14)
//
// `PoolTask.can_claim` görünürlükten AYRI olan claim kararını taşır; kararı
// ÜRETMEZ, `WfeExecutor::can_claim_loaded` → `Engine::can_claim`'den ödünç alır.
// Bu repoda DB'li test koşmadığı için bekçiler o SAF çekirdeğin üstünde durur:
// havuz alanı yalnız bu cevabın aktarımıdır.

/// (a) Node `c_a`'sına uyan aktör claim EDEBİLİR → havuzda `can_claim: true`.
#[tokio::test]
async fn pool_claim_decision_true_for_node_c_a_actor() {
    let org = MockOrg {
        role_assigned: true,
    };
    let orgu = Uuid::new_v4();
    // self__creditAnalyst node c_a = {c_orgu: self, c_r: [creditAnalyst]}
    let a = analyst(orgu);
    let mut wfes = wfes_at("self__creditAnalyst", None, start_input(30_000));
    wfes.origin_orgu_id = Some(orgu);

    assert_eq!(
        claim_check(&golden(), &wfes, &a, None, &org).await,
        ClaimCheck::Ok
    );
}

/// (b) KRİTİK İDDİA: satırı YALNIZ `listable` / `wf_admin` üzerinden gören aktör
/// claim EDEMEZ. Havuz görünürlüğü tek predicate'e bağlanınca kapsam tam bu iki
/// kanalla genişledi; alan bu yüzden var — kullanıcı görebildiği ama
/// alamayacağı satırı ayırt edebilsin, düğmeye basıp 403 yemesin.
#[tokio::test]
async fn pool_claim_decision_false_for_listable_and_wf_admin_only_viewers() {
    let org = MockOrg {
        role_assigned: true,
    };
    let orgu = Uuid::new_v4();
    // Kök `listable[0]` = branchManager (guard'sız), `wf_admin` = creditDeptManager.
    let mut wfd = golden();
    wfd.wf_admin = vec![CaGrantRule {
        c_a: CandidateActor {
            c_orgu: Some(COrgu::Selector("self".into())),
            c_r: Some(vec!["creditDeptManager".into()]),
            c_u: None,
        },
        when: None,
    }];
    let mut wfes = wfes_at("self__creditAnalyst", None, start_input(30_000));
    wfes.origin_orgu_id = Some(orgu);

    let m = manager(orgu); // kök listable
    let admin = credit_dept_manager(orgu); // wf_admin (listable[1]'in when'i 30k'da false)

    // İkisi de havuzda satır ÜRETİR (görünürlük).
    assert!(can_view(&wfd, &wfes, &m, &org).await.unwrap());
    assert!(can_view(&wfd, &wfes, &admin, &org).await.unwrap());

    // Ama HİÇBİRİ claim edemez: node `c_a`'sı creditAnalyst.
    assert_eq!(
        claim_check(&wfd, &wfes, &m, None, &org).await,
        ClaimCheck::NotEligible,
        "kök listable görünürlük verir, claim VERMEZ"
    );
    assert_eq!(
        claim_check(&wfd, &wfes, &admin, None, &org).await,
        ClaimCheck::NotEligible,
        "wf_admin görünürlük verir, claim VERMEZ"
    );
}

/// (c) Paralel kol satırında karar KOLUN node'una göre verilir — WFE seviyesine
/// göre DEĞİL. Havuz kol satırı için hedef olarak kolun node'unu geçirir.
#[tokio::test]
async fn pool_claim_decision_is_per_branch_node() {
    let org = MockOrg {
        role_assigned: true,
    };
    let orgu = Uuid::new_v4();
    let fin = role_actor(orgu, "financeApprover");
    let mut wfes = parallel_wfes(vec![
        branch("self__financeApprover", BranchStatus::Active, None),
        branch("self__legalApprover", BranchStatus::Active, None),
    ]);
    wfes.origin_orgu_id = Some(orgu);
    let wfd = paralel();

    // Kendi kolunda uygun…
    assert_eq!(
        claim_check(&wfd, &wfes, &fin, Some("self__financeApprover"), &org).await,
        ClaimCheck::Ok
    );
    // …kardeş kolda DEĞİL. WFE'yi görüyor olması (kol c_a kanalı) o kolu
    // alabildiği anlamına gelmez.
    assert!(can_view(&wfd, &wfes, &fin, &org).await.unwrap());
    assert_eq!(
        claim_check(&wfd, &wfes, &fin, Some("self__legalApprover"), &org).await,
        ClaimCheck::NotEligible
    );
    // Kol ipucu olmadan paralel modda karar verilemez (current_node NULL).
    assert_eq!(
        claim_check(&wfd, &wfes, &fin, None, &org).await,
        ClaimCheck::NotEligible
    );
}

/// Zaten claim EDİLMİŞ satır: claim BAŞKASINDAysa uygunluk yok
/// (`AlreadyClaimed` → havuzda `can_claim: false`). Kol claim'i KOL-bazlı
/// okunur: kardeş kol hâlâ boşsa o kol claim edilebilir kalır.
#[tokio::test]
async fn pool_claim_decision_false_when_another_actor_holds_the_claim() {
    let org = MockOrg {
        role_assigned: true,
    };
    let orgu = Uuid::new_v4();

    // Tek-kol: wfe-seviyesi assignment başkasında.
    let a = analyst(orgu);
    let mut single = wfes_at(
        "self__creditAnalyst",
        Some(Uuid::new_v4()),
        start_input(30_000),
    );
    single.origin_orgu_id = Some(orgu);
    assert_eq!(
        claim_check(&golden(), &single, &a, None, &org).await,
        ClaimCheck::AlreadyClaimed
    );

    // Paralel: kolu başkası tutuyor → o kol AlreadyClaimed, kardeş kol serbest.
    let fin = role_actor(orgu, "financeApprover");
    let leg = role_actor(orgu, "legalApprover");
    let mut par = parallel_wfes(vec![
        branch(
            "self__financeApprover",
            BranchStatus::Active,
            Some(Uuid::new_v4()),
        ),
        branch("self__legalApprover", BranchStatus::Active, None),
    ]);
    par.origin_orgu_id = Some(orgu);
    assert_eq!(
        claim_check(&paralel(), &par, &fin, Some("self__financeApprover"), &org).await,
        ClaimCheck::AlreadyClaimed
    );
    assert_eq!(
        claim_check(&paralel(), &par, &leg, Some("self__legalApprover"), &org).await,
        ClaimCheck::Ok
    );
}
