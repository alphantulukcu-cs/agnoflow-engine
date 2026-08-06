//! v2.2 ZEN expression context'i (WOR-40, M7).
//! Namespace seti: $ctx, $wfah, $prev, $first, $node, $actor, $timestamp, $wfe_id,
//! $action.input.*, $exec.result.*, $call.* (WFC-RETURN bağlamı)

use crate::error::EngineError;
use crate::types::{
    actor::Actor,
    wfah::{Wfah, WfahEntry},
};
use serde_json::{json, Map, Value};
use uuid::Uuid;

/// Bir WFAH girdisinin ZEN'e açılan izdüşümü. `seq` ve `input` DE açıktır (WOR-84):
/// "önceki onayda girilen tutar" gibi koşullar aksi hâlde sessizce `null` okuyordu.
/// `input` ham `$action.input` ağacıdır (girdi ctx'e yazılmamış olsa da geçmişte durur).
fn project_entry(e: &WfahEntry) -> Value {
    json!({
        "seq": e.seq,
        "action": e.action,
        "actor": e.actor,
        "input": e.input.clone().unwrap_or(Value::Null),
        "at": crate::timestamp::timestamp_string(e.applied_at),
    })
}

/// `$prev`/`$first` boş geçmişte bu kabuğu döner. Neden `Value::Null` DEĞİL: null'ın
/// alanına erişmek ifadeyi patlatır; kabuk sayesinde `$prev.action == "x"` false okur
/// ($call ve $branches ile aynı gerekçe).
fn empty_entry_shell() -> Value {
    json!({
        "seq": Value::Null,
        "action": Value::Null,
        "actor": Value::Null,
        "input": Value::Null,
        "at": Value::Null,
    })
}

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
        self.wfah = wfah.entries().iter().map(project_entry).collect();
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
        // WOR-84: geçmişin uç girdilerine kısayol. `$wfah[len($wfah) - 1]` ifadesi BOŞ
        // geçmişte indeks -1'e düşüp VM'i patlatıyordu (parse aşaması yakalamaz); tasarımcı
        // her seferinde `len($wfah) > 0 and ...` guard'ı yazmak zorunda kalıyordu.
        map.insert(
            "$prev".into(),
            self.wfah.last().cloned().unwrap_or_else(empty_entry_shell),
        );
        map.insert(
            "$first".into(),
            self.wfah.first().cloned().unwrap_or_else(empty_entry_shell),
        );
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
            Value::from(crate::timestamp::now_timestamp()),
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

    /// WOR-84: `seq` ve `input` de izdüşümde — aksi hâlde `#.input.tutar` sessizce null.
    #[test]
    fn wfah_projection_exposes_seq_and_input() {
        let wfah = Wfah::empty()
            .push("start".into(), actor(), None)
            .push("skor_gir".into(), actor(), Some(json!({"tutar": 1500})));
        let env = EvalEnv::new(&json!({})).with_wfah(&wfah);
        assert!(evaluate_bool("$wfah[1].seq == 2", &env).unwrap());
        assert!(evaluate_bool("$wfah[1].input.tutar == 1500", &env).unwrap());
        // Sayısal karşılaştırma AKSİYONA KAPILANMALIDIR: yordam tüm geçmişte koşar ve
        // girdisi olmayan satırda `null > 1000` zen'de "Unsupported type" hatasıdır
        // (null'a `==` sorun değil, sıralama operatörleri sorun). Bu zen davranışıdır,
        // izdüşümün eksiği değil — `#.input` artık dolu geliyor.
        assert!(evaluate_bool(
            "some($wfah, #.action == 'skor_gir' and #.input.tutar > 1000)",
            &env
        )
        .unwrap());
        assert!(!evaluate_bool(
            "some($wfah, #.action == 'skor_gir' and #.input.tutar > 5000)",
            &env
        )
        .unwrap());
        assert!(evaluate_bool("some($wfah, #.input.tutar > 1000)", &env).is_err());
    }

    /// WOR-84: `$prev` = son giriş, `$first` = ilk giriş.
    #[test]
    fn prev_and_first_namespaces() {
        let wfah = Wfah::empty()
            .push("basvuru".into(), actor(), None)
            .push("analyst_approve".into(), actor(), Some(json!({"not": "ok"})));
        let env = EvalEnv::new(&json!({})).with_wfah(&wfah);
        assert!(evaluate_bool("$prev.action == 'analyst_approve'", &env).unwrap());
        assert!(evaluate_bool("$prev.seq == 2", &env).unwrap());
        assert!(evaluate_bool("$prev.input.not == 'ok'", &env).unwrap());
        assert!(evaluate_bool("$prev.actor.role == 'creditAnalyst'", &env).unwrap());
        assert!(evaluate_bool("$first.action == 'basvuru'", &env).unwrap());
        assert!(evaluate_bool("$first.seq == 1", &env).unwrap());
    }

    /// Tek girişli geçmişte `$prev` ve `$first` AYNI girdiyi gösterir.
    #[test]
    fn prev_equals_first_for_single_entry() {
        let wfah = Wfah::empty().push("basvuru".into(), actor(), None);
        let env = EvalEnv::new(&json!({})).with_wfah(&wfah);
        assert!(evaluate_bool("$prev.action == $first.action", &env).unwrap());
    }

    /// KRİTİK (WOR-84): boş geçmişte `$prev.*` PATLAMAZ, null okur. Elle yazılan
    /// `$wfah[len($wfah) - 1].action` burada VMError veriyordu.
    #[test]
    fn prev_is_null_shell_on_empty_history() {
        let env = EvalEnv::new(&json!({}));
        assert!(!evaluate_bool("$prev.action == 'x'", &env).unwrap());
        assert!(evaluate_bool("$prev.action != 'x'", &env).unwrap());
        assert!(evaluate_bool("$prev.action == null", &env).unwrap());
        assert!(!evaluate_bool("$first.action == 'x'", &env).unwrap());
        // Karşılaştırma: elle indeksleme aynı bağlamda hata döner.
        assert!(evaluate_bool("$wfah[len($wfah) - 1].action == 'x'", &env).is_err());
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
