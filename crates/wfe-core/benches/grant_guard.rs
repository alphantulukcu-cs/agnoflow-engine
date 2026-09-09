//! `E04`/S4 ÖLÇÜM KALEMİ (`M1`, WOR-111) — grant guard maliyeti.
//!
//! `E04` yetki kümesini `node.c_a ∪ açılmış grantlar` yaptı ve grant'ın `when`
//! guard'ı HER yetki sorgusunda değerlendiriliyor. Kayıt önbelleklemeyi ölçülmeden
//! REDDETTİ ve uygulama fazına bu ölçümü yazdı: maliyet
//! **havuz satır sayısı × açık grant sayısı × |WFAH|** ile büyüyor mu, ne kadar?
//!
//! Ölçülen gövde havuz listesinin per-satır kararıdır: `Engine::can_claim`
//! (`WfeExecutor::can_claim_loaded` → `can_claim_many` → `routes/portal/pool.rs`
//! zincirinin CPU ucu). DB burada YOK, çünkü ölçülen şey de DB değil: `can_claim_many`
//! tek `load_many` + sürüm başına bir `fetch` atar, satır sayısıyla büyüyen kısım saf
//! CPU'dur. Havuz satır sayısı (`R`) bu yüzden ölçülen değil ÇARPILAN değişkendir ve
//! rapor onu `pool_sql`de `LIMIT` OLMADIĞI için gerçekçi tenant profilleriyle çarpar.
//!
//! Koşum:
//!
//! ```text
//! cargo bench -p wfe-core --bench grant_guard
//! ```
//!
//! Çıktı markdown tablodur; sayılar issue'ya böyle yazılır. **Bu araç karar VERMEZ**
//! (bkz. WOR-111 kabul kriteri): "önbellek gerekli mi" sorusuna veri üretir, cevabı
//! ayrı bir karar penceresi verir.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde_json::{json, Value};
use uuid::Uuid;
use wfe_core::error::EngineError;
use wfe_core::ports::OrgPort;
use wfe_core::types::actor::{Actor, OrgUnit};
use wfe_core::types::dynctx::DynCtx;
use wfe_core::types::wfah::{Wfah, WfahEntry};
use wfe_core::types::wfd_v22::{AutoexecDef, JoinRule, Wfd};
use wfe_core::types::wfe::WfeStatus;
use wfe_core::v22::pipeline::{ClaimCheck, Engine};
use wfe_core::v22::ports::{AutoexecRunner, ExecEnv, ExecFailure, Wfes};

const FIXTURE: &str = include_str!("../../../docs/spec/examples/kredi-basvuru.golden.json");

/// Ölçülen node. Golden belgede escalation TAŞIYAN havuz node'u.
const NODE: &str = "self__creditAnalyst";

/// Node'un kendi havuzu (`c_a`) — aktör BUNA UYMAZ, yani ölçüm grant yolunu koşar.
const NODE_ROLE: &str = "creditAnalyst";

/// Grant'ların dağıttığı rol — ölçüm aktörü BUNU taşır, yani her grant'ın `c_a`'sı
/// eşleşir ve `when` guard'ı GERÇEKTEN değerlendirilir. Guard'lar `false` döner:
/// ölçülen ilk-eşleşende-çık DEĞİL, TÜM açık grantların değerlendirildiği EN KÖTÜ hâl.
const GRANT_ROLE: &str = "branchManager";

/// Grant `c_a`'sı eşleşMEYEN rol — `c_a-only` profili bunu kullanır: guard hiç
/// değerlendirilmez, ölçülen yalnız kural başına matcher + `open_grants` maliyetidir.
/// Guard'lı profillerle farkı "guard'ın kendi payı"nı verir.
const UNMATCHED_ROLE: &str = "auditor";

/// Ölçüm profilleri: `(ad, guard ifadesi, grant'ın dağıttığı rol)`.
///
/// Üçü de DENIED ile biter (en kötü hâl: tüm açık grantlar değerlendirilir).
///
/// * `c_a-only` — grant `c_a`'sı eşleşmez → guard yolu HİÇ açılmaz.
/// * `ctx` — guard defteri hiç okumaz. Buna rağmen `EvalEnv` kurulumu
///   (`with_wfah` → `project_entry` × |WFAH| + `project_valid`) ödenir, çünkü ortam
///   kural BAŞINA kuruluyor (`matches_grant_rules` gövdesi).
/// * `valid` — guard defteri tarar; `ctx` ile farkı ifadenin kendi payıdır.
const PROFILES: &[(&str, Option<&str>, &str)] = &[
    ("c_a-only", None, UNMATCHED_ROLE),
    ("ctx", Some(GUARD_CTX), GRANT_ROLE),
    ("valid", Some(GUARD_VALID), GRANT_ROLE),
];

