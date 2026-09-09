//! **`S29` — `$valid` / `invalid_reason` SİM PARİTE KAPISI** (`A04` FEDA EDİLENLER).
//!
//! `$valid` eleme kuralları DEFTERDEN türer (`wfe_core::v22::valid`), yani `sim.rs`
//! de aynı cevabı üretmek ZORUNDADIR. Ama sim'in store'u yoktur: satırları kendisi
//! biriktirir (`SimState::apply_commit`) ve adapter'ın yazdığı bir satırı (`_join`)
//! elle taklit eder. Eksik yazılan bir alan (`branch_entry` / `to_node`) ya da
//! atlanan bir marker satırı, simülasyonda "geçerli" görünen bir onayın gerçek
//! akışta ELENMİŞ çıkması demektir — `Ç2`/`Ç13`/`R04`/`E05` bu riski kabul edip
//! test kapısını buraya ertelemişti. `A04` alanı API'ye açtığı an ayrışma
//! GÖRÜNÜR olur, bu yüzden kapı artık burada.
//!
//! ## Ne karşılaştırılıyor
//!
//! Aynı senaryo İKİ yolda koşar — `sim::step` (store'suz) ve `WfeExecutor` +
//! `ParStore` (adapter semantiğinin taklidi, `common`) — sonra iki defter
//! **satır satır** karşılaştırılır:
//!
//! 1. `$valid` KÜMESİ birebir aynı,
//! 2. elenen satırların `invalid_reason` DEĞERLERİ birebir aynı.
//!
//! `seq` karşılaştırmaya GİRMEZ: gerçek akış sahiplik satırları da yazar
//! (`claim_taken:`), sim yazmaz — numaralar kayar. Kayan numara bir ayrışma
//! DEĞİLDİR; ayrışma, aynı satırın iki yolda farklı YARGILANMASIDIR.
//!
//! ## Sim'in BİLİNEN körlüğü — kapının asıl konusu
//!
//! Sim claim akışını atlar (`sim.rs`: "apply öncesi state aktöre atanır"), yani
//! sahiplik satırları sim defterinde HİÇ yoktur. Bu, `$valid`i BOZMAZ çünkü o
//! satırlar zaten `Ownership` ile elenir (`Ç13`) — ve kapının kanıtlaması gereken
//! şey tam olarak budur: körlük eşik sayımına sızmıyor. Bu yüzden sahiplik
//! satırları karşılaştırmadan ÇIKARILMAZ, AYRICA sınanır.

mod common;
use common::*;

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use uuid::Uuid;
use wf_wfe::sim::{step, SimState};
use wf_wfe::WfeExecutor;
use wfe_core::types::wfah::{Wfah, WfahEntry};
use wfe_core::types::wfd_v22::Wfd;
use wfe_core::v22::pipeline::Engine;
use wfe_core::v22::valid::{invalid_reason, InvalidReason, ValidRules};
use wfe_core::v22::wfah_kind::parse_marker;

static MOCK_ORG: MockOrg = MockOrg;
static MOCK_RUNNER: MockRunner = MockRunner;

fn engine() -> Engine<'static> {
    Engine {
        org: &MOCK_ORG,
        exec: &MOCK_RUNNER,
        env: Default::default(),
    }
}

fn start_input() -> Value {
    json!({"request": {"title": "Sunucu alımı", "amount": 150000}})
}

// ---- senaryo ------------------------------------------------------------------

/// Bir adım — İKİ yolda da AYNI sırayla, AYNI rolle, AYNI kolda uygulanır.
/// Aktörün kimliği (uuid) yollara göre değişir ve zaten karşılaştırmaya girmez;
/// önemli olan hangi ROLün hangi KOLda hangi aksiyonu aldığıdır.
struct Step {
    role: &'static str,
    action: &'static str,
    /// Kol ipucu (`ApplyBody.node`) — paralel modda ZORUNLU, tekil modda `None`.
    branch: Option<&'static str>,
    /// Geri gönderme hedefi (`ApplyBody.target`).
    target: Option<&'static str>,
}

const fn wfe_step(role: &'static str, action: &'static str) -> Step {
    Step {
        role,
        action,
        branch: None,
        target: None,
    }
}

const fn branch_step(role: &'static str, action: &'static str, branch: &'static str) -> Step {
    Step {
        role,
        action,
        branch: Some(branch),
        target: None,
    }
}

