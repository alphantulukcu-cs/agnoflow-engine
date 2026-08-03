//! v2.2 ZEN expression context'i (WOR-40, M7).
//! Namespace seti: $ctx, $wfah, $node, $actor, $timestamp, $wfe_id,
//! $action.input.*, $exec.result.*, $call.* (WFC-RETURN bağlamı)

use crate::error::EngineError;
use crate::types::{actor::Actor, wfah::Wfah};
use serde_json::{json, Map, Value};
use uuid::Uuid;

/// Bir expression değerlendirmesinin görebileceği tüm adlar.
#[derive(Debug, Clone, Default)]
pub struct EvalEnv {
    pub ctx: Value,
    pub wfah: Vec<Value>,
    pub node: Option<String>,
    pub actor: Option<Actor>,
    pub wfe_id: Option<Uuid>,
    pub action_input: Option<Value>,
    pub exec_result: Option<Value>,
    /// WFC-OUT — yalnız WFC-RETURN bağlamında bağlanır (`$call.*`).
    pub call: Option<CallOutcome>,
    /// WOR-73 — yalnız paralel join koşulu (`join_when`) değerlendirilirken bağlanır
    /// (`$branches.*`, `$arrived`).
    pub join: Option<JoinEnv>,
}

/// WOR-73: join koşulunun gördüğü kol durumu. Kol kimliği **giriş node'udur**
/// (`BranchState::entry_node`) — `branch_node` kol içinde aksiyon alındıkça değişir,
/// dolayısıyla ifadede kullanılamaz.
#[derive(Debug, Clone, Default)]
pub struct JoinEnv {
    /// Fork'un TÜM kollarının giriş node'ları (sıra = `parallel.branches` sırası).
    pub all: Vec<String>,
    /// Join'e VARMIŞ kolların giriş node'ları — değerlendirilen varış DAHİL.
    pub arrived: Vec<String>,
}

impl JoinEnv {
    fn to_json(&self) -> (Value, Value) {
        // `$branches` her kol için bool taşır: hiç varmamış kol `false` döner
        // (eksik alanın null olmasına güvenmek zorunda kalınmasın).
        let map: Map<String, Value> = self
            .all
            .iter()
            .map(|b| (b.clone(), Value::Bool(self.arrived.contains(b))))
            .collect();
        (
            Value::Object(map),
            Value::Array(self.arrived.iter().cloned().map(Value::from).collect()),
        )
    }
}

/// Çağrılan WFE'nin sonucu — `$call.result.*` / `$call.status` / `$call.wfe_id`.
/// `$exec.result.*` ile BİRLEŞTİRİLMEZ: autoexec bir sistem çağrısıdır, WFC bir WFE
/// örneğidir; ayrı kavramlar ayrı namespace taşır.
#[derive(Debug, Clone)]
pub struct CallOutcome {
    /// Çağrılanın `wfe_end_response`'u. `detached` modda daima `Value::Null`.
    pub result: Value,
    /// "completed" | "failed" | "terminated" | "timeout" | "started"
    pub status: String,
    pub wfe_id: Option<Uuid>,
}

impl CallOutcome {
    fn to_json(&self) -> Value {
        json!({
            "result": self.result.clone(),
            "status": self.status.clone(),
            "wfe_id": self.wfe_id.map(|id| Value::from(id.to_string())).unwrap_or(Value::Null),
        })
    }
}

impl EvalEnv {
    pub fn new(ctx: &Value) -> Self {
        Self {
            ctx: ctx.clone(),
            ..Default::default()
        }
    }

    pub fn with_wfah(mut self, wfah: &Wfah) -> Self {
        self.wfah = wfah
            .entries()
            .iter()
            .map(|e| {
                json!({
                    "action": e.action,
                    "actor": e.actor,
                    "at": e.applied_at.to_rfc3339(),
                })
            })
            .collect();
        self
    }

    pub fn with_node(mut self, node: Option<&str>) -> Self {
        self.node = node.map(String::from);
        self
    }

    pub fn with_actor(mut self, actor: &Actor) -> Self {
        self.actor = Some(actor.clone());
        self
    }

