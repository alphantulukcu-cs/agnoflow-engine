//! M9/WOR-42 — wfes_effects uygulama + $-string çözümleme.
//! v2.2'de {ref}/{ctx} obje formları ve `_step_` injection KALDIRILDI;
//! effect değerleri düz JSON'dur, $-önekli string'ler çözülür:
//! `$actor`, `$timestamp`, `$wfe_id`, `$node`, `$ctx.<path>`,
//! `$action.input.<path>`, `$exec.result.<path>`, `$call.*` (WFC-RETURN).

use crate::error::EngineError;
use crate::types::actor::Actor;
use crate::types::wfah::Wfah;
use crate::types::wfd_v22::WfesEffects;
use crate::v22::env::{self, PublicEnv};
use crate::v22::eval::{evaluate_bool, CallOutcome, EvalEnv};
use crate::v22::valid::ValidRules;
use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use uuid::Uuid;

#[derive(Clone)]
pub struct EffectEnv<'a> {
    pub actor: &'a Actor,
    pub wfe_id: Uuid,
    pub node: Option<&'a str>,
    pub action_input: Option<&'a Value>,
    pub exec_result: Option<&'a Value>,
    /// WFC-OUT — yalnız WFC-RETURN bağlamında `Some`. Diğer bağlamlarda `$call.*`
    /// çözülmez ve `null` yazar (validator `call_result_in_detached` /
    /// `call_next_result_ref` bu durumu tasarım anında yakalar).
    pub call: Option<&'a CallOutcome>,
    /// Ortam konfigürasyonu (`$env`) — **secret'sız** görünüm. Effects ctx'e yazar;
    /// secret bir değerin buradan geçmesi onu portalda görünür kılardı.
    pub env: &'a PublicEnv,
    pub now: DateTime<Utc>,
    /// `E09`/B — `set_when[].when`in gördüğü defter. Alan olmadan `Ç10`un
    /// *"`evaluate_bool` ile değerlendirilir"* satırı DERLENMİYORDU.
    ///
    /// ⚠️ **Yalnız COMMIT EDİLMİŞ geçmiş** (`E09`/B2): o turda üretilmekte olan
    /// satırlar (`wfah_entries` yerel vektörü) buraya GİRMEZ — autoexec effect'i kendi
    /// `trigger:` satırını görmez. B'nin tek gerekçesi *"`when` her yerde AYNI şeyi
    /// görür"*di; uçuştaki satırları eklemek yönlendirme `when`i ile effect `when`i
    /// arasında yeni bir ayrım açardı.
    pub wfah: &'a Wfah,
    /// `$wfah` izdüşümünün eleme/hesaplama kuralları (`E05`). Karar yazıldığında
    /// `with_wfah` tek argümanlıydı; `E05` `$valid` yüzeyini açınca kural seti
    /// parametre oldu. Emsal `grants::matches_grant_rules`: *"bu fonksiyonun elinde
    /// WFD olmadığı için kural seti parametredir"*.
    pub valid_rules: &'a ValidRules,
}

