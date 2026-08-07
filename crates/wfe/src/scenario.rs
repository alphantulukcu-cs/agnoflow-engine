//! Senaryo = kaydedilmiş simülasyon koşusu (start + adımlar + beklenti).
//! Depolama `{orgtnt}/wfd/{wfd_id}/{version}.scenarios.json` sidecar'ında
//! (bkz. `wf_wfd::storage::scenarios_key`); bu modül SAF'tır — I/O yapmaz,
//! yalnız `Engine` + `Wfd` + `Scenario` alıp `sim` makinesini sürer.
//!
//! Alan adları editörün bugünkü `WfdScenario`'suyla AYNIDIR (camelCase aktör
//! dahil) — mevcut localStorage blob'ları dönüştürmesiz yüklenebilsin diye.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
use wfe_core::types::actor::Actor;

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
}
