//! Senaryo = kaydedilmiş simülasyon koşusu (start + adımlar + beklenti).
//! Depolama `{orgtnt}/wfd/{wfd_id}/{version}.scenarios.json` sidecar'ında
//! (bkz. `wf_wfd::storage::scenarios_key`); bu modül SAF'tır — I/O yapmaz,
//! yalnız `Engine` + `Wfd` + `Scenario` alıp `sim` makinesini sürer.
//!
//! Alan adları editörün bugünkü `WfdScenario`'suyla AYNIDIR (camelCase aktör
//! dahil) — mevcut localStorage blob'ları dönüştürmesiz yüklenebilsin diye.

use crate::sim::step;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
use wfe_core::types::actor::Actor;
use wfe_core::types::wfd_v22::Wfd;
use wfe_core::v22::pipeline::Engine;

fn default_set_version() -> String {
    "1".into()
}

fn default_call_status() -> String {
    "completed".into()
}

/// Bir WFD versiyonunun senaryo seti — sidecar dosyasının kökü.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioSet {
    #[serde(default = "default_set_version")]
    pub scenarios_version: String,
    #[serde(default)]
    pub scenarios: Vec<Scenario>,
}

impl Default for ScenarioSet {
    fn default() -> Self {
        Self {
            scenarios_version: default_set_version(),
            scenarios: Vec::new(),
        }
    }
}

/// Editörün sakladığı aktör şekli — motorun snake_case `Actor`'undan FARKLI
/// (camelCase). Çeviri koşu anında `to_actor()` ile yapılır.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioActor {
    #[serde(rename = "orguId")]
    pub orgu_id: Uuid,
    #[serde(rename = "userId")]
    pub user_id: Uuid,
    pub role: String,
}