/// Defteri HİÇ okumayan guard.
const GUARD_CTX: &str = "$ctx.credit_info.amount_requested >= 999999999";
/// Defteri tarayan guard (v2.3'ün `$valid` görünümü üzerinden).
const GUARD_VALID: &str = r#"count($valid, #.action == "analyst_approve") >= 999999"#;

/// |WFAH| — defter uzunluğu. Uzun koşan bir WFE'de bu ve açık grant sayısı AYNI
/// yönde büyüyor (`E04` / feda edilenler).
const WFAH_LENS: &[usize] = &[5, 25, 100, 500];

/// Açık grant sayısı (ateşlenmiş escalation kademesi).
const GRANT_COUNTS: &[usize] = &[0, 1, 3, 10];

/// Gerçekçi tenant profilleri: havuz listesinin döndürdüğü satır sayısı.
/// `pool_sql` SQL'inde `LIMIT` YOK — üst sınır tenant'ın açık iş sayısıdır.
const POOL_ROWS: &[usize] = &[25, 200, 1000];

/// Hücre başına örnek sayısı (p50/p95 için).
const SAMPLES: usize = 200;
const WARMUP: usize = 20;

fn main() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    rt.block_on(run());
}

async fn run() {
    let exec = NoRunner;
    // Grant yolunu ölçen aktör YALNIZ grant rolünü taşır: node `c_a`'sı (creditAnalyst)
    // onu KABUL ETMEZ, dolayısıyla karar grant kümesine düşer.
    let grant_org = MockOrg {
        held_role: GRANT_ROLE,
    };
    let engine = Engine {
        org: &grant_org,
        exec: &exec,
        env: Default::default(),
    };
    let orgu = Uuid::new_v4();
    let actor = Actor {
        orgu_id: orgu,
        user_id: Uuid::new_v4(),
        role: GRANT_ROLE.into(),
    };

    println!("# E04/S4 — grant guard maliyeti (WOR-111 / M1)");
    println!();
    println!(
        "Ölçülen gövde: `Engine::can_claim` — havuz satırı BAŞINA karar \
         (`can_claim_loaded` → `can_claim_many` → `routes/portal/pool.rs`). \
         Aktör node `c_a`'sına UYMAZ ve hiçbir grant onu yetkilendirmez, yani her \
         açık grant değerlendirilir: EN KÖTÜ hâl. Örnek/hücre: {SAMPLES} (+{WARMUP} ısınma)."
    );
    println!();
    println!(
        "`W` = defterdeki satır sayısı. Marker'lar deftere yazıldığı için W ≥ 1 + G'dir; \
         tabloda GERÇEKLEŞEN W yazar."
    );

    // Taban 1: node `c_a`'sı UYUYOR → `authorize_node_decision` grant yoluna hiç
    // girmez. "Grant yoksa maliyet ne" sorusunun cevabı budur.
    println!();
    println!("## Taban — node `c_a` eşleşiyor (grant yolu KAPALI)");
    println!();
    println!("| \\|WFAH\\| (W) | p50 (µs/satır) | p95 (µs/satır) |");
    println!("|---:|---:|---:|");
    let node_org = MockOrg {
        held_role: NODE_ROLE,
    };
    let node_engine = Engine {
        org: &node_org,
        exec: &exec,
        env: Default::default(),
    };
    let node_hit_actor = Actor {
        orgu_id: orgu,
        user_id: Uuid::new_v4(),
        role: NODE_ROLE.into(),
    };
    for &w in WFAH_LENS {
        // Grant AÇIK ve guard'lı; node `c_a`'sı ÖNCE tuttuğu için grant hiç sorulmaz.
        let wfd = wfd_with_grants(1, Some(GUARD_CTX), GRANT_ROLE);
        let wfes = wfes_with_wfah(w, 1, orgu);
        let (p50, p95, _) =
            measure(&node_engine, &wfd, &wfes, &node_hit_actor, ClaimCheck::Ok).await;
        println!("| {} | {p50:.1} | {p95:.1} |", wfes.wfah.entries().len());
    }

    for (name, guard, grant_role) in PROFILES {
        println!();
        match guard {
            Some(expr) => println!("## profil `{name}` — guard `{expr}`"),
            None => println!("## profil `{name}` — grant `c_a`'sı eşleşmez, guard KOŞMAZ"),
        }
        println!();
        println!(
            "| açık grant (G) | \\|WFAH\\| (W) | p50 (µs/satır) | p95 (µs/satır) | \
             taban (G=0) çarpanı | grant başına marjinal (µs) |"
        );
        println!("|---:|---:|---:|---:|---:|---:|");
        let mut p50_table: Vec<(usize, usize, f64)> = Vec::new();
        for &grants in GRANT_COUNTS {
            for &wfah_len in WFAH_LENS {
                let wfd = wfd_with_grants(grants, *guard, grant_role);
                let wfes = wfes_with_wfah(wfah_len, grants, orgu);
                let real_w = wfes.wfah.entries().len();
                let (p50, p95, _) =
                    measure(&engine, &wfd, &wfes, &actor, ClaimCheck::NotEligible).await;
                let base = p50_table
                    .iter()
                    .find(|(g, w, _)| *g == 0 && *w == real_w)
                    .map(|(_, _, v)| *v);
                let (ratio, marginal) = match base {
                    Some(b) if b > 0.0 && grants > 0 => (
                        format!("{:.1}×", p50 / b),
                        format!("{:.1}", (p50 - b) / grants as f64),
                    ),
                    _ => ("—".into(), "—".into()),
                };
                p50_table.push((grants, real_w, p50));
                println!("| {grants} | {real_w} | {p50:.1} | {p95:.1} | {ratio} | {marginal} |");
            }
        }

        // İSTEK BAŞINA maliyet: per-satır p50 × havuz satır sayısı. `can_claim_many`
        // satır başına aynı gövdeyi çağırdığı için çarpım doğrusaldır ve `pool_sql`de
        // `LIMIT` YOK — satır sayısının üst sınırı tenant'ın açık iş sayısıdır.
        let w = *WFAH_LENS.last().expect("WFAH_LENS boş değil");
        println!();
        println!("### İstek başına (havuz listesi), W≈{w} — p50 × satır sayısı");
        println!();
        print!("| havuz satırı (R) |");
        for &g in GRANT_COUNTS {
            print!(" G={g} |");
        }
        println!();
        print!("|---:|");
        for _ in GRANT_COUNTS {
            print!("---:|");
        }
        println!();
        for &rows in POOL_ROWS {
            print!("| {rows} |");
            for &g in GRANT_COUNTS {
                let per_row = p50_table
                    .iter()
                    .filter(|(gg, _, _)| *gg == g)
                    .max_by_key(|(_, ww, _)| *ww)
                    .map(|(_, _, v)| *v)
                    .expect("hücre ölçüldü");
                print!(" {:.0} ms |", per_row * rows as f64 / 1000.0);
            }
            println!();
        }
    }

    cost_breakdown().await;
}

