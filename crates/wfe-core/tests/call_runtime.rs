//! WFC runtime testleri — outbox (`stage_calls`) ve WFC-RETURN (`fire_call_return`).
//!
//! Store/DB YOK: bunlar saf engine testleridir. Ele alınan davranışlar:
//!   • `wait`   → çağrı node'una girişte outbox satırı; dönüşte `$call.*` ile ilerleme
//!   • `detached` → outbox satırı, ama sonuç beklenmez
//!   • `terminal` → başarılı bitişte ardıl outbox satırı; `Terminated`'da YOK
//!   • WFC-IN çözümü ve süre sınırının mutlak zamana çevrilmesi
//!
//! Executor katmanının işleri (derinlik frenleri, cascade, start_as) `wf-wfe`
//! tarafındadır — burada engine'in ürettiği outbox satırının doğruluğu sınanır.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use uuid::Uuid;
use wfe_core::error::EngineError;
use wfe_core::ports::OrgPort;
use wfe_core::types::actor::{Actor, OrgUnit};
use wfe_core::types::dynctx::DynCtx;
use wfe_core::types::wfah::Wfah;
use wfe_core::types::wfd_v22::{CallMode, JoinRule, StartAs, Wfd};
use wfe_core::types::wfe::WfeStatus;
use wfe_core::v22::pipeline::Engine;
use wfe_core::v22::ports::{AutoexecRunner, CallSite, CommitOutcome, ExecEnv, ExecFailure, Wfes};

const CALLER: &str = include_str!("fixtures/akis-cagrisi.json");

fn caller_wfd() -> Wfd {
    Wfd::from_json(CALLER).unwrap()
}

/// Gerçek `OrgAdapter` gibi davranır: **nil anchor'ı REDDEDER.**
///
/// Bu, testlerin daha önce kaçırdığı hata sınıfını yakalar: engine-tetiklemeli
/// kenarlar (WFC-RETURN, SLA) saf sistem aktörü kullanıyordu ve onun `orgu_id`'si
/// nil'di; hedef node'un kuralı `self`-çapalıysa gerçek ortamda
/// "orgu 00000000-...-0000 bulunamadı" hatası veriyordu. Anchor'ı körü körüne geri
/// veren bir mock bunu göremez.
struct MockOrg;

