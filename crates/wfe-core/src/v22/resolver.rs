//! v2.2 c_orgu çözümlemesi — Selector (ORGTRVLANG) ve Anchor (ctx / wfah) formları.
//! Spec: terminology.md; wfah occurrence default "last" (M9).

use crate::error::EngineError;
use crate::ports::OrgPort;
use crate::types::actor::OrgUnit;
use crate::types::wfah::Wfah;
use crate::types::wfd_v22::{AnchorFrom, COrgu};
use serde_json::Value;
use uuid::Uuid;

/// Bir c_orgu ifadesini ORGU kümesine çözer.
///
/// `default_anchor` — `Selector` biçiminin `self` kökü (genelde aktörün orgu'su). Yalnız
/// O biçimde kullanılır; `Anchor` biçiminde anchor'ın kaynağı ctx/wfah'tır.
///
/// **Anchor çözülemezse BOŞ küme döner — aktörün birimine DÜŞMEZ.** Eskiden düşüyordu ve bu
/// bir mantık hatasıydı: `{from: "$ctx.initiated_by", traverse: "self"}` kuralı "talebin
/// açıldığı birimin müdürü" demek isterken, alan o an yazılmamışsa anchor aktörün kendi
/// birimine düşüyor ve kapı `actor.orgu ∈ {actor.orgu}` sorusuna dönüşüyordu — DAİMA doğru.
/// Yani kısıt kısıt olmaktan çıkıyor, o rolü taşıyan herkes geçiyordu; sessizce, iz
/// bırakmadan. Hata `Selector` dalı için doğru olan varsayılanın (`self` = aktörün birimi)
/// anlamı tersine çeviren bu dala kopyalanmasından geliyordu.
///
/// Boş küme = kimse yetkilenmez → node görünür biçimde durur ve `claim_timeout`/`escalation`
/// devreye girer. "Verilmeyen alan false'dur, wildcard değil" (§3) ile de tutarlıdır.
pub async fn resolve_c_orgu(
    c_orgu: &COrgu,
    default_anchor: Uuid,
    ctx: &Value,
    wfah: &Wfah,
    orgtnt_id: Uuid,
    org: &dyn OrgPort,
) -> Result<Vec<OrgUnit>, EngineError> {
    match c_orgu {
        COrgu::Selector(expr) => org.resolve_c_orgu(default_anchor, expr, orgtnt_id).await,
        COrgu::Anchor { from, traverse } => {
            let Some(anchor) = resolve_anchor(from, ctx, wfah)? else {
                return Ok(Vec::new());
            };
            let expr = normalize_traverse(traverse);
            org.resolve_c_orgu(anchor, &expr, orgtnt_id).await
        }
    }
}

fn resolve_anchor(
    from: &AnchorFrom,
    ctx: &Value,
    wfah: &Wfah,
) -> Result<Option<Uuid>, EngineError> {
    match from {
        AnchorFrom::Ctx(path) => anchor_from_ctx(path, ctx),
        AnchorFrom::Wfah {
            wfah: action,
            field,
            occurrence,
        } => anchor_from_wfah(action, field, occurrence.as_deref(), wfah),
    }
}

fn anchor_from_ctx(path: &str, ctx: &Value) -> Result<Option<Uuid>, EngineError> {
    let stripped = path.strip_prefix("$ctx.").unwrap_or(path);
    let mut current = ctx;
    for part in stripped.split('.') {
        current = match current
            .get(part)
            .or_else(|| current.get(format!("{part}_id")))
        {
            Some(v) => v,
            None => return Ok(None),
        };
    }
    extract_orgu_uuid(current, path)
}

fn anchor_from_wfah(
    action: &str,
    field: &str,
    occurrence: Option<&str>,
    wfah: &Wfah,
) -> Result<Option<Uuid>, EngineError> {
    let entries = wfah.entries();
    let entry = match occurrence.unwrap_or("last") {
        "first" => entries.iter().find(|e| e.action == action),
        _ => entries.iter().rev().find(|e| e.action == action),
    };
    let Some(entry) = entry else {
        return Ok(None);
    };
    let entry_json = serde_json::to_value(entry)
        .map_err(|e| EngineError::EffectValue(format!("wfah entry serileştirilemedi: {e}")))?;
    let mut current = &entry_json;
    for part in field.split('.') {
        // spec "actor.orgu" der; Actor "orgu_id" ile serileşir — _id fallback'i
        current = match current
            .get(part)
            .or_else(|| current.get(format!("{part}_id")))
        {
            Some(v) => v,
            None => return Ok(None),
        };
    }
    extract_orgu_uuid(current, field)
}

