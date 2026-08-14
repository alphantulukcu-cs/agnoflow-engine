//! §4 Visibility Matcher (WOR-38, M13) — authorization'dan AYRI fonksiyon, kriterler arası OR:
//!
//! ```text
//! visible(vis, actor, wfe) :=
//!      (vis.c_orgu var ve actor.orgu ∈ resolve(vis.c_orgu, wfe))
//!   OR (vis.c_r    var ve actor.role ∈ vis.c_r)          # scope'suz
//!   OR (vis.c_u    var ve actor.user ∈ vis.c_u)          # scope'suz
//!   OR (vis.c_a    var ve match(vis.c_a, actor, wfe))    # scope'lu tam kural
//! ```
//!
//! V yalnızca field okunurluğunu filtreler; ACT/claim/listability üretmez.

use crate::error::EngineError;
use crate::ports::OrgPort;
use crate::types::actor::Actor;
use crate::types::wfd_v22::{COrgu, CandidateActor, CuItem, Wfd};
use crate::v22::grants::matches_grant_rules;
use crate::v22::matcher::{authorize_or_delegated, authorize_or_delegated_anchored, MatchEnv};
use crate::v22::ports::{BranchStatus, Wfes};
use crate::v22::resolver::{resolve_c_orgu, resolve_cu_ident};
use serde::Deserialize;
use uuid::Uuid;
use serde_json::Value;

/// `x-visibility` bloğu — context şemasındaki bir property üzerinde.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct XVisibility {
    #[serde(default)]
    pub c_orgu: Option<COrgu>,
    #[serde(default)]
    pub c_r: Option<Vec<String>>,
    /// Şekli C_A'nın `c_u`'suyla AYNIDIR (terminology.md: "şekli C_A ile aynıdır") —
    /// sabit kimlik ya da `{from: "$ctx..."}` referansı. İki yüzeyin ayrışması, editörde
    /// bulduğumuz "her yüzey şemanın yarısını destekliyor" hatasının aynısı olurdu.
    #[serde(default)]
    pub c_u: Option<Vec<CuItem>>,
    #[serde(default)]
    pub c_a: Option<CandidateActor>,
}

