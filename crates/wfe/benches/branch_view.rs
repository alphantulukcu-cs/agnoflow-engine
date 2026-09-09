//! `E03` ÖLÇÜM KALEMİ (`M2`, WOR-112) — kol görünümü okuma + `reproject` maliyeti.
//!
//! `E02`/`E03` grant'ın yazma yolunu (`CommitOutcome::StayAt`) ve projeksiyonunu
//! bağladı. Grant yetki kümesini genişlettiği için `view_c_a` / `current_c_a` /
//! kol `c_a` kolonları BÜYÜR; iki yol ölçülmemişti:
//!
//!   1. **Kol görünümü okuma** — `repo::branch::load_all` → `BranchView`
//!      (`WfeExecutor::query`in kol döngüsü). Kol BAŞINA `node_candidates` +
//!      `claim_decision` koşar; ikincisi `M1`in ölçtüğü grant guard'ını koşar, yani
//!      guard maliyeti KOL SAYISI ile çarpılır.
//!   2. **`reproject`** — `wf_wfe::reproject::reproject_wfe`in motor ucu:
//!      `view_grants` + `node_candidates` + `node_view_grants` (+ terminal) +
//!      kol başına iki çağrı, ardından kolonların jsonb serileşmesi.
//!
//! Ayrıca **grant boyutu**: kolonların aday sayısı ve bayt büyüklüğü G (açık grant)
//! ve birim fan-out'u ile nasıl büyüyor — `visibility_report`un yeni
//! "grant boyutu" bölümünün ölçtüğü şeyin sentetik karşılığı.
//!
//! `M1` (WOR-111, `wfe-core/benches/grant_guard.rs`) ile AYNI aile: aynı
//! `matches_grant_rules` gövdesi ödenir. Bu araç onun ölçtüğü birim maliyeti KOL ve
//! REPROJECT çarpanlarıyla çarpar.
//!
//! DB burada YOK — ölçülen şey de DB değil. Kol okuma yolunda satırlar tek
//! `load_all` sorgusuyla gelir (kol sayısıyla sorgu sayısı büyümez); `reproject`
//! ise WFE başına 1 + kol sayısı kadar `UPDATE` atar ve o kısım ölçümün DIŞINDADIR
//! (tabloda hangi kısmın dışarıda kaldığı yazar).
//!
//! Koşum:
//!
//! ```text
//! cargo bench -p wf-wfe --bench branch_view
//! ```
//!
//! Çıktı markdown tablodur; sayılar issue'ya böyle yazılır. **Bu araç karar VERMEZ**
//! (WOR-112 kabul kriteri): önbellek/ortam kararı `S34` ailesinin işidir.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde_json::{json, Value};
use uuid::Uuid;
use wf_wfe::executor::{BranchView, ClaimProvenance};
use wfe_core::error::EngineError;
use wfe_core::ports::OrgPort;
use wfe_core::types::actor::{Actor, CandidateActor, OrgUnit};
use wfe_core::types::dynctx::DynCtx;
use wfe_core::types::wfah::{Wfah, WfahEntry};
use wfe_core::types::wfd_v22::{AutoexecDef, JoinRule, Wfd};
use wfe_core::types::wfe::WfeStatus;
use wfe_core::v22::matcher::AuthDecision;
use wfe_core::v22::pipeline::Engine;
use wfe_core::v22::ports::{AutoexecRunner, BranchState, BranchStatus, ExecEnv, ExecFailure, Wfes};

const FIXTURE: &str = include_str!("../../../docs/spec/examples/kredi-basvuru.golden.json");

/// Ölçülen node. Golden belgede escalation TAŞIYAN havuz node'u; kol satırları da
/// bu node'da bekletilir (kol başına maliyet node'un kural sayısına bağlıdır,
/// hangi node olduğuna değil).
const NODE: &str = "self__creditAnalyst";

/// Node'un kendi havuzu (`c_a`) — okuyucu aktör BUNA UYMAZ, yani ölçüm grant
/// yolunu koşar.
const NODE_ROLE: &str = "creditAnalyst";

/// Grant'ların dağıttığı rol — ölçüm aktörü BUNU taşır, yani her grant'ın `c_a`'sı
/// eşleşir ve `when` guard'ı GERÇEKTEN değerlendirilir. Guard `false` döner: ölçülen
/// ilk-eşleşende-çık DEĞİL, TÜM açık grantların değerlendirildiği EN KÖTÜ hâl.
const GRANT_ROLE: &str = "branchManager";

/// Defteri HİÇ okumayan, DAİMA `false` guard — zaman ölçümünün guard'ı (`M1`in
/// bulgusu: ifadenin kendisi önemsiz, ödenen şey kural başına kurulan ortam).
const GUARD_FALSE: &str = "$ctx.credit_info.amount_requested >= 999999999";