/// SİM yolu: store YOK, defter `SimState` içinde birikir.
async fn run_sim(wfd: &Wfd, steps: &[Step]) -> Wfah {
    let eng = engine();
    let mut state = SimState::from_new_wfe(
        &eng.start(
            wfd,
            &actor("requester"),
            Uuid::nil(),
            None,
            &start_input(),
            Uuid::new_v4(),
            None,
        )
        .await
        .unwrap(),
    );
    for s in steps {
        let a = actor(s.role);
        step::apply(
            &eng,
            wfd,
            &mut state,
            &a,
            s.action,
            &json!({}),
            s.branch,
            s.target,
        )
        .await
        .unwrap_or_else(|e| panic!("sim adımı reddedildi ({}): {e}", s.action));
    }
    Wfah(state.wfah.clone())
}

/// GERÇEK akış: `WfeExecutor` + `ParStore` (adapter semantiği). Her adımdan ÖNCE
/// claim alınır — sim'de olmayan sahiplik satırları tam olarak buradan doğar.
async fn run_real(wfd: &Wfd, steps: &[Step]) -> Wfah {
    let store = Arc::new(ParStore::default());
    let exec = WfeExecutor::new(
        Arc::new(MockOrg),
        Arc::new(FixtureWfdStore(wfd.clone())),
        store.clone(),
        Arc::new(MockRunner),
    );
    let started = exec
        .start(
            Uuid::new_v4(),
            1,
            &actor("requester"),
            None,
            &start_input(),
            None,
        )
        .await
        .unwrap();
    let wfe_id = started.wfe_id;
    for s in steps {
        let a = actor(s.role);
        assert!(
            exec.claim(wfe_id, &a, s.branch, None)
                .await
                .unwrap()
                .success,
            "claim: {} / {:?}",
            s.action,
            s.branch
        );
        exec.apply(wfe_id, &a, s.action, &json!({}), s.branch, s.target, None)
            .await
            .unwrap_or_else(|e| panic!("gerçek akış adımı reddedildi ({}): {e}", s.action));
    }
    store.snapshot(wfe_id).wfah
}

// ---- karşılaştırma ------------------------------------------------------------

/// Bir satırın KARŞILAŞTIRILAN yüzü. `seq` ve `actor` BİLEREK yok: ikisi de iki
/// yolda meşru olarak farklıdır (sahiplik satırları seq'i kaydırır, aktör uuid'si
/// her koşumda yenidir) ve ikisi de eleme kurallarının GİRDİSİ değil.
#[derive(Debug, PartialEq, Eq)]
struct Row {
    action: String,
    from_node: Option<String>,
    to_node: Option<String>,
    branch_entry: Option<String>,
    branch_round: Option<u32>,
    reason: Option<InvalidReason>,
}

fn row_of(wfah: &Wfah, rules: &ValidRules, e: &WfahEntry) -> Row {
    Row {
        action: e.action.clone(),
        from_node: e.from_node.clone(),
        to_node: e.to_node.clone(),
        branch_entry: e.branch_entry.clone(),
        branch_round: e.branch_round,
        reason: invalid_reason(wfah, rules, e),
    }
}

fn is_ownership(e: &WfahEntry) -> bool {
    parse_marker(&e.action).kind.is_ownership()
}

/// `$valid` — elenmemiş satırlar. Sahiplik satırları burada zaten yoktur.
fn valid_rows(wfah: &Wfah, rules: &ValidRules) -> Vec<Row> {
    wfah.entries()
        .iter()
        .map(|e| row_of(wfah, rules, e))
        .filter(|r| r.reason.is_none())
        .collect()
}

/// Sahiplik DIŞINDAKİ her satır, SEBEBİYLE — sim'in yapısal olarak üretemediği
/// tek sınıf çıkarılır, geri kalan her satır iki yolda da aynı yargıyı almalıdır.
fn judged_rows(wfah: &Wfah, rules: &ValidRules) -> Vec<Row> {
    wfah.entries()
        .iter()
        .filter(|e| !is_ownership(e))
        .map(|e| row_of(wfah, rules, e))
        .collect()
}

fn reason_set(rows: &[Row]) -> BTreeSet<String> {
    rows.iter()
        .filter_map(|r| r.reason.map(|x| format!("{x:?}")))
        .collect()
}