/// Effects'i staged ctx üzerine uygular; yeni bir ctx döner (immutable).
/// Set path'leri dotted olabilir — ara objeler oluşturulur.
///
/// WOR-70b — **gönderilmeyen opsiyonel input `null` yazar.** Her `set` satırı KOŞULSUZ
/// uygulanır; `$action.input.<yol>` çözülemezse (yol istekte yok) `null` yazılır. Bu,
/// `required`/`optional` ayrımının tek anlamıdır: ikisi de effects ile ctx'e eşlenmek
/// zorundadır (validator `unused_action_input`), ama `required` gönderilmek zorunda ve
/// `null` olamaz (pipeline `validate_action_input`), `optional` gönderilmezse alan
/// `null` kalır.
///
/// Sonuç: bir alanı hem opsiyonel girdi hem başka bir yazar (escalation/autoexec)
/// yazıyorsa, girdi gönderilmediğinde önceki değer `null`'a döner. Validator bunu
/// tasarım anında `optional_input_nulls_other_writer` UYARISI ile bildirir (yayın
/// engellenmez — bilinçli bir tasarım olabilir).
pub fn apply_effects(
    ctx: &Value,
    effects: &WfesEffects,
    env: &EffectEnv<'_>,
) -> Result<Value, EngineError> {
    let mut root = match ctx {
        Value::Object(m) => m.clone(),
        _ => Map::new(),
    };
    for (path, raw) in &effects.set {
        let resolved = resolve_value(raw, ctx, env)?;
        set_path(&mut root, path, resolved);
    }

    // `Ç10`/c + `E09` — KOŞULA BAĞLI yazım.
    //
    // Sıra anlam taşır: `set`ten SONRA, dizi sırasıyla. Koşulu uyan BÜTÜN girdiler
    // uygulanır (ilk-match DEĞİL) ve aynı alana yazan iki girdide SONRAKİ kazanır.
    //
    // ⚠️ Değerlendirme ortamı BİR KEZ kurulur ve GİRDİ ctx'ini taşır (`E09`/B3):
    // yukarıdaki `set` döngüsünün ve önceki girdilerin yazdıkları koşula GÖRÜNMEZ.
    // Bu, `resolve_value` semantiğinin aynısıdır — effect değerleri de daima girdi
    // ctx'ini görür, yazılmakta olan `root`u değil. Zincirleme koşul AÇILMAZ.
    //
    // ⚠️ Koşul değerlendirmesi bu fonksiyondan DIŞARI ÇIKMAZ (`Ç10`: "uygulama tek
    // fonksiyondan geçer"). Çağıranın süzmesi seçeneği o hükmü doğrudan ihlal ederdi.
    if !effects.set_when.is_empty() {
        let eval_env = EvalEnv::new(ctx)
            .with_wfah(env.wfah, env.valid_rules)
            .with_actor(env.actor)
            .with_wfe_id(env.wfe_id)
            .with_node(env.node)
            .with_env(env.env);
        let eval_env = match env.action_input {
            Some(input) => eval_env.with_action_input(input),
            None => eval_env,
        };
        for entry in &effects.set_when {
            // `E09`/S4: çalışma anı hatası (ZEN patlaması ya da "boolean sonuç
            // üretmedi") COMMIT'İ DÜŞÜRÜR — girdi sessizce ATLANMAZ. `set` tarafında
            // `resolve_value` hatası zaten commit'i düşürüyor; aynı blokta iki farklı
            // hata rejimi olmaz.
            if !evaluate_bool(&entry.when, &eval_env)? {
                continue;
            }
            for (path, raw) in &entry.set {
                let resolved = resolve_value(raw, ctx, env)?;
                set_path(&mut root, path, resolved);
            }
        }
    }
    Ok(Value::Object(root))
}

/// Bir effect/terminal değerini çözer. String'ler $-kurallarına göre,
/// obje/array'ler recursive işlenir, diğerleri literal kalır.
pub fn resolve_value(raw: &Value, ctx: &Value, env: &EffectEnv<'_>) -> Result<Value, EngineError> {
    match raw {
        Value::String(s) => resolve_dollar_string(s, ctx, env),
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                out.insert(k.clone(), resolve_value(v, ctx, env)?);
            }
            Ok(Value::Object(out))
        }
        Value::Array(arr) => Ok(Value::Array(
            arr.iter()
                .map(|v| resolve_value(v, ctx, env))
                .collect::<Result<_, _>>()?,
        )),
        other => Ok(other.clone()),
    }
}

fn resolve_dollar_string(s: &str, ctx: &Value, env: &EffectEnv<'_>) -> Result<Value, EngineError> {
    // `$env` EN BAŞTA: diğer $-formlarının aksine ara-değer de çözülür, yani string'in
    // tamamı olmak zorunda değil. İçinde `$env.` geçmiyorsa `None` döner ve akış aşağıdaki
    // tam-eşleşme kurallarına devam eder.
    if let Some(v) = env::resolve_string(s, env.env)? {
        return Ok(v);
    }
    match s {
        "$actor" => serde_json::to_value(env.actor)
            .map_err(|e| EngineError::EffectValue(format!("$actor serileştirilemedi: {e}"))),
        "$timestamp" => Ok(Value::from(crate::timestamp::timestamp_string(env.now))),
        "$wfe_id" => Ok(Value::from(env.wfe_id.to_string())),
        "$node" => Ok(env.node.map(Value::from).unwrap_or(Value::Null)),
        _ => {
            if let Some(path) = s.strip_prefix("$ctx.") {
                return Ok(get_path(ctx, path).cloned().unwrap_or(Value::Null));
            }
            if let Some(path) = s.strip_prefix("$action.input.") {
                let input = env.action_input.unwrap_or(&Value::Null);
                return Ok(get_path(input, path).cloned().unwrap_or(Value::Null));
            }
            if let Some(path) = s.strip_prefix("$exec.result.") {
                let result = env.exec_result.unwrap_or(&Value::Null);
                return Ok(get_path(result, path).cloned().unwrap_or(Value::Null));
            }
            if let Some(path) = s.strip_prefix("$call.result.") {
                let result = env.call.map(|c| &c.result).unwrap_or(&Value::Null);
                return Ok(get_path(result, path).cloned().unwrap_or(Value::Null));
            }
            if s == "$call.status" {
                return Ok(env
                    .call
                    .map(|c| Value::from(c.status.clone()))
                    .unwrap_or(Value::Null));
            }
            if s == "$call.wfe_id" {
                return Ok(env
                    .call
                    .and_then(|c| c.wfe_id)
                    .map(|id| Value::from(id.to_string()))
                    .unwrap_or(Value::Null));
            }
            if s.starts_with("$exec.response.") {
                return Err(EngineError::EffectValue(
                    "'$exec.response.*' kaldırıldı (M7) — '$exec.result.*' kullanın".into(),
                ));
            }
            Ok(Value::from(s))
        }
    }
}