/// **Maliyet nerede** — `matches_grant_rules` gövdesinin iki parçası ayrı ayrı.
///
/// Guard'lı ölçüm `G × W` ile büyüyor; büyüyen parçanın HANGİSİ olduğu önbellek
/// kararının (ifade / sonuç / WFE seviyesi) girdisidir:
///
/// * **ortam kurulumu** — `EvalEnv::new(ctx).with_wfah(..)`: `project_entry` × W +
///   `project_valid` × W. Kural DÖNGÜSÜNÜN İÇİNDE kuruluyor (`grants.rs`de
///   `matches_grant_rules` gövdesi), oysa hepsi döngü-değişmezi.
/// * **değerlendirme** — `evaluate_bool`: `zen_context()` (projeksiyonların DERİN
///   kopyası) + ifade parse + koşum. Kural başına ödenmesi ZORUNLU.
///
/// Tablo "ortam döngü dışına alınsa ne kazanılırdı" sorusunu ölçerek cevaplar:
/// `hoisted = kurulum + G × değerlendirme`, `bugün = G × (kurulum + değerlendirme)`.
async fn cost_breakdown() {
    use wfe_core::v22::eval::{evaluate_bool, EvalEnv};
    use wfe_core::v22::valid::ValidRules;

    println!();
    println!("## Maliyet nerede — ortam kurulumu vs. değerlendirme");
    println!();
    println!(
        "`kurulum` = `EvalEnv::new(ctx).with_wfah(..)` (kural döngüsünün İÇİNDE, \
         oysa döngü-değişmezi). `değerlendirme` = `evaluate_bool` (`zen_context()` \
         derin kopyası + parse + koşum). `hoisted(G=10)` = kurulum + 10 × değerlendirme."
    );
    println!();
    println!(
        "| \\|WFAH\\| (W) | kurulum p50 (µs) | değerlendirme p50 (µs) | \
         bugün G=10 (µs) | hoisted G=10 (µs) | kazanç |"
    );
    println!("|---:|---:|---:|---:|---:|---:|");
    let orgu = Uuid::new_v4();
    for &w in WFAH_LENS {
        let wfd = wfd_with_grants(1, Some(GUARD_VALID), GRANT_ROLE);
        let rules = ValidRules::for_version(&wfd);
        let wfes = wfes_with_wfah(w, 1, orgu);
        let ctx = wfes.dynctx.as_value();
        let actor = Actor {
            orgu_id: orgu,
            user_id: Uuid::new_v4(),
            role: GRANT_ROLE.into(),
        };

        let build = || {
            EvalEnv::new(ctx)
                .with_wfah(&wfes.wfah, &rules)
                .with_node(wfes.current_node.as_deref())
                .with_actor(&actor)
                .with_wfe_id(wfes.wfe_id)
        };
        let setup = time_p50(SAMPLES, || {
            std::hint::black_box(build());
        });
        let env = build();
        let eval = time_p50(SAMPLES, || {
            std::hint::black_box(evaluate_bool(GUARD_VALID, &env).expect("guard"));
        });
        let today = 10.0 * (setup + eval);
        let hoisted = setup + 10.0 * eval;
        println!(
            "| {w} | {setup:.1} | {eval:.1} | {today:.1} | {hoisted:.1} | {:.1}× |",
            today / hoisted
        );
    }
}

