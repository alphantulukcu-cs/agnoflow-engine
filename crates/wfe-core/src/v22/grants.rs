//! C_A tabanlı grant kayıtlarına (`{c_a, when?}`) karşı yetki denetimi.
//!
//! İki tüketici aynı soruyu soruyor: `wfd.listable[]` "bu WFE'yi görebilir mi?",
//! `wfd.wf_admin[]` "bu akışa müdahale edebilir mi?". Kural şekli ve değerlendirme
//! sırası ikisinde de aynıdır — c_a eşleşir VE `when` guard'ı (varsa) true.
//!
//! Ayrı bir modül olmasının nedeni: `visibility` görünürlüğün, `pipeline` geçişlerin
//! yeri. Ortak kapıyı ikisinden birine koymak, diğerinin ona ters yönde bağımlı
//! olmasına yol açardı.

use crate::error::EngineError;
use crate::ports::OrgPort;
use crate::types::actor::Actor;
use crate::types::wfd_v22::CaGrantRule;
use crate::v22::eval::{evaluate_bool, EvalEnv};
use crate::v22::matcher::{authorize_or_delegated, MatchEnv};
use crate::v22::ports::Wfes;

/// Kurallardan HERHANGİ biri aktörü yetkilendiriyor mu (OR).
///
/// Vekâlet dahildir (`authorize_or_delegated`): bir kişiye vekâlet eden aktör onun
/// grant'larını da taşır — `listable` bugün böyle davranıyor, `wf_admin` de öyle
/// davranmalı, aksi halde vekil akışı görebilir ama yönetemez.
pub async fn matches_grant_rules(
    rules: &[CaGrantRule],
    actor: &Actor,
    wfes: &Wfes,
    org: &dyn OrgPort,
) -> Result<bool, EngineError> {
    let ctx = wfes.dynctx.as_value();
    for rule in rules {
        let env = MatchEnv {
            ctx,
            wfah: &wfes.wfah,
            orgtnt_id: wfes.orgtnt_id,
        };
        if !authorize_or_delegated(&rule.c_a, actor, env, org).await? {
            continue;
        }
        let when_ok = match &rule.when {
            None => true,
            Some(expr) => {
                let eval_env = EvalEnv::new(ctx)
                    .with_wfah(&wfes.wfah)
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