/// `anchor`: ORGTRVLANG çapası — WFE'nin birimi (`origin_orgu_id`), soran kişinin
/// DEĞİL (2026-08-13, WFE-seviyesi görünürlükle AYNI kural). Çapa viewer olduğunda
/// `{c_orgu:"self"}` birim karşılaştırmasını kendisiyle yapıp DAİMA geçiyordu:
/// tasarımcının "bu alanı yalnız kendi birimim görsün" diye yazdığı kural sessizce
/// "her birim görsün"e dönüşüyordu. `None` = eski davranış (backfill bekleyen satır).
///
/// `c_r` kanalı birim kısıtı TAŞIMAZ ve bu değişiklikten etkilenmez — orada
/// "bu rol, hangi birimde olursa olsun" kastı zaten açıktır.
pub async fn visible(
    vis: &XVisibility,
    actor: &Actor,
    anchor: Option<Uuid>,
    env: MatchEnv<'_>,
    org: &dyn OrgPort,
) -> Result<bool, EngineError> {
    if let Some(c_orgu) = &vis.c_orgu {
        let resolved = resolve_c_orgu(
            c_orgu,
            anchor.unwrap_or(actor.orgu_id),
            env.ctx,
            env.wfah,
            env.orgtnt_id,
            org,
        )
        .await?;
        if resolved.iter().any(|u| u.orgu_id == actor.orgu_id) {
            return Ok(true);
        }
    }
    if let Some(c_r) = &vis.c_r {
        if c_r.iter().any(|r| r == &actor.role) {
            return Ok(true);
        }
    }
    if let Some(c_u) = &vis.c_u {
        // `matcher::authorize` 3. adımıyla aynı çözüm: referanslar ctx'ten okunur,
        // çözülemeyen referans aday üretmez.
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
        if !wanted.is_empty() {
            if let Some(ident) = org.user_ident(actor.user_id).await? {
                if wanted.iter().any(|u| u == &ident) {
                    return Ok(true);
                }
            }
        }
    }
    if let Some(c_a) = &vis.c_a {
        if authorize_or_delegated_anchored(c_a, actor, anchor, env, org).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// DynCtx'i context şemasındaki `x-visibility` kurallarına göre filtreler.
/// `x-visibility` olmayan field herkese görünür. Match etmeyen field response'tan çıkarılır.
/// Nested obje şemaları recursive işlenir.
pub async fn filter_dynctx(
    context_schema: &Value,
    dynctx: &Value,
    actor: &Actor,
    anchor: Option<Uuid>,
    env: MatchEnv<'_>,
    org: &dyn OrgPort,
) -> Result<Value, EngineError> {
    let Value::Object(data) = dynctx else {
        return Ok(dynctx.clone());
    };
    let props = context_schema.get("properties").and_then(Value::as_object);

    let mut out = serde_json::Map::new();
    for (key, value) in data {
        let field_schema = props.and_then(|p| p.get(key));
        if let Some(schema) = field_schema {
            if let Some(vis_value) = schema.get("x-visibility") {
                let vis: XVisibility = serde_json::from_value(vis_value.clone())
                    .map_err(|e| EngineError::InvalidWfd(format!("x-visibility parse: {e}")))?;
                if !visible(&vis, actor, anchor, env, org).await? {
                    continue;
                }
            }
            // nested obje şeması varsa recursive filtrele
            if schema.get("properties").is_some() && value.is_object() {
                let filtered =
                    Box::pin(filter_dynctx(schema, value, actor, anchor, env, org)).await?;
                out.insert(key.clone(), filtered);
                continue;
            }
        }
        out.insert(key.clone(), value.clone());
    }
    Ok(Value::Object(out))
}

/// §4/L — WFE-seviyesi VIEW kapısı (spec Terminology VISIBILITY/LISTABLE):
/// bir WFE şu durumlardan biri doğruysa görüntülenebilir (OR):
///   (a) viewer sahibi mi (`assigned_to == viewer.user_id`; paralel modda
///       aktif bir kolun `claimed_by`'ı da sahiplik sayılır — WOR-31)
///   (b) viewer WFAH'ta eylemi bulunan gerçek bir katılımcı mı (system hariç)
///   (c) viewer aktif node'un c_a'sına (§3) authorize mi (paralel modda
///       aktif kol node'larından HERHANGİ birinin c_a'sı — WOR-31)
///   (d) viewer `wfd.listable[]` kurallarından birine authorize VE kuralın
///       `when` guard'ı (varsa) staged ctx üzerinde true mü
///   (e) viewer `wfd.wf_admin[]` kurallarından birine authorize (aynı şekil) —
///       akış yöneticisi yönettiği akışı görür (T‑A5)
///   (f) aktif node'lardan HERHANGİ birinin `node.listable[]` kurallarından birine
///       viewer authorize VE kuralın `when` guard'ı (varsa) true — 2026-08-13, kök
///       `listable[]`in DURUMA BAĞLI karşılığı: WFE bu node'dan çıkınca (b) gibi
///       görünürlük de biter. ACT/claim VERMEZ (`matches_grant_rules` yalnız görme
///       sorar), aynı "aktif node" kümesi (c) ile PAYLAŞILIR.
/// `visible`/`filter_dynctx`'ten AYRIDIR: onlar field-level x-visibility'dir,
/// bu fonksiyon WFE'nin bütünüyle görünür olup olmadığını belirler.
pub async fn can_view(
    wfd: &Wfd,
    wfes: &Wfes,
    viewer: &Actor,
    org: &dyn OrgPort,
) -> Result<bool, EngineError> {
    // 2026-08-13 ürün kararı: KALICI grant'lar (listable/wf_admin) durumdan
    // bağımsız; geri kalan her kriter YALNIZ aktif WFE'de geçerlidir. İş bitince
    // "iş kimin havuzundaydı" sorusu anlamını yitirir, geriye yalnız "kim
    // görmeye yetkili" kalır. Bu sıra `server::visibility::sql`in sırasıyla
    // AYNIDIR — ikisi aynı kuralın iki okumasıdır (biri belgeden, biri
    // proje­ksiyondan) ve kontrat testiyle eşitlikleri korunur.
    if matches_grant_rules(&wfd.listable, viewer, wfes, org).await? {
        return Ok(true);
    }
    if matches_grant_rules(&wfd.wf_admin, viewer, wfes, org).await? {
        return Ok(true);
    }
    if wfes.status != crate::types::wfe::WfeStatus::Active {
        return Ok(false);
    }

    // (a) sahiplik — paralel modda assignment KOL-bazlıdır (wfe-seviyesi
    // assigned_to fork'ta temizlenir): aktif bir kolu claim eden de sahiptir.
    if wfes.assigned_to == Some(viewer.user_id) {
        return Ok(true);
    }
    if wfes
        .branches
        .iter()
        .any(|b| b.status == BranchStatus::Active && b.claimed_by == Some(viewer.user_id))
    {
        return Ok(true);
    }

    // (b) KATILIMCI kriteri 2026-08-13'te KALDIRILDI: "bu işe dokunmuş olmak"
    // artık görünürlük üretmez. Ölçüldü (bkz. `visibility_report`): eski kuralda
    // görünen 205 (aktör × WFE) çiftinin 57'si yalnız bu kritere dayanıyordu ve
    // 46'sı bitmiş işlerdi. Yerine geçen kural: takip edilmesi istenen akış
    // `listable[]` ile açıkça işaretlenir — yetki belgeden okunur, geçmişten
    // türetilmez.

    let ctx = wfes.dynctx.as_value();

    // (c) aktif node'un c_a'sı (§3) — paralel modda (WOR-31) "aktif node"
    // kümesi AKTİF kolların node'larıdır (current_node NULL'dır); herhangi bir
    // aktif kolun c_a'sına authorize olan viewer WFE'yi görüntüleyebilir.
    let active_nodes: Vec<&str> = if wfes.join_target.is_some() {
        wfes.branches
            .iter()
            .filter(|b| b.status == BranchStatus::Active)
            .map(|b| b.branch_node.as_str())
            .collect()
    } else {
        wfes.current_node.as_deref().into_iter().collect()
    };
    for node_key in active_nodes {
        if let Some(node) = wfd.nodes.get(node_key) {
            let env = MatchEnv {
                ctx,
                wfah: &wfes.wfah,
                orgtnt_id: wfes.orgtnt_id,
            };
            // Çapa WFE'nin kendi birimi — aksiyon kapısıyla AYNI (2026-08-13).
            // Görebilen ile yapabilen aynı kümeden okunur.
            if authorize_or_delegated_anchored(
                &node.c_a,
                viewer,
                wfes.origin_orgu_id,
                env,
                org,
            )
            .await?
            {
                return Ok(true);
            }
            // (f) node-seviyesi `listable[]` — DURUMA BAĞLI görme (2026-08-13): aynı
            // "aktif node" kümesi (c) ile paylaşılır, ikinci bir liste kurulmaz. Aynı
            // ÇAPA (`origin_orgu_id`) ve aynı matcher (`matches_grant_rules`, kök
            // `listable`/`wf_admin` (d)/(e) ile birebir) — ACT/claim'e dokunmaz.
            if matches_grant_rules(&node.listable, viewer, wfes, org).await? {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::actor::OrgUnit;
    use crate::types::wfah::Wfah;
    use async_trait::async_trait;
    use serde_json::json;
    use uuid::Uuid;

    struct MockOrg {
        units: Vec<OrgUnit>,
        role_assigned: bool,
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
    }

    fn actor(orgu: Uuid, role: &str) -> Actor {
        Actor {
            orgu_id: orgu,
            user_id: Uuid::new_v4(),
            role: role.into(),
        }
    }

    static EMPTY_CTX: std::sync::LazyLock<Value> = std::sync::LazyLock::new(|| json!({}));
    static EMPTY_WFAH: std::sync::LazyLock<Wfah> = std::sync::LazyLock::new(Wfah::empty);

    fn env<'a>() -> MatchEnv<'a> {
        MatchEnv {
            ctx: &EMPTY_CTX,
            wfah: &EMPTY_WFAH,
            orgtnt_id: Uuid::nil(),
        }
    }

    #[tokio::test]
    async fn c_r_criterion_is_scopeless_or() {
        // rol listede ama ORGU resolve edilen kümede DEĞİL — yine de görünür (scope'suz)
        let org = MockOrg {
            units: vec![],
            role_assigned: false,
        };
        let vis = XVisibility {
            c_r: Some(vec!["creditDeptManager".into(), "branchManager".into()]),
            ..Default::default()
        };
        let a = actor(Uuid::new_v4(), "branchManager");
        assert!(visible(&vis, &a, None, env(), &org).await.unwrap());

        let b = actor(Uuid::new_v4(), "clerk");
        assert!(!visible(&vis, &b, None, env(), &org).await.unwrap());
    }

    #[tokio::test]
    async fn c_a_criterion_uses_full_authorization_rule() {
        let orgu = Uuid::new_v4();
        let org = MockOrg {
            units: vec![OrgUnit {
                orgu_id: orgu,
                orgu_type: json!({}),
                path: "1".into(),
            }],
            role_assigned: true,
        };
        let vis = XVisibility {
            c_a: Some(CandidateActor {
                c_orgu: Some(COrgu::Selector("self".into())),
                c_r: Some(vec!["auditor".into()]),
                c_u: None,
            }),
            ..Default::default()
        };
        let a = actor(orgu, "auditor");
        assert!(visible(&vis, &a, None, env(), &org).await.unwrap());
        let b = actor(orgu, "clerk");
        assert!(!visible(&vis, &b, None, env(), &org).await.unwrap());
    }

    #[tokio::test]
    async fn filter_hides_restricted_field_from_non_matching_actor() {
        let org = MockOrg {
            units: vec![],
            role_assigned: false,
        };
        let schema = json!({
            "type": "object",
            "properties": {
                "amount": {"type": "number"},
                "internal_notes": {
                    "type": "string",
                    "x-visibility": {"c_r": ["creditDeptManager", "branchManager"]}
                }
            }
        });
        let ctx = json!({"amount": 5000, "internal_notes": "gizli"});

        let manager = actor(Uuid::new_v4(), "branchManager");
        let filtered = filter_dynctx(&schema, &ctx, &manager, None, env(), &org)
            .await
            .unwrap();
        assert_eq!(filtered["internal_notes"], json!("gizli"));

        let clerk = actor(Uuid::new_v4(), "branchClerk");
        let filtered = filter_dynctx(&schema, &ctx, &clerk, None, env(), &org)
            .await
            .unwrap();
        assert!(
            filtered.get("internal_notes").is_none(),
            "match etmeyen actor'a gizlenmeli"
        );
        assert_eq!(filtered["amount"], json!(5000), "kısıtsız field görünmeli");
    }

    #[tokio::test]
    async fn fields_without_visibility_are_visible_to_all() {
        let org = MockOrg {
            units: vec![],
            role_assigned: false,
        };
        let schema = json!({"type": "object", "properties": {"x": {"type": "string"}}});
        let ctx = json!({"x": "açık", "undeclared": 1});
        let a = actor(Uuid::new_v4(), "anyone");
        let filtered = filter_dynctx(&schema, &ctx, &a, None, env(), &org).await.unwrap();
        assert_eq!(filtered["x"], json!("açık"));
        assert_eq!(filtered["undeclared"], json!(1));
    }

    // ---- 2026-08-13: alan bazlı gizlilikte de çapa WFE'nin birimi ----

    /// Her ifadeyi ÇAPAYA çözen org (gerçek `self` davranışı).
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

    fn secret_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "amount": {"type": "number"},
                "internal_notes": {
                    "type": "string",
                    "x-visibility": {"c_orgu": "self"}
                }
            }
        })
    }

    /// Çapa WFE'nin birimi: alan YALNIZ o birimdekilere görünür.
    #[tokio::test]
    async fn field_visibility_anchors_at_wfe_origin_unit() {
        let origin = Uuid::new_v4();
        let ctx = json!({"amount": 5000, "internal_notes": "gizli"});

        // Aynı birim → görür.
        let inside = actor(origin, "clerk");
        let f = filter_dynctx(&secret_schema(), &ctx, &inside, Some(origin), env(), &AnchorOrg)
            .await
            .unwrap();
        assert_eq!(f["internal_notes"], json!("gizli"));

        // Başka birim → GÖRMEZ. Eski davranışta (çapa = viewer) `self` daima
        // geçtiği için bu alan HERKESE görünüyordu; regresyon burada patlar.
        let outside = actor(Uuid::new_v4(), "clerk");
        let f = filter_dynctx(&secret_schema(), &ctx, &outside, Some(origin), env(), &AnchorOrg)
            .await
            .unwrap();
        assert!(f.get("internal_notes").is_none(), "başka birime sızdı");
        assert_eq!(f["amount"], json!(5000), "kısıtsız alan görünmeli");
    }

    /// Çapa yoksa (backfill bekleyen satır) eski davranış: `self` viewer'a çözülür
    /// ve alan herkese görünür. Geçiş sırasında hiçbir alan KAYBOLMASIN diye.
    #[tokio::test]
    async fn field_visibility_without_anchor_keeps_legacy_behaviour() {
        let ctx = json!({"amount": 1, "internal_notes": "gizli"});
        let anyone = actor(Uuid::new_v4(), "clerk");
        let f = filter_dynctx(&secret_schema(), &ctx, &anyone, None, env(), &AnchorOrg)
            .await
            .unwrap();
        assert_eq!(f["internal_notes"], json!("gizli"));
    }

    /// `c_r` kanalı birim kısıtı TAŞIMAZ — çapa değişikliği onu ETKİLEMEZ.
    /// ("bu rol, hangi birimde olursa olsun" kastı zaten açık.)
    #[tokio::test]
    async fn role_only_field_visibility_is_unit_agnostic() {
        let schema = json!({
            "type": "object",
            "properties": {
                "internal_notes": {
                    "type": "string",
                    "x-visibility": {"c_r": ["mudur"]}
                }
            }
        });
        let ctx = json!({"internal_notes": "gizli"});
        let origin = Uuid::new_v4();
        let elsewhere_manager = actor(Uuid::new_v4(), "mudur");
        let f = filter_dynctx(&schema, &ctx, &elsewhere_manager, Some(origin), env(), &AnchorOrg)
            .await
            .unwrap();
        assert_eq!(f["internal_notes"], json!("gizli"));
    }
}