/// Bir JSON değerinden ORGU UUID'si çıkarır: uuid string ya da {orgu|orgu_id: "..."} objesi.
fn extract_orgu_uuid(value: &Value, source: &str) -> Result<Option<Uuid>, EngineError> {
    let raw = if let Some(s) = value.as_str() {
        Some(s)
    } else if let Some(obj) = value.as_object() {
        obj.get("orgu")
            .or_else(|| obj.get("orgu_id"))
            .and_then(|v| v.as_str())
    } else {
        None
    };
    raw.map(|s| {
        Uuid::parse_str(s).map_err(|e| {
            EngineError::EffectValue(format!("'{source}' anchor'ı geçerli UUID değil: {e}"))
        })
    })
    .transpose()
}

/// `c_u`'nun `Ref` öğesini context'ten bir KİŞİ kimliğine çözer.
///
/// `resolve_c_orgu`'nun anchor çözümünün simetriği; aynı iki kolaylığı taşır:
/// `$ctx.` öneki opsiyoneldir ve yol yürünürken `<ad>_id` soneki denenir. Değer ya ham
/// string (username veya UUID) ya da içinde `user_id`/`user` bulunan bir nesnedir — yani
/// `{from: "$ctx.talep_sahibi"}` (son ek yazmadan) da çalışır, çünkü `actor` kind'lı alan
/// `{user_id, orgu_id, role}` tutar.
///
/// **Çözülemezse `None` — HATA DEĞİL.** `$ctx`'in "eksik = null" sözleşmesiyle tutarlı;
/// `$env`'in "eksik = hata" kuralı burada geçmez, çünkü sonuç bir domain/URL üretmiyor,
/// yalnızca aday havuzunu daraltıyor. Kanal eşleşmez, o kadar. (`c_orgu` anchor'ından
/// farklı: orada boş küme TÜM kuralı kapatır ve bu bilinçlidir; burada `c_r` kanalı
/// bağımsız olarak hâlâ eşleşebilir.)
pub fn resolve_cu_ident(path: &str, ctx: &Value) -> Option<String> {
    let stripped = path.strip_prefix("$ctx.").unwrap_or(path);
    let mut current = ctx;
    for part in stripped.split('.') {
        current = current
            .get(part)
            .or_else(|| current.get(format!("{part}_id")))?;
    }
    if let Some(s) = current.as_str() {
        return Some(s.to_string());
    }
    current
        .as_object()?
        .get("user_id")
        .or_else(|| current.get("user"))
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// ORGTRVLANG traversal'ı "self" köküne bağlar ("parent" → "self.parent").
fn normalize_traverse(traverse: &str) -> String {
    if traverse == "self" || traverse.starts_with("self.") {
        traverse.to_string()
    } else {
        format!("self.{traverse}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::actor::Actor;
    use async_trait::async_trait;
    use serde_json::json;

    struct RecordingOrg {
        units: Vec<OrgUnit>,
        last_call: std::sync::Mutex<Option<(Uuid, String)>>,
    }

    #[async_trait]
    impl OrgPort for RecordingOrg {
        async fn resolve_c_orgu(
            &self,
            anchor: Uuid,
            expr: &str,
            _orgtnt_id: Uuid,
        ) -> Result<Vec<OrgUnit>, EngineError> {
            *self.last_call.lock().unwrap() = Some((anchor, expr.to_string()));
            Ok(self.units.clone())
        }
        async fn check_user_role(&self, _: Uuid, _: Uuid, _: &str) -> Result<bool, EngineError> {
            Ok(true)
        }
        async fn orgtnt_for_orgu(&self, _: Uuid) -> Result<Uuid, EngineError> {
            Ok(Uuid::nil())
        }
    }

    fn org_with(units: Vec<OrgUnit>) -> RecordingOrg {
        RecordingOrg {
            units,
            last_call: std::sync::Mutex::new(None),
        }
    }

    fn unit(id: Uuid) -> OrgUnit {
        OrgUnit {
            orgu_id: id,
            orgu_type: json!({"type": "branch"}),
            path: "1.2".into(),
        }
    }

    fn actor(orgu: Uuid) -> Actor {
        Actor {
            orgu_id: orgu,
            user_id: Uuid::new_v4(),
            role: "clerk".into(),
        }
    }

    #[tokio::test]
    async fn selector_uses_default_anchor() {
        let anchor = Uuid::new_v4();
        let org = org_with(vec![unit(anchor)]);
        let result = resolve_c_orgu(
            &COrgu::Selector("self".into()),
            anchor,
            &json!({}),
            &Wfah::empty(),
            Uuid::nil(),
            &org,
        )
        .await
        .unwrap();
        assert_eq!(result.len(), 1);
        let (called_anchor, expr) = org.last_call.lock().unwrap().clone().unwrap();
        assert_eq!(called_anchor, anchor);
        assert_eq!(expr, "self");
    }

    #[tokio::test]
    async fn ctx_anchor_resolves_from_actor_object() {
        let stored_orgu = Uuid::new_v4();
        let org = org_with(vec![unit(stored_orgu)]);
        let ctx = json!({"initiated_by": {"orgu_id": stored_orgu.to_string(), "user_id": Uuid::nil(), "role": "clerk"}});
        let c_orgu = COrgu::Anchor {
            from: AnchorFrom::Ctx("$ctx.initiated_by".into()),
            traverse: "parent".into(),
        };
        resolve_c_orgu(
            &c_orgu,
            Uuid::new_v4(),
            &ctx,
            &Wfah::empty(),
            Uuid::nil(),
            &org,
        )
        .await
        .unwrap();
        let (called_anchor, expr) = org.last_call.lock().unwrap().clone().unwrap();
        assert_eq!(
            called_anchor, stored_orgu,
            "anchor ctx'teki actor'ün orgu'su olmalı"
        );
        assert_eq!(expr, "self.parent", "traverse self köküne bağlanmalı");
    }

    #[tokio::test]
    async fn wfah_anchor_default_occurrence_is_last() {
        let first_orgu = Uuid::new_v4();
        let last_orgu = Uuid::new_v4();
        let org = org_with(vec![]);
        let wfah = Wfah::empty()
            .push("submit".into(), actor(first_orgu), None)
            .push("submit".into(), actor(last_orgu), None);
        let c_orgu = COrgu::Anchor {
            from: AnchorFrom::Wfah {
                wfah: "submit".into(),
                field: "actor.orgu_id".into(),
                occurrence: None,
            },
            traverse: "self".into(),
        };
        resolve_c_orgu(
            &c_orgu,
            Uuid::new_v4(),
            &json!({}),
            &wfah,
            Uuid::nil(),
            &org,
        )
        .await
        .unwrap();
        let (called_anchor, _) = org.last_call.lock().unwrap().clone().unwrap();
        assert_eq!(
            called_anchor, last_orgu,
            "default occurrence 'last' olmalı (M9)"
        );
    }

    #[tokio::test]
    async fn wfah_anchor_occurrence_first() {
        let first_orgu = Uuid::new_v4();
        let last_orgu = Uuid::new_v4();
        let org = org_with(vec![]);
        let wfah = Wfah::empty()
            .push("submit".into(), actor(first_orgu), None)
            .push("submit".into(), actor(last_orgu), None);
        let c_orgu = COrgu::Anchor {
            from: AnchorFrom::Wfah {
                wfah: "submit".into(),
                field: "actor.orgu_id".into(),
                occurrence: Some("first".into()),
            },
            traverse: "self".into(),
        };
        resolve_c_orgu(
            &c_orgu,
            Uuid::new_v4(),
            &json!({}),
            &wfah,
            Uuid::nil(),
            &org,
        )
        .await
        .unwrap();
        let (called_anchor, _) = org.last_call.lock().unwrap().clone().unwrap();
        assert_eq!(called_anchor, first_orgu);
    }

    /// Çözülemeyen anchor aktörün birimine DÜŞMEZ — org'a hiç sorulmaz, küme boştur.
    ///
    /// Eski davranış (`unwrap_or(default_anchor)`) bir mantık hatasıydı: `traverse: "self"`
    /// ile kapı `actor.orgu ∈ {actor.orgu}` sorusuna dönüşüp DAİMA doğru oluyordu, yani
    /// kısıt kalkıyordu. Aşağıdaki iki assert birlikte onu geri getirmeyi engelliyor:
    /// küme boş OLMALI ve org'a çağrı YAPILMAMALI.
    #[tokio::test]
    async fn missing_anchor_resolves_to_empty_set() {
        let default_anchor = Uuid::new_v4();
        let org = org_with(vec![unit(default_anchor)]);
        let c_orgu = COrgu::Anchor {
            from: AnchorFrom::Ctx("$ctx.ghost".into()),
            traverse: "self".into(),
        };
        let result = resolve_c_orgu(
            &c_orgu,
            default_anchor,
            &json!({}),
            &Wfah::empty(),
            Uuid::nil(),
            &org,
        )
        .await
        .unwrap();
        assert!(
            result.is_empty(),
            "çözülemeyen anchor kimseyi yetkilendirmemeli"
        );
        assert!(
            org.last_call.lock().unwrap().is_none(),
            "anchor yoksa traversal hiç koşmamalı — aktörün birimiyle koşmak yetkiyi genişletirdi"
        );
    }

    /// Aynı kural, eksik WFAH anchor'ı için de geçerli (iki dal aynı yerden geçiyor).
    #[tokio::test]
    async fn missing_wfah_anchor_resolves_to_empty_set() {
        let default_anchor = Uuid::new_v4();
        let org = org_with(vec![unit(default_anchor)]);
        let c_orgu = COrgu::Anchor {
            from: AnchorFrom::Wfah {
                wfah: "hic_olmayan_aksiyon".into(),
                field: "actor.orgu".into(),
                occurrence: None,
            },
            traverse: "self".into(),
        };
        let result = resolve_c_orgu(
            &c_orgu,
            default_anchor,
            &json!({}),
            &Wfah::empty(),
            Uuid::nil(),
            &org,
        )
        .await
        .unwrap();
        assert!(result.is_empty());
        assert!(org.last_call.lock().unwrap().is_none());
    }
}