/// DAİMA `true` guard — BOYUT ölçümünün guard'ı: grant'ın adayları kolona GİRSİN.
const GUARD_TRUE: &str = "$ctx.credit_info.amount_requested >= 1";

/// |WFAH| — defter uzunluğu.
const WFAH_LENS: &[usize] = &[5, 25, 100, 500];

/// Açık grant sayısı (ateşlenmiş escalation kademesi).
const GRANT_COUNTS: &[usize] = &[0, 1, 3, 10];

/// Aktif kol sayısı. Fork'un ürettiği kol sayısı belgedeki `wft` hedef sayısıdır;
/// `wf.wfe_branch` üzerinde ÜST SINIR YOK.
const BRANCH_COUNTS: &[usize] = &[1, 3, 10];

/// Node `listable[]` kural sayısı — `reproject`in görünürlük ucunun
/// (`node_view_grants`) çarpanı. Escalation grant'ı bu ekseni BÜYÜTMEZ (`E03`:
/// grant'ın `listable`a etkisi guard'ı yeniden koşturmaktır, kural SAYISI belgeden
/// gelir).
const NODE_LISTABLE_COUNTS: &[usize] = &[0, 2];

/// `c_orgu` selector'ının çözüldüğü birim sayısı — BOYUT tablosunun fan-out'u.
const ORG_FANOUT: &[usize] = &[1, 5, 25];

/// Hücre başına örnek sayısı (p50/p95 için).
const SAMPLES: usize = 100;
const WARMUP: usize = 10;

fn main() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    rt.block_on(run());
}

async fn run() {
    println!("# E03 — kol görünümü okuma + `reproject` maliyeti (WOR-112 / M2)");
    println!();
    println!(
        "Örnek/hücre: {SAMPLES} (+{WARMUP} ısınma). Okuyucu aktör node `c_a`'sına \
         UYMAZ ve hiçbir grant onu yetkilendirmez → her açık grant değerlendirilir: \
         EN KÖTÜ hâl. `G` = açık grant, `W` = defter satırı, `B` = aktif kol, \
         `L` = node `listable[]` kural sayısı."
    );
    println!();
    println!(
        "Saat payı (`Instant::now()` çifti) bu makinede **{:.2} µs** — mikrosaniyenin \
         altındaki gövdeler bu yüzden tekrarlı ölçülüp bölünür (bkz. ölçüm \
         yardımcıları).",
        clock_overhead_us()
    );

    branch_read().await;
    branch_read_per_request().await;
    reproject_cost().await;
    reproject_breakdown().await;
    grant_size().await;

    println!();
    println!(
        "> Karar VERİLMEDİ. Bu tablolar `S34` ailesinin (ortam/önbellek) girdisidir; \
         `M2` yalnız ölçer."
    );
}

// ── 1. Kol görünümü okuma ────────────────────────────────────────────────────