#[async_trait]
impl OrgPort for MockOrg {
    async fn resolve_c_orgu(
        &self,
        anchor: Uuid,
        _expr: &str,
        _orgtnt: Uuid,
    ) -> Result<Vec<OrgUnit>, EngineError> {
        if anchor.is_nil() {
            return Err(EngineError::OrgPort(format!("not found: orgu {anchor}")));
        }
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
    async fn run(
        &self,
        _def: &wfe_core::types::wfd_v22::AutoexecDef,
        _env: &ExecEnv,
    ) -> Result<Value, ExecFailure> {
        Err(ExecFailure::failed("bu fixture'da autoexec yok"))
    }
}

fn actor(role: &str) -> Actor {
    Actor {
        orgu_id: Uuid::new_v4(),
        user_id: Uuid::new_v4(),
        role: role.into(),
    }
}

/// `assigned`: node'dan aksiyon alınabilmesi için claim gerekir (§7.1). Çağrı dönüşü
/// bunu gerektirmez (system tetikler), ama insan aksiyonu testleri sahibi verir.
fn wfes_owned(node: &str, ctx: Value, owner: Option<Uuid>) -> Wfes {
    let wfah = Wfah::empty().push("basvuru_olustur".into(), actor("branchClerk"), None);
    let created_at = wfah.entries()[0].applied_at;
    Wfes {
        wfe_id: Uuid::new_v4(),
        orgtnt_id: Uuid::nil(),
        wfd_id: Uuid::new_v4(),
        wfd_version: 1,
        dynctx: DynCtx(ctx),
        wfah,
        status: WfeStatus::Active,
        current_node: Some(node.into()),
        assigned_to: owner,
        end_response: None,
        deadline: None,
        claimed_at: owner.map(|_| created_at),
        created_at,
        branches: vec![],
        join_target: None,
        join_rule: JoinRule::All,
    }
}

fn wfes_at(node: &str, ctx: Value) -> Wfes {
    wfes_owned(node, ctx, None)
}

fn ctx_with_basvuru() -> Value {
    json!({
        "basvuru": { "musteri_no": "M-42", "tutar": 50000 },
        "initiated_by": {},
    })
}

// ================================================== outbox: çağrı node'una giriş

/// Start kuralının wft'si doğrudan çağrı node'una gider → outbox satırı üretilmeli.
#[tokio::test]
async fn entering_a_call_node_stages_the_call() {
    let engine = Engine {
        org: &MockOrg,
        exec: &NoRunner,
    };
    let wfd = caller_wfd();
    let a = actor("branchClerk");
    let new = engine
        .start(
            &wfd,
            &a,
            Uuid::nil(),
            Some("basvuru_olustur"),
            &json!({ "basvuru": { "musteri_no": "M-42", "tutar": 50000 } }),
            Uuid::new_v4(),
            None,
        )
        .await
        .expect("start başarılı olmalı");

    assert!(
        matches!(&new.outcome, CommitOutcome::MoveTo { node } if node == "self__creditAnalyst"),
        "çağrı node'una taşınmalı, outcome: {:?}",
        new.outcome
    );
    assert_eq!(new.staged_calls.len(), 1, "tek outbox satırı beklenir");
    let call = &new.staged_calls[0];
    assert_eq!(call.call_key, "kredi_skor_sorgusu");
    assert_eq!(call.mode, CallMode::Wait);
    assert_eq!(call.site, CallSite::Node("self__creditAnalyst".into()));

    // WFC-IN: `$ctx.*` kaynakları ÇÖZÜLMÜŞ olarak taşınır — çağrılan ham ifade almaz.
    assert_eq!(call.input["musteri_no"], json!("M-42"));
    assert_eq!(call.input["talep_tutari"], json!(50000));

    // `timeout: "P2D"` mutlak zamana çevrilir (her tick'te ISO parse edilmemesi için).
    let deadline = call.deadline.expect("wait modunda süre sınırı çözülmeli");
    let delta = deadline - Utc::now();
    assert!(
        delta.num_hours() >= 47 && delta.num_hours() <= 48,
        "P2D ~48 saat olmalı, hesaplanan: {} saat",
        delta.num_hours()
    );
}

/// `detached`: çağrı yapılır ama süre sınırı ANLAMSIZDIR (sonuç beklenmiyor).
#[tokio::test]
async fn detached_mode_stages_without_deadline() {
    let mut wfd = caller_wfd();
    {
        let node = wfd.nodes.get_mut("self__creditAnalyst").unwrap();
        let call = node.call.as_mut().unwrap();
        call.mode = CallMode::Detached;
    }
    let engine = Engine {
        org: &MockOrg,
        exec: &NoRunner,
    };
    let new = engine
        .start(
            &wfd,
            &actor("branchClerk"),
            Uuid::nil(),
            Some("basvuru_olustur"),
            &json!({ "basvuru": { "musteri_no": "M-1", "tutar": 10 } }),
            Uuid::new_v4(),
            None,
        )
        .await
        .unwrap();
    let call = &new.staged_calls[0];
    assert_eq!(call.mode, CallMode::Detached);
    assert!(
        call.deadline.is_none(),
        "detached modda süre sınırı çözülmemeli — sonuç hiç beklenmiyor"
    );
}

// ================================================== WFC-RETURN

/// Çağrılan `completed` döndü ve skor eşiği geçti → müdür havuzuna ilerlemeli,
/// `$call.result.*` ctx'e yazılmalı.
#[tokio::test]
async fn successful_return_writes_result_and_routes_on_it() {
    let engine = Engine {
        org: &MockOrg,
        exec: &NoRunner,
    };
    let wfd = caller_wfd();
    let wfes = wfes_at("self__creditAnalyst", ctx_with_basvuru());
    let callee = Uuid::new_v4();

    let commit = engine
        .fire_call_return(
            &wfd,
            &wfes,
            "completed",
            Some(callee),
            Some(&json!({ "skor": 780, "karar": "uygun" })),
            &[],
            Utc::now(),
        )
        .await
        .expect("dönüş uygulanmalı");

    assert!(
        matches!(&commit.outcome, CommitOutcome::MoveTo { node } if node == "self__branchManager"),
        "skor 780 → müdür havuzuna gitmeli, outcome: {:?}",
        commit.outcome
    );
    assert_eq!(commit.new_dynctx["skor"], json!(780));
    assert_eq!(commit.new_dynctx["skor_durumu"], json!("uygun"));

    // Dönüş bir insan ACT'i DEĞİLDİR: WFAH'a `call:<key>` marker'ı düşer.
    let marker = &commit.wfah_entries[0];
    assert_eq!(marker.action, "call:kredi_skor_sorgusu");
    assert_eq!(marker.actor.role, "system");
    assert_eq!(marker.input.as_ref().unwrap()["status"], json!("completed"));
}

/// Skor eşiğin altında → `default` hedefe (red terminali) gitmeli.
#[tokio::test]
async fn low_score_return_routes_to_default_target() {
    let engine = Engine {
        org: &MockOrg,
        exec: &NoRunner,
    };
    let commit = engine
        .fire_call_return(
            &caller_wfd(),
            &wfes_at("self__creditAnalyst", ctx_with_basvuru()),
            "completed",
            Some(Uuid::new_v4()),
            Some(&json!({ "skor": 400, "karar": "uygun_degil" })),
            &[],
            Utc::now(),
        )
        .await
        .unwrap();
    assert!(
        matches!(&commit.outcome, CommitOutcome::Terminal { .. }),
        "düşük skor terminale gitmeli, outcome: {:?}",
        commit.outcome
    );
}

/// Süre aşımı / hata da bir dönüştür: akış `$call.status` ile karar verir ve
/// ÇÖKMEZ. Fixture'ın ilk koşulu tam bunu yakalar.
#[tokio::test]
async fn timeout_return_is_a_normal_decision_not_a_crash() {
    let engine = Engine {
        org: &MockOrg,
        exec: &NoRunner,
    };
    for status in ["timeout", "failed", "terminated"] {
        let commit = engine
            .fire_call_return(
                &caller_wfd(),
                &wfes_at("self__creditAnalyst", ctx_with_basvuru()),
                status,
                None,
                None,
                &[],
                Utc::now(),
            )
            .await
            .unwrap_or_else(|e| panic!("'{status}' dönüşü uygulanmalı, hata: {e}"));
        assert!(
            matches!(&commit.outcome, CommitOutcome::Terminal { .. }),
            "'{status}' → ilk koşul (status != completed) terminale götürmeli"
        );
        // Sonuç yoksa alan `null` yazılır — sessizce eski değer KALMAZ.
        assert_eq!(commit.new_dynctx["skor"], Value::Null);
    }
}

/// Çağrı node'u olmayan bir node'da dönüş uygulanamaz — sessiz geçiş yerine hata.
#[tokio::test]
async fn return_on_a_non_call_node_is_rejected() {
    let engine = Engine {
        org: &MockOrg,
        exec: &NoRunner,
    };
    let err = engine
        .fire_call_return(
            &caller_wfd(),
            &wfes_at("self__branchManager", ctx_with_basvuru()),
            "completed",
            None,
            None,
            &[],
            Utc::now(),
        )
        .await
        .expect_err("çağrı node'u olmayan node'da dönüş reddedilmeli");
    assert!(matches!(err, EngineError::InvalidWfd(_)), "hata: {err}");
}

// ================================================== ardıl (mode: terminal)

/// Başarılı bitiş ardıl çağrıyı stage eder; sıra kesindir: terminal effects
/// uygulanmış ctx'e göre WFC-IN çözülür.
#[tokio::test]
async fn successful_terminal_stages_the_successor_call() {
    let engine = Engine {
        org: &MockOrg,
        exec: &NoRunner,
    };
    let wfd = caller_wfd();
    let mut ctx = ctx_with_basvuru();
    ctx["kullandirim_tutari"] = json!(25000);
    let boss = actor("branchManager");
    let wfes = wfes_owned("self__branchManager", ctx, Some(boss.user_id));

    let commit = engine
        .apply(
            &wfd,
            &wfes,
            &boss,
            "manager_decide",
            &json!({ "kullandirim_tutari": 25000 }),
            None,
        )
        .await
        .expect("müdür kararı uygulanmalı");

    assert!(matches!(&commit.outcome, CommitOutcome::Terminal { .. }));
    assert_eq!(commit.staged_calls.len(), 1, "ardıl çağrı stage edilmeli");
    let next = &commit.staged_calls[0];
    assert_eq!(next.call_key, "kredi_kullandirim");
    assert_eq!(next.mode, CallMode::Terminal);
    assert_eq!(next.site, CallSite::Terminal("terminal_approved".into()));
    assert_eq!(next.start_as, StartAs::System);
    // Ardılda dönüş yoktur → süre sınırı da yoktur.
    assert!(next.deadline.is_none());
    // WFC-IN, terminal effects UYGULANDIKTAN SONRAKİ ctx'e göre çözülür.
    assert_eq!(next.input["musteri_no"], json!("M-42"));
    assert_eq!(next.input["tutar"], json!(25000));
}

/// Ardıl çağrı taşımayan terminal outbox satırı üretmez.
#[tokio::test]
async fn terminal_without_a_successor_stages_nothing() {
    let engine = Engine {
        org: &MockOrg,
        exec: &NoRunner,
    };
    let boss = actor("branchManager");
    let commit = engine
        .apply(
            &caller_wfd(),
            &wfes_owned(
                "self__branchManager",
                ctx_with_basvuru(),
                Some(boss.user_id),
            ),
            &boss,
            "manager_decide",
            &json!({ "kullandirim_tutari": 0 }),
            None,
        )
        .await
        .unwrap();
    // tutar 0 → default hedef = terminal_rejected (ardıl çağrısı YOK)
    assert!(matches!(&commit.outcome, CommitOutcome::Terminal { .. }));
    assert!(
        commit.staged_calls.is_empty(),
        "ardıl taşımayan terminal çağrı stage etmemeli"
    );
}

/// SLA-3 ile sonlanma `Terminated`'dır — BAŞARILI bitiş değil, ardıl TETİKLEMEZ.
/// Bu, "ardılın üç sert kuralı"ndan biridir (decisions.md → WFC).
#[tokio::test]
async fn sla_deadline_termination_does_not_trigger_the_successor() {
    let engine = Engine {
        org: &MockOrg,
        exec: &NoRunner,
    };
    let mut wfes = wfes_at("self__branchManager", ctx_with_basvuru());
    wfes.deadline = Some(Utc::now() - chrono::Duration::seconds(1));

    let commit = engine.fire_deadline_timeout(&wfes, Utc::now());
    assert!(matches!(&commit.outcome, CommitOutcome::Terminated { .. }));
    assert!(
        commit.staged_calls.is_empty(),
        "zaman aşımıyla sonlanma ardıl başlatmamalı"
    );
}

// ============================================== çapa (anchor) regresyonu

/// WFC-RETURN, hedef node'un `c_orgu`'su `self`-çapalıyken çözebilmeli.
///
/// Regresyon: dönüşü saf sistem aktörü (nil `orgu_id`) ile çözüyordu → gerçek org
/// adapter'ında "orgu 00000000-...-0000 bulunamadı" ile patlıyordu; çağıran o node'da
/// sonsuza kadar bekliyor, sweeper her turda aynı hatayı logluyordu. Artık çapa
/// WFAH'taki son gerçek aktörün birimidir (`system_actor_anchored`).
#[tokio::test]
async fn call_return_resolves_self_anchored_target() {
    let engine = Engine {
        org: &MockOrg,
        exec: &NoRunner,
    };
    // Fixture'ın çağrı node'u `self__branchManager`'a (c_orgu: "self") gidiyor.
    let wfes = wfes_at("self__creditAnalyst", ctx_with_basvuru());
    // WFAH'ta gerçek bir aktör var (başlatan memur) — çapa oradan gelir.
    assert!(
        !wfes.wfah.entries()[0].actor.orgu_id.is_nil(),
        "test kurulumu: WFAH'ta gerçek aktör olmalı"
    );

    let commit = engine
        .fire_call_return(
            &caller_wfd(),
            &wfes,
            "completed",
            Some(Uuid::new_v4()),
            Some(&json!({ "skor": 800, "karar": "uygun" })),
            &[],
            Utc::now(),
        )
        .await
        .expect("self-çapalı hedef çözülebilmeli");

    assert!(
        matches!(&commit.outcome, CommitOutcome::MoveTo { node } if node == "self__branchManager")
    );
    // Aday listesi gerçekten doldu — çapa çözüldü demektir.
    assert!(
        !commit.resolved_c_a.is_empty(),
        "hedef node'un aday listesi boş kalmamalı"
    );
    // Audit izi DEĞİŞMEDİ: marker hâlâ system rolüyle yazılır.
    assert_eq!(commit.wfah_entries[0].actor.role, "system");
}

/// WFAH'ta hiç gerçek aktör yoksa çapa bulunamaz ve hata ANLAŞILIR biçimde yüzeye
/// çıkar — sessizce boş aday listesiyle devam edilmez.
#[tokio::test]
async fn call_return_without_any_real_actor_fails_loudly() {
    let engine = Engine {
        org: &MockOrg,
        exec: &NoRunner,
    };
    let mut wfes = wfes_at("self__creditAnalyst", ctx_with_basvuru());
    // Tüm WFAH aktörlerini nil'e çek (gerçekte oluşmaz; savunma testi).
    for e in wfes.wfah.0.iter_mut() {
        e.actor.orgu_id = Uuid::nil();
    }
    let err = engine
        .fire_call_return(
            &caller_wfd(),
            &wfes,
            "completed",
            None,
            Some(&json!({ "skor": 800, "karar": "uygun" })),
            &[],
            Utc::now(),
        )
        .await
        .expect_err("çapa yoksa hata dönmeli");
    assert!(matches!(err, EngineError::OrgPort(_)), "hata: {err}");
}

// ====================================== alt akış geçmişinin çağırana işlenmesi

/// Çağrılanın WFAH'ı — gerçekte çağrı node'una girildikten SONRA, dönüş işlenmeden
/// ÖNCE oluşur. Zaman damgaları bu aralığa yerleştirilir.
fn callee_history(base: DateTime<Utc>) -> Vec<wfe_core::types::wfah::WfahEntry> {
    use wfe_core::types::wfah::WfahEntry;
    let uzman = actor("riskAnalyst");
    vec![
        WfahEntry {
            seq: 1,
            action: "skor_talebi_olustur".into(),
            actor: actor("branchClerk"),
            input: Some(json!({ "musteri_no": "M-1" })),
            applied_at: base + chrono::Duration::minutes(5),
        },
        WfahEntry {
            seq: 2,
            action: "skor_gir".into(),
            actor: uzman,
            input: Some(json!({ "skor": 780 })),
            applied_at: base + chrono::Duration::minutes(40),
        },
    ]
}

/// Çekirdek gereksinim: alt akışın geçmişi çağıranın geçmişine işlenir ve
/// **tarihsel akış bozulmaz** — `seq` artışı `applied_at` artışıyla uyumlu kalır.
#[tokio::test]
async fn callee_history_is_inlined_in_chronological_order() {
    let engine = Engine {
        org: &MockOrg,
        exec: &NoRunner,
    };
    let wfes = wfes_at("self__creditAnalyst", ctx_with_basvuru());
    let base = wfes.wfah.entries()[0].applied_at;
    let history = callee_history(base);
    let callee = Uuid::new_v4();

    let commit = engine
        .fire_call_return(
            &caller_wfd(),
            &wfes,
            "completed",
            Some(callee),
            Some(&json!({ "skor": 780, "karar": "uygun" })),
            &history,
            base + chrono::Duration::hours(1),
        )
        .await
        .unwrap();

    // Çağıranın TAM geçmişi: mevcut kayıtlar + bu commit'in stage ettikleri.
    let mut full = wfes.wfah.entries().to_vec();
    full.extend(commit.wfah_entries.iter().cloned());

    // 1. `seq` kesintisiz ve artan.
    for pair in full.windows(2) {
        assert_eq!(
            pair[1].seq,
            pair[0].seq + 1,
            "seq kesintisiz artmalı: {:?}",
            full.iter().map(|e| (e.seq, &e.action)).collect::<Vec<_>>()
        );
    }
    // 2. ZAMAN da artan — asıl gereksinim. Alt akış satırları sona eklenseydi
    //    kapanış marker'ından sonra daha ESKİ damgalar gelirdi.
    for pair in full.windows(2) {
        assert!(
            pair[1].applied_at >= pair[0].applied_at,
            "tarihsel akış bozuldu: '{}' ({}) sonra '{}' ({})",
            pair[0].action,
            pair[0].applied_at,
            pair[1].action,
            pair[1].applied_at
        );
    }
    // 3. Sıra: alt akış satırları kapanış marker'ından ÖNCE.
    let actions: Vec<&str> = commit
        .wfah_entries
        .iter()
        .map(|e| e.action.as_str())
        .collect();
    assert_eq!(
        actions,
        vec![
            "call:kredi_skor_sorgusu/skor_talebi_olustur",
            "call:kredi_skor_sorgusu/skor_gir",
            "call:kredi_skor_sorgusu",
        ]
    );
}

/// Denetimin asıl değeri: işi KİMİN yaptığı ve NE ZAMAN yaptığı korunur.
#[tokio::test]
async fn inlined_entries_keep_original_actor_and_timestamp() {
    let engine = Engine {
        org: &MockOrg,
        exec: &NoRunner,
    };
    let wfes = wfes_at("self__creditAnalyst", ctx_with_basvuru());
    let base = wfes.wfah.entries()[0].applied_at;
    let history = callee_history(base);

    let commit = engine
        .fire_call_return(
            &caller_wfd(),
            &wfes,
            "completed",
            Some(Uuid::new_v4()),
            Some(&json!({ "skor": 780, "karar": "uygun" })),
            &history,
            base + chrono::Duration::hours(1),
        )
        .await
        .unwrap();

    let skor_gir = &commit.wfah_entries[1];
    assert_eq!(skor_gir.actor.role, "riskAnalyst", "özgün aktör korunmalı");
    assert_eq!(skor_gir.actor.user_id, history[1].actor.user_id);
    assert_eq!(
        skor_gir.applied_at, history[1].applied_at,
        "özgün zaman korunmalı"
    );
    // Provenance: hangi WFE'nin kaçıncı kaydı olduğu ve özgün girdi.
    let input = skor_gir.input.as_ref().unwrap();
    assert_eq!(input["callee_seq"], json!(2));
    assert_eq!(input["action"], json!("skor_gir"));
    assert_eq!(input["input"]["skor"], json!(780));
}

/// Ad-alanı: alt akışın aksiyonu çağıranın `$wfah` ifadelerinde KAZARA eşleşmemeli.
#[tokio::test]
async fn inlined_actions_are_namespaced_to_avoid_wfah_collisions() {
    let engine = Engine {
        org: &MockOrg,
        exec: &NoRunner,
    };
    let wfes = wfes_at("self__creditAnalyst", ctx_with_basvuru());
    let base = wfes.wfah.entries()[0].applied_at;
    // Çağrılanın aksiyonu, çağıranın kendi aksiyonuyla AYNI ada sahip.
    let clash = vec![wfe_core::types::wfah::WfahEntry {
        seq: 1,
        action: "manager_decide".into(),
        actor: actor("branchManager"),
        input: None,
        applied_at: base + chrono::Duration::minutes(5),
    }];

    let commit = engine
        .fire_call_return(
            &caller_wfd(),
            &wfes,
            "completed",
            Some(Uuid::new_v4()),
            Some(&json!({ "skor": 780, "karar": "uygun" })),
            &clash,
            base + chrono::Duration::hours(1),
        )
        .await
        .unwrap();

    assert!(
        !commit
            .wfah_entries
            .iter()
            .any(|e| e.action == "manager_decide"),
        "alt akış aksiyonu ham adla eklenmemeli — çağıranın wfah sorgularını kirletir"
    );
    assert_eq!(
        commit.wfah_entries[0].action,
        "call:kredi_skor_sorgusu/manager_decide"
    );
}

/// Uzun alt akışlar çağıranın WFAH'ını şişirmemeli, ama kırpma SESSİZ olmamalı.
#[tokio::test]
async fn long_callee_history_is_truncated_with_an_explicit_marker() {
    use wfe_core::types::wfah::WfahEntry;
    let engine = Engine {
        org: &MockOrg,
        exec: &NoRunner,
    };
    let wfes = wfes_at("self__creditAnalyst", ctx_with_basvuru());
    let base = wfes.wfah.entries()[0].applied_at;
    let history: Vec<WfahEntry> = (1..=250)
        .map(|i| WfahEntry {
            seq: i,
            action: format!("adim_{i}"),
            actor: actor("riskAnalyst"),
            input: None,
            applied_at: base + chrono::Duration::seconds(i as i64),
        })
        .collect();

    let commit = engine
        .fire_call_return(
            &caller_wfd(),
            &wfes,
            "completed",
            Some(Uuid::new_v4()),
            Some(&json!({ "skor": 780, "karar": "uygun" })),
            &history,
            base + chrono::Duration::hours(1),
        )
        .await
        .unwrap();

    // 100 satır + kırpma marker'ı + kapanış marker'ı
    assert_eq!(commit.wfah_entries.len(), 102);
    let trunc = &commit.wfah_entries[100];
    assert_eq!(trunc.action, "call:kredi_skor_sorgusu/…");
    assert_eq!(trunc.input.as_ref().unwrap()["omitted"], json!(150));
    // Kırpılmış hâlde bile zaman sırası korunur.
    for pair in commit.wfah_entries.windows(2) {
        assert!(pair[1].applied_at >= pair[0].applied_at);
    }
}

/// Çağrılan yüklenemediyse (silinmiş/iptal) dönüş yine uygulanır — geçmiş eksik
/// kalır ama akış TIKANMAZ.
#[tokio::test]
async fn empty_callee_history_still_applies_the_return() {
    let engine = Engine {
        org: &MockOrg,
        exec: &NoRunner,
    };
    let commit = engine
        .fire_call_return(
            &caller_wfd(),
            &wfes_at("self__creditAnalyst", ctx_with_basvuru()),
            "completed",
            Some(Uuid::new_v4()),
            Some(&json!({ "skor": 780, "karar": "uygun" })),
            &[],
            Utc::now(),
        )
        .await
        .unwrap();
    assert_eq!(commit.wfah_entries.len(), 1);
    assert_eq!(commit.wfah_entries[0].action, "call:kredi_skor_sorgusu");
}

// ====================================== çağrı node'unda claim yoktur

/// WFC node'u bir bekleme HAVUZU değildir: buradan aksiyon alınamaz, dolayısıyla
/// claim de anlamsızdır. `c_a` burada yalnız GÖRÜNÜRLÜK verir.
///
/// Regresyon: node'un c_a'sına uyan biri işi claim edip havuzdan çekebiliyordu ama
/// hiçbir şey yapamıyordu — üstelik dönüş commit'i assignment'ı zaten sıfırlıyor.
#[tokio::test]
async fn claim_is_refused_on_a_call_node() {
    use wfe_core::v22::pipeline::ClaimCheck;
    let engine = Engine {
        org: &MockOrg,
        exec: &NoRunner,
    };
    let wfd = caller_wfd();
    let wfes = wfes_at("self__creditAnalyst", ctx_with_basvuru());
    // Node'un c_a'sına TAM uyan aktör — yine de claim edemez.
    let check = engine
        .can_claim(&wfd, &wfes, &actor("creditAnalyst"), None)
        .await
        .unwrap();
    assert_eq!(check, ClaimCheck::CallInProgress);
}

/// Görünürlük kapısı da AYNI kuralı uygular — aksi halde portal "Claim et"
/// düğmesini gösterir, kullanıcı tıklar ve claim reddedilir.
#[tokio::test]
async fn claim_button_is_hidden_on_a_call_node() {
    use wfe_core::v22::matcher::AuthDecision;
    let engine = Engine {
        org: &MockOrg,
        exec: &NoRunner,
    };
    let decision = engine
        .claim_decision(
            &caller_wfd(),
            &wfes_at("self__creditAnalyst", ctx_with_basvuru()),
            &actor("creditAnalyst"),
            None,
        )
        .await
        .unwrap();
    assert_eq!(decision, AuthDecision::Denied);
}

/// Muafiyet çağrı node'una ÖZGÜ: normal havuzlarda claim davranışı değişmedi.
#[tokio::test]
async fn claim_still_works_on_a_normal_pool_node() {
    use wfe_core::v22::pipeline::ClaimCheck;
    let engine = Engine {
        org: &MockOrg,
        exec: &NoRunner,
    };
    let check = engine
        .can_claim(
            &caller_wfd(),
            &wfes_at("self__branchManager", ctx_with_basvuru()),
            &actor("branchManager"),
            None,
        )
        .await
        .unwrap();
    assert_eq!(check, ClaimCheck::Ok);
}
