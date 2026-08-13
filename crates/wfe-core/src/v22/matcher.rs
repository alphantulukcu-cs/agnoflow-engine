//! §3 C_A Authorization Matcher (WOR-37) — kanonik semantik:
//!
//! ```text
//! match(rule, actor, wfe) :=
//!   actor.orgu ∈ resolve(rule.c_orgu, wfe)
//!   AND ( (rule.c_r var ve actor.role ∈ rule.c_r ve rol ataması doğrulanır)
//!         OR (rule.c_u var ve actor.user ∈ rule.c_u) )   # rol-agnostik
//! ```
//!
//! Verilmeyen alan false'dur (wildcard değil). Bu matcher node c_a, start `from`
//! node'unun c_a'sı, transition ek-kısıt c_a ve listable[].c_a için AYNIDIR.

use crate::error::EngineError;
use crate::ports::OrgPort;
use crate::types::actor::Actor;
use crate::types::wfah::Wfah;
use crate::types::wfd_v22::{CandidateActor, CuItem};
use crate::v22::resolver::{resolve_c_orgu, resolve_cu_ident};
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

/// Matcher'ın ihtiyaç duyduğu WFE bağlamı.
#[derive(Clone, Copy)]
pub struct MatchEnv<'a> {
    pub ctx: &'a Value,
    pub wfah: &'a Wfah,
    pub orgtnt_id: Uuid,
}

pub async fn authorize(
    rule: &CandidateActor,
    actor: &Actor,
    env: MatchEnv<'_>,
    org: &dyn OrgPort,
) -> Result<bool, EngineError> {
    authorize_anchored(rule, actor, None, env, org).await
}

/// `authorize`ın çapa (ORGTRVLANG `self`/`parent`/… başlangıcı) ÜSTÜNDEN
/// belirlenebilen hâli (2026-08-13).
///
/// `anchor = None` → aktörün kendi birimi (node `c_a`'sının davranışı: kural
/// "geçişi yapanın konumuna göre" çözülür).
///
/// `anchor = Some(u)` → verilen birim. `listable`/`wf_admin` grant'ları bunu
/// WFE'nin kendi birimiyle (`origin_orgu_id`) kullanır. Aksi halde `self` gibi
/// bir selector, çapası SORAN KİŞİ olduğu için birim karşılaştırmasını
/// kendisiyle yapar ve daima true döner — yani kural sessizce "tenant'taki o
/// roldeki herkes"e dönüşür. Aynı sebeple viewer'a bağlı bir grant
/// PROJEKSİYONA da yazılamaz (`Engine::view_grants`); iki okumanın aynı cevabı
/// vermesi bu parametreye bağlıdır.
pub async fn authorize_anchored(
    rule: &CandidateActor,
    actor: &Actor,
    anchor: Option<Uuid>,
    env: MatchEnv<'_>,
    org: &dyn OrgPort,
) -> Result<bool, EngineError> {
    let anchor = anchor.unwrap_or(actor.orgu_id);
    // 1. ORGU kanalı: actor.orgu resolve edilen kümede olmalı.
    //    `c_orgu` HİÇ verilmemişse (çapasız biçim) bu kanal kısıtsızdır — kural yalnız
    //    kişi kanalıyla yaşar (aşağıda 3), kapı "şu kişi, hangi birimde olursa olsun".
    if let Some(c_orgu) = &rule.c_orgu {
        let resolved =
            resolve_c_orgu(c_orgu, anchor, env.ctx, env.wfah, env.orgtnt_id, org).await?;
        if !resolved.iter().any(|u| u.orgu_id == actor.orgu_id) {
            return Ok(false);
        }
    }

    // 2. Rol kanalı: c_r listesinde VE gerçekten atanmış.
    //    ÇAPASIZ kuralda rol kanalı HİÇ sorulmaz: çapasız bir rol grantı ("tenant'taki tüm
    //    müdürler") kazara kurulabilecek en geniş kapıdır. Şema ve validator o belgeyi
    //    reddeder; matcher ayrıca reddeder ki elle yazılmış/eski bir kayıt bir yolla
    //    sızarsa kapı yine açılmasın (savunma katmanı, tek noktaya güvenmiyoruz).
    if let (Some(c_r), true) = (&rule.c_r, rule.c_orgu.is_some()) {
        if c_r.iter().any(|r| r == &actor.role)
            && org
                .check_user_role(actor.user_id, actor.orgu_id, &actor.role)
                .await?
        {
            return Ok(true);
        }
    }

    // 3. Kullanıcı kanalı: rol-agnostik — username veya UUID string eşleşmesi.
    //    Öğeler sabit kimlik (`Literal`) ya da context referansı (`Ref`) olabilir; referans
    //    çalışma anında çözülür. Çözülemeyen referans sessizce eşleşmez (hata değil) —
    //    `$ctx`'in "eksik = null" sözleşmesi; `c_r` kanalı bundan etkilenmez.
    if let Some(c_u) = &rule.c_u {
        let uuid_str = actor.user_id.to_string();
        let wanted: Vec<String> = c_u
            .iter()
            .filter_map(|item| match item {
                CuItem::Literal(s) => Some(s.clone()),
                CuItem::Ref { from } => resolve_cu_ident(from, env.ctx),
            })
            .collect();
        if wanted.iter().any(|u| u == &uuid_str) {
            return Ok(true);
        }
        // `user_ident` I/O'dur — yalnız UUID eşleşmesi tutmadıysa sorulur.
        if !wanted.is_empty() {
            if let Some(ident) = org.user_ident(actor.user_id).await? {
                if wanted.iter().any(|u| u == &ident) {
                    return Ok(true);
                }
            }
        }
    }

    Ok(false)
}