/// Dotted path okuma.
pub fn get_path<'a>(value: &'a Value, dotted: &str) -> Option<&'a Value> {
    let mut current = value;
    for part in dotted.split('.') {
        current = current.get(part)?;
    }
    Some(current)
}

/// Dotted path yazma — ara segmentler obje değilse objeyle değiştirilir.
pub fn set_path(root: &mut Map<String, Value>, dotted: &str, value: Value) {
    let mut parts = dotted.split('.').peekable();
    let mut current = root;
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            current.insert(part.to_string(), value);
            return;
        }
        let entry = current
            .entry(part.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        current = entry.as_object_mut().expect("az önce obje yapıldı");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::wfah::Wfah;
    use crate::types::wfd_v22::WfesEffects;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::sync::LazyLock;

    static EMPTY_WFAH: LazyLock<Wfah> = LazyLock::new(|| Wfah(vec![]));
    static TEST_RULES: LazyLock<ValidRules> = LazyLock::new(ValidRules::default);

    fn actor() -> Actor {
        Actor {
            orgu_id: Uuid::nil(),
            user_id: Uuid::nil(),
            role: "creditAnalyst".into(),
        }
    }

    fn env<'a>(a: &'a Actor, input: Option<&'a Value>, exec: Option<&'a Value>) -> EffectEnv<'a> {
        EffectEnv {
            actor: a,
            wfe_id: Uuid::nil(),
            node: Some("self__creditAnalyst"),
            action_input: input,
            exec_result: exec,
            call: None,
            env: &env::EMPTY_PUBLIC_ENV,
            now: Utc::now(),
            wfah: &EMPTY_WFAH,
            valid_rules: &TEST_RULES,
        }
    }

    /// `$env`'li varyant — enterpolasyon ve secret ayrımı testleri için.
    fn env_with<'a>(a: &'a Actor, e: &'a PublicEnv) -> EffectEnv<'a> {
        EffectEnv {
            actor: a,
            wfe_id: Uuid::nil(),
            node: Some("self__creditAnalyst"),
            action_input: None,
            exec_result: None,
            call: None,
            env: e,
            now: Utc::now(),
            wfah: &EMPTY_WFAH,
            valid_rules: &TEST_RULES,
        }
    }

    fn effects(pairs: &[(&str, Value)]) -> WfesEffects {
        WfesEffects {
            set: pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect::<BTreeMap<_, _>>(),
            set_when: Vec::new(),
        }
    }

    #[test]
    fn special_dollar_strings_resolve() {
        let a = actor();
        let e = env(&a, None, None);
        let out = apply_effects(
            &json!({}),
            &effects(&[
                ("initiated_by", json!("$actor")),
                ("at", json!("$timestamp")),
                ("wfe", json!("$wfe_id")),
                ("node", json!("$node")),
            ]),
            &e,
        )
        .unwrap();
        assert_eq!(out["initiated_by"]["role"], json!("creditAnalyst"));
        assert_eq!(out["wfe"], json!(Uuid::nil().to_string()));
        assert_eq!(out["node"], json!("self__creditAnalyst"));
        // `$timestamp` = UTC `yyyyMMddHHmmss`, 14 rakam — ayırıcı YOK (bkz. crate::timestamp).
        let at = out["at"].as_str().unwrap();
        assert_eq!(at.len(), crate::timestamp::TIMESTAMP_LEN, "damga: {at}");
        assert!(at.bytes().all(|c| c.is_ascii_digit()), "damga: {at}");
    }

    #[test]
    fn ctx_action_exec_refs_resolve() {
        let a = actor();
        let input = json!({"manager_decision": "approve"});
        let exec = json!({"score": 740, "grade": "A"});
        let e = env(&a, Some(&input), Some(&exec));
        let ctx = json!({"credit_info": {"amount_requested": 5000}});
        let out = apply_effects(
            &ctx,
            &effects(&[
                ("amount", json!("$ctx.credit_info.amount_requested")),
                ("decision", json!("$action.input.manager_decision")),
                ("credit_score", json!("$exec.result.score")),
                ("credit_grade", json!("$exec.result.grade")),
            ]),
            &e,
        )
        .unwrap();
        assert_eq!(out["amount"], json!(5000));
        assert_eq!(out["decision"], json!("approve"));
        assert_eq!(out["credit_score"], json!(740));
        assert_eq!(out["credit_grade"], json!("A"));
    }

    #[test]
    fn missing_refs_become_null() {
        let a = actor();
        let e = env(&a, None, None);
        let out =
            apply_effects(&json!({}), &effects(&[("x", json!("$ctx.ghost.path"))]), &e).unwrap();
        assert_eq!(out["x"], Value::Null);
    }

    #[test]
    fn absent_optional_input_writes_null() {
        // WOR-70b: gönderilmeyen OPSİYONEL input ctx'e `null` yazar — bu, optional'ın
        // required'dan tek farkıdır. Önceki değer (escalation notu) null'a döner;
        // validator bunu tasarım anında uyarı olarak bildirir.
        let a = actor();
        let input = json!({ "manager_decision": "approve" });
        let e = env(&a, Some(&input), None);
        let ctx = json!({ "internal_notes": "escalation notu" });
        let out = apply_effects(
            &ctx,
            &effects(&[
                ("manager_decision", json!("$action.input.manager_decision")),
                ("internal_notes", json!("$action.input.internal_notes")),
            ]),
            &e,
        )
        .unwrap();
        assert_eq!(out["manager_decision"], json!("approve"));
        assert_eq!(
            out["internal_notes"],
            Value::Null,
            "gönderilmeyen opsiyonel input null yazmalı"
        );
    }

    #[test]
    fn explicit_null_optional_input_writes_null_too() {
        // "Yok" ile "açıkça null gönderildi" aynı sonuca varır (optional için).
        let a = actor();
        let input = json!({ "internal_notes": null });
        let e = env(&a, Some(&input), None);
        let out = apply_effects(
            &json!({ "internal_notes": "eski" }),
            &effects(&[("internal_notes", json!("$action.input.internal_notes"))]),
            &e,
        )
        .unwrap();
        assert_eq!(out["internal_notes"], Value::Null);
    }

    #[test]
    fn exec_response_namespace_is_error() {
        let a = actor();
        let e = env(&a, None, None);
        let err = apply_effects(
            &json!({}),
            &effects(&[("x", json!("$exec.response.score"))]),
            &e,
        )
        .unwrap_err();
        assert!(err.to_string().contains("$exec.result"));
    }

    #[test]
    fn dotted_set_path_creates_nested_objects() {
        let a = actor();
        let e = env(&a, None, None);
        let out = apply_effects(
            &json!({"credit_info": {"purpose": "ev"}}),
            &effects(&[("credit_info.amount_requested", json!(9000))]),
            &e,
        )
        .unwrap();
        assert_eq!(out["credit_info"]["amount_requested"], json!(9000));
        assert_eq!(
            out["credit_info"]["purpose"],
            json!("ev"),
            "kardeş alan korunmalı"
        );
    }

    #[test]
    fn plain_strings_and_literals_pass_through() {
        let a = actor();
        let e = env(&a, None, None);
        let out = apply_effects(
            &json!({}),
            &effects(&[
                ("note", json!("düz metin")),
                ("n", json!(42)),
                ("b", json!(true)),
            ]),
            &e,
        )
        .unwrap();
        assert_eq!(out["note"], json!("düz metin"));
        assert_eq!(out["n"], json!(42));
        assert_eq!(out["b"], json!(true));
    }

    #[test]
    fn original_ctx_is_not_mutated() {
        let a = actor();
        let e = env(&a, None, None);
        let ctx = json!({"a": 1});
        let _ = apply_effects(&ctx, &effects(&[("a", json!(2))]), &e).unwrap();
        assert_eq!(ctx["a"], json!(1));
    }

    fn public_env() -> PublicEnv {
        env::EnvSet::new(std::collections::BTreeMap::from([
            (
                "AUTH_API".to_string(),
                env::EnvValue::public(json!("https://auth.test")),
            ),
            ("RETRIES".to_string(), env::EnvValue::public(json!(3))),
            (
                "API_KEY".to_string(),
                env::EnvValue::secret(json!("sk-live-xyz")),
            ),
        ]))
        .public()
    }

    /// Effects `$env`'i çözer: tam eşleşme tipi korur, ara-değer string üretir.
    #[test]
    fn env_resolves_in_effects() {
        let a = actor();
        let p = public_env();
        let e = env_with(&a, &p);
        let out = apply_effects(
            &json!({}),
            &effects(&[
                ("uc", json!("$env.AUTH_API/v1/skor")),
                ("deneme", json!("$env.RETRIES")),
            ]),
            &e,
        )
        .unwrap();
        assert_eq!(out["uc"], json!("https://auth.test/v1/skor"));
        assert_eq!(out["deneme"], json!(3), "tam eşleşme sayıyı sayı bırakır");
    }

    /// KRİTİK: secret bir değer effects üzerinden ctx'e YAZILAMAZ. Yazılabilseydi
    /// portalda görünür ve `$exec` üzerinden dışarı sızardı.
    #[test]
    fn secret_cannot_reach_ctx_through_effects() {
        let a = actor();
        let p = public_env();
        let e = env_with(&a, &p);
        let err = apply_effects(&json!({}), &effects(&[("k", json!("$env.API_KEY"))]), &e)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("API_KEY"),
            "secret anahtar tanımsız gibi davranmalı, sessizce null yazmamalı: {err}"
        );
    }

    /// Eksik anahtar `$ctx`'in aksine null DEĞİL, hata — null bir domain
    /// `https://null/...` üretirdi.
    #[test]
    fn missing_env_key_fails_effects() {
        let a = actor();
        let p = public_env();
        let e = env_with(&a, &p);
        assert!(apply_effects(&json!({}), &effects(&[("k", json!("$env.YOK"))]), &e).is_err());
    }
}

