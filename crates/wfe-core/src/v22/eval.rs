//! v2.2 ZEN expression context'i (WOR-40, M7).
//! Namespace seti: $ctx, $wfah, $prev, $first, $node, $actor, $timestamp, $wfe_id,
//! $action.input.*, $exec.result.*, $call.* (WFC-RETURN bağlamı)

use crate::error::EngineError;
use crate::types::{
    actor::Actor,
    wfah::{Wfah, WfahEntry},
};
use crate::v22::env::{self, PublicEnv};
use crate::v22::wfah_kind::parse_marker;
use serde_json::{json, Map, Value};
use uuid::Uuid;

/// Bir WFAH girdisinin ZEN'e açılan izdüşümü. `seq` ve `input` DE açıktır (WOR-84):
/// "önceki onayda girilen tutar" gibi koşullar aksi hâlde sessizce `null` okuyordu.
/// `input` ham `$action.input` ağacıdır (girdi ctx'e yazılmamış olsa da geçmişte durur).
///
/// E14: `branch_round` de açılır — `#.branch_entry == "hukuk"` iki turu TOPLAR, ayrım
/// yalnız bu alanla yazılabilir (`#.branch_round == $branch_round - 1`). Satırın kalan
/// v2.3 alanları (`from_node`/`to_node`/`branch_entry`) `E05` ile açılır; sıra
/// bağlayıcıdır (E14, E05'in ÖN ŞARTI).
fn project_entry(e: &WfahEntry) -> Value {
    json!({
        "seq": e.seq,
        "action": e.action,
        "actor": e.actor,
        "input": e.input.clone().unwrap_or(Value::Null),
        "at": crate::timestamp::timestamp_string(e.applied_at),
        "branch_round": e.branch_round,
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
        // E14: kabuk da alanı TAŞIR — `$prev.branch_round == 1` boş defterde false
        // okumalı, eksik alan yüzünden patlamamalı.
        "branch_round": Value::Null,
    })
}

/// Bir expression değerlendirmesinin görebileceği tüm adlar.
#[derive(Debug, Clone, Default)]
pub struct EvalEnv {
    pub ctx: Value,
    pub wfah: Vec<Value>,
    /// R04 — `$prev`/`$first`: defterin son/ilk **AKSİYON** satırının izdüşümü.
    ///
    /// Ham `$wfah` dizisinden AYRI tutulur (dizi süzülmez, R04/S3). `None` = defterde
    /// hiç aksiyon satırı yok → `zen_context` `empty_entry_shell` bağlar.
    ///
    /// `with_wfah` tarafından AYNI çağrıda kurulur: uçları ayrı bir `with_*` ile
    /// vermek, çağıranın birini bağlayıp diğerini atlamasına yer bırakırdı
    /// (`$branch_round` ile aynı gerekçe).
    pub prev: Option<Value>,
    pub first: Option<Value>,
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
    /// Ortam konfigürasyonu (`$env`) — **secret'sız** görünüm. Secret bir değer ZEN'e
    /// hiç girmez: girseydi bir `calc` ifadesi onu ctx'e yazar ve portalda görünürdü.
    pub env: PublicEnv,
    /// E14 — `$branch_round`: YAŞAYAN turun numarası, paralel mod dışında `None`.
    ///
    /// `with_wfah` tarafından DEFTERDEN türetilir (`valid::live_round`), ayrı bir
    /// `with_*` çağrısıyla verilmez: `$wfah` ile `$branch_round` aynı defterin iki
    /// okumasıdır ve tek yerde bağlanmazsa çağıran biri bağlayıp diğerini atlar —
    /// o zaman tasarımcının ifadesi bir yüzeyde çalışıp diğerinde sessizce false
    /// okurdu.
    pub branch_round: Option<u32>,
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
        // R04: uç kısayolları HAM defter üzerinde, AKSİYON satırlarına süzülerek
        // hesaplanır. Dizi süzülmez (`self.wfah` yukarıda ham kaldı); süzülen yalnız
        // iki uçtur. Ölçüt SINIFTIR (`parse_marker` → `WfahKind::Action`), koda ad
        // listesi girmez.
        //
        // Tarama O(n) ve WFAH boyu sınırlı; alternatif (izdüşümdeki `#.kind` üzerinden
        // filtrelemek) uç hesabını `E05`in alan kümesine bağımlı kılardı.
        let mut actions = wfah
            .entries()
            .iter()
            .filter(|e| parse_marker(&e.action).kind.is_action());
        self.first = actions.next().map(project_entry);
        self.prev = actions.last().map(project_entry).or_else(|| self.first.clone());
        // E14: tur, defterin bir okumasıdır — `$wfah` ile AYNI yerde bağlanır.
        self.branch_round = crate::v22::valid::live_round(wfah);
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