/// `WfeExecutor::query`in kol döngüsü — KOL BAŞINA gövde, üç parçası ayrı ayrı.
///
/// Döngü gövdesi (`executor.rs`): aktif kol için `node_candidates` (havuz =
/// `node.c_a ∪ açılmış grantlar`) + `claim_decision` (viewer bu kolu claim edebilir mi)
/// + `BranchView::new` (node anahtarlarını `Ref`e çevirir).
///
/// ⚠️ Guard'ı yalnız `claim_decision` koşar (`authorize_node_decision` →
/// `matches_grant_rules`). `node_candidates` guard'ları `E03` gereği YOK SAYAR (kolon
/// over-inclusive önbellektir), `BranchView::new` de yalnız etiket araması yapar.
/// Üçünü ayrı ayrı basmanın sebebi bu: "kol okuma pahalandı" cümlesinin öznesi hangi
/// çağrıdır sorusunun cevabı tabloda durur.
async fn branch_read() {
    println!();
    println!("## 1. Kol görünümü okuma — KOL BAŞINA");
    println!();
    println!(
        "Ölçülen gövde `WfeExecutor::query`in kol döngüsü: `node_candidates` + \
         `claim_decision` + `BranchView::new`. `repo::branch::load_all` TEK sorgudur \
         (kol sayısıyla büyümez) → ölçümün dışında."
    );
    println!();
    println!(
        "| G | W | `node_candidates` (µs) | `claim_decision` (µs) | \
         `BranchView::new` (µs) | kol toplamı p50 (µs) | p95 (µs) |"
    );
    println!("|---:|---:|---:|---:|---:|---:|---:|");

    let exec = NoRunner;
    let org = MockOrg {
        held_role: GRANT_ROLE,
        units: 1,
    };
    let engine = Engine {
        org: &org,
        exec: &exec,
        env: Default::default(),
    };
    let orgu = Uuid::new_v4();
    let actor = Actor {
        orgu_id: orgu,
        user_id: Uuid::new_v4(),
        role: GRANT_ROLE.into(),
    };

    for &g in GRANT_COUNTS {
        for &w in WFAH_LENS {
            let wfd = wfd_with_grants(g, Some(GUARD_FALSE), 0, false);
            let wfes = wfes_with_branches(w, g, orgu, 1);
            let branch = &wfes.branches[0];
            let ctx = wfes.dynctx.as_value();

            // Kapı: ölçüm gerçekten grant yolunu koşuyor mu (guard `false` →
            // aktör reddedilir). Yanlış yolu ölçen bir sayı, ölçüm olmamasından kötü.
            let decision = engine
                .claim_decision(&wfd, &wfes, &actor, Some(&branch.branch_node))
                .await
                .expect("claim_decision");
            assert_eq!(decision, AuthDecision::Denied, "ölçüm beklenen yolu koşmadı");

            let cand = time_p50_async(|| async {
                engine
                    .node_candidates(
                        &branch.branch_node,
                        &wfd,
                        ctx,
                        &wfes.wfah,
                        actor.orgu_id,
                        wfes.orgtnt_id,
                    )
                    .await
                    .expect("node_candidates")
            })
            .await;
            let claim = time_p50_async(|| async {
                engine
                    .claim_decision(&wfd, &wfes, &actor, Some(&branch.branch_node))
                    .await
                    .expect("claim_decision")
            })
            .await;
            let view = time_p50(|| {
                std::hint::black_box(BranchView::new(&wfd, branch, Vec::new(), None));
            });
            let (p50, p95) = measure_async(|| async {
                let c_a = engine
                    .node_candidates(
                        &branch.branch_node,
                        &wfd,
                        ctx,
                        &wfes.wfah,
                        actor.orgu_id,
                        wfes.orgtnt_id,
                    )
                    .await
                    .expect("node_candidates");
                let claim_as = provenance(
                    engine
                        .claim_decision(&wfd, &wfes, &actor, Some(&branch.branch_node))
                        .await
                        .expect("claim_decision"),
                );
                BranchView::new(&wfd, branch, c_a, claim_as)
            })
            .await;
            let real_w = wfes.wfah.entries().len();
            println!(
                "| {g} | {real_w} | {cand:.1} | {claim:.1} | {view:.2} | {p50:.1} | {p95:.1} |"
            );
        }
    }
}

/// İSTEK başına: `GET /wfe/:id` kol sayısı kadar gövde koşar.
async fn branch_read_per_request() {
    let w = *WFAH_LENS.last().expect("WFAH_LENS boş değil");
    println!();
    println!("### 1b. İstek başına (`GET /wfe/:id`), W≈{w}");
    println!();
    println!(
        "Kol döngüsü ardışıktır (`for b in &wfes.branches`), yani maliyet kol sayısıyla \
         DOĞRUSAL çarpılır."
    );
    println!();
    print!("| aktif kol (B) |");
    for &g in GRANT_COUNTS {
        print!(" G={g} |");
    }
    println!();
    print!("|---:|");
    for _ in GRANT_COUNTS {
        print!("---:|");
    }
    println!();

    let exec = NoRunner;
    let org = MockOrg {
        held_role: GRANT_ROLE,
        units: 1,
    };
    let engine = Engine {
        org: &org,
        exec: &exec,
        env: Default::default(),
    };
    let orgu = Uuid::new_v4();
    let actor = Actor {
        orgu_id: orgu,
        user_id: Uuid::new_v4(),
        role: GRANT_ROLE.into(),
    };

    for &b in BRANCH_COUNTS {
        print!("| {b} |");
        for &g in GRANT_COUNTS {
            let wfd = wfd_with_grants(g, Some(GUARD_FALSE), 0, false);
            let wfes = wfes_with_branches(w, g, orgu, b);
            let ctx = wfes.dynctx.as_value();
            let (p50, _) = measure_async(|| async {
                let mut out = Vec::with_capacity(wfes.branches.len());
                for br in &wfes.branches {
                    let c_a = engine
                        .node_candidates(
                            &br.branch_node,
                            &wfd,
                            ctx,
                            &wfes.wfah,
                            actor.orgu_id,
                            wfes.orgtnt_id,
                        )
                        .await
                        .expect("node_candidates");
                    let claim_as = provenance(
                        engine
                            .claim_decision(&wfd, &wfes, &actor, Some(&br.branch_node))
                            .await
                            .expect("claim_decision"),
                    );
                    out.push(BranchView::new(&wfd, br, c_a, claim_as));
                }
                out
            })
            .await;
            print!(" {} |", dur(p50));
        }
        println!();
    }
}