impl ScenarioActor {
    pub fn to_actor(&self) -> Actor {
        Actor {
            orgu_id: self.orgu_id,
            user_id: self.user_id,
            role: self.role.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scenario {
    pub id: String,
    pub name: String,
    /// Dizin yolu — `"Onaylar/Müdür"`. Ağaç BU alandan türetilir; ayrı klasör
    /// kaydı yoktur, dolayısıyla öksüz/döngülü klasör oluşamaz. Boş = kök.
    #[serde(default)]
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// `$env` bağlaması; verilmezse boş ortam.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    #[serde(rename = "startActor", default, skip_serializing_if = "Option::is_none")]
    pub start_actor: Option<ScenarioActor>,
    /// M16: birden çok start kuralı olan WFD'de hangisinin seçileceği.
    #[serde(
        rename = "startAction",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub start_action: Option<String>,
    #[serde(rename = "startInput", default)]
    pub start_input: Value,
    #[serde(default)]
    pub steps: Vec<ScenarioStep>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect: Option<Expect>,
}

/// Adımın iki çeşidi. `untagged`: `action` anahtarı taşıyan nesne aksiyon adımı,
/// `call_return` taşıyan nesne WFC çağrı dönüşüdür. Sıra ÖNEMLİ — `Action`
/// `action`'ı zorunlu kıldığı için `call_return` nesnesi ona uymaz ve ikinci
/// varyanta düşer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScenarioStep {
    Action {
        action: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<ScenarioActor>,
        #[serde(default)]
        input: Value,
        /// WOR-31: paralel modda kol seçimi.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        node: Option<String>,
    },
    CallReturn {
        call_return: CallReturn,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallReturn {
    /// completed | failed | terminated | timeout
    #[serde(default = "default_call_status")]
    pub status: String,
    /// Çağrılanın `wfe_end_response`'u; yalnız `completed` için anlamlı.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Expect {
    /// Beklenen terminal id'si.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<String>,
    /// Final dynctx'in içermesi gereken ALT KÜME.
    #[serde(
        rename = "contextContains",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub context_contains: Option<Value>,
}

/// Tek senaryonun koşu sonucu — HTTP yanıtına doğrudan serileşir.
#[derive(Debug, Clone, Serialize)]
pub struct ScenarioResult {
    pub id: String,
    pub name: String,
    pub ok: bool,
    pub failures: Vec<String>,
    pub steps_executed: usize,
    pub terminal: bool,
    pub terminal_id: Option<String>,
    pub dynctx: Value,
}

/// `expected`'in `actual` içinde ALT KÜME olarak var olup olmadığını derinlemesine
/// denetler. Sözleşme editördeki `deepContains` ile birebirdir: **nesneler alt
/// küme** (recursive), **diziler ve skalerler tam eşleşme**.
fn deep_contains(expected: &Value, actual: Option<&Value>, path: &str, failures: &mut Vec<String>) {
    let label = if path.is_empty() { "context" } else { path };
    match expected {
        Value::Object(exp) => {
            let Some(Value::Object(act)) = actual else {
                failures.push(format!(
                    "{label}: beklenen {expected}, gelen {}",
                    actual.unwrap_or(&Value::Null)
                ));
                return;
            };
            for (k, v) in exp {
                let next = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                match act.get(k) {
                    None => failures.push(format!("{next} alanı yok — beklenen {v}")),
                    Some(a) => deep_contains(v, Some(a), &next, failures),
                }
            }
        }
        _ => {
            if actual != Some(expected) {
                failures.push(format!(
                    "{label}: beklenen {expected}, gelen {}",
                    actual.unwrap_or(&Value::Null)
                ));
            }
        }
    }
}

/// Beklentiyi koşu sonucuyla karşılaştırır; boş dönüş = geçti.
/// `expect` yoksa (veya iki alanı da boşsa) HER ZAMAN geçer.
pub fn check_expectations(
    expect: Option<&Expect>,
    terminal: bool,
    terminal_id: Option<&str>,
    dynctx: &Value,
) -> Vec<String> {
    let Some(e) = expect else {
        return Vec::new();
    };
    if e.terminal.is_none() && e.context_contains.is_none() {
        return Vec::new();
    }
    let mut failures = Vec::new();

    if let Some(want) = &e.terminal {
        if !terminal {
            failures.push(format!("terminal beklendi (\"{want}\") ama WFE hâlâ aktif"));
        } else {
            match terminal_id {
                None => failures.push(format!(
                    "terminal \"{want}\" beklendi ama ulaşılan terminal belirlenemedi"
                )),
                Some(got) if got != want => {
                    failures.push(format!("terminal beklendi \"{want}\", gelen \"{got}\""))
                }
                Some(_) => {}
            }
        }
    }

    if let Some(want) = &e.context_contains {
        deep_contains(want, Some(dynctx), "", &mut failures);
    }

    failures
}

/// Ulaşılan terminali dynctx'ten best-effort çözer. Sözleşme editördeki
/// `inferTerminalId` ile aynıdır: `terminals[].wfes_effects.set` etkileri
/// dynctx'in alt kümesi olan TEK aday varsa onun id'si; 0 veya >1 eşleşmede
/// `None` (belirsizlik sessizce yanlış pozitife dönüşmesin).
///
/// Etkisi BOŞ olan aday hiçbir zaman eşleşmez — her dynctx'e uyardı.
pub fn infer_terminal_id(wfd_json: &Value, dynctx: &Value) -> Option<String> {
    let terminals = wfd_json.get("terminals")?.as_array()?;
    let mut hit: Option<String> = None;
    for t in terminals {
        let Some(id) = t.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        let effects = t
            .get("wfes_effects")
            .and_then(|w| w.get("set"))
            .and_then(|s| s.as_object());
        let Some(effects) = effects else { continue };
        if effects.is_empty() {
            continue;
        }
        let mut failures = Vec::new();
        deep_contains(
            &Value::Object(effects.clone()),
            Some(dynctx),
            "",
            &mut failures,
        );
        if failures.is_empty() {
            if hit.is_some() {
                return None; // birden çok aday — belirsiz
            }
            hit = Some(id.to_string());
        }
    }
    hit
}

/// Senaryoyu uçtan uca koşar. **Hiçbir şey yazmaz** — `sim` durumsuzdur, WFE
/// yaratılmaz, WFAH'a iz düşmez.
///
/// `wfd_json` beklenti çözümü içindir (`infer_terminal_id` `terminals[]`
/// kataloğuna bakar); `wfd` motorun parse edilmiş hâlidir. İkisi AYNI belgeden
/// gelmelidir — çağıran ikisini de aynı gövdeden üretir.
///
/// `fallback_actor`: senaryo/adım aktörü eksikse kullanılır (editörün bugünkü
/// `readStoredEngineConfig` davranışının sunucu karşılığı).
///
/// Motor hatası senaryoyu KALDIRIR, koşuyu patlatmaz: bir senaryonun kalması
/// normal bir sonuçtur, `Err` değil. Hata anında kalan adımlar atlanır.
pub async fn run(
    engine: &Engine<'_>,
    wfd: &Wfd,
    wfd_json: &Value,
    scenario: &Scenario,
    fallback_actor: Option<&Actor>,
) -> ScenarioResult {
    let resolve = |a: &Option<ScenarioActor>| -> Option<Actor> {
        a.as_ref()
            .map(|x| x.to_actor())
            .or_else(|| fallback_actor.cloned())
    };

    let fail = |failures: Vec<String>, steps_executed: usize| ScenarioResult {
        id: scenario.id.clone(),
        name: scenario.name.clone(),
        ok: false,
        failures,
        steps_executed,
        terminal: false,
        terminal_id: None,
        dynctx: Value::Null,
    };

    let Some(start_actor) = resolve(&scenario.start_actor) else {
        return fail(
            vec!["başlangıç aktörü çözülemedi (senaryoda yok, yedek aktör de verilmedi)".into()],
            0,
        );
    };

    let orgtnt_id =
        match wfe_core::ports::OrgPort::orgtnt_for_orgu(engine.org, start_actor.orgu_id).await {
            Ok(id) => id,
            Err(e) => return fail(vec![format!("aktörün tenant'ı çözülemedi: {e}")], 0),
        };

    let mut state = match step::start(
        engine,
        wfd,
        &start_actor,
        orgtnt_id,
        scenario.start_action.as_deref(),
        &scenario.start_input,
    )
    .await
    {
        Ok(s) => s,
        Err(e) => return fail(vec![format!("start: {e}")], 0),
    };

    let mut steps_executed = 0usize;
    for (i, s) in scenario.steps.iter().enumerate() {
        if step::is_terminal(&state) {
            break; // terminale ulaşıldı — kalan adımlar sessizce atlanır
        }
        let outcome = match s {
            ScenarioStep::Action {
                action,
                actor,
                input,
                node,
            } => match resolve(actor) {
                None => Err(format!(
                    "Adım {} (\"{action}\") için aktör çözülemedi",
                    i + 1
                )),
                Some(a) => step::apply(engine, wfd, &mut state, &a, action, input, node.as_deref())
                    .await
                    .map_err(|e| format!("Adım {} (\"{action}\"): {e}", i + 1)),
            },
            // Her iki başarısızlık da (bekleyen çağrı yok / motor hatası) senaryoyu
            // kaldırır — koşucu için ikisi arasında fark yok, HTTP durumu yok.
            ScenarioStep::CallReturn { call_return } => step::call_return(
                engine,
                wfd,
                &mut state,
                &call_return.status,
                call_return.result.as_ref(),
            )
            .await
            .map_err(|e| format!("Adım {} (çağrı dönüşü): {e}", i + 1)),
        };
        if let Err(msg) = outcome {
            return fail(vec![msg], steps_executed);
        }
        steps_executed += 1;
    }

    let terminal = step::is_terminal(&state);
    let dynctx = state.dynctx.clone();
    let terminal_id = if terminal {
        infer_terminal_id(wfd_json, &dynctx)
    } else {
        None
    };
    let failures = check_expectations(
        scenario.expect.as_ref(),
        terminal,
        terminal_id.as_deref(),
        &dynctx,
    );

    ScenarioResult {
        id: scenario.id.clone(),
        name: scenario.name.clone(),
        ok: failures.is_empty(),
        failures,
        steps_executed,
        terminal,
        terminal_id,
        dynctx,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bugün localStorage'da duran şekil (camelCase aktör, `node`/`startAction` yok)
    /// DÖNÜŞTÜRMESİZ parse olmalı — göç yolu bunun üstünde duruyor.
    #[test]
    fn legacy_localstorage_shape_parses_verbatim() {
        let raw = serde_json::json!({
            "scenarios": [{
                "id": "s1",
                "name": "Mutlu yol",
                "startInput": { "tutar": 10 },
                "startActor": { "orguId": "00000000-0000-0000-0000-000000000001",
                                "userId": "00000000-0000-0000-0000-000000000002",
                                "role": "mudur" },
                "steps": [{ "action": "onayla", "input": {} }],
                "expect": { "terminal": "onaylandi" }
            }]
        });
        let set: ScenarioSet = serde_json::from_value(raw).unwrap();
        assert_eq!(set.scenarios_version, "1", "eksik sürüm alanı 1'e düşer");
        let s = &set.scenarios[0];
        assert_eq!(s.path, "", "path yoksa kök");
        assert!(
            matches!(&s.steps[0], ScenarioStep::Action { action, node: None, .. } if action == "onayla")
        );
    }

    #[test]
    fn call_return_step_parses_as_its_own_variant() {
        let raw = serde_json::json!({ "call_return": { "status": "failed" } });
        let step: ScenarioStep = serde_json::from_value(raw).unwrap();
        match step {
            ScenarioStep::CallReturn { call_return } => {
                assert_eq!(call_return.status, "failed");
                assert!(call_return.result.is_none());
            }
            _ => panic!("call_return adımı Action olarak parse edildi"),
        }
    }

    #[test]
    fn call_return_status_defaults_to_completed() {
        let step: ScenarioStep =
            serde_json::from_value(serde_json::json!({ "call_return": {} })).unwrap();
        match step {
            ScenarioStep::CallReturn { call_return } => assert_eq!(call_return.status, "completed"),
            _ => panic!("yanlış varyant"),
        }
    }

    #[test]
    fn scenario_actor_converts_to_engine_actor() {
        let a = ScenarioActor {
            orgu_id: uuid::Uuid::nil(),
            user_id: uuid::Uuid::nil(),
            role: "mudur".into(),
        };
        assert_eq!(a.to_actor().role, "mudur");
    }

    // ── beklenti denetimi ───────────────────────────────────────────────────

    fn dynctx(v: serde_json::Value) -> serde_json::Value {
        v
    }

    #[test]
    fn no_expectation_always_passes() {
        let f = check_expectations(None, true, Some("x"), &dynctx(serde_json::json!({})));
        assert!(f.is_empty());
    }

    #[test]
    fn object_expectation_is_a_subset_match() {
        let e = Expect {
            terminal: None,
            context_contains: Some(serde_json::json!({ "musteri": { "ad": "Ay" } })),
        };
        // Fazladan alan sorun değil — alt küme yeterli.
        let ok = check_expectations(
            Some(&e),
            false,
            None,
            &dynctx(serde_json::json!({ "musteri": { "ad": "Ay", "yas": 30 }, "x": 1 })),
        );
        assert!(ok.is_empty(), "{ok:?}");

        let bad = check_expectations(
            Some(&e),
            false,
            None,
            &dynctx(serde_json::json!({ "musteri": { "ad": "Bora" } })),
        );
        assert_eq!(bad.len(), 1);
        assert!(bad[0].contains("musteri.ad"), "{bad:?}");
    }

    #[test]
    fn arrays_must_match_exactly_not_as_subset() {
        let e = Expect {
            terminal: None,
            context_contains: Some(serde_json::json!({ "l": [1, 2] })),
        };
        assert!(
            check_expectations(Some(&e), false, None, &dynctx(serde_json::json!({ "l": [1, 2] })))
                .is_empty()
        );
        assert_eq!(
            check_expectations(
                Some(&e),
                false,
                None,
                &dynctx(serde_json::json!({ "l": [1, 2, 3] }))
            )
            .len(),
            1
        );
    }

    #[test]
    fn missing_field_is_reported_with_its_path() {
        let e = Expect {
            terminal: None,
            context_contains: Some(serde_json::json!({ "a": { "b": 1 } })),
        };
        let f = check_expectations(Some(&e), false, None, &dynctx(serde_json::json!({ "a": {} })));
        assert_eq!(f.len(), 1);
        assert!(f[0].contains("a.b"), "{f:?}");
    }

    #[test]
    fn terminal_expectation_needs_a_reached_terminal() {
        let e = Expect {
            terminal: Some("onaylandi".into()),
            context_contains: None,
        };
        let still_active = check_expectations(Some(&e), false, None, &dynctx(serde_json::json!({})));
        assert_eq!(still_active.len(), 1);
        assert!(still_active[0].contains("aktif"), "{still_active:?}");

        let wrong = check_expectations(
            Some(&e),
            true,
            Some("reddedildi"),
            &dynctx(serde_json::json!({})),
        );
        assert_eq!(wrong.len(), 1);
        assert!(wrong[0].contains("reddedildi"));

        assert!(check_expectations(
            Some(&e),
            true,
            Some("onaylandi"),
            &dynctx(serde_json::json!({}))
        )
        .is_empty());
    }

    /// TS `inferTerminalId` sözleşmesi: etkileri dynctx'in alt kümesi olan TEK
    /// aday varsa o; 0 veya >1 ise None (sessiz yanlış pozitif üretmez).
    #[test]
    fn terminal_id_is_inferred_only_when_exactly_one_candidate_matches() {
        let wfd = serde_json::json!({ "terminals": [
            { "id": "Onaylandı", "wfes_effects": { "set": { "durum": "onaylandi" } } },
            { "id": "Reddedildi", "wfes_effects": { "set": { "durum": "reddedildi" } } },
            { "id": "Etkisiz",    "wfes_effects": { "set": {} } }
        ]});
        assert_eq!(
            infer_terminal_id(&wfd, &serde_json::json!({ "durum": "onaylandi", "x": 1 })),
            Some("Onaylandı".to_string())
        );
        assert_eq!(
            infer_terminal_id(&wfd, &serde_json::json!({ "durum": "beklemede" })),
            None
        );
        // Etkisiz aday (boş set) hiçbir zaman eşleşmez — yoksa her dynctx'e uyardı.
        assert_eq!(infer_terminal_id(&wfd, &serde_json::json!({})), None);
    }

    /// Birden çok aday eşleşirse belirsizdir — None döner, biri seçilmez.
    #[test]
    fn ambiguous_terminal_candidates_resolve_to_none() {
        let wfd = serde_json::json!({ "terminals": [
            { "id": "A", "wfes_effects": { "set": { "durum": "bitti" } } },
            { "id": "B", "wfes_effects": { "set": { "durum": "bitti" } } }
        ]});
        assert_eq!(
            infer_terminal_id(&wfd, &serde_json::json!({ "durum": "bitti" })),
            None
        );
    }
}
