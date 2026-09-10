//! `P05` ÖLÇÜM KALEMİ (`M4`, WOR-114) — portal timeline'ının **gürültü** ölçümü.
//!
//! `P05`/S2 timeline'da **gizleme YOK** dedi: elenen satır da (`A04`), sahiplik
//! satırı da (`Ç13`/`E12`) görünür kalır; gürültü kontrolü `group` üzerinden bir
//! FİLTREdir ve `ownership` grubu varsayılan KAPALI gelir. `P05`/C — sahiplik
//! geçmişinin filtreli panonun ötesinde AYRI bir tam ekrana çıkması (kuyruk `S15`)
//! — bu ölçüme bağlandı: **gürültü gerçekten sorun mu?**
//!
//! Ölçülen şey `GET /wfe/:id`in `wfah[]` dizisidir; portal timeline'ı satır satır
//! ONU basar (`InstanceDetail.tsx:801`, `buildNoteTimeline` notları aynı listeye
//! harmanlar — notlar bu ölçümün DIŞINDA, gürültü sorusu motorun yazdığı satırlarla
//! ilgili). Sayım `WfahView.group`tan okunur: sınıflandırma `P04` ile motorda, yani
//! araç ikinci bir gruplama tanımı YAZMAZ.
//!
//! `M1` (WOR-111) / `M2` (WOR-112) ile aynı aile: DB YOK, araç KARAR VERMEZ. Fark,
//! ölçülen birimin süre değil **SATIR SAYISI** olmasıdır — `S15` sorusu bir maliyet
//! sorusu değil, bir okunabilirlik sorusudur.
//!
//! # E12 öncesi/sonrası
//!
//! `E12` (commit `aa800cc`) İNDİ; "önce"si artık koşturulamaz, ama satır satır
//! TÜRETİLEBİLİR — eski defterde her sahiplik olayının karşılığı bilinir:
//!
//! | bugünkü satır | `E12` öncesi karşılığı | delta |
//! |---|---|---|
//! | `claim_taken` `via=self` | **YOKTU** (`executor.rs` `_ => None`) | **+1** |
//! | `claim_taken` `via=delegated` | `claim:delegated` | 0 |
//! | `claim_released` + `claim_taken` çifti (kişiden kişiye devir) | tek `reassign` | **+1** |
//! | `claim_taken` `via=assigned` (havuzdan atama) | tek `reassign` | 0 |
//! | `claim_released` `reason=self`/`admin` (havuza bırakma) | tek `unclaim` | 0 |
//! | `claim_released` `reason=timeout` | `claim_timeout:<node>` | 0 |
//! | `claim_released` `reason=grant_guard_false` | — (`Ç9`/`E02` kalemi, `E12` dışı) | 0 |
//!
//! Yani `E12`nin defter büyümesi İKİ yerden gelir: **doğrudan alma** ve **kişiden
//! kişiye devrin ikinci satırı**. Araç bu iki sayıyı ayrı basar.
//!
//! Koşum:
//!
//! ```text
//! cargo bench -p wf-wfe --bench timeline_noise
//! ```
//!
//! Çıktı markdown tablodur; sayılar issue'ya böyle yazılır.

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{Duration as ChronoDuration, Utc};
use serde_json::{json, Value};
use uuid::Uuid;
use wf_wfe::executor::{Ref, WfahView};
use wf_wfe::WfeExecutor;
use wfe_core::types::actor::Actor;
use wfe_core::types::wfd_v22::Wfd;
use wfe_core::v22::wfah_kind::{WfahGroup, WfahKind};
use wfe_core::v22::wfah_payload::WfahPayload;

#[path = "../tests/common/mod.rs"]
mod common;

use common::{
    actor, branch_approve, claim_all_branches, claim_owner, collapse_executor, executor,
    fork_setup, FixtureWfdStore, MockOrg, MockRunner, ParStore,
};

const GOLDEN: &str = include_str!("../../../docs/spec/examples/kredi-basvuru.golden.json");

/// Golden'ın analist havuzu — ölçümün sahiplik çalkantısı burada yaşanır.
const ANALYST_NODE: &str = "self__creditAnalyst";

// ---------------------------------------------------------------- fixture varyantları