// ── 2. reproject ─────────────────────────────────────────────────────────────

/// `reproject_wfe`in motor ucu — WFE BAŞINA.
///
/// Çağrı dizisi `reproject.rs` ile BİREBİR aynıdır (kolon başına hangi çağrı, hangi
/// `guard_node` ile): ayrışırsa ölçüm başka bir şeyi ölçer.
async fn reproject_cost() {
    println!();
    println!("## 2. `reproject` — WFE BAŞINA (motor ucu)");
    println!();
    println!(
        "Çağrı dizisi `reproject::reproject_wfe` ile birebir: `view_grants` → \
         `node_candidates` + `node_view_grants` (tek-kol) → kol başına \
         `node_candidates` + `node_view_grants` → kolonların `serde_json::to_value`'ü. \
         `UPDATE`ler (1 + B sorgu) ölçümün DIŞINDA."
    );
    println!();
    println!("| B | G | W | L | motor p50 (ms) | p95 (ms) | jsonb serileşme p50 (µs) |");
    println!("|---:|---:|---:|---:|---:|---:|---:|");

    let exec = NoRunner;
    let org = MockOrg {
        held_role: GRANT_ROLE,
        units: 1,
    };
    let engine = Engine {
        org: &org,
        exec: &exec,
        env: Default::default(),
    };
    let orgu = Uuid::new_v4();
    let w = *WFAH_LENS.last().expect("WFAH_LENS boş değil");

    for &b in BRANCH_COUNTS {
        for &g in GRANT_COUNTS {
            for &l in NODE_LISTABLE_COUNTS {
                let wfd = wfd_with_grants(g, Some(GUARD_FALSE), l, false);
                let wfes = wfes_with_branches(w, g, orgu, b);
                let (p50, p95) =
                    measure_async(|| async { reproject_engine(&engine, &wfd, &wfes, orgu).await })
                        .await;
                let cols = reproject_engine(&engine, &wfd, &wfes, orgu).await;
                let ser = time_p50(|| {
                    for c in &cols {
                        std::hint::black_box(serde_json::to_value(c).expect("jsonb"));
                    }
                });
                println!(
                    "| {b} | {g} | {} | {l} | {:.2} | {:.2} | {ser:.1} |",
                    wfes.wfah.entries().len(),
                    p50 / 1000.0,
                    p95 / 1000.0
                );
            }
        }
    }
}

/// `reproject`in hangi çağrısı ne kadar — pay tablosu.
async fn reproject_breakdown() {
    let w = *WFAH_LENS.last().expect("WFAH_LENS boş değil");
    let b = 3usize;
    let g = 3usize;
    let l = 2usize;
    println!();
    println!("### 2b. Çağrı kırılımı (B={b}, G={g}, W≈{w}, L={l})");
    println!();
    println!("| çağrı | kez | p50/çağrı (µs) | toplam (µs) |");
    println!("|---|---:|---:|---:|");

    let exec = NoRunner;
    let org = MockOrg {
        held_role: GRANT_ROLE,
        units: 1,
    };
    let engine = Engine {
        org: &org,
        exec: &exec,
        env: Default::default(),
    };
    let orgu = Uuid::new_v4();
    let wfd = wfd_with_grants(g, Some(GUARD_FALSE), l, false);
    let wfes = wfes_with_branches(w, g, orgu, b);
    let ctx = wfes.dynctx.as_value();
    let node = wfes.current_node.clone().expect("tek-kol node");

    let view = time_p50_async(|| async {
        engine
            .view_grants(
                &wfd,
                ctx,
                &wfes.wfah,
                wfes.current_node.as_deref(),
                wfes.wfe_id,
                orgu,
                wfes.orgtnt_id,
            )
            .await
            .expect("view_grants")
    })
    .await;
    let cand = time_p50_async(|| async {
        engine
            .node_candidates(&node, &wfd, ctx, &wfes.wfah, orgu, wfes.orgtnt_id)
            .await
            .expect("node_candidates")
    })
    .await;
    let node_view = time_p50_async(|| async {
        engine
            .node_view_grants(
                &wfd,
                &node,
                ctx,
                &wfes.wfah,
                Some(node.as_str()),
                wfes.wfe_id,
                orgu,
                wfes.orgtnt_id,
            )
            .await
            .expect("node_view_grants")
    })
    .await;

    let rows: &[(&str, usize, f64)] = &[
        ("`view_grants` (kök `listable`/`wf_admin`)", 1, view),
        ("`node_candidates` (tek-kol + kol başına)", 1 + b, cand),
        (
            "`node_view_grants` (tek-kol + kol başına)",
            1 + b,
            node_view,
        ),
    ];
    let mut total = 0.0;
    for (name, times, per) in rows {
        let sum = per * *times as f64;
        total += sum;
        println!("| {name} | {times} | {per:.1} | {sum:.1} |");
    }
    println!("| **toplam** |  |  | **{total:.1}** |");
    println!();
    println!(
        "`terminal_view_grants` bu satırda YOK: aktif WFE'de `end_terminal` `None`'dır \
         ve `reproject` kolona hiç dokunmaz (bitmiş satırda bir kez, tek-kol \
         `node_view_grants` ile aynı mertebede)."
    );
}

