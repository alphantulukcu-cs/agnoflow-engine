//! C_A tabanlı grant kayıtlarına (`{c_a, when?}`) karşı yetki denetimi.
//!
//! İki tüketici aynı soruyu soruyor: `wfd.listable[]` "bu WFE'yi görebilir mi?",
//! `wfd.wf_admin[]` "bu akışa müdahale edebilir mi?". Kural şekli ve değerlendirme
//! sırası ikisinde de aynıdır — c_a eşleşir VE `when` guard'ı (varsa) true.
//!
//! Ayrı bir modül olmasının nedeni: `visibility` görünürlüğün, `pipeline` geçişlerin
//! yeri. Ortak kapıyı ikisinden birine koymak, diğerinin ona ters yönde bağımlı
//! olmasına yol açardı.

use std::collections::BTreeSet;

use crate::error::EngineError;
use crate::ports::OrgPort;
use crate::types::actor::Actor;
use crate::types::wfd_v22::{CaGrantRule, GlobalAction, WfAdminRule};
use crate::v22::eval::{evaluate_bool, EvalEnv};
use crate::v22::matcher::{
    authorize_or_delegated_anchored, authorize_with_delegation_anchored, AuthDecision, MatchEnv,
};
use crate::v22::ports::Wfes;
use crate::v22::valid::ValidRules;

/// Kurallardan HERHANGİ biri aktörü yetkilendiriyor mu (OR).
///
/// Vekâlet dahildir (`authorize_or_delegated_anchored`): bir kişiye vekâlet eden aktör onun
/// grant'larını da taşır — `listable` bugün böyle davranıyor, `wf_admin` de öyle
/// davranmalı, aksi halde vekil akışı görebilir ama yönetemez.
pub async fn matches_grant_rules<'r, I>(
    rules: I,
    actor: &Actor,
    wfes: &Wfes,
    // E05: guard ifadesi `$valid`/`#.is_send_back` görebilir ve ikisi de BELGEDEN
    // türer; bu fonksiyonun elinde WFD olmadığı için kural seti parametredir.
    valid_rules: &ValidRules,
    org: &dyn OrgPort,
) -> Result<bool, EngineError>
where
    I: IntoIterator<Item = &'r CaGrantRule>,
{
    let ctx = wfes.dynctx.as_value();
    for rule in rules {
        let env = MatchEnv {
            ctx,
            wfah: &wfes.wfah,
            orgtnt_id: wfes.orgtnt_id,
        };
        // ÇAPA: WFE'nin kendi birimi (`origin_orgu_id`). Viewer'ın birimi
        // DEĞİL — aksi halde `{"c_orgu":"self"}` gibi bir kural birim
        // karşılaştırmasını kendisiyle yapıp daima geçer ve sessizce "tenant'ta
        // o roldeki herkes" anlamına gelir. `None` (backfill bekleyen eski
        // satır) → eski davranış korunur.
        if !authorize_or_delegated_anchored(&rule.c_a, actor, wfes.origin_orgu_id, env, org)
            .await?
        {
            continue;
        }
        let when_ok = match &rule.when {
            None => true,
            Some(expr) => {
                let eval_env = EvalEnv::new(ctx)
                    .with_wfah(&wfes.wfah, valid_rules)
                    .with_node(wfes.current_node.as_deref())
                    .with_actor(actor)
                    .with_wfe_id(wfes.wfe_id);
                evaluate_bool(expr, &eval_env)?
            }
        };
        if when_ok {
            return Ok(true);
        }
    }
    Ok(false)
}

/// **Adminin BU WFE'de alabileceği global aksiyonlar** (A-1, 2026-08-21).
///
/// Yetki OR'lanır ama küme BİRLEŞTİRİLİR: aktör iki `wf_admin` kuralına da uyuyorsa
/// ikisinin `allowed_global_actions`ı toplanır. Tek bir kuralı seçip diğerini yok saymak,
/// "çoklu grant = çoklu kayıt" deseninde ilk kuralı yazana sessiz bir öncelik verirdi.
///
/// `when` guard'ı kural BAŞINA işler (`matches_grant_rules` ile birebir aynı sıra):
/// c_a eşleşmeyen kural hiç değerlendirilmez, guard'ı false olan kural küme dışıdır.
///
/// Boş küme dönmesi "admin değil" DEMEZ — admin olup hiçbir müdahaleye yetkili olmamak
/// meşru bir yapılandırmadır (görme yetkisi ayrıdır, bkz. `can_view` (e)). "Admin mi"
/// sorusunun cevabı `matches_grant_rules(wf_admin.iter().map(grant), ..)`dır.
pub async fn wf_admin_global_actions(
    rules: &[WfAdminRule],
    actor: &Actor,
    wfes: &Wfes,
    valid_rules: &ValidRules,
    org: &dyn OrgPort,
) -> Result<BTreeSet<GlobalAction>, EngineError> {
    let mut out = BTreeSet::new();
    for rule in rules {
        // Kuralın kendi kümesi boşsa eşleşmeyi sormak gereksiz I/O olurdu
        // (org traverse + delegation sorgusu) — sonuç kümeyi değiştirmez.
        if rule.allowed_global_actions.is_empty() {
            continue;
        }
        if matches_grant_rules(std::iter::once(&rule.grant), actor, wfes, valid_rules, org)
            .await?
        {
            out.extend(rule.allowed_global_actions.iter().copied());
        }
    }
    Ok(out)
}