/// Golden'a `reassign` yetkisi + `claim_timeout` ekler.
///
/// İkisi de gerçek bir kurulumun taşıdığı ama golden'ın taşımadığı alanlardır:
/// golden bir ŞEMA örneğidir, üretim akışı değil. Sahiplik gürültüsü tam olarak bu
/// iki alandan doğduğu için ölçüm onlarsız kendi sorusunu soramaz.
fn golden_with_ownership() -> Wfd {
    let mut v: Value = serde_json::from_str(GOLDEN).unwrap();
    v["nodes"][ANALYST_NODE]["reassign"] =
        json!({"c_orgu": "self", "c_r": ["creditAnalyst", "branchManager"]});
    v["nodes"][ANALYST_NODE]["claim_timeout"] = json!({"after": "PT4H"});
    // Müdür akış yöneticisidir: eskalasyonu elle ateşleyebilir (`T-A5`/A-1 gereği
    // yetki LİSTELİDİR — örtük değil).
    v["wf_admin"] = json!([{
        "c_a": {"c_orgu": "self", "c_r": ["branchManager"]},
        "allowed_global_actions": ["fire_escalation", "reassign", "reclaim_to_pool"]
    }]);
    Wfd::from_value(v).unwrap()
}

/// Aynı şubenin insanları — `MockOrg` çapayı olduğu gibi çözdüğü için görünürlük
/// projeksiyonu ancak aktörler AYNI birimdeyken eşleşir (gerçek kurulumda da bir
/// kredi başvurusunu aynı şubenin memuru/analisti/müdürü taşır).
fn person(orgu: Uuid, role: &str) -> Actor {
    Actor {
        orgu_id: orgu,
        user_id: Uuid::new_v4(),
        role: role.into(),
    }
}

fn golden_executor(store: Arc<ParStore>, wfd: Wfd) -> WfeExecutor {
    WfeExecutor::new(
        Arc::new(MockOrg),
        Arc::new(FixtureWfdStore(wfd)),
        store,
        Arc::new(MockRunner),
    )
}

// ---------------------------------------------------------------- sayım

#[derive(Default)]
struct Counts {
    total: usize,
    by_group: BTreeMap<&'static str, usize>,
    by_kind: BTreeMap<&'static str, usize>,
    /// Sahiplik satırlarının kırılımı: `taken:self`, `released:timeout` …
    ownership_detail: BTreeMap<String, usize>,
    /// `A04`: `$valid`den elenen satırlar, sebep başına.
    invalid: BTreeMap<String, usize>,
    /// `E12`nin defterе EKLEDİĞİ satır: doğrudan alma.
    e12_direct_claims: usize,
    /// `E12`nin defterе EKLEDİĞİ satır: kişiden kişiye devrin İKİNCİ satırı.
    e12_reassign_pairs: usize,
}

fn group_name(g: WfahGroup) -> &'static str {
    match g {
        WfahGroup::Sla => "sla",
        WfahGroup::Ownership => "ownership",
        WfahGroup::Parallel => "parallel",
        WfahGroup::CollapseGroup => "collapse",
        WfahGroup::Call => "call",
        WfahGroup::None_ => "none",
    }
}

fn kind_name(k: WfahKind) -> &'static str {
    match k {
        WfahKind::Action => "action",
        WfahKind::Deadline => "deadline",
        WfahKind::Escalation => "escalation",
        WfahKind::EscalationSkipped => "escalation_skipped",
        WfahKind::ClaimTaken => "claim_taken",
        WfahKind::ClaimReleased => "claim_released",
        WfahKind::Trigger => "trigger",
        WfahKind::CallReturn => "call_return",
        WfahKind::CallTruncated => "call_truncated",
        WfahKind::Fork => "fork",
        WfahKind::BranchArrived => "branch_arrived",
        WfahKind::Join => "join",
        WfahKind::Collapse => "collapse",
        WfahKind::BranchCancelled => "branch_cancelled",
        WfahKind::BranchSuperseded => "branch_superseded",
    }
}

fn payload_str(p: &WfahPayload<Ref>, field: &str) -> Option<String> {
    let v: &Value = match p {
        WfahPayload::ClaimTaken(v) | WfahPayload::ClaimReleased(v) => v,
        _ => return None,
    };
    v.get(field)?.as_str().map(str::to_string)
}

