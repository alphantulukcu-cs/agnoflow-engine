//! v2.2 c_orgu çözümlemesi — Selector (ORGTRVLANG) ve Anchor (ctx / wfah) formları.
//! Spec: Terminology_v2_2.MD; wfah occurrence default "last" (M9).

use crate::error::EngineError;
use crate::ports::OrgPort;
use crate::types::actor::OrgUnit;
use crate::types::wfah::Wfah;
use crate::types::wfd_v22::{AnchorFrom, COrgu};
use serde_json::Value;
use uuid::Uuid;

/// Bir c_orgu ifadesini ORGU kümesine çözer.
/// `default_anchor` — anchor çözülemezse kullanılacak ORGU (genelde actor'ün orgu'su).
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
            let anchor = resolve_anchor(from, ctx, wfah)?.unwrap_or(default_anchor);
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
        resolve_c_orgu(&c_orgu, Uuid::new_v4(), &ctx, &Wfah::empty(), Uuid::nil(), &org)
            .await
            .unwrap();
        let (called_anchor, expr) = org.last_call.lock().unwrap().clone().unwrap();
        assert_eq!(called_anchor, stored_orgu, "anchor ctx'teki actor'ün orgu'su olmalı");
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
        resolve_c_orgu(&c_orgu, Uuid::new_v4(), &json!({}), &wfah, Uuid::nil(), &org)
            .await
            .unwrap();
        let (called_anchor, _) = org.last_call.lock().unwrap().clone().unwrap();
        assert_eq!(called_anchor, last_orgu, "default occurrence 'last' olmalı (M9)");
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
        resolve_c_orgu(&c_orgu, Uuid::new_v4(), &json!({}), &wfah, Uuid::nil(), &org)
            .await
            .unwrap();
        let (called_anchor, _) = org.last_call.lock().unwrap().clone().unwrap();
        assert_eq!(called_anchor, first_orgu);
    }

    #[tokio::test]
    async fn missing_anchor_falls_back_to_default() {
        let default_anchor = Uuid::new_v4();
        let org = org_with(vec![]);
        let c_orgu = COrgu::Anchor {
            from: AnchorFrom::Ctx("$ctx.ghost".into()),
            traverse: "self".into(),
        };
        resolve_c_orgu(&c_orgu, default_anchor, &json!({}), &Wfah::empty(), Uuid::nil(), &org)
            .await
            .unwrap();
        let (called_anchor, _) = org.last_call.lock().unwrap().clone().unwrap();
        assert_eq!(called_anchor, default_anchor);
    }
}