/// `reproject_wfe`in ürettiği kolon kümesi — motor çağrıları AYNI SIRADA.
async fn reproject_engine(
    engine: &Engine<'_>,
    wfd: &Wfd,
    wfes: &Wfes,
    origin: Uuid,
) -> Vec<Vec<CandidateActor>> {
    let ctx = wfes.dynctx.as_value();
    let mut out = Vec::with_capacity(3 + 2 * wfes.branches.len());
    out.push(
        engine
            .view_grants(
                wfd,
                ctx,
                &wfes.wfah,
                wfes.current_node.as_deref(),
                wfes.wfe_id,
                origin,
                wfes.orgtnt_id,
            )
            .await
            .expect("view_grants"),
    );
    if let (Some(node), WfeStatus::Active) = (&wfes.current_node, &wfes.status) {
        out.push(
            engine
                .node_candidates(node, wfd, ctx, &wfes.wfah, origin, wfes.orgtnt_id)
                .await
                .expect("node_candidates"),
        );
        out.push(
            engine
                .node_view_grants(
                    wfd,
                    node,
                    ctx,
                    &wfes.wfah,
                    Some(node.as_str()),
                    wfes.wfe_id,
                    origin,
                    wfes.orgtnt_id,
                )
                .await
                .expect("node_view_grants"),
        );
    }
    for b in wfes
        .branches
        .iter()
        .filter(|b| b.status == BranchStatus::Active)
    {
        out.push(
            engine
                .node_candidates(&b.branch_node, wfd, ctx, &wfes.wfah, origin, wfes.orgtnt_id)
                .await
                .expect("node_candidates"),
        );
        out.push(
            engine
                .node_view_grants(
                    wfd,
                    &b.branch_node,
                    ctx,
                    &wfes.wfah,
                    None,
                    wfes.wfe_id,
                    origin,
                    wfes.orgtnt_id,
                )
                .await
                .expect("node_view_grants"),
        );
    }
    out
}

// ── 3. Grant boyutu ──────────────────────────────────────────────────────────

/// Kolonların BOYUTU: aday sayısı ve jsonb bayt.
///
/// ⚠️ **Act kolonu (`current_c_a` / kol `c_a`) guard'a BAKMAZ** — `node_candidates`
/// guard'ları BİLEREK yok sayar (`E03`: kolon over-inclusive bir önbellektir, commit
/// anında viewer bilinmez). Yani act kolonunun boyutu guard'ın `true`/`false`
/// olmasından BAĞIMSIZ, yalnız açık grant sayısına ve fan-out'a bağlıdır. Guard `true`
/// seçilmesi GÖRÜNÜRLÜK kolonu (`current_view_c_a`) içindir: orada guard uygulanır ve
/// `false` guard kolonu boşaltırdı.
///
/// Kural şekli `{c_orgu: "self", c_r: [rol]}`: bir kural `U` aday üretir (`U` =
/// selector'ın çözdüğü birim sayısı). Tekilleştirme `same_actor` ile yapılır, yani
/// AYNI (birim, rol) çiftini veren kurallar tek satıra iner — `U = 1` satırlarında
/// bütün grant'lar tek adaya çöker, fan-out büyüdükçe çakışma azalır.
async fn grant_size() {
    println!();
    println!("## 3. Grant boyutu — kolon başına aday sayısı ve bayt");
    println!();
    println!(
        "Act kolonu (`current_c_a`) guard'a BAKMAZ (`node_candidates` guard'ları \
         `E03` gereği yok sayar) — boyutu yalnız `G` ve fan-out belirler. Görünürlük \
         kolonunda (`current_view_c_a`) guard UYGULANIR, o yüzden tablo `true` guard \
         (`{GUARD_TRUE}`) ile koşar. `U` = `c_orgu` selector'ının çözdüğü birim sayısı. \
         Bayt = `serde_json::to_vec` uzunluğu."
    );
    println!();
    println!(
        "| U | G | `current_c_a` aday | bayt | aday/grant | `current_view_c_a` aday | bayt |"
    );
    println!("|---:|---:|---:|---:|---:|---:|---:|");

    let exec = NoRunner;
    let orgu = Uuid::new_v4();
    for &units in ORG_FANOUT {
        let org = MockOrg {
            held_role: GRANT_ROLE,
            units,
        };
        let engine = Engine {
            org: &org,
            exec: &exec,
            env: Default::default(),
        };
        let mut base = 0usize;
        for &g in GRANT_COUNTS {
            let wfd = wfd_with_grants(g, Some(GUARD_TRUE), 2, true);
            let wfes = wfes_with_branches(25, g, orgu, 1);
            let ctx = wfes.dynctx.as_value();
            let node = wfes.current_node.clone().expect("tek-kol node");
            let c_a = engine
                .node_candidates(&node, &wfd, ctx, &wfes.wfah, orgu, wfes.orgtnt_id)
                .await
                .expect("node_candidates");
            let view = engine
                .node_view_grants(
                    &wfd,
                    &node,
                    ctx,
                    &wfes.wfah,
                    Some(node.as_str()),
                    wfes.wfe_id,
                    orgu,
                    wfes.orgtnt_id,
                )
                .await
                .expect("node_view_grants");
            let c_a_bytes = serde_json::to_vec(&c_a).expect("jsonb").len();
            let view_bytes = serde_json::to_vec(&view).expect("jsonb").len();
            if g == 0 {
                base = c_a.len();
            }
            let per_grant = if g > 0 {
                format!("{:.1}", (c_a.len() as f64 - base as f64) / g as f64)
            } else {
                "—".into()
            };
            println!(
                "| {units} | {g} | {} | {c_a_bytes} | {per_grant} | {} | {view_bytes} |",
                c_a.len(),
                view.len()
            );
        }
    }
    println!();
    println!(
        "Kol kolonları (`wf.wfe_branch.c_a` / `view_c_a`) AYNI gövdeden doğar → satır \
         başına aynı boyut, WFE'nin toplam grant yükü `≈ (1 + B) ×` yukarıdaki bayt."
    );
}