/// Madde 6: yetki kararı — doğrudan mı, vekaleten mi (audit için provenance taşır).
#[derive(Debug, Clone, PartialEq)]
pub enum AuthDecision {
    Denied,
    /// Aktör kurala DOĞRUDAN uyuyor (bugünkü davranış).
    Direct,
    /// Aktör kurala VEKALETEN uyuyor: bir vekâlet vereni (delegator) temsil ediyor.
    Delegated {
        delegation_id: Uuid,
        delegator_user_id: Uuid,
        seat_orgu_id: Uuid,
        seat_role: String,
    },
}

impl AuthDecision {
    pub fn is_authorized(&self) -> bool {
        !matches!(self, AuthDecision::Denied)
    }
}

/// Madde 6: vekalet-farkında yetkilendirme. Önce doğrudan `authorize`; olmazsa
/// claimant'a o an geçerli her vekâlet için (a) claimant `grantee`'ye uyuyor mu VE
/// (b) delegator'ın koltuğunu temsil eden SENTETİK aktör kurala uyuyor mu.
///
/// İki iç çağrı da düz `authorize`'dır (vekalet-farkında DEĞİL) → tek seviye; zincir
/// (transitif vekalet) oluşmaz. `grantee` claimant'ın kendi anchor'ıyla değerlendirilir
/// (kişi = c_u eşleşmesi; havuz = claimant'ın rol/orgu'su).
pub async fn authorize_with_delegation(
    rule: &CandidateActor,
    actor: &Actor,
    env: MatchEnv<'_>,
    org: &dyn OrgPort,
    now: DateTime<Utc>,
) -> Result<AuthDecision, EngineError> {
    authorize_with_delegation_anchored(rule, actor, None, env, org, now).await
}