/// Kapının gövdesi. `expected` = bu senaryonun sahiplik DIŞINDA ürettiği sebepler;
/// yazılı olması, senaryonun sessizce "hiç eleme üretmeyen" bir koşuma dönüşmesini
/// (yani kapının yeşil ama BOŞ kalmasını) engeller.
fn assert_valid_parity(wfd: &Wfd, sim: &Wfah, real: &Wfah, expected: &[InvalidReason]) {
    let rules = ValidRules::for_version(wfd);

    assert_eq!(
        valid_rows(sim, &rules),
        valid_rows(real, &rules),
        "$valid kümesi AYRIŞTI: simülasyonda geçerli görünen satır gerçek akışta elenmiş (ya da tersi)"
    );
    assert_eq!(
        judged_rows(sim, &rules),
        judged_rows(real, &rules),
        "invalid_reason AYRIŞTI: aynı satır iki yolda farklı sebeple yargılandı"
    );

    let expected: BTreeSet<String> = expected.iter().map(|r| format!("{r:?}")).collect();
    assert_eq!(
        reason_set(&judged_rows(sim, &rules)),
        expected,
        "senaryonun ürettiği sebep kümesi değişti — kapı beklediği elemeleri artık üretmiyor"
    );

    // Sim'in BİLİNEN körlüğü: sahiplik satırları yalnız gerçek akışta var ve
    // hepsi `Ownership` ile elenir. Elenmeselerdi `$valid` kümeleri de ayrışırdı.
    let real_ownership: Vec<&WfahEntry> = real.entries().iter().filter(|e| is_ownership(e)).collect();
    assert!(
        !real_ownership.is_empty(),
        "gerçek akış claim yazmadıysa körlük hiç sınanmamış olur"
    );
    for e in real_ownership {
        assert_eq!(
            invalid_reason(real, &rules, e),
            Some(InvalidReason::Ownership),
            "sahiplik satırı $valid'e sızdı: {}",
            e.action
        );
    }
    assert!(
        !sim.entries().iter().any(is_ownership),
        "sim claim yazmaya başladıysa bu kapının körlük varsayımı bayatlamıştır"
    );
}

// ---- fixture varyantı ---------------------------------------------------------

/// Hukuk kolunu İKİ ADIMA çıkarır: `hukuk_onay` artık join'e değil kol içindeki
/// `self__legalReviewer` node'una gider (`BranchMoveTo`), oradan `hukuk_son` ile
/// join'e varılır.
///
/// Gerekçe: tek adımlı bir kolda İPTAL EDİLEN kolun `$valid`e giren hiçbir aksiyon
/// satırı olmuyor (kol ya varmıştır → `branch_superseded`, ya da hiç aksiyon
/// almamıştır) — `branch_cancelled` sebebi ancak kol içinde YAŞAYAN bir satır varken
/// doğar. `c_a` yeni bir role bağlanır: aynı canonical `c_a` iki node'da olamaz
/// (Değişmez #4, `duplicate_c_a`).
fn paralel_with_two_step_legal_branch() -> Wfd {
    let mut v: Value = serde_json::from_str(PARALLEL_FIXTURE).unwrap();
    v["nodes"]["self__legalReviewer"] = json!({
        "label": "Hukuk İkinci Okuma",
        "description": "Kol içi ikinci adım — kolun tek adımdan uzun olduğu hâl.",
        "c_a": {"c_orgu": "self", "c_r": ["legalReviewer"]}
    });
    v["actions"]["hukuk_onay"]["wft"] = json!({"node": "self__legalReviewer"});
    v["actions"]["hukuk_son"] = json!({
        "label": "Hukuk Son Onay",
        "description": "Kol içi ikinci adımın onayı — join'e bu satırla varılır.",
        "input": {"required": [], "optional": []},
        "from": "self__legalReviewer",
        "wft": {"node": "self__resultCoordinator"}
    });
    Wfd::from_value(v).unwrap()
}

// ---- kapılar ------------------------------------------------------------------

/// **Kural 1 (kol iptali/geçersizleşmesi).** İK kolu varır (`arrived`), hukuk kolu
/// kol içinde ilerler (hâlâ `active`), finans reddeder → WFE terminal:
/// İK'nın onayı `branch_superseded`, hukukun kol-içi adımı `branch_cancelled`.
///
/// Sim'de bu iki sebep ancak kol etiketleri (`branch_entry`) ve iptal marker'ları
/// deftere EKSİKSİZ düşerse doğar — kapının ilk sorusu bu.
#[tokio::test]
async fn cancelled_and_superseded_branches_are_judged_the_same_in_both_paths() {
    let wfd = paralel_with_two_step_legal_branch();
    let steps = [
        wfe_step("coordinator", "start_review"),
        branch_step("hrApprover", "ik_onay", "self__hrApprover"),
        branch_step("legalApprover", "hukuk_onay", "self__legalApprover"),
        branch_step("financeApprover", "finans_ret", "self__financeApprover"),
    ];
    let sim = run_sim(&wfd, &steps).await;
    let real = run_real(&wfd, &steps).await;
    assert_valid_parity(
        &wfd,
        &sim,
        &real,
        &[InvalidReason::BranchSuperseded, InvalidReason::BranchCancelled],
    );
}