fn count(rows: &[WfahView]) -> Counts {
    let mut c = Counts {
        total: rows.len(),
        ..Default::default()
    };
    for (i, r) in rows.iter().enumerate() {
        *c.by_group.entry(group_name(r.group)).or_default() += 1;
        *c.by_kind.entry(kind_name(r.kind)).or_default() += 1;
        if let Some(reason) = r.invalid_reason {
            let key = serde_json::to_value(reason)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| "?".into());
            *c.invalid.entry(key).or_default() += 1;
        }
        match r.kind {
            WfahKind::ClaimTaken => {
                let via = payload_str(&r.detail, "via").unwrap_or_else(|| "?".into());
                *c.ownership_detail
                    .entry(format!("claim_taken via={via}"))
                    .or_default() += 1;
                // E12 deltası: doğrudan alma eskiden HİÇ satır yazmıyordu; yetkili
                // devirde ise ikinci satır yeni. Çifti, kendinden ÖNCEKİ satırın
                // bırakma olmasından tanırız (aynı transaction, ardışık `seq`).
                if via == "self" {
                    c.e12_direct_claims += 1;
                } else if via == "assigned" || via == "admin_assigned" {
                    let paired = i
                        .checked_sub(1)
                        .map(|p| rows[p].kind == WfahKind::ClaimReleased)
                        .unwrap_or(false);
                    if paired {
                        c.e12_reassign_pairs += 1;
                    }
                }
            }
            WfahKind::ClaimReleased => {
                let reason = payload_str(&r.detail, "reason").unwrap_or_else(|| "?".into());
                *c.ownership_detail
                    .entry(format!("claim_released reason={reason}"))
                    .or_default() += 1;
            }
            _ => {}
        }
    }
    c
}

impl Counts {
    fn g(&self, k: &str) -> usize {
        self.by_group.get(k).copied().unwrap_or(0)
    }
    /// `P05`/S2 varsayılanı: `ownership` filtresi KAPALI.
    fn visible_default(&self) -> usize {
        self.total - self.g("ownership")
    }
    fn e12_added(&self) -> usize {
        self.e12_direct_claims + self.e12_reassign_pairs
    }
    /// `E12` inmeden önceki satır sayısı.
    fn before_e12(&self) -> usize {
        self.total - self.e12_added()
    }
}

// ---------------------------------------------------------------- profiller

/// Ölçülen bir WFE: ad + defterin görünüm hâli.
struct Profile {
    name: &'static str,
    note: &'static str,
    counts: Counts,
}

async fn view_rows(exec: &WfeExecutor, wfe_id: Uuid, viewer: &Actor) -> Vec<WfahView> {
    // Görünürlük kapısı burada da GERÇEKTİR: ölçüm, kullanıcının GÖREBİLDİĞİ
    // defteri sayar — `query` yetkisiz okuyucuya satır vermez.
    match exec.query(wfe_id, viewer).await {
        Ok(v) => v.wfah,
        Err(e) => panic!("`{}` rolündeki okuyucu defteri göremedi: {e:?}", viewer.role),
    }
}