/// `wf_admin` yetki kapısı: aktör bu global aksiyonu alabiliyor mu?
///
/// Kapı TEK yerdedir çünkü altı global aksiyonun yolu farklı (biri claim yazar, biri
/// node taşır, biri WFE'yi sonlandırır) ama sorusu AYNI: "bu aktör + bu aksiyon".
/// Ayrı ayrı yazılsa biri kapıyı atlar.
pub async fn require_global_action(
    rules: &[WfAdminRule],
    actor: &Actor,
    action: GlobalAction,
    wfes: &Wfes,
    valid_rules: &ValidRules,
    org: &dyn OrgPort,
) -> Result<(), EngineError> {
    if wf_admin_global_actions(rules, actor, wfes, valid_rules, org)
        .await?
        .contains(&action)
    {
        return Ok(());
    }
    Err(EngineError::Unauthorized)
}

// ─────────────────────────────────────────────────────────────────────────────
// v2.3 (`E04`) — `c_a ∪ grant` YETKİ KÜMESİ
//
// Escalation artık havuzu GENİŞLETİYOR (`Ç9`), yani "bu kişi bu işi alabilir mi"
// sorusu artık `node.c_a`ya değil **`node.c_a ∪ açılmış grantlar`**a bakmak zorunda.
// Mantık üç satır; kararın asıl konusu NEREYE konacağıydı.
//
// **Neden `grants.rs`, `matcher.rs` DEĞİL:** `matcher` saf eşleştiricidir ve ZEN
// görmez. Grant'ın `when` guard'ı bir ZEN ifadesidir → genişletmeyi matcher'a koymak
// saf eşleştiriciye ifade değerlendirmesi sokardı. `matcher`ın `authorize` ailesinin
// imzaları ve `MatchEnv` bu yüzden DEĞİŞMEZ.
//
// **Grant sırası `matches_grant_rules` gövdesiyle BİREBİR AYNIDIR:** önce `c_a`
// (vekâlet dahil), sonra `when` guard'ı. İki ayrı sıra iki ayrı cevap üretirdi.
// ─────────────────────────────────────────────────────────────────────────────

/// Bir WFE'nin ŞU ANDA bulunduğu node'a girdiği an.
///
/// ⚠️ **Bu hesap `R02`nin tanımıdır ve TEK YERDE durmak ZORUNDADIR.** Üç tüketici
/// aynı fonksiyonu çağırır: `next_escalation` (kademe vadesi), `waited_for_seconds`
/// (`Ç13`/`E12`) ve açık grant kümesi (aşağısı). İki yerde iki ayrı taban tanımı
/// yazılırsa sayaçlar sessizce ayrışır — bir kademe vadesini geçmiş sayılırken grant'ı
/// henüz açılmamış görünür.
///
/// **Bugünkü tanım geçicidir:** escalation marker'larını önek filtresiyle eleyip son
/// satırı alır. `R02` bunu `to_node != null` olan son satıra çevirecek — o alan
/// (`WfahEntry.to_node`) henüz YOK (`Ç2`/`Ç3` işi). Gövde değişince ÜÇ tüketici
/// birlikte doğru cevaba geçer; çağrı yerlerine dokunmak gerekmez. Bu, fonksiyonun
/// tek yerde olmasının asıl kazancıdır.
pub fn node_entered_at(wfah: &crate::types::wfah::Wfah) -> Option<chrono::DateTime<chrono::Utc>> {
    wfah.entries()
        .iter()
        .filter(|e| !e.action.starts_with("escalate:"))
        .last()
        .map(|e| e.applied_at)
}