    pub fn with_wfe_id(mut self, wfe_id: Uuid) -> Self {
        self.wfe_id = Some(wfe_id);
        self
    }

    pub fn with_action_input(mut self, input: &Value) -> Self {
        self.action_input = Some(input.clone());
        self
    }

    pub fn with_exec_result(mut self, result: &Value) -> Self {
        self.exec_result = Some(result.clone());
        self
    }

    /// WFC-RETURN bağlamı — `$call.*` bu çağrıyla görünür olur.
    pub fn with_call(mut self, call: CallOutcome) -> Self {
        self.call = Some(call);
        self
    }

    /// WOR-73: paralel join koşulu bağlamı — `$branches.*` ve `$arrived` görünür olur.
    pub fn with_join(mut self, join: JoinEnv) -> Self {
        self.join = Some(join);
        self
    }

    fn zen_context(&self) -> Value {
        let mut map = Map::new();
        map.insert("$ctx".into(), self.ctx.clone());
        map.insert("$wfah".into(), Value::Array(self.wfah.clone()));
        map.insert(
            "$node".into(),
            self.node.as_deref().map(Value::from).unwrap_or(Value::Null),
        );
        map.insert(
            "$actor".into(),
            self.actor
                .as_ref()
                .and_then(|a| serde_json::to_value(a).ok())
                .unwrap_or(Value::Null),
        );
        map.insert(
            "$wfe_id".into(),
            self.wfe_id
                .map(|id| Value::from(id.to_string()))
                .unwrap_or(Value::Null),
        );
        map.insert(
            "$action".into(),
            json!({ "input": self.action_input.clone().unwrap_or(Value::Null) }),
        );
        map.insert(
            "$exec".into(),
            json!({ "result": self.exec_result.clone().unwrap_or(Value::Null) }),
        );
        // WFC-RETURN dışındaki bağlamlarda `$call` boş bir kabuktur — `$call.status`
        // null döner, ifade patlamaz (eksik ctx alanının null olması gibi).
        map.insert(
            "$call".into(),
            self.call.as_ref().map(CallOutcome::to_json).unwrap_or_else(
                || json!({ "result": Value::Null, "status": Value::Null, "wfe_id": Value::Null }),
            ),
        );
        map.insert(
            "$timestamp".into(),
            Value::from(chrono::Utc::now().to_rfc3339()),
        );
        // WOR-73: join bağlamı DIŞINDA `$branches` boş obje, `$arrived` boş dizidir —
        // `$call` ile aynı gerekçe: ifade patlamak yerine "hiç kol varmamış" okur.
        let (branches, arrived) = self
            .join
            .as_ref()
            .map(JoinEnv::to_json)
            .unwrap_or_else(|| (Value::Object(Map::new()), Value::Array(vec![])));
        map.insert("$branches".into(), branches);
        map.insert("$arrived".into(), arrived);
        Value::Object(map)
    }
}

/// Boolean sonuç bekleyen değerlendirme (`when`, `terminal_when`, guard'lar).
pub fn evaluate_bool(expr: &str, env: &EvalEnv) -> Result<bool, EngineError> {
    let result = zen_expression::evaluate_expression(expr, env.zen_context().into())
        .map_err(|e| EngineError::ZenEvaluation(format!("'{expr}': {e}")))?;
    result
        .as_bool()
        .ok_or_else(|| EngineError::ZenEvaluation(format!("'{expr}' boolean sonuç üretmedi")))
}