/// P1 — tek-kol mutlu yol: başlat → al → onayla → al → karar ver.
async fn p1_tek_kol_mutlu() -> Counts {
    let store = Arc::new(ParStore::default());
    let exec = golden_executor(store.clone(), Wfd::from_json(GOLDEN).unwrap());
    let branch = Uuid::new_v4();
    let clerk = person(branch, "branchClerk");
    let started = exec
        .start(
            Uuid::new_v4(),
            1,
            &clerk,
            Some("create_application"),
            &json!({
                "applicant": {"name": "Ayşe Yılmaz"},
                "credit_info": {"amount_requested": 250000}
            }),
            None,
        )
        .await
        .unwrap();
    let wfe_id = started.wfe_id;
    store.set_origin_orgu(wfe_id, branch);

    let analyst = person(branch, "creditAnalyst");
    assert!(exec.claim(wfe_id, &analyst, None, None).await.unwrap().success);
    exec.apply(
        wfe_id,
        &analyst,
        "analyst_approve",
        &json!({"credit_info": {"amount_requested": 250000}}),
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let manager = person(branch, "branchManager");
    if exec.claim(wfe_id, &manager, None, None).await.unwrap().success {
        let _ = exec
            .apply(wfe_id, &manager, "manager_decide", &json!({"decision": "approve"}), None, None, None)
            .await;
    }
    count(&view_rows(&exec, wfe_id, &manager).await)
}

/// P2 — tek-kol, SAHİPLİK ÇALKANTILI: al → havuza bırak → başkası al → kişiden
/// kişiye devir → üstlenme süresi dolar → eskalasyon → onayla.
///
/// Gerçek hayatta bir işin başına gelen sahiplik olaylarının hepsi tek WFE'de.
/// Bilinçli olarak KÖTÜ hâl: gürültü sorusunun üst sınırı budur.
async fn p2_tek_kol_calkantili() -> Counts {
    let store = Arc::new(ParStore::default());
    let exec = golden_executor(store.clone(), golden_with_ownership());
    let branch = Uuid::new_v4();
    let clerk = person(branch, "branchClerk");
    let started = exec
        .start(
            Uuid::new_v4(),
            1,
            &clerk,
            Some("create_application"),
            &json!({
                "applicant": {"name": "Ayşe Yılmaz"},
                "credit_info": {"amount_requested": 250000}
            }),
            None,
        )
        .await
        .unwrap();
    let wfe_id = started.wfe_id;
    store.set_origin_orgu(wfe_id, branch);
    let manager = person(branch, "branchManager");

    // 1) analist A alır, sonra kendi havuza bırakır → taken(self) + released(self)
    let a = person(branch, "creditAnalyst");
    assert!(exec.claim(wfe_id, &a, None, None).await.unwrap().success);
    exec.reassign(wfe_id, &a, None, None).await.unwrap();

    // 2) analist B alır → taken(self)
    let b = person(branch, "creditAnalyst");
    assert!(exec.claim(wfe_id, &b, None, None).await.unwrap().success);

    // 3) müdür işi C'ye devreder → released(taken_by_other) + taken(assigned)
    let c = person(branch, "creditAnalyst");
    exec.reassign(wfe_id, &manager, Some(&c), None).await.unwrap();

    // 4) C'nin üstlenme süresi dolar (SLA-1) → released(timeout)
    {
        let mut w = store.snapshot(wfe_id);
        w.claimed_at = Some(Utc::now() - ChronoDuration::hours(9));
        store.seed(w);
    }
    exec.tick_timers(wfe_id).await.unwrap();

    // 5) eskalasyon kademesi ateşlenir (SLA-2) → escalation
    exec.fire_escalation_now(wfe_id, &manager, None).await.unwrap();

    // 6) D alır ve onaylar → taken(self) + action
    let d = person(branch, "creditAnalyst");
    assert!(exec.claim(wfe_id, &d, None, None).await.unwrap().success);
    exec.apply(
        wfe_id,
        &d,
        "analyst_approve",
        &json!({"credit_info": {"amount_requested": 250000}}),
        None,
        None,
        None,
    )
    .await
    .unwrap();

    // 7) müdür alır ve karar verir
    if exec.claim(wfe_id, &manager, None, None).await.unwrap().success {
        let _ = exec
            .apply(wfe_id, &manager, "manager_decide", &json!({"decision": "approve"}), None, None, None)
            .await;
    }
    count(&view_rows(&exec, wfe_id, &manager).await)
}

/// P3 — paralel mutlu yol: fork → üç kol claim → üç onay → join.
async fn p3_paralel_mutlu() -> Counts {
    let store = Arc::new(ParStore::default());
    let exec = executor(store.clone());
    let wfe_id = fork_setup(&exec).await;
    let owners = claim_all_branches(&exec, &store, wfe_id).await;
    for (owner, node) in owners.iter().zip([
        "self__financeApprover",
        "self__legalApprover",
        "self__hrApprover",
    ]) {
        exec.apply(wfe_id, owner, branch_approve(node), &json!({}), Some(node), None, None)
            .await
            .unwrap();
    }
    // Join sonrası akış `self__resultCoordinator`a geçti; defteri o havuz okur.
    count(&view_rows(&exec, wfe_id, &actor("resultCoordinator")).await)
}

/// P4 — paralel + COLLAPSE: bir kol reddeder, kalan kollar iptal olur.
/// `A04`ün elediği satırlar (iptal edilen kolda kalan onaylar) burada doğar.
async fn p4_paralel_collapse() -> Counts {
    let store = Arc::new(ParStore::default());
    let exec = collapse_executor(store.clone());
    let wfe_id = fork_setup(&exec).await;
    let owners = claim_all_branches(&exec, &store, wfe_id).await;

    // finans onaylar, hukuk REDDEDER → collapse; İK kolu iptal olur ve finansın
    // onayı `branch_cancelled` sebebiyle `$valid`den elenir.
    exec.apply(
        wfe_id,
        &owners[0],
        branch_approve("self__financeApprover"),
        &json!({}),
        Some("self__financeApprover"),
        None,
        None,
    )
    .await
    .unwrap();
    let legal = claim_owner(&store, wfe_id, "self__legalApprover");
    exec.apply(wfe_id, &legal, "hukuk_ret", &json!({}), Some("self__legalApprover"), None, None)
        .await
        .unwrap();
    // Collapse sonrası akış `self__coordinator`a döndü.
    count(&view_rows(&exec, wfe_id, &actor("coordinator")).await)
}

/// P5(n) — ÖLÇEK: aynı işin `n` kez alınıp havuza bırakılması.
///
/// `S15`in asıl sorusu "bugün kaç satır" değil, **büyüme hızı**dır: sahiplik
/// satırları iş uzadıkça kaç tane olur. Her al-bırak turu TAM İKİ satır yazar
/// (`claim_taken` + `claim_released`), yani büyüme `n`'de DOĞRUSALdır ve tur
/// başına sabittir — bu profil o sabiti ölçer, varsaymaz.
async fn p5_calkanti_olcegi(n: usize) -> Counts {
    let store = Arc::new(ParStore::default());
    let exec = golden_executor(store.clone(), golden_with_ownership());
    let branch = Uuid::new_v4();
    let clerk = person(branch, "branchClerk");
    let started = exec
        .start(
            Uuid::new_v4(),
            1,
            &clerk,
            Some("create_application"),
            &json!({
                "applicant": {"name": "Ayşe Yılmaz"},
                "credit_info": {"amount_requested": 250000}
            }),
            None,
        )
        .await
        .unwrap();
    let wfe_id = started.wfe_id;
    store.set_origin_orgu(wfe_id, branch);

    for _ in 0..n {
        let a = person(branch, "creditAnalyst");
        assert!(exec.claim(wfe_id, &a, None, None).await.unwrap().success);
        exec.reassign(wfe_id, &a, None, None).await.unwrap();
    }
    let last = person(branch, "creditAnalyst");
    assert!(exec.claim(wfe_id, &last, None, None).await.unwrap().success);
    exec.apply(
        wfe_id,
        &last,
        "analyst_approve",
        &json!({"credit_info": {"amount_requested": 250000}}),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let manager = person(branch, "branchManager");
    count(&view_rows(&exec, wfe_id, &manager).await)
}

// ---------------------------------------------------------------- rapor

fn main() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let profiles = rt.block_on(async {
        vec![
            Profile {
                name: "P1 tek-kol mutlu yol",
                note: "başlat → al → onayla → al → karar",
                counts: p1_tek_kol_mutlu().await,
            },
            Profile {
                name: "P2 tek-kol sahiplik çalkantılı",
                note: "bırak/al/devir/timeout/eskalasyon — kötü hâl",
                counts: p2_tek_kol_calkantili().await,
            },
            Profile {
                name: "P3 paralel mutlu yol",
                note: "fork → 3 kol claim → 3 onay → join",
                counts: p3_paralel_mutlu().await,
            },
            Profile {
                name: "P4 paralel + collapse",
                note: "bir kol reddeder, kalanlar iptal → elenen satırlar",
                counts: p4_paralel_collapse().await,
            },
        ]
    });

    println!("\n# M4 — timeline gürültü ölçümü (WOR-114, P05/S2)\n");
    println!("Ölçülen: `GET /wfe/:id` → `wfah[]`. Gruplama motorun `WfahView.group`u (`P04`).\n");

    println!("## 1. Satır sayısı — grup kırılımı\n");
    println!("| profil | toplam | none (aksiyon+trigger) | sla | ownership | parallel | collapse | call |");
    println!("|---|--:|--:|--:|--:|--:|--:|--:|");
    for p in &profiles {
        let c = &p.counts;
        println!(
            "| {} | **{}** | {} | {} | **{}** | {} | {} | {} |",
            p.name,
            c.total,
            c.g("none"),
            c.g("sla"),
            c.g("ownership"),
            c.g("parallel"),
            c.g("collapse"),
            c.g("call"),
        );
    }

    println!("\n## 2. `ownership` grubunun payı ve varsayılan filtre\n");
    println!("`P05`/S2: `ownership` varsayılan KAPALI — \"görünen\" sütunu kullanıcının ekranda ilk gördüğü satır sayısıdır.\n");
    println!("| profil | toplam | ownership | pay | varsayılanda görünen |");
    println!("|---|--:|--:|--:|--:|");
    for p in &profiles {
        let c = &p.counts;
        let pct = if c.total == 0 { 0.0 } else { 100.0 * c.g("ownership") as f64 / c.total as f64 };
        println!(
            "| {} | {} | {} | %{:.0} | **{}** |",
            p.name,
            c.total,
            c.g("ownership"),
            pct,
            c.visible_default()
        );
    }

    println!("\n## 3. `E12` öncesi / sonrası\n");
    println!("Delta türetilir (modül başlığındaki tablo): `E12` YALNIZ iki yerde satır EKLER — doğrudan alma ve kişiden kişiye devrin ikinci satırı.\n");
    println!("| profil | E12 öncesi | bugün | E12 farkı | doğrudan alma | devir çifti |");
    println!("|---|--:|--:|--:|--:|--:|");
    for p in &profiles {
        let c = &p.counts;
        println!(
            "| {} | {} | {} | **+{}** | {} | {} |",
            p.name,
            c.before_e12(),
            c.total,
            c.e12_added(),
            c.e12_direct_claims,
            c.e12_reassign_pairs,
        );
    }

    println!("\n## 4. Sahiplik satırlarının kırılımı\n");
    for p in &profiles {
        if p.counts.ownership_detail.is_empty() {
            continue;
        }
        println!("**{}**", p.name);
        for (k, v) in &p.counts.ownership_detail {
            println!("- `{k}` × {v}");
        }
        println!();
    }

    println!("## 5. `A04` — `$valid`den elenen satırlar\n");
    println!("`ownership` elemesi YAPISALDIR (sahiplik olayı aksiyon değildir, `Ç13`) — her sahiplik satırı elenir ve `A04` rozeti taşır. \"gerçek\" eleme, bir AKSİYONUN sayıma girmemesidir: onu ayrı sütun sayar.\n");
    println!("| profil | elenen (toplam) | bunun sahiplik olanı | gerçek eleme | sebep kırılımı |");
    println!("|---|--:|--:|--:|---|");
    for p in &profiles {
        let c = &p.counts;
        let total: usize = c.invalid.values().sum();
        let own = c.invalid.get("ownership").copied().unwrap_or(0);
        let detail = if c.invalid.is_empty() {
            "—".to_string()
        } else {
            c.invalid
                .iter()
                .map(|(k, v)| format!("`{k}` × {v}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        println!("| {} | {} | {} | **{}** | {} |", p.name, total, own, total - own, detail);
    }

    let scale: Vec<(usize, Counts)> = rt.block_on(async {
        let mut out = Vec::new();
        for n in [1usize, 5, 20, 50] {
            out.push((n, p5_calkanti_olcegi(n).await));
        }
        out
    });
    println!("\n## 6. Ölçek — al/bırak turu başına büyüme\n");
    println!("Aynı iş, `n` kez alınıp havuza bırakılıyor; sonunda onaylanıyor.\n");
    println!("| al/bırak turu | toplam satır | ownership | varsayılanda görünen |");
    println!("|--:|--:|--:|--:|");
    for (n, c) in &scale {
        println!("| {} | {} | **{}** | {} |", n, c.total, c.g("ownership"), c.visible_default());
    }
    if let (Some((n0, c0)), Some((n1, c1))) = (scale.first(), scale.last()) {
        let dn = n1 - n0;
        let d_own = c1.g("ownership") - c0.g("ownership");
        let d_vis = c1.visible_default() - c0.visible_default();
        println!(
            "\nTur başına: **{:.1}** sahiplik satırı · varsayılan görünümde **{:.1}** satır.",
            d_own as f64 / dn as f64,
            d_vis as f64 / dn as f64
        );
    }

    println!("\n## 7. Satır sınıfı kırılımı (`kind`)\n");
    for p in &profiles {
        let detail = p
            .counts
            .by_kind
            .iter()
            .map(|(k, v)| format!("`{k}`×{v}"))
            .collect::<Vec<_>>()
            .join(" · ");
        println!("- **{}** ({}): {}", p.name, p.note, detail);
    }
    println!();
}