/// **Kural 2 (geri gönderme penceresi).** Finans kolu fork ÖNCESİNE geri gönderir;
/// bu bir COLLAPSE'tır (`_collapse` + `reason: "sent_back"`) ve pencere TÜM kolları
/// kapsar.
///
/// Pencerenin girdisi `to_node`dur: sim o kolonu eksik yazsaydı sol kenar (T'ye son
/// giriş) bulunamaz ve pencere sessizce genişler/daralırdı.
#[tokio::test]
async fn the_send_back_window_is_judged_the_same_in_both_paths() {
    let wfd = paralel_with_send_back_before_fork();
    let steps = [
        wfe_step("coordinator", "start_review"),
        branch_step("legalApprover", "hukuk_onay", "self__legalApprover"),
        Step {
            target: Some("self__coordinator"),
            ..branch_step("financeApprover", "finans_ret", "self__financeApprover")
        },
    ];
    let sim = run_sim(&wfd, &steps).await;
    let real = run_real(&wfd, &steps).await;
    assert_valid_parity(
        &wfd,
        &sim,
        &real,
        &[InvalidReason::SentBackWindow, InvalidReason::BranchSuperseded],
    );
}

/// **Kural 5 (tur).** Aynı fork'a ikinci kez girilir (ret → fork node'una collapse):
/// birinci turun onayı `old_round` ile elenir, ikinci turunki `$valid`de kalır.
///
/// Turun çapası `_fork` satırının `input.branches` payload'ıdır; sim o marker'ı
/// eksik ya da payload'sız yazsaydı iki tur tek turmuş gibi toplanırdı — eşik
/// sayımının v2.3'te düzeltmek için var olduğu hatanın ta kendisi.
#[tokio::test]
async fn the_two_rounds_of_the_same_fork_are_judged_the_same_in_both_paths() {
    let wfd = paralel_with_collapse_to_node();
    let steps = [
        wfe_step("coordinator", "start_review"),
        branch_step("legalApprover", "hukuk_onay", "self__legalApprover"),
        branch_step("financeApprover", "finans_ret", "self__financeApprover"),
        wfe_step("coordinator", "start_review"),
        branch_step("legalApprover", "hukuk_onay", "self__legalApprover"),
    ];
    let sim = run_sim(&wfd, &steps).await;
    let real = run_real(&wfd, &steps).await;
    assert_valid_parity(&wfd, &sim, &real, &[InvalidReason::OldRound]);
}

/// **Adapter İSTİSNASI: `_join` satırı.** Mutlu yol — üç kol da onaylar, join dolar,
/// sonuç koordinatörü akışı bitirir. Hiçbir satır ELENMEZ; kapının buradaki sorusu
/// eleme değil, defterin AYNI satırlardan oluşmasıdır.
///
/// `_join`i motor yazmaz: gerçek akışta adapter (`wfe_adapter.rs`), simülasyonda
/// `SimState::apply_commit` ekler. İki üretici de `wfah_payload::join_row`u çağırır;
/// biri elle kurulsaydı satır sessizce ayrışır ve `count($wfah, …)` ile karar veren
/// akışlar simülasyonda başka cevap alırdı.
#[tokio::test]
async fn the_adapter_written_join_row_matches_the_simulated_one() {
    let wfd = Wfd::from_json(PARALLEL_FIXTURE).unwrap();
    let steps = [
        wfe_step("coordinator", "start_review"),
        branch_step("financeApprover", "finans_onay", "self__financeApprover"),
        branch_step("legalApprover", "hukuk_onay", "self__legalApprover"),
        branch_step("hrApprover", "ik_onay", "self__hrApprover"),
        wfe_step("resultCoordinator", "finalize"),
    ];
    let sim = run_sim(&wfd, &steps).await;
    let real = run_real(&wfd, &steps).await;
    for (path, wfah) in [("sim", &sim), ("gerçek", &real)] {
        assert_eq!(
            wfah.entries().iter().filter(|e| e.action == "_join").count(),
            1,
            "{path} defterinde `_join` satırı yok"
        );
    }
    // Mutlu yolda eleme YOKTUR: `$valid` ham defterin (sahiplik satırları hariç)
    // tamamıdır ve iki yolda da aynı satırlardan oluşur.
    assert_valid_parity(&wfd, &sim, &real, &[]);
}
