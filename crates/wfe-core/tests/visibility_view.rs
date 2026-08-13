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
use wfe_core::types::wfd_v22::{CaGrantRule, COrgu, CandidateActor, JoinRule};
use wfe_core::types::wfd_v22::Wfd;
use wfe_core::types::wfd_v22::WftTarget;
use wfe_core::types::wfe::WfeStatus;
use wfe_core::v22::ports::{BranchState, BranchStatus, Wfes};
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