/// Bir node'da AÇILMIŞ escalation grant'ları — defterden TÜRETİLİR.
///
/// **`Wfes`e alan EKLENMEZ.** Kaynak defterdeki `escalate:<node_key>:<idx>` satırları:
/// `applied_at`i node'a giriş anından SONRA olanların `idx`leri o node'un
/// `escalation[idx].grant`ına karşılık gelir. Alan eklemek aynı gerçeği iki yerde
/// tutmak (ve senkronda kalmasını ummak) olurdu.
///
/// Dönüş sırası belge sırasıdır (kademe sırası), tekrarsız.
pub fn open_grants<'w>(
    wfd: &'w crate::types::wfd_v22::Wfd,
    wfah: &crate::types::wfah::Wfah,
    node_key: &str,
) -> Vec<&'w CaGrantRule> {
    let Some(node) = wfd.nodes.get(node_key) else {
        return Vec::new();
    };
    if node.escalation.is_empty() {
        return Vec::new();
    }
    let entered = node_entered_at(wfah);
    let mut fired: BTreeSet<usize> = BTreeSet::new();
    for entry in wfah.entries() {
        // `escalate:<node>:<idx>` — `:skipped` soneki de sayılır: WF Admin'in elle
        // atlaması kademeyi ATEŞLENMİŞ sayar (`E13`: sayaç kaymaz), dolayısıyla
        // grant'ı da açılmış sayılır.
        let Some(rest) = entry.action.strip_prefix("escalate:") else {
            continue;
        };
        let rest = rest.strip_suffix(":skipped").unwrap_or(rest);
        let Some((node_part, idx_part)) = rest.rsplit_once(':') else {
            continue;
        };
        if node_part != node_key {
            continue;
        }
        let Ok(idx) = idx_part.parse::<usize>() else {
            continue;
        };
        // Node'a girişten ÖNCEki satırlar önceki bir turun kalıntısıdır — o turun
        // grant'ı bu turda açık değildir.
        if let Some(entered) = entered {
            if entry.applied_at < entered {
                continue;
            }
        }
        if idx < node.escalation.len() {
            fired.insert(idx);
        }
    }
    fired
        .into_iter()
        .map(|i| &node.escalation[i].grant)
        .collect()
}

/// "Bu aktör bu node'da iş alabilir mi" — **`node.c_a ∪ açılmış grantlar`** üzerinde
/// karar.
///
/// Bu, `authorize(&node.c_a, …)`nın v2.3 karşılığıdır ve node yetkisi soran YEDİ çağrı
/// yeri yalnız bunu çağırır. Doğrudan `node.c_a`ya bakan bir yol bırakmak, grant'ı
/// görmeyen sessiz bir kapı bırakmak olurdu — bu yüzden `NodeDef::act_c_a()`
/// accessor'ı ayrı bir adla durur ve "havuz" sorusu buradan geçer.
pub async fn authorize_node(
    wfd: &crate::types::wfd_v22::Wfd,
    wfes: &Wfes,
    node_key: &str,
    actor: &Actor,
    org: &dyn OrgPort,
) -> Result<bool, EngineError> {
    Ok(authorize_node_decision(wfd, wfes, node_key, actor, org)
        .await?
        .is_authorized())
}

/// `authorize_node`ın PROVENANS taşıyan hâli — claim marker'ı "doğrudan mı vekaleten
/// mi" yazmak zorunda (Madde 6 / `Ç13`).
///
/// İki soru TEK gövdede cevaplanır: `can_claim` uygunluğu, `claim_decision` de aynı
/// kararın gerekçesini sorar ve ikisi AYRI gövdeye bakarsa portal "Claim et" düğmesini
/// gösterip `claim` reddedebilir.
///
/// ⚠️ Grant'la gelen yetki bugün `Direct` döner. `Ç13`ün `authority` alanı için doğru
/// değer `grant` olurdu; o alanın kapalı listesi `E12`nin işi ve bu kayıt onu
/// GENİŞLETMEZ — CLAUDE.md'nin *"`authority` bugün DAİMA `c_a`"* notu yerinde durur.
pub async fn authorize_node_decision(
    wfd: &crate::types::wfd_v22::Wfd,
    wfes: &Wfes,
    node_key: &str,
    actor: &Actor,
    org: &dyn OrgPort,
) -> Result<AuthDecision, EngineError> {
    let Some(node) = wfd.nodes.get(node_key) else {
        return Err(EngineError::InvalidWfd(format!(
            "bilinmeyen node '{node_key}'"
        )));
    };
    let ctx = wfes.dynctx.as_value();
    // ⚠️ `MatchEnv`/`EvalEnv` kurulumu kural döngüsünün DIŞINDA (`E04`/S4). Bu bir
    // optimizasyon değil, aynı işi kural başına tekrarlamayı bırakmaktır.
    let env = MatchEnv {
        ctx,
        wfah: &wfes.wfah,
        orgtnt_id: wfes.orgtnt_id,
    };
    // 1) Node'un kendi havuzu — vekâlet dahil, provenans KORUNUR.
    let direct = authorize_with_delegation_anchored(
        node.act_c_a(),
        actor,
        wfes.origin_orgu_id,
        env,
        org,
        chrono::Utc::now(),
    )
    .await?;
    if direct.is_authorized() {
        return Ok(direct);
    }
    // 2) Açılmış grantlar — `matches_grant_rules` ile AYNI sıra (c_a → when).
    let grants = open_grants(wfd, &wfes.wfah, node_key);
    if grants.is_empty() {
        return Ok(AuthDecision::Denied);
    }
    let by_grant =
        matches_grant_rules(grants, actor, wfes, &ValidRules::for_version(wfd), org).await?;
    Ok(if by_grant {
        AuthDecision::Direct
    } else {
        AuthDecision::Denied
    })
}