#[cfg(test)]
mod set_when_tests {
    use super::*;
    use crate::types::wfah::Wfah;
    use std::sync::LazyLock;

    static RULES: LazyLock<ValidRules> = LazyLock::new(ValidRules::default);

    use crate::types::wfd_v22::{SetWhenEntry, WfesEffects};
    use serde_json::json;
    use std::collections::BTreeMap;

    fn actor() -> Actor {
        Actor {
            orgu_id: Uuid::nil(),
            user_id: Uuid::nil(),
            role: "creditAnalyst".into(),
        }
    }

    fn env<'a>(a: &'a Actor, wfah: &'a Wfah) -> EffectEnv<'a> {
        EffectEnv {
            actor: a,
            wfe_id: Uuid::nil(),
            node: Some("self__creditAnalyst"),
            action_input: None,
            exec_result: None,
            call: None,
            env: &env::EMPTY_PUBLIC_ENV,
            now: Utc::now(),
            wfah,
            valid_rules: &RULES,
        }
    }

    fn entry(when: &str, set: &[(&str, Value)]) -> SetWhenEntry {
        SetWhenEntry {
            when: when.into(),
            set: set
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    /// `Ç10`/c: `set` KOŞULSUZ, `set_when[]` koşula bağlı yazar.
    #[test]
    fn conditional_write_lands_only_when_its_condition_holds() {
        let a = actor();
        let wfah = Wfah(vec![]);
        let effects = WfesEffects {
            set: BTreeMap::from([("durum".to_string(), json!("onaylandi"))]),
            set_when: vec![entry(
                "$ctx.miktar < 100000",
                &[("hizli_onay", json!(true))],
            )],
        };

        let küçük = apply_effects(&json!({"miktar": 50000}), &effects, &env(&a, &wfah)).unwrap();
        assert_eq!(
            küçük["durum"],
            json!("onaylandi"),
            "koşulsuz yazım her hâlde"
        );
        assert_eq!(küçük["hizli_onay"], json!(true), "koşul tuttu");

        let büyük = apply_effects(&json!({"miktar": 500000}), &effects, &env(&a, &wfah)).unwrap();
        assert_eq!(büyük["durum"], json!("onaylandi"));
        assert_eq!(
            büyük.get("hizli_onay"),
            None,
            "koşul tutmadı — alan HİÇ yazılmamalı, `null` da değil"
        );
    }

    /// `Ç10`/c: **ilk-match DEĞİL** — koşulu uyan BÜTÜN girdiler uygulanır, ve aynı
    /// alana yazan iki girdide SONRAKİ kazanır (dizi sırası anlam taşır).
    #[test]
    fn every_matching_entry_applies_and_the_later_one_wins() {
        let a = actor();
        let wfah = Wfah(vec![]);
        let effects = WfesEffects {
            set: BTreeMap::new(),
            set_when: vec![
                entry(
                    "$ctx.tutar > 10",
                    &[("a", json!(1)), ("ortak", json!("ilk"))],
                ),
                entry(
                    "$ctx.tutar > 20",
                    &[("b", json!(2)), ("ortak", json!("son"))],
                ),
            ],
        };
        let out = apply_effects(&json!({"tutar": 30}), &effects, &env(&a, &wfah)).unwrap();
        assert_eq!(
            out["a"],
            json!(1),
            "ilk girdi de uygulanmalı (ilk-match DEĞİL)"
        );
        assert_eq!(out["b"], json!(2));
        assert_eq!(out["ortak"], json!("son"), "sonraki kazanır");
    }

    /// `E09`/B3: `when` GİRDİ ctx'ini görür — `set`in ve önceki `set_when`lerin
    /// yazdıkları koşula GÖRÜNMEZ. Zincirleme koşul AÇILMAZ.
    #[test]
    fn the_condition_sees_the_input_ctx_not_what_this_block_writes() {
        let a = actor();
        let wfah = Wfah(vec![]);
        let effects = WfesEffects {
            set: BTreeMap::from([("bayrak".to_string(), json!(true))]),
            set_when: vec![
                entry("$ctx.bayrak == true", &[("set_gordu", json!(true))]),
                entry("$ctx.zincir == true", &[("zincir", json!(true))]),
                entry("$ctx.zincir == true", &[("zincir_gordu", json!(true))]),
            ],
        };
        let out = apply_effects(&json!({}), &effects, &env(&a, &wfah)).unwrap();
        assert_eq!(out["bayrak"], json!(true));
        assert_eq!(
            out.get("set_gordu"),
            None,
            "`set`in yazdığı koşula görünmez"
        );
        assert_eq!(out.get("zincir"), None);
        assert_eq!(
            out.get("zincir_gordu"),
            None,
            "önceki `set_when`in yazdığı da görünmez — zincirleme YOK"
        );
    }

    /// `E09`/B: `when` `$wfah`ı görür — B seçeneğinin tek gerekçesi "`when` her yerde
    /// AYNI şeyi görür"dü.
    #[test]
    fn the_condition_can_read_the_committed_ledger() {
        let a = actor();
        let wfah = Wfah(vec![crate::types::wfah::WfahEntry {
            seq: 1,
            action: "itiraz".into(),
            actor: actor(),
            input: None,
            applied_at: Utc::now(),
            from_node: None,
            to_node: None,
            branch_entry: None,
            branch_round: None,
        }]);
        let effects = WfesEffects {
            set: BTreeMap::new(),
            set_when: vec![entry(
                "count($wfah, #.action == \"itiraz\") >= 1",
                &[("itiraz_var", json!(true))],
            )],
        };
        let out = apply_effects(&json!({}), &effects, &env(&a, &wfah)).unwrap();
        assert_eq!(out["itiraz_var"], json!(true));

        let boş = Wfah(vec![]);
        let out = apply_effects(&json!({}), &effects, &env(&a, &boş)).unwrap();
        assert_eq!(out.get("itiraz_var"), None);
    }

    /// `E09`/S4: çalışma anı `when` hatası COMMIT'İ DÜŞÜRÜR — girdi sessizce
    /// ATLANMAZ. `set` tarafında `resolve_value` hatası zaten commit'i düşürüyor;
    /// aynı blokta iki farklı hata rejimi olmaz.
    #[test]
    fn a_runtime_condition_error_propagates_instead_of_being_swallowed() {
        let a = actor();
        let wfah = Wfah(vec![]);
        let effects = WfesEffects {
            set: BTreeMap::new(),
            // Boolean DEĞİL bir sonuç: "boolean sonuç üretmedi" yolu.
            set_when: vec![entry("$ctx.tutar", &[("x", json!(1))])],
        };
        let err = apply_effects(&json!({"tutar": 5}), &effects, &env(&a, &wfah)).unwrap_err();
        assert!(
            matches!(err, EngineError::ZenEvaluation(_)),
            "beklenen ZenEvaluation, gelen: {err:?}"
        );
    }
}
