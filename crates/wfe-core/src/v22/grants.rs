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
use crate::v22::matcher::{authorize_or_delegated_anchored, MatchEnv};
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