// ── ölçüm yardımcıları ───────────────────────────────────────────────────────
//
// ⚠️ **Saat maliyeti ÖLÇÜMÜN İÇİNDEDİR ve bu makinede ~1,4 µs.** Tek çağrıyı
// `Instant::now()` çifti arasına almak, mikrosaniyenin altındaki gövdelerde ölçülen
// şeyi SAATE çevirir: ilk koşumda parçaların toplamı (1,5 + 3,3 + 1,5) bütünden
// (3,4) BÜYÜK çıktı, çünkü her parça kendi saat payını da sayıyordu. Bu yüzden her
// örnek `reps` KEZ koşar ve bölünür; `reps` gövdenin kabaca ölçülen süresinden
// türetilir (hedef: örnek başına ≥ 200 µs). Saatin kendi payı ayrıca basılır.

/// Örnek başına hedef süre — saat payı (~1,4 µs) bunun yanında ihmal edilir.
const TARGET_SAMPLE_US: f64 = 200.0;

/// Kabaca ölçülen `est` µs'lik bir gövde için tekrar sayısı.
fn reps_for(est: f64) -> usize {
    if est <= 0.0 {
        return 1000;
    }
    ((TARGET_SAMPLE_US / est).ceil() as usize).clamp(1, 1000)
}

/// `Instant::now()` çiftinin kendi payı (µs) — tabloların altına basılır ki
/// mikrosaniyenin altındaki satırların neden bölünerek ölçüldüğü görünsün.
fn clock_overhead_us() -> f64 {
    let mut out = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let t0 = Instant::now();
        std::hint::black_box(());
        out.push(t0.elapsed());
    }
    out.sort_unstable();
    micros(percentile(&out, 0.50))
}

/// Bir senkron kapanışın p50'si (µs/çağrı) — saat payı `reps` ile amortize edilir.
fn time_p50(mut f: impl FnMut()) -> f64 {
    for _ in 0..WARMUP {
        f();
    }
    let t0 = Instant::now();
    f();
    let reps = reps_for(micros(t0.elapsed()));
    let mut out = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let t0 = Instant::now();
        for _ in 0..reps {
            f();
        }
        out.push(t0.elapsed());
    }
    out.sort_unstable();
    micros(percentile(&out, 0.50)) / reps as f64
}

/// Async kapanışın p50'si (µs/çağrı).
async fn time_p50_async<F, Fut, T>(mut f: F) -> f64
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let (p50, _) = measure_async(&mut f).await;
    p50
}

/// Async kapanışın (p50, p95)'i (µs/çağrı). Saat payı `reps` ile amortize edilir.
async fn measure_async<F, Fut, T>(mut f: F) -> (f64, f64)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    for _ in 0..WARMUP {
        std::hint::black_box(f().await);
    }
    let t0 = Instant::now();
    std::hint::black_box(f().await);
    let reps = reps_for(micros(t0.elapsed()));
    let mut samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let t0 = Instant::now();
        for _ in 0..reps {
            std::hint::black_box(f().await);
        }
        samples.push(t0.elapsed());
    }
    samples.sort_unstable();
    (
        micros(percentile(&samples, 0.50)) / reps as f64,
        micros(percentile(&samples, 0.95)) / reps as f64,
    )
}