/// Bir kapanışın p50'si (µs).
fn time_p50(samples: usize, mut f: impl FnMut()) -> f64 {
    for _ in 0..WARMUP {
        f();
    }
    let mut out = Vec::with_capacity(samples);
    for _ in 0..samples {
        let t0 = Instant::now();
        f();
        out.push(t0.elapsed());
    }
    out.sort_unstable();
    micros(percentile(&out, 0.50))
}

/// Bir hücrenin p50/p95'i. `expect` beklenen `ClaimCheck`tir ve KAPIDIR: yanlış yolu
/// ölçen bir sayı, ölçüm olmamasından kötüdür.
async fn measure(
    engine: &Engine<'_>,
    wfd: &Wfd,
    wfes: &Wfes,
    actor: &Actor,
    expect: ClaimCheck,
) -> (f64, f64, usize) {
    let check = engine
        .can_claim(wfd, wfes, actor, None)
        .await
        .expect("can_claim");
    assert_eq!(check, expect, "ölçüm beklenen yolu koşmadı");
    let mut samples = Vec::with_capacity(SAMPLES);
    for i in 0..(WARMUP + SAMPLES) {
        let t0 = Instant::now();
        let out = engine.can_claim(wfd, wfes, actor, None).await;
        let dt = t0.elapsed();
        std::hint::black_box(out.expect("can_claim"));
        if i >= WARMUP {
            samples.push(dt);
        }
    }
    samples.sort_unstable();
    (
        micros(percentile(&samples, 0.50)),
        micros(percentile(&samples, 0.95)),
        samples.len(),
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

// ── sentetik belge / durum ───────────────────────────────────────────────────

/// Golden belge + ölçülen node'a `count` kademe escalation. Her kademenin grant'ı
/// `grant_role`'ü dağıtır; `guard` verilirse `when` olarak yazılır.
fn wfd_with_grants(count: usize, guard: Option<&str>, grant_role: &str) -> Wfd {
    let mut doc: Value = serde_json::from_str(FIXTURE).expect("fixture JSON");
    let escalation: Vec<Value> = (0..count)
        .map(|i| {
            {
                let mut grant = json!({ "c_a": { "c_orgu": "self", "c_r": [grant_role] } });
                if let Some(expr) = guard {
                    grant["when"] = Value::String(expr.into());
                }
                json!({
                    // Kademe vadeleri artan; ölçüm ateşlemeyi DEFTERDEN okuduğu için
                    // süre değeri sonucu ETKİLEMEZ, yalnız belge geçerli olsun diye var.
                    "after": format!("PT{}M", (i + 1) * 10),
                    "grant": grant
                })
            }
        })
        .collect();
    doc["nodes"][NODE]["escalation"] = Value::Array(escalation);
    doc["nodes"][NODE]["c_a"] = json!({ "c_orgu": "self", "c_r": [NODE_ROLE] });
    Wfd::from_value(doc).expect("sentetik WFD geçerli")
}

/// `len` satırlık defter: 1 giriş satırı (`to_node`) + `open_grants` kademe marker'ı +
/// gerisi aksiyon satırı.
///
/// Marker'lar giriş satırından SONRA yazılır, yoksa `open_grants` onları önceki turun
/// kalıntısı sayıp grant'ı KAPALI okur (ve ölçüm sessizce `G=0`'ı ölçerdi).
fn wfes_with_wfah(len: usize, open_grants: usize, orgu: Uuid) -> Wfes {
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
        branches: vec![],
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

/// `c_orgu` daima çapanın kendi birimine çözülür; ölçüm org sorgularının maliyetini
/// DEĞİL, guard değerlendirmesini ölçüyor.
///
/// `held_role`: ölçüm kullanıcısının GERÇEKTEN taşıdığı tek koltuk. Kuralın istediği
/// rol bu değilse eşleşme başarısızdır — profillerin "node `c_a`'sı tutar / grant
/// `c_a`'sı tutar / hiçbiri tutmaz" ayrımı buradan gelir.
struct MockOrg {
    held_role: &'static str,
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
            orgu_type: json!({ "type": "branch" }),
            path: "1".into(),
        }])
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