/// Herhangi bir değer üreten değerlendirme (calc autoexec).
pub fn evaluate_value(expr: &str, env: &EvalEnv) -> Result<Value, EngineError> {
    let result = zen_expression::evaluate_expression(expr, env.zen_context().into())
        .map_err(|e| EngineError::ZenEvaluation(format!("'{expr}': {e}")))?;
    serde_json::to_value(result)
        .map_err(|e| EngineError::ZenEvaluation(format!("'{expr}' sonucu serileştirilemedi: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor() -> Actor {
        Actor {
            orgu_id: Uuid::nil(),
            user_id: Uuid::nil(),
            role: "creditAnalyst".into(),
        }
    }

    #[test]
    fn ctx_namespace() {
        let env = EvalEnv::new(&json!({"score_fetch_failed": true, "credit_score": 720}));
        assert!(evaluate_bool("$ctx.score_fetch_failed == true", &env).unwrap());
        assert!(evaluate_bool("$ctx.credit_score >= 700", &env).unwrap());
        assert!(!evaluate_bool("$ctx.credit_score < 700", &env).unwrap());
    }

    #[test]
    fn missing_ctx_field_is_null_not_error() {
        let env = EvalEnv::new(&json!({}));
        assert!(!evaluate_bool("$ctx.within_limit == true", &env).unwrap());
        assert!(evaluate_bool("$ctx.within_limit != true", &env).unwrap());
    }

    /// WOR-73: `$branches` her kol için bool taşır, `$arrived` varmış kolların dizisi.
    #[test]
    fn join_namespace() {
        let env = EvalEnv::new(&json!({})).with_join(JoinEnv {
            all: vec!["self__fin".into(), "self__legal".into(), "self__hr".into()],
            arrived: vec!["self__fin".into(), "self__hr".into()],
        });
        assert!(evaluate_bool("$branches.self__fin", &env).unwrap());
        assert!(!evaluate_bool("$branches.self__legal", &env).unwrap());
        assert!(evaluate_bool(
            "($branches.self__fin and $branches.self__legal) or $branches.self__hr",
            &env
        )
        .unwrap());
        assert!(evaluate_bool("len($arrived) >= 2", &env).unwrap());
        assert!(!evaluate_bool("len($arrived) >= 3", &env).unwrap());
        assert!(evaluate_bool("'self__hr' in $arrived", &env).unwrap());
    }

    /// Join bağlamı DIŞINDA ifade patlamaz: `$branches.x` null → false, `$arrived` boş.
    #[test]
    fn join_namespace_is_empty_outside_join_context() {
        let env = EvalEnv::new(&json!({}));
        assert!(!evaluate_bool("$branches.self__fin == true", &env).unwrap());
        assert!(evaluate_bool("len($arrived) == 0", &env).unwrap());
    }

    #[test]
    fn node_namespace() {
        let env = EvalEnv::new(&json!({})).with_node(Some("self__creditAnalyst"));
        assert!(evaluate_bool("$node == 'self__creditAnalyst'", &env).unwrap());
    }

    #[test]
    fn action_input_namespace() {
        let env =
            EvalEnv::new(&json!({})).with_action_input(&json!({"manager_decision": "approve"}));
        assert!(evaluate_bool("$action.input.manager_decision == 'approve'", &env).unwrap());
    }

    #[test]
    fn exec_result_namespace() {
        let env = EvalEnv::new(&json!({})).with_exec_result(&json!({"score": 750}));
        assert!(evaluate_bool("$exec.result.score > 700", &env).unwrap());
    }

    #[test]
    fn wfah_namespace_supports_zen_functions() {
        let wfah = Wfah::empty().push("start".into(), actor(), None).push(
            "analyst_approve".into(),
            actor(),
            None,
        );
        let env = EvalEnv::new(&json!({})).with_wfah(&wfah);
        assert!(evaluate_bool(
            "len(filter($wfah, #.action == 'analyst_approve')) >= 1",
            &env
        )
        .unwrap());
        assert!(evaluate_bool("some($wfah, #.action == 'start')", &env).unwrap());
    }

    #[test]
    fn actor_namespace() {
        let env = EvalEnv::new(&json!({})).with_actor(&actor());
        assert!(evaluate_bool("$actor.role == 'creditAnalyst'", &env).unwrap());
    }

    #[test]
    fn non_boolean_result_is_error() {
        let env = EvalEnv::new(&json!({"x": 5}));
        assert!(evaluate_bool("$ctx.x + 1", &env).is_err());
    }

    #[test]
    fn evaluate_value_returns_json() {
        let env = EvalEnv::new(&json!({"amount": 400, "limit": 1000}));
        let v = evaluate_value("$ctx.amount <= $ctx.limit", &env).unwrap();
        assert_eq!(v, json!(true));
        let v = evaluate_value("$ctx.amount / 4", &env).unwrap();
        assert_eq!(v, json!(100));
    }
}