fn percentile(sorted: &[Duration], q: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let idx = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[idx]
}

fn micros(d: Duration) -> f64 {
    d.as_secs_f64() * 1_000_000.0
}

/// µs'yi okunur birimle basar — `1b` tablosunda G=0 hücreleri `0.0 ms` diye
/// yuvarlanıp bilgi kaybediyordu.
fn dur(us: f64) -> String {
    if us >= 1000.0 {
        format!("{:.1} ms", us / 1000.0)
    } else {
        format!("{us:.1} µs")
    }
}

/// `ClaimProvenance::from_auth` görünüm dönüşümünün ölçüm karşılığı — o metot
/// `pub` DEĞİL. Ölçülen maliyet karardadır (`claim_decision`), dönüşüm bir `match`.
fn provenance(d: AuthDecision) -> Option<ClaimProvenance> {
    match d {
        AuthDecision::Denied => None,
        _ => Some(ClaimProvenance::Direct),
    }
}

// ── sentetik belge / durum ───────────────────────────────────────────────────

/// Golden belge + ölçülen node'a `count` kademe escalation ve `node_listable` tane
/// `listable` kuralı.
///
/// `guard` verilirse hem grant'ların hem `listable` kurallarının `when`i olur.
/// İki eksen AYNI şekli taşır ki birim maliyetleri karşılaştırılabilir olsun.
///
/// `distinct_grant_roles`: kademe başına ayrı rol (`branchManager_0`, `_1`, …).
/// BOYUT tablosu bunu ister — gerçek escalation kademeleri farklı yetkilileri çağırır
/// ve aynı `c_a`'yı tekrarlayan kademeler `same_actor` ile tek satıra inip büyümeyi
/// gizler. ZAMAN tablolarında `false`: aktör tek bir rol taşır ve her kademenin `c_a`'sı
/// ona uymalı ki `when` guard'ı GERÇEKTEN değerlendirilsin.
fn wfd_with_grants(
    count: usize,
    guard: Option<&str>,
    node_listable: usize,
    distinct_grant_roles: bool,
) -> Wfd {
    let mut doc: Value = serde_json::from_str(FIXTURE).expect("fixture JSON");
    let escalation: Vec<Value> = (0..count)
        .map(|i| {
            // Kademe başına AYRI rol (boyut tablosu) ya da hepsinde AYNI rol (zaman
            // tabloları — aktör o rolü taşır, guard GERÇEKTEN koşar).
            let role = if distinct_grant_roles {
                format!("{GRANT_ROLE}_{i}")
            } else {
                GRANT_ROLE.to_string()
            };
            let mut grant = json!({ "c_a": { "c_orgu": "self", "c_r": [role] } });
            if let Some(expr) = guard {
                grant["when"] = Value::String(expr.into());
            }
            json!({
                // Kademe vadeleri artan; ölçüm ateşlemeyi DEFTERDEN okuduğu için
                // süre değeri sonucu ETKİLEMEZ, yalnız belge geçerli olsun diye var.
                "after": format!("PT{}M", (i + 1) * 10),
                "grant": grant
            })
        })
        .collect();
    let listable: Vec<Value> = (0..node_listable)
        .map(|_| {
            let mut rule = json!({ "c_a": { "c_orgu": "self", "c_r": [GRANT_ROLE] } });
            if let Some(expr) = guard {
                rule["when"] = Value::String(expr.into());
            }
            rule
        })
        .collect();
    doc["nodes"][NODE]["escalation"] = Value::Array(escalation);
    doc["nodes"][NODE]["c_a"] = json!({ "c_orgu": "self", "c_r": [NODE_ROLE] });
    if node_listable > 0 {
        doc["nodes"][NODE]["listable"] = Value::Array(listable);
    }
    Wfd::from_value(doc).expect("sentetik WFD geçerli")
}