/// `authorize_with_delegation`ın çapa üstünden belirlenebilen hâli — bkz.
/// `authorize_anchored`. Vekâlet kanalında `grantee` kuralı DAİMA aktörün kendi
/// birimine çapalanır (o kural "vekili kim" sorusudur, WFE'nin birimiyle ilgisi
/// yoktur); çapa yalnız ASIL kurala uygulanır.
pub async fn authorize_with_delegation_anchored(
    rule: &CandidateActor,
    actor: &Actor,
    anchor: Option<Uuid>,
    env: MatchEnv<'_>,
    org: &dyn OrgPort,
    now: DateTime<Utc>,
) -> Result<AuthDecision, EngineError> {
    if authorize_anchored(rule, actor, anchor, env, org).await? {
        return Ok(AuthDecision::Direct);
    }
    let grants = org
        .active_delegations_for(actor.user_id, env.orgtnt_id, now)
        .await?;
    for g in grants {
        // (a) claimant gerçekten bu vekâletin alıcısı mı? (kişi veya havuz)
        if !authorize(&g.grantee, actor, env, org).await? {
            continue;
        }
        // (b) delegator'ın koltuğu bu kurala uyuyor mu? (sentetik aktör)
        let synthetic = Actor {
            orgu_id: g.seat_orgu_id,
            user_id: g.delegator_user_id,
            role: g.seat_role.clone(),
        };
        if authorize_anchored(rule, &synthetic, anchor, env, org).await? {
            return Ok(AuthDecision::Delegated {
                delegation_id: g.delegation_id,
                delegator_user_id: g.delegator_user_id,
                seat_orgu_id: g.seat_orgu_id,
                seat_role: g.seat_role,
            });
        }
    }
    Ok(AuthDecision::Denied)
}

/// `authorize_with_delegation`'ın bool kısayolu — provenance gerekmeyen çağıranlar
/// (visibility, listable, field x-visibility) için. `now = Utc::now()`.
pub async fn authorize_or_delegated(
    rule: &CandidateActor,
    actor: &Actor,
    env: MatchEnv<'_>,
    org: &dyn OrgPort,
) -> Result<bool, EngineError> {
    authorize_or_delegated_anchored(rule, actor, None, env, org).await
}

