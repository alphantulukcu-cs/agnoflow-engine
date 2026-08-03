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
use chrono::Utc;
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
            Utc::now(),
        )
        .await
        .expect_err("çapa yoksa hata dönmeli");
    assert!(matches!(err, EngineError::OrgPort(_)), "hata: {err}");
}