/// `len` satırlık defter + `branches` tane aktif kol.
///
/// Marker'lar giriş satırından SONRA yazılır, yoksa `open_grants` onları önceki turun
/// kalıntısı sayıp grant'ı KAPALI okur (ve ölçüm sessizce `G=0`'ı ölçerdi).
///
/// Kollar tek-kol alanlarını (`current_node`) BOŞALTMAZ: `reproject` tablosu
/// tek-kol + kol maliyetini AYNI satırda gösteriyor, gerçek paralel modda
/// `current_node` NULL olduğu için o iki çağrı düşer (yani tablo ÜST SINIRDIR).
fn wfes_with_branches(len: usize, open_grants: usize, orgu: Uuid, branches: usize) -> Wfes {
    let system = Actor {
        orgu_id: Uuid::nil(),
        user_id: Uuid::nil(),
        role: "system".into(),
    };
    let t0 = Utc::now() - ChronoDuration::days(30);
    let mut entries: Vec<WfahEntry> = Vec::with_capacity(len);
    entries.push(entry(1, "create_application", &system, t0, Some(NODE)));
    for i in 0..open_grants {
        let seq = entries.len() as u32 + 1;
        entries.push(entry(
            seq,
            &format!("escalate:{NODE}:{i}"),
            &system,
            t0 + ChronoDuration::minutes(10 * (i as i64 + 1)),
            None,
        ));
    }
    while entries.len() < len.max(entries.len()) {
        let seq = entries.len() as u32 + 1;
        entries.push(entry(
            seq,
            "analyst_approve",
            &system,
            t0 + ChronoDuration::hours(seq as i64),
            None,
        ));
    }
    let wfah = Wfah::empty().extended(&entries);
    let branch_states: Vec<BranchState> = (0..branches)
        .map(|i| BranchState {
            branch_node: NODE.into(),
            entry_node: NODE.into(),
            status: BranchStatus::Active,
            claimed_by: None,
            claimed_at: None,
            entered_at: t0 + ChronoDuration::minutes(i as i64),
        })
        .collect();
    Wfes {
        wfe_id: Uuid::new_v4(),
        orgtnt_id: Uuid::nil(),
        environment_id: None,
        wfd_id: Uuid::new_v4(),
        wfd_version: 1,
        dynctx: DynCtx(json!({
            "credit_info": { "amount_requested": 50000, "purpose": "konut" },
            "internal_notes": "ölçüm"
        })),
        wfah,
        status: WfeStatus::Active,
        visited_nodes: vec![NODE.into()],
        current_node: Some(NODE.into()),
        end_terminal: None,
        assigned_to: None,
        end_response: None,
        deadline: None,
        claimed_at: None,
        created_at: t0,
        branches: branch_states,
        join_target: None,
        join_rule: JoinRule::All,
        // ÇAPA: grant kuralları WFE'nin kendi birimine çapalanır (`E04`/S5).
        origin_orgu_id: Some(orgu),
    }
}

fn entry(
    seq: u32,
    action: &str,
    actor: &Actor,
    at: DateTime<Utc>,
    to_node: Option<&str>,
) -> WfahEntry {
    WfahEntry {
        seq,
        action: action.into(),
        actor: actor.clone(),
        input: Some(json!({ "note": "ölçüm satırı" })),
        applied_at: at,
        from_node: None,
        to_node: to_node.map(String::from),
        branch_entry: None,
        branch_round: None,
    }
}

// ── portlar ─────────────────────────────────────────────────────────────────

/// `c_orgu` `units` tane birime çözülür — BOYUT tablosunun fan-out'u. Zaman
/// tablolarında `units = 1`: ölçülen org sorgularının maliyeti DEĞİL, guard
/// değerlendirmesidir.
///
/// `held_role`: ölçüm kullanıcısının GERÇEKTEN taşıdığı tek koltuk.
struct MockOrg {
    held_role: &'static str,
    units: usize,
}

#[async_trait]
impl OrgPort for MockOrg {
    async fn resolve_c_orgu(
        &self,
        anchor: Uuid,
        _expr: &str,
        _orgtnt: Uuid,
    ) -> Result<Vec<OrgUnit>, EngineError> {
        // DETERMİNİST: aynı selector her çağrıda AYNI kümeye çözülür. Rastgele id
        // üretmek, aynı `c_a`'yı taşıyan iki grant'ı farklı birimlere düşürüp
        // tekilleştirmeyi sahte biçimde bozuyordu (boyut tablosu şişerdi).
        Ok((0..self.units)
            .map(|i| OrgUnit {
                orgu_id: if i == 0 {
                    anchor
                } else {
                    Uuid::from_u128(i as u128)
                },
                orgu_type: json!({ "type": "branch" }),
                path: format!("1.{i}"),
            })
            .collect())
    }
    async fn check_user_role(
        &self,
        _user_id: Uuid,
        _orgu_id: Uuid,
        role_name: &str,
    ) -> Result<bool, EngineError> {
        Ok(role_name == self.held_role)
    }
    async fn orgtnt_for_orgu(&self, _: Uuid) -> Result<Uuid, EngineError> {
        Ok(Uuid::nil())
    }
}

struct NoRunner;

#[async_trait]
impl AutoexecRunner for NoRunner {
    async fn run(&self, _def: &AutoexecDef, _env: &ExecEnv) -> Result<Value, ExecFailure> {
        Err(ExecFailure::failed("ölçümde autoexec koşmaz"))
    }
}