/// `authorize_or_delegated`ın çapalı hâli — `listable`/`wf_admin` grant'ları
/// (bkz. `authorize_anchored`).
pub async fn authorize_or_delegated_anchored(
    rule: &CandidateActor,
    actor: &Actor,
    anchor: Option<Uuid>,
    env: MatchEnv<'_>,
    org: &dyn OrgPort,
) -> Result<bool, EngineError> {
    Ok(
        authorize_with_delegation_anchored(rule, actor, anchor, env, org, Utc::now())
            .await?
            .is_authorized(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::actor::OrgUnit;
    use crate::types::wfd_v22::COrgu;
    use async_trait::async_trait;
    use serde_json::json;

    struct MockOrg {
        units: Vec<OrgUnit>,
        role_assigned: bool,
        ident: Option<String>,
    }

    #[async_trait]
    impl OrgPort for MockOrg {
        async fn resolve_c_orgu(
            &self,
            _: Uuid,
            _: &str,
            _: Uuid,
        ) -> Result<Vec<OrgUnit>, EngineError> {
            Ok(self.units.clone())
        }
        async fn check_user_role(&self, _: Uuid, _: Uuid, _: &str) -> Result<bool, EngineError> {
            Ok(self.role_assigned)
        }
        async fn orgtnt_for_orgu(&self, _: Uuid) -> Result<Uuid, EngineError> {
            Ok(Uuid::nil())
        }
        async fn user_ident(&self, _: Uuid) -> Result<Option<String>, EngineError> {
            Ok(self.ident.clone())
        }
    }

    fn unit(id: Uuid) -> OrgUnit {
        OrgUnit {
            orgu_id: id,
            orgu_type: json!({}),
            path: "1".into(),
        }
    }

    fn rule(c_r: Option<Vec<&str>>, c_u: Option<Vec<&str>>) -> CandidateActor {
        CandidateActor {
            c_orgu: Some(COrgu::Selector("self".into())),
            c_r: c_r.map(|v| v.into_iter().map(String::from).collect()),
            c_u: c_u.map(|v| v.into_iter().map(|x| CuItem::Literal(x.into())).collect()),
        }
    }

    fn actor(orgu: Uuid, role: &str) -> Actor {
        Actor {
            orgu_id: orgu,
            user_id: Uuid::new_v4(),
            role: role.into(),
        }
    }

    fn env<'a>(ctx: &'a Value, wfah: &'a Wfah) -> MatchEnv<'a> {
        MatchEnv {
            ctx,
            wfah,
            orgtnt_id: Uuid::nil(),
        }
    }

    static EMPTY_CTX: std::sync::LazyLock<Value> = std::sync::LazyLock::new(|| json!({}));
    static EMPTY_WFAH: std::sync::LazyLock<Wfah> = std::sync::LazyLock::new(Wfah::empty);

    #[tokio::test]
    async fn role_match_with_assignment_succeeds() {
        let orgu = Uuid::new_v4();
        let org = MockOrg {
            units: vec![unit(orgu)],
            role_assigned: true,
            ident: None,
        };
        let a = actor(orgu, "creditAnalyst");
        assert!(authorize(
            &rule(Some(vec!["creditAnalyst"]), None),
            &a,
            env(&EMPTY_CTX, &EMPTY_WFAH),
            &org
        )
        .await
        .unwrap());
    }

    #[tokio::test]
    async fn role_match_without_assignment_fails() {
        let orgu = Uuid::new_v4();
        let org = MockOrg {
            units: vec![unit(orgu)],
            role_assigned: false,
            ident: None,
        };
        let a = actor(orgu, "creditAnalyst");
        assert!(!authorize(
            &rule(Some(vec!["creditAnalyst"]), None),
            &a,
            env(&EMPTY_CTX, &EMPTY_WFAH),
            &org
        )
        .await
        .unwrap());
    }

    /// Çapasız kural (`c_orgu` yok): kişi HANGİ birimden gelirse gelsin eşleşir.
    /// `MockOrg.units` boş — yani `resolve_c_orgu` çağrılmış olsaydı kimse geçemezdi;
    /// test bu yüzden aynı zamanda "orgu kanalı hiç sorulmadı" iddiasını da sabitler.
    #[tokio::test]
    async fn anchorless_c_u_matches_from_any_orgu() {
        let org = MockOrg {
            units: vec![],
            role_assigned: false,
            ident: Some("ayse".into()),
        };
        let rule = CandidateActor {
            c_orgu: None,
            c_r: None,
            c_u: Some(vec![CuItem::Literal("ayse".into())]),
        };
        for orgu in [Uuid::new_v4(), Uuid::new_v4()] {
            let a = actor(orgu, "herhangiRol");
            assert!(
                authorize(&rule, &a, env(&EMPTY_CTX, &EMPTY_WFAH), &org)
                    .await
                    .unwrap(),
                "çapasız c_u kuralı {orgu} biriminden de eşleşmeli"
            );
        }
    }

    /// Çapasız kuralda rol kanalı KAPALIDIR. Şema ve validator `c_r`li çapasız belgeyi
    /// reddeder; bu test matcher'ın da reddettiğini sabitler — belge bir yolla sızarsa
    /// (elle yazılmış eski kayıt, başka bir istemci) kapı yine açılmasın.
    #[tokio::test]
    async fn anchorless_role_channel_is_closed() {
        let org = MockOrg {
            units: vec![],
            role_assigned: true,
            ident: None,
        };
        let smuggled = CandidateActor {
            c_orgu: None,
            c_r: Some(vec!["mudur".into()]),
            c_u: None,
        };
        let a = actor(Uuid::new_v4(), "mudur");
        assert!(
            !authorize(&smuggled, &a, env(&EMPTY_CTX, &EMPTY_WFAH), &org)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn actor_outside_resolved_orgu_fails_even_with_role() {
        let orgu = Uuid::new_v4();
        let other = Uuid::new_v4();
        let org = MockOrg {
            units: vec![unit(other)],
            role_assigned: true,
            ident: None,
        };
        let a = actor(orgu, "creditAnalyst");
        assert!(!authorize(
            &rule(Some(vec!["creditAnalyst"]), None),
            &a,
            env(&EMPTY_CTX, &EMPTY_WFAH),
            &org
        )
        .await
        .unwrap());
    }

    #[tokio::test]
    async fn c_u_match_is_role_agnostic() {
        let orgu = Uuid::new_v4();
        let org = MockOrg {
            units: vec![unit(orgu)],
            role_assigned: false, // rol ataması YOK — yine de c_u geçmeli
            ident: Some("user_ayse".into()),
        };
        let a = actor(orgu, "branchClerk");
        assert!(authorize(
            &rule(None, Some(vec!["user_ayse"])),
            &a,
            env(&EMPTY_CTX, &EMPTY_WFAH),
            &org
        )
        .await
        .unwrap());
    }

    #[tokio::test]
    async fn c_u_match_by_uuid_string() {
        let orgu = Uuid::new_v4();
        let org = MockOrg {
            units: vec![unit(orgu)],
            role_assigned: false,
            ident: None,
        };
        let a = actor(orgu, "x");
        let uuid_str = a.user_id.to_string();
        let r = CandidateActor {
            c_orgu: Some(COrgu::Selector("self".into())),
            c_r: None,
            c_u: Some(vec![CuItem::Literal(uuid_str)]),
        };
        assert!(authorize(&r, &a, env(&EMPTY_CTX, &EMPTY_WFAH), &org)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn missing_both_channels_is_false_not_wildcard() {
        let orgu = Uuid::new_v4();
        let org = MockOrg {
            units: vec![unit(orgu)],
            role_assigned: true,
            ident: Some("u".into()),
        };
        let a = actor(orgu, "any");
        assert!(
            !authorize(&rule(None, None), &a, env(&EMPTY_CTX, &EMPTY_WFAH), &org)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn role_in_list_but_different_actor_role_fails() {
        let orgu = Uuid::new_v4();
        let org = MockOrg {
            units: vec![unit(orgu)],
            role_assigned: true,
            ident: None,
        };
        let a = actor(orgu, "branchClerk");
        assert!(!authorize(
            &rule(Some(vec!["creditAnalyst"]), None),
            &a,
            env(&EMPTY_CTX, &EMPTY_WFAH),
            &org
        )
        .await
        .unwrap());
    }

    // ---- ctx-anchor'lı c_orgu: çözülemeyen anchor yetkiyi GENİŞLETMEZ ----
    //
    // Bu iki test bir mantık hatasının geri gelmesini engelliyor. Kural
    // `{from: "$ctx.initiated_by", traverse: "self"}` "talebin açıldığı birimin müdürü"
    // demek ister. Alan o an yazılmamışsa anchor eskiden AKTÖRÜN KENDİ birimine düşüyordu;
    // `traverse: "self"` ile çözülen küme {aktörün birimi} olduğundan kapı
    // `actor.orgu ∈ {actor.orgu}` sorusuna dönüşüp DAİMA doğru oluyordu → o rolü taşıyan
    // HERKES geçiyordu, sessizce.
    //
    // `MockOrg` bunu gösteremez (anchor'ı yok sayıp sabit liste döner), o yüzden anchor'ı
    // yansıtan ayrı bir mock gerekiyor: `resolve(anchor, "self") = {anchor}` — gerçek
    // ORGTRVLANG semantiği.
    struct EchoAnchorOrg;

    #[async_trait]
    impl OrgPort for EchoAnchorOrg {
        async fn resolve_c_orgu(
            &self,
            anchor: Uuid,
            _: &str,
            _: Uuid,
        ) -> Result<Vec<OrgUnit>, EngineError> {
            Ok(vec![unit(anchor)])
        }
        async fn check_user_role(&self, _: Uuid, _: Uuid, _: &str) -> Result<bool, EngineError> {
            Ok(true)
        }
        async fn orgtnt_for_orgu(&self, _: Uuid) -> Result<Uuid, EngineError> {
            Ok(Uuid::nil())
        }
    }

    fn ctx_anchored_rule(path: &str) -> CandidateActor {
        CandidateActor {
            c_orgu: Some(COrgu::Anchor {
                from: crate::types::wfd_v22::AnchorFrom::Ctx(path.into()),
                traverse: "self".into(),
            }),
            c_r: Some(vec!["subeMuduru".into()]),
            c_u: None,
        }
    }

    #[tokio::test]
    async fn unresolved_ctx_anchor_denies_even_with_matching_role() {
        // Rol doğru VE atanmış; tek eksik anchor. Yetki verilmemeli.
        let a = actor(Uuid::new_v4(), "subeMuduru");
        assert!(
            !authorize(
                &ctx_anchored_rule("$ctx.initiated_by"),
                &a,
                env(&EMPTY_CTX, &EMPTY_WFAH),
                &EchoAnchorOrg
            )
            .await
            .unwrap(),
            "anchor çözülemezken yetki vermek, kuralı 'o rolü taşıyan herkes' haline getirir"
        );
    }

    #[tokio::test]
    async fn resolved_ctx_anchor_authorizes_only_the_anchored_unit() {
        let anchored_orgu = Uuid::new_v4();
        let ctx = json!({ "initiated_by": { "orgu_id": anchored_orgu.to_string() } });

        // Anchor'ın işaret ettiği birimdeki müdür geçer.
        let inside = actor(anchored_orgu, "subeMuduru");
        assert!(authorize(
            &ctx_anchored_rule("$ctx.initiated_by"),
            &inside,
            env(&ctx, &EMPTY_WFAH),
            &EchoAnchorOrg
        )
        .await
        .unwrap());

        // BAŞKA bir birimdeki müdür geçmez — eski davranışta geçiyordu.
        let outside = actor(Uuid::new_v4(), "subeMuduru");
        assert!(
            !authorize(
                &ctx_anchored_rule("$ctx.initiated_by"),
                &outside,
                env(&ctx, &EMPTY_WFAH),
                &EchoAnchorOrg
            )
            .await
            .unwrap(),
            "anchor birimi dışındaki müdür yetkilenmemeli"
        );
    }

    // ---- dinamik c_u: `Ref` öğesi context'ten çözülür ----

    /// Verilen c_u öğeleriyle kural (`c_r` yok — kişi kanalı yalnız test edilsin).
    fn cu_rule(items: Vec<CuItem>) -> CandidateActor {
        CandidateActor {
            c_orgu: Some(COrgu::Selector("self".into())),
            c_r: None,
            c_u: Some(items),
        }
    }

    fn cu_ref(path: &str) -> CuItem {
        CuItem::Ref { from: path.into() }
    }

    #[tokio::test]
    async fn cu_ref_matches_by_uuid_from_ctx() {
        let orgu = Uuid::new_v4();
        let a = actor(orgu, "clerk");
        let ctx = json!({ "talep_sahibi": { "user_id": a.user_id.to_string() } });
        let org = MockOrg { units: vec![unit(orgu)], role_assigned: false, ident: None };
        assert!(authorize(
            &cu_rule(vec![cu_ref("$ctx.talep_sahibi.user_id")]),
            &a,
            env(&ctx, &EMPTY_WFAH),
            &org
        )
        .await
        .unwrap());
    }

    #[tokio::test]
    async fn cu_ref_matches_actor_object_without_suffix() {
        // `resolve_cu_ident` nesne içinde user_id arar — `actor` kind'lı alanın kendisi yeter.
        let orgu = Uuid::new_v4();
        let a = actor(orgu, "clerk");
        let ctx = json!({ "talep_sahibi": { "user_id": a.user_id.to_string(), "role": "clerk" } });
        let org = MockOrg { units: vec![unit(orgu)], role_assigned: false, ident: None };
        assert!(authorize(&cu_rule(vec![cu_ref("$ctx.talep_sahibi")]), &a, env(&ctx, &EMPTY_WFAH), &org)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn cu_ref_matches_by_username_from_ctx() {
        let orgu = Uuid::new_v4();
        let a = actor(orgu, "clerk");
        let ctx = json!({ "talep_sahibi": "ahmet.yilmaz" });
        let org = MockOrg {
            units: vec![unit(orgu)],
            role_assigned: false,
            ident: Some("ahmet.yilmaz".into()),
        };
        assert!(authorize(&cu_rule(vec![cu_ref("$ctx.talep_sahibi")]), &a, env(&ctx, &EMPTY_WFAH), &org)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn unresolved_cu_ref_does_not_match() {
        // Alan yazılmamış → o öğe aday üretmez. HATA DEĞİL, sadece eşleşme yok.
        let orgu = Uuid::new_v4();
        let a = actor(orgu, "clerk");
        let org = MockOrg {
            units: vec![unit(orgu)],
            role_assigned: false,
            ident: Some("ahmet.yilmaz".into()),
        };
        let result = authorize(
            &cu_rule(vec![cu_ref("$ctx.hic_yazilmadi")]),
            &a,
            env(&EMPTY_CTX, &EMPTY_WFAH),
            &org,
        )
        .await;
        assert!(result.is_ok(), "çözülemeyen referans HATA üretmemeli");
        assert!(!result.unwrap());
    }

    #[tokio::test]
    async fn unresolved_cu_ref_does_not_break_role_channel() {
        // İki kanal bağımsızdır: çözülemeyen bir kişi referansı rol kanalını kapatmamalı.
        let orgu = Uuid::new_v4();
        let a = actor(orgu, "subeMuduru");
        let org = MockOrg { units: vec![unit(orgu)], role_assigned: true, ident: None };
        let rule = CandidateActor {
            c_orgu: Some(COrgu::Selector("self".into())),
            c_r: Some(vec!["subeMuduru".into()]),
            c_u: Some(vec![cu_ref("$ctx.hic_yazilmadi")]),
        };
        assert!(authorize(&rule, &a, env(&EMPTY_CTX, &EMPTY_WFAH), &org).await.unwrap());
    }

    #[tokio::test]
    async fn cu_literal_and_ref_mix() {
        let orgu = Uuid::new_v4();
        let a = actor(orgu, "clerk");
        let ctx = json!({ "talep_sahibi": { "user_id": Uuid::new_v4().to_string() } });
        let org = MockOrg {
            units: vec![unit(orgu)],
            role_assigned: false,
            ident: Some("ahmet.yilmaz".into()),
        };
        // Referans BAŞKA birine çözülüyor; sabit kimlik bu aktörü tutuyor.
        assert!(authorize(
            &cu_rule(vec![CuItem::Literal("ahmet.yilmaz".into()), cu_ref("$ctx.talep_sahibi.user_id")]),
            &a,
            env(&ctx, &EMPTY_WFAH),
            &org
        )
        .await
        .unwrap());
    }

    #[tokio::test]
    async fn cu_ref_outside_resolved_orgu_still_denied() {
        // ORGU kapısı geçilmez: ctx doğru kişiyi verse bile aktörün birimi
        // resolve(c_orgu) içinde değilse yetki YOKTUR (spec §3 erken çıkış).
        let a = actor(Uuid::new_v4(), "clerk");
        let ctx = json!({ "talep_sahibi": { "user_id": a.user_id.to_string() } });
        let org = MockOrg {
            units: vec![unit(Uuid::new_v4())], // BAŞKA bir birim
            role_assigned: false,
            ident: None,
        };
        assert!(!authorize(
            &cu_rule(vec![cu_ref("$ctx.talep_sahibi.user_id")]),
            &a,
            env(&ctx, &EMPTY_WFAH),
            &org
        )
        .await
        .unwrap());
    }

    // ---- 2026-08-13: çapa (anchor) üstünden belirlenen yetki ----

    /// Her ifadeyi ÇAPAYA çözen org: `self` → `[anchor]`. Gerçek ORGTRVLANG'ın
    /// `self` davranışının aynısı, dolayısıyla çapanın hangi birim olduğunu
    /// doğrudan sınayabiliyoruz.
    struct AnchorOrg;

    #[async_trait]
    impl OrgPort for AnchorOrg {
        async fn resolve_c_orgu(
            &self,
            anchor: Uuid,
            _: &str,
            _: Uuid,
        ) -> Result<Vec<OrgUnit>, EngineError> {
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

    /// Çapa verilince `self` YALNIZ o birimi kapsar: başka birimdeki aynı rol
    /// eşleşmez.
    ///
    /// Bu, görünürlük projeksiyonunun (`view_c_a`) dayandığı garantidir —
    /// `listable`/`wf_admin` grant'ları WFE'nin birimine çapalanır.
    #[tokio::test]
    async fn anchored_self_matches_only_the_anchor_unit() {
        let origin = Uuid::new_v4();
        let elsewhere = Uuid::new_v4();
        let ctx = json!({});

        let in_origin = actor(origin, "clerk");
        assert!(authorize_anchored(
            &rule(Some(vec!["clerk"]), None),
            &in_origin,
            Some(origin),
            env(&ctx, &EMPTY_WFAH),
            &AnchorOrg
        )
        .await
        .unwrap());

        let outsider = actor(elsewhere, "clerk");
        assert!(!authorize_anchored(
            &rule(Some(vec!["clerk"]), None),
            &outsider,
            Some(origin),
            env(&ctx, &EMPTY_WFAH),
            &AnchorOrg
        )
        .await
        .unwrap());
    }

    /// ÇAPASIZ çağrı (node c_a'sının eski davranışı) `self`i aktörün kendi
    /// birimine çözer → karşılaştırma kendisiyle yapılır ve HERKES geçer.
    ///
    /// Test bu dejenerasyonu BELGELEMEK için var: `{c_orgu:"self"}` çapasız
    /// sorulduğunda "tenant'ta o roldeki herkes" demektir. Grant ve node
    /// kapılarının çapa vermesi bu yüzden zorunludur (bkz. `docs/spec/decisions.md`).
    #[tokio::test]
    async fn unanchored_self_matches_every_unit() {
        let ctx = json!({});
        for _ in 0..3 {
            let a = actor(Uuid::new_v4(), "clerk");
            assert!(authorize_anchored(
                &rule(Some(vec!["clerk"]), None),
                &a,
                None,
                env(&ctx, &EMPTY_WFAH),
                &AnchorOrg
            )
            .await
            .unwrap());
        }
    }

    /// `authorize` = `authorize_anchored(None)` — kısayolun sözleşmesi.
    #[tokio::test]
    async fn authorize_is_anchorless_alias() {
        let ctx = json!({});
        let a = actor(Uuid::new_v4(), "clerk");
        let r = rule(Some(vec!["clerk"]), None);
        let direct = authorize(&r, &a, env(&ctx, &EMPTY_WFAH), &AnchorOrg)
            .await
            .unwrap();
        let anchorless = authorize_anchored(&r, &a, None, env(&ctx, &EMPTY_WFAH), &AnchorOrg)
            .await
            .unwrap();
        assert_eq!(direct, anchorless);
    }
}