    /// Ortam konfigürasyonunu bağlar (`$env.*`). Secret'sız görünüm beklenir.
    pub fn with_env(mut self, env: &PublicEnv) -> Self {
        self.env = env.clone();
        self
    }

    fn zen_context(&self) -> Value {
        let mut map = Map::new();
        map.insert("$ctx".into(), self.ctx.clone());
        map.insert("$wfah".into(), Value::Array(self.wfah.clone()));
        // WOR-84: geçmişin uç girdilerine kısayol. `$wfah[len($wfah) - 1]` ifadesi BOŞ
        // geçmişte indeks -1'e düşüp VM'i patlatıyordu (parse aşaması yakalamaz); tasarımcı
        // her seferinde `len($wfah) > 0 and ...` guard'ı yazmak zorunda kalıyordu.
        //
        // R04: uçlar artık son/ilk **AKSİYON** satırıdır — `$wfah[len($wfah)-1]` ile
        // AYNI SATIRI VERMEZLER. Marker satırları (fork, escalation, sahiplik, trigger,
        // çağrı kapanışı, kol olayları) elenir; ham dizi DEĞİŞMEZ.
        map.insert(
            "$prev".into(),
            self.prev.clone().unwrap_or_else(empty_entry_shell),
        );
        map.insert(
            "$first".into(),
            self.first.clone().unwrap_or_else(empty_entry_shell),
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
        map.insert("$env".into(), self.env.to_json());
        // E14: paralel mod dışında `null` — karşılaştırma sessizce false okur, ifade
        // patlamaz ($call/$branches ile aynı gerekçe).
        map.insert(
            "$branch_round".into(),
            self.branch_round.map(Value::from).unwrap_or(Value::Null),
        );
        Value::Object(map)
    }
}

/// Değerlendirme öncesi `$env` ön-kontrolü.
///
/// ZEN'de eksik alan `null` okur; `$env` için bunu kabul edemeyiz — null bir domain
/// `https://null/...` üretir. ZEN'in attribute erişimine giremediğimiz için ifadenin METNİ
/// taranır: tanımsız (ya da secret olduğu için görünmeyen) bir anahtar, değerlendirme hiç
/// başlamadan hata verir. Gerekçenin tamamı: `v22::env` modül başlığı.
fn check_env_refs(expr: &str, env: &EvalEnv) -> Result<(), EngineError> {
    env::assert_refs_defined(expr, &env.env)
}

/// Boolean sonuç bekleyen değerlendirme (`when`, `terminal_when`, guard'lar).
pub fn evaluate_bool(expr: &str, env: &EvalEnv) -> Result<bool, EngineError> {
    check_env_refs(expr, env)?;
    let result = zen_expression::evaluate_expression(expr, env.zen_context().into())
        .map_err(|e| EngineError::ZenEvaluation(format!("'{expr}': {e}")))?;
    result
        .as_bool()
        .ok_or_else(|| EngineError::ZenEvaluation(format!("'{expr}' boolean sonuç üretmedi")))
}

/// Herhangi bir değer üreten değerlendirme (calc autoexec).
pub fn evaluate_value(expr: &str, env: &EvalEnv) -> Result<Value, EngineError> {
    check_env_refs(expr, env)?;
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

    /// E14 — `$branch_round` paralel mod dışında `null`. EŞİTLİK sessizce false okur;
    /// SIRALAMA ve ARİTMETİK ise zen'de PATLAR.
    ///
    /// Bu test o sınırı ÇİVİLER: E14 kaydı *"karşılaştırma sessizce false okur, ifade
    /// patlamaz"* diyor ve bu, kaydın kendi önerdiği kanonik yazım (`== $branch_round
    /// - 1`) için DOĞRU DEĞİL — `null - 1` `Opcode Subtract: Unsupported type` verir.
    /// Tasarımcı paralel modun dışında da değerlendirilebilecek bir ifadede kapı
    /// yazmak zorundadır: `$branch_round != null and some(...)` (zen `and`i tembel
    /// değerlendirir; `#.input.*` sıralama kapısıyla AYNI desen). Kapının tasarım
    /// zamanında zorlanması ayrı bir karar ister — bkz. `zen_input_needs_action_gate`
    /// emsali.
    #[test]
    fn branch_round_is_null_outside_parallel_mode() {
        let wfah = Wfah::empty().push("basvuru".into(), actor(), None);
        let env = EvalEnv::new(&json!({})).with_wfah(&wfah);
        assert!(evaluate_bool("$branch_round == null", &env).unwrap());
        assert!(!evaluate_bool("$branch_round == 1", &env).unwrap());
        assert!(!evaluate_bool("$branch_round in [1, 2]", &env).unwrap());
        // SINIR 1: SIRALAMA karşılaştırması null'da PATLAR (`Opcode Compare:
        // Unsupported type`) — projedeki `#.input.*` sıralama tuzağının aynısı.
        assert!(evaluate_bool("$branch_round > 0", &env).is_err());
        // SINIR 2: ARİTMETİK de patlar (`Opcode Subtract: Unsupported type`), yani
        // kaydın kanonik yazımı kapısız hâlde paralel mod dışında 500 üretir.
        assert!(evaluate_value("$branch_round - 1", &env).is_err());
        // Kapı ÇALIŞIR: zen `and`i tembel değerlendirir.
        assert!(!evaluate_bool(
            "$branch_round != null and $branch_round - 1 == 0",
            &env
        )
        .unwrap());
        // Kolda olmayan satırın alanı da `null` — ham `$wfah` üzerinde sayım
        // yazan tasarımcı hata almaz, sadece eşleşme bulamaz.
        assert!(!evaluate_bool("some($wfah, #.branch_round == 1)", &env).unwrap());
    }

    /// E14 — GEÇEN TURU bulmanın kanonik yazımı: `$branch_round - 1`.
    /// Kural 5 `$valid`ten eleyecek olsa da soru HAM `$wfah` üzerinde sorulabilir.
    #[test]
    fn previous_round_is_addressable_on_the_raw_ledger() {
        let branch_row = |seq: u32, action: &str, round: u32| WfahEntry {
            seq,
            action: action.into(),
            actor: actor(),
            input: None,
            applied_at: chrono::Utc::now(),
            from_node: None,
            to_node: None,
            branch_entry: Some("hukuk".into()),
            branch_round: Some(round),
        };
        let fork = |seq: u32| WfahEntry {
            seq,
            action: "_fork".into(),
            actor: actor(),
            input: Some(json!({"branches": ["hukuk"]})),
            applied_at: chrono::Utc::now(),
            from_node: None,
            to_node: None,
            branch_entry: None,
            branch_round: None,
        };
        let closer = |seq: u32| WfahEntry {
            seq,
            action: "_join".into(),
            actor: actor(),
            input: None,
            applied_at: chrono::Utc::now(),
            from_node: None,
            to_node: None,
            branch_entry: None,
            branch_round: None,
        };
        // 1. tur onaylandı, join doldu, fork'a İKİNCİ kez girildi.
        let wfah = Wfah(vec![
            fork(1),
            branch_row(2, "hukuk_onay", 1),
            closer(3),
            fork(4),
            branch_row(5, "hukuk_inceleme", 2),
        ]);
        let env = EvalEnv::new(&json!({})).with_wfah(&wfah);
        assert!(evaluate_bool("$branch_round == 2", &env).unwrap());
        assert!(
            evaluate_bool(r#"some($wfah, #.branch_round == $branch_round - 1)"#, &env).unwrap(),
            "geçen turun satırı bulunmalı"
        );
        assert!(
            !evaluate_bool(
                r#"some($wfah, #.branch_round == $branch_round - 2)"#,
                &env
            )
            .unwrap(),
            "iki tur öncesi yok"
        );
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

    /// R04/S1 — uçlar son/ilk **AKSİYON** satırıdır; marker satırları ELENİR.
    ///
    /// Kapı (a): fork commit'i `_fork` marker'ını yazar ve editörün ürettiği gate
    /// (`$prev.action == "start_review"`) fork biter bitmez ters dönüyordu. Ölçüt
    /// SINIF olduğu için fork AKSİYONU görünür kalır, `_fork` MARKER'ı elenir.
    #[test]
    fn prev_sees_the_last_action_row_not_the_marker() {
        let wfah = Wfah::empty()
            .push("basvuru".into(), actor(), None)
            .push("start_review".into(), actor(), None)
            .push("_fork".into(), actor(), Some(json!({"branches": ["hukuk"]})));
        let env = EvalEnv::new(&json!({})).with_wfah(&wfah);
        assert!(evaluate_bool("$prev.action == 'start_review'", &env).unwrap());
        assert!(evaluate_bool("$prev.seq == 2", &env).unwrap());
        assert!(evaluate_bool("$first.action == 'basvuru'", &env).unwrap());
    }

    /// Kapı (b): sahiplik / escalation / trigger / kol / çağrı KAPANIŞ marker'larının
    /// hiçbiri ucu kaydırmaz. Liste kapsayıcıdır ama ölçüt ADA bakmaz — yeni bir
    /// marker türü eklendiğinde bu testi güncellemek gerekmez, `WfahKind` zorlar.
    #[test]
    fn markers_do_not_move_the_ends() {
        for marker in [
            "claim_taken:self__memur",
            "claim_released:self__memur",
            "escalate:self__memur:0",
            "escalate:self__memur:0:skipped",
            "trigger:use_skor",
            "timeout:deadline",
            "_fork",
            "_branch_arrived",
            "_branch_cancelled",
            "_branch_superseded",
            "_collapse",
            "_join",
            "call:krediler",
            "call:krediler/…",
        ] {
            let wfah = Wfah::empty()
                .push("onayla".into(), actor(), None)
                .push(marker.into(), actor(), None);
            let env = EvalEnv::new(&json!({})).with_wfah(&wfah);
            assert!(
                evaluate_bool("$prev.action == 'onayla'", &env).unwrap(),
                "'{marker}' $prev'i kaydırmamalı"
            );
        }
    }

    /// Kapı (c): alt akış AKSİYONU (`call:<anahtar>/<aksiyon>`) özyinelemeli
    /// çözümlemeyle `Action`a düşer → uçta GÖRÜNÜR. Bilinçli bedel (R04, FEDA
    /// EDİLENLER): alt akış opak DEĞİLDİR, çağıranın `$prev`i onu görebilir.
    /// Değer `action` alanının HAM hâlidir — izdüşüm marker adını sökmez.
    #[test]
    fn sub_flow_actions_stay_visible_at_the_ends() {
        let wfah = Wfah::empty()
            .push("onayla".into(), actor(), None)
            .push("call:krediler/skor_gir".into(), actor(), None);
        let env = EvalEnv::new(&json!({})).with_wfah(&wfah);
        assert!(evaluate_bool("$prev.action == 'call:krediler/skor_gir'", &env).unwrap());
    }

    /// Kapı (d): YALNIZ marker taşıyan defterde uçlar kabuk döner — ifade PATLAMAZ,
    /// hep-false okur (boş defterle aynı davranış).
    #[test]
    fn marker_only_ledger_yields_the_empty_shell() {
        let wfah = Wfah::empty()
            .push("_fork".into(), actor(), None)
            .push("trigger:use_skor".into(), actor(), None);
        let env = EvalEnv::new(&json!({})).with_wfah(&wfah);
        assert!(evaluate_bool("$prev.action == null", &env).unwrap());
        assert!(!evaluate_bool("$prev.action == '_fork'", &env).unwrap());
        assert!(evaluate_bool("$first.action == null", &env).unwrap());
        assert!(evaluate_bool("$prev.branch_round == null", &env).unwrap());
    }

    /// Kapı (e): dizi HAM kalır (R04/S3) — yayınlanmış `count($wfah, …)` sayımları
    /// marker'ları saymaya DEVAM eder. Süzülen yalnız iki uçtur.
    #[test]
    fn the_array_itself_stays_raw() {
        let wfah = Wfah::empty()
            .push("onayla".into(), actor(), None)
            .push("_fork".into(), actor(), None)
            .push("escalate:self__memur:0".into(), actor(), None);
        let env = EvalEnv::new(&json!({})).with_wfah(&wfah);
        assert!(evaluate_bool("len($wfah) == 3", &env).unwrap());
        assert!(evaluate_bool("some($wfah, #.action == '_fork')", &env).unwrap());
        assert!(
            evaluate_bool("count($wfah, #.action == 'escalate:self__memur:0') == 1", &env).unwrap()
        );
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

    fn env_set() -> crate::v22::env::EnvSet {
        use crate::v22::env::{EnvSet, EnvValue};
        EnvSet::new(std::collections::BTreeMap::from([
            ("REGION".to_string(), EnvValue::public(json!("tr-central"))),
            ("MAX_TUTAR".to_string(), EnvValue::public(json!(50000))),
            ("DEBUG".to_string(), EnvValue::public(json!(true))),
            (
                "API_KEY".to_string(),
                EnvValue::secret(json!("sk-live-xyz")),
            ),
        ]))
    }

    /// `$env` ZEN'de okunur ve TİPLİ gelir — `value_type` bu yüzden var: string tutulsaydı
    /// `$env.MAX_TUTAR > 1000` zen'de "Compare: Unsupported type" verirdi.
    #[test]
    fn env_namespace() {
        let e = EvalEnv::new(&json!({})).with_env(&env_set().public());
        assert!(evaluate_bool("$env.REGION == 'tr-central'", &e).unwrap());
        assert!(evaluate_bool("$env.MAX_TUTAR > 1000", &e).unwrap());
        assert!(evaluate_bool("$env.DEBUG", &e).unwrap());
        assert_eq!(evaluate_value("$env.MAX_TUTAR", &e).unwrap(), json!(50000));
    }

    /// KRİTİK: secret bir anahtar ZEN'de YOKTUR. Olsaydı bir `calc` ifadesi onu ctx'e
    /// yazar ve portalda görünürdü. Sessizce `null` da okumaz — hata verir.
    #[test]
    fn secret_is_invisible_to_zen() {
        let e = EvalEnv::new(&json!({})).with_env(&env_set().public());
        let err = evaluate_bool("$env.API_KEY == 'sk-live-xyz'", &e)
            .unwrap_err()
            .to_string();
        assert!(err.contains("API_KEY"), "{err}");
        assert_eq!(e.zen_context()["$env"].get("API_KEY"), None);
    }

    /// Tanımsız anahtar `$ctx`'in aksine null okumaz — değerlendirme başlamadan patlar.
    #[test]
    fn undefined_env_key_fails_before_eval() {
        let e = EvalEnv::new(&json!({})).with_env(&env_set().public());
        assert!(evaluate_bool("$env.YOK == 'x'", &e).is_err());
        // Ortam hiç bağlanmamışsa da aynı: sessiz null yok.
        assert!(evaluate_bool("$env.REGION == 'x'", &EvalEnv::new(&json!({}))).is_err());
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
