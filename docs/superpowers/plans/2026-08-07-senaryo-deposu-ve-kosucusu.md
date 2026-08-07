# Senaryo Deposu ve Sunucu Koşucusu — Uygulama Planı

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Senaryolar (kaydedilmiş simülasyon koşuları) localStorage'dan sunucudaki bir sidecar'a taşınır, koşucu tarayıcıdan motora iner ve senaryolar `path` ile klasörlenir.

**Architecture:** Senaryo seti WFD dokümanının yanında opak bir JSON blob'u olarak durur (`{orgtnt}/wfd/{wfd_id}/{version}.scenarios.json`) — editör layout'unun birebir emsali. Koşucu `crates/wfe/src/scenario.rs`'te saf bir modüldür: `Engine` + `Wfd` + `Scenario` alır, `sim` durum makinesini uçtan uca sürer, `ScenarioResult` döner; I/O yapmaz. Sunucu rotaları ince sarmalayıcıdır. Dizin ağacı ayrı bir veri yapısı değil, senaryonun `path` alanından türetilir.

**Tech Stack:** Rust (axum, utoipa, opendal, serde, sqlx) · TypeScript/React (vitest) · PostgreSQL

**Tasarım dokümanı:** `docs/superpowers/specs/2026-08-07-senaryo-deposu-ve-kosucusu-design.md`

---

## Ön bilgi — bu kod tabanında bilmen gerekenler

**Repo'lar:** engine `~/Desktop/agnoflow-engine` (Rust workspace), editör `~/Desktop/agnoflow-frontend` (Vite + React + TS).

**Testler:** engine `cargo test --workspace`, editör `npm test` (vitest). Her task sonunda ilgili testi koştur.

**Değiştirilmez kural:** `docs/spec/examples/kredi-basvuru.golden.json` ve `crates/wfe-core/tests/fixtures/*` **değiştirilmez** — kod fixture'a uyar, tersi değil.

**Commit mesajları:** Türkçe, `feat(...)`/`fix(...)`/`test(...)` öneki. **`Co-Authored-By: Claude` benzeri imza YAZILMAZ.**

**Simülasyon nasıl çalışıyor (koşucunun sürdüğü makine):** `wfe_core::v22::pipeline::Engine` üç alanlı bir struct'tır (`org`, `exec`, `env`) ve ödünç alır — `Engine<'a>`. `wf_wfe::sim::SimState` store'suz WFE durumudur:
- `SimState::from_new_wfe(&engine.start(...))` → başlangıç
- `sim_state.to_wfes(Some(user_id))` → engine'in beklediği `Wfes`
- `engine.apply(&wfd, &wfes, &actor, action, &input, node)` → `TransitionCommit`
- `sim_state.apply_commit(&commit)` → durumu ilerlet
- `sim_state.awaited_call()` / `clear_awaited_call(site_key)` → WFC durağı

Canlı örnek: `crates/wfe/tests/sim_fork_join.rs` (mock `OrgPort`/`AutoexecRunner` ile ağsız Engine kurar). **Yeni testlerde o dosyanın mock kalıbını kopyala.**

---

# FAZ 1 — Depo (crates/wfd)

## Task 1: Senaryo sidecar'ının storage anahtarı

**Files:**
- Modify: `crates/wfd/src/storage.rs` (`layout_key`'in hemen altına, satır ~87)
- Test: `crates/wfd/src/storage.rs` içindeki `#[cfg(test)] mod tests`

- [ ] **Step 1: Failing test yaz**

`crates/wfd/src/storage.rs` içindeki `mod tests`'e ekle:

```rust
    #[test]
    fn scenarios_key_sits_next_to_the_document() {
        let t = uuid::Uuid::nil();
        let w = uuid::Uuid::nil();
        assert_eq!(
            scenarios_key(t, w, 3),
            "00000000-0000-0000-0000-000000000000/wfd/00000000-0000-0000-0000-000000000000/3.scenarios.json"
        );
        // Layout ile aynı dizinde, aynı versiyon önekinde.
        let layout = layout_key(t, w, 3);
        let scenarios = scenarios_key(t, w, 3);
        assert_eq!(
            layout.rsplit_once('/').unwrap().0,
            scenarios.rsplit_once('/').unwrap().0
        );
    }
```

- [ ] **Step 2: Testin FAIL ettiğini gör**

Run: `cargo test -p wf-wfd storage::tests::scenarios_key`
Expected: derleme hatası — `cannot find function 'scenarios_key' in this scope`

- [ ] **Step 3: Fonksiyonu ekle**

`crates/wfd/src/storage.rs`, `layout_key`'in hemen altına:

```rust
/// Senaryo sidecar'ının storage anahtarı — kaydedilmiş simülasyon koşuları
/// (`{version}.scenarios.json`). Layout ile aynı gerekçe: doküman
/// `additionalProperties:false` ve `(wfd_id, version)` immutable olduğundan
/// senaryolar gövdeye giremez, dokümanın YANINDA durur.
///
/// Layout'un aksine legacy (tenant-öncesi) karşılığı YOKTUR — bu anahtar tenant
/// prefix'i yerleştikten sonra doğdu.
pub fn scenarios_key(orgtnt_id: uuid::Uuid, wfd_id: uuid::Uuid, version: i32) -> String {
    format!("{orgtnt_id}/wfd/{wfd_id}/{version}.scenarios.json")
}
```

- [ ] **Step 4: Testin GEÇTİĞİNİ gör**

Run: `cargo test -p wf-wfd storage::tests::scenarios_key`
Expected: `test result: ok. 1 passed`

- [ ] **Step 5: Commit**

```bash
git add crates/wfd/src/storage.rs
git commit -m "feat(wfd): senaryo sidecar'ının storage anahtarı"
```

---

## Task 2: Sidecar oku/yaz + yaşam döngüsü

`fetch_layout`/`save_layout` ile birebir simetrik iki metot, artı yeni-draft kopyalama ve draft-silme temizliği.

**Files:**
- Modify: `crates/wfd/src/adapter.rs` (`save_layout`/`fetch_layout` bloğunun altına; `new_draft_from` ~satır 428; `delete_draft` ~satır 485)

- [ ] **Step 1: Metotları yaz**

`crates/wfd/src/adapter.rs`, `fetch_layout`'un hemen altına:

```rust
    /// Senaryo sidecar'ını (opaque JSON) yazar. Şema-VALID doküman DEĞİLDİR;
    /// parse/validate YOK — şekli `wf_wfe::scenario::ScenarioSet` tarafında
    /// koşu anında doğrulanır. Versiyonun var olduğunu doğrular (herhangi status:
    /// yayınlanmış akışa test eklemek akışı değiştirmez).
    pub async fn save_scenarios(
        &self,
        wfd_id: Uuid,
        version: i32,
        scenarios: &Value,
    ) -> Result<(), crate::error::WfdError> {
        let meta = repo::get_meta_any(&self.pool, wfd_id, version).await?;
        let key = storage::scenarios_key(meta.orgtnt_id, wfd_id, version);
        let bytes = serde_json::to_vec(scenarios)
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))?;
        self.storage
            .write(&key, bytes)
            .await
            .map_err(|e| crate::error::WfdError::Storage(e.to_string()))?;
        Ok(())
    }

    /// Versiyonun HAM JSON'unu döner — status'e bakmaz. `fetch_draft_json`
    /// yalnız draft'a izin verir, `fetch` ise parse edilmiş `Wfd` döner;
    /// senaryo koşucusuna ise `terminals[]` kataloğunu okuyabilmek için ham
    /// belge lazım (bkz. `scenario::infer_terminal_id`).
    pub async fn fetch_json_any(
        &self,
        wfd_id: Uuid,
        version: i32,
    ) -> Result<Value, crate::error::WfdError> {
        let meta = repo::get_meta_any(&self.pool, wfd_id, version).await?;
        let bytes = self
            .storage
            .read(&meta.s3_key)
            .await
            .map_err(|e| crate::error::WfdError::Storage(e.to_string()))?
            .to_bytes();
        serde_json::from_slice(&bytes)
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))
    }

    /// Senaryo sidecar'ını döner; blob yoksa None (hata değil — henüz senaryo
    /// yazılmamış WFD'ler bu yoldan geçer).
    pub async fn fetch_scenarios(
        &self,
        wfd_id: Uuid,
        version: i32,
    ) -> Result<Option<Value>, crate::error::WfdError> {
        let meta = repo::get_meta_any(&self.pool, wfd_id, version).await?;
        let key = storage::scenarios_key(meta.orgtnt_id, wfd_id, version);
        match self.storage.read(&key).await {
            Ok(buf) => Ok(Some(
                serde_json::from_slice(&buf.to_bytes())
                    .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))?,
            )),
            Err(e) if e.kind() == opendal::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(crate::error::WfdError::Storage(e.to_string())),
        }
    }
```

- [ ] **Step 2: `new_draft_from`'a kopyalamayı ekle**

`crates/wfd/src/adapter.rs`, layout kopyalayan `if let`'in **hemen altına** (~satır 431):

```rust
        // Senaryolar (kaydedilmiş simülasyon koşuları) da yeni drafta taşınır:
        // yeni versiyon eskinin regresyon testleriyle karşılanmalı (best-effort).
        if let Ok(Some(scenarios)) = self.fetch_scenarios(src_id, src_version).await {
            let _ = self.save_scenarios(created.0, created.1, &scenarios).await;
        }
```

- [ ] **Step 3: `delete_draft`'a temizliği ekle**

`crates/wfd/src/adapter.rs`, `delete_draft` içinde `let _ = self.storage.delete(&meta.s3_key).await;` satırını **şununla değiştir**:

```rust
        let _ = self.storage.delete(&meta.s3_key).await;
        // Sidecar'lar da gider — aksi halde storage'da öksüz blob birikir.
        // (Layout bugüne kadar temizlenmiyordu; senaryo sidecar'ını eklerken
        // yanına ikinci bir öksüz bırakmak anlamsız olurdu.)
        let _ = self
            .storage
            .delete(&storage::layout_key(meta.orgtnt_id, wfd_id, version))
            .await;
        let _ = self
            .storage
            .delete(&storage::scenarios_key(meta.orgtnt_id, wfd_id, version))
            .await;
```

- [ ] **Step 4: Derlendiğini ve workspace'in yeşil kaldığını gör**

Run: `cargo test --workspace`
Expected: PASS

> **Neden bu task'ın otomatik testi yok:** adapter metotlarının hepsi
> `repo::get_meta_any` üzerinden DB'ye gidiyor ve bu repoda DB'ye bağlanan
> **hiç test yok** (`grep -rl "PgPool\|DATABASE_URL" crates/*/tests/` → boş).
> Spec §9'un "storage/adapter" testleri bu yüzden Task 8-9'daki elle `curl`
> doğrulamasına devredildi. Test altyapısı kurmak ayrı bir iştir; bu planın
> kapsamında değil ama bilinerek atlanıyor, unutularak değil.

- [ ] **Step 5: Commit**

```bash
git add crates/wfd/src/adapter.rs
git commit -m "feat(wfd): senaryo sidecar'ı oku/yaz + yeni-draft kopyası + draft silmede temizlik

delete_draft bugüne dek layout blob'unu da öksüz bırakıyordu; senaryo
sidecar'ını eklerken ikisi birden temizleniyor."
```

---

# FAZ 2 — Koşucu (crates/wfe)

## Task 3: Senaryo tipleri

**Files:**
- Create: `crates/wfe/src/scenario.rs`
- Modify: `crates/wfe/src/lib.rs` (modül listesi)
- Test: `crates/wfe/src/scenario.rs` içindeki `#[cfg(test)] mod tests`

- [ ] **Step 1: Failing test yaz**

`crates/wfe/src/scenario.rs` oluştur, ŞİMDİLİK yalnız testleri koy:

```rust
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
        assert!(matches!(&s.steps[0], ScenarioStep::Action { action, node: None, .. } if action == "onayla"));
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
        let step: ScenarioStep = serde_json::from_value(serde_json::json!({ "call_return": {} })).unwrap();
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
```

- [ ] **Step 2: Testin FAIL ettiğini gör**

Önce `crates/wfe/src/lib.rs`'e modülü ekle (alfabetik sırada, `runner`'dan sonra):

```rust
pub mod scenario;
```

Run: `cargo test -p wf-wfe scenario::tests`
Expected: derleme hatası — `cannot find type 'ScenarioSet' in this scope`

- [ ] **Step 3: Tipleri yaz**

`crates/wfe/src/scenario.rs`'in BAŞINA (test modülünün üstüne):

```rust
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
    #[serde(rename = "startAction", default, skip_serializing_if = "Option::is_none")]
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
    #[serde(rename = "contextContains", default, skip_serializing_if = "Option::is_none")]
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
```

- [ ] **Step 4: Testlerin GEÇTİĞİNİ gör**

Run: `cargo test -p wf-wfe scenario::tests`
Expected: `test result: ok. 4 passed`

- [ ] **Step 5: Commit**

```bash
git add crates/wfe/src/scenario.rs crates/wfe/src/lib.rs
git commit -m "feat(wfe): senaryo tipleri — ScenarioSet/Scenario/ScenarioStep

Adım ayrık birleşim: aksiyon | call_return (WFC durağı). Alan adları
editörün localStorage şekliyle aynı tutuldu, göç dönüştürmesiz olsun diye."
```

---

## Task 4: Beklenti denetimi (saf fonksiyonlar)

TypeScript'teki `checkScenarioExpectations` / `deepContains` / `inferTerminalId`'nin Rust karşılığı.

**Files:**
- Modify: `crates/wfe/src/scenario.rs`

- [ ] **Step 1: Failing test yaz**

`mod tests` içine ekle:

```rust
    fn dynctx(v: serde_json::Value) -> serde_json::Value { v }

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
        let ok = check_expectations(Some(&e), false, None,
            &dynctx(serde_json::json!({ "musteri": { "ad": "Ay", "yas": 30 }, "x": 1 })));
        assert!(ok.is_empty(), "{ok:?}");

        let bad = check_expectations(Some(&e), false, None,
            &dynctx(serde_json::json!({ "musteri": { "ad": "Bora" } })));
        assert_eq!(bad.len(), 1);
        assert!(bad[0].contains("musteri.ad"), "{bad:?}");
    }

    #[test]
    fn arrays_must_match_exactly_not_as_subset() {
        let e = Expect { terminal: None, context_contains: Some(serde_json::json!({ "l": [1, 2] })) };
        assert!(check_expectations(Some(&e), false, None, &dynctx(serde_json::json!({ "l": [1, 2] }))).is_empty());
        assert_eq!(check_expectations(Some(&e), false, None, &dynctx(serde_json::json!({ "l": [1, 2, 3] }))).len(), 1);
    }

    #[test]
    fn missing_field_is_reported_with_its_path() {
        let e = Expect { terminal: None, context_contains: Some(serde_json::json!({ "a": { "b": 1 } })) };
        let f = check_expectations(Some(&e), false, None, &dynctx(serde_json::json!({ "a": {} })));
        assert_eq!(f.len(), 1);
        assert!(f[0].contains("a.b"), "{f:?}");
    }

    #[test]
    fn terminal_expectation_needs_a_reached_terminal() {
        let e = Expect { terminal: Some("onaylandi".into()), context_contains: None };
        let still_active = check_expectations(Some(&e), false, None, &dynctx(serde_json::json!({})));
        assert_eq!(still_active.len(), 1);
        assert!(still_active[0].contains("aktif"), "{still_active:?}");

        let wrong = check_expectations(Some(&e), true, Some("reddedildi"), &dynctx(serde_json::json!({})));
        assert_eq!(wrong.len(), 1);
        assert!(wrong[0].contains("reddedildi"));

        assert!(check_expectations(Some(&e), true, Some("onaylandi"), &dynctx(serde_json::json!({}))).is_empty());
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
        assert_eq!(infer_terminal_id(&wfd, &serde_json::json!({ "durum": "beklemede" })), None);
        // Etkisiz aday (boş set) hiçbir zaman eşleşmez — yoksa her dynctx'e uyardı.
        assert_eq!(infer_terminal_id(&wfd, &serde_json::json!({})), None);
    }
```

- [ ] **Step 2: Testin FAIL ettiğini gör**

Run: `cargo test -p wf-wfe scenario::tests`
Expected: derleme hatası — `cannot find function 'check_expectations'`

- [ ] **Step 3: Fonksiyonları yaz**

`crates/wfe/src/scenario.rs`'e, tiplerin altına:

```rust
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
                let next = if path.is_empty() { k.clone() } else { format!("{path}.{k}") };
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
    let Some(e) = expect else { return Vec::new() };
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
        let id = t.get("id")?.as_str()?;
        let effects = t
            .get("wfes_effects")
            .and_then(|w| w.get("set"))
            .and_then(|s| s.as_object());
        let Some(effects) = effects else { continue };
        if effects.is_empty() {
            continue;
        }
        let mut failures = Vec::new();
        deep_contains(&Value::Object(effects.clone()), Some(dynctx), "", &mut failures);
        if failures.is_empty() {
            if hit.is_some() {
                return None; // birden çok aday — belirsiz
            }
            hit = Some(id.to_string());
        }
    }
    hit
}
```

- [ ] **Step 4: Testlerin GEÇTİĞİNİ gör**

Run: `cargo test -p wf-wfe scenario::tests`
Expected: `test result: ok. 10 passed`

- [ ] **Step 5: Commit**

```bash
git add crates/wfe/src/scenario.rs
git commit -m "feat(wfe): beklenti denetimi motora indi (check_expectations, infer_terminal_id)

Sözleşme editördeki deepContains/inferTerminalId ile birebir: nesneler alt
küme, dizi/skaler tam eşleşme; terminal ancak TEK aday eşleşirse çözülür."
```

---

## Task 5: Adım mantığını `sim`'e çıkar (davranış değişmez)

Bugün `routes/simulate.rs`'in içinde yaşayan üç adım mantığı `wfe::sim`'e taşınır ki koşucu ile route aynı kodu koşsun.

**Files:**
- Modify: `crates/wfe/src/sim.rs` (dosya sonuna)
- Modify: `crates/server/src/routes/simulate.rs` (`sim_start` ~156, `sim_apply` ~234, `sim_call_return` ~342)

- [ ] **Step 1: Yardımcıları yaz**

`crates/wfe/src/sim.rs` sonuna:

```rust
/// Bir adımın motor tarafı — `routes/simulate.rs` ve `scenario::run` ORTAK kullanır.
/// Ayrı yazılsalardı simülasyonda geçen bir senaryo koşucuda kalabilirdi.
pub mod step {
    use super::SimState;
    use serde_json::Value;
    use uuid::Uuid;
    use wfe_core::types::actor::Actor;
    use wfe_core::types::wfd_v22::Wfd;
    use wfe_core::types::wfe::WfeStatus;
    use wfe_core::v22::pipeline::Engine;
    use wfe_core::EngineError;

    /// WFE'nin bittiğini söyleyen tek yer — iki durum da "artık adım atılamaz".
    pub fn is_terminal(state: &SimState) -> bool {
        matches!(state.status, WfeStatus::Terminal | WfeStatus::Terminated)
    }

    /// `POST /wfe/simulate/start` gövdesi.
    pub async fn start(
        engine: &Engine<'_>,
        wfd: &Wfd,
        actor: &Actor,
        orgtnt_id: Uuid,
        action: Option<&str>,
        input: &Value,
    ) -> Result<SimState, EngineError> {
        let new = engine
            .start(wfd, actor, orgtnt_id, action, input, Uuid::new_v4(), None)
            .await?;
        Ok(SimState::from_new_wfe(&new))
    }

    /// `POST /wfe/simulate/apply` gövdesi — claim YAZILMAZ ama uygunluk
    /// çağıranın sorumluluğundadır (route `sim_eligible` ile denetler).
    pub async fn apply(
        engine: &Engine<'_>,
        wfd: &Wfd,
        state: &mut SimState,
        actor: &Actor,
        action: &str,
        input: &Value,
        node: Option<&str>,
    ) -> Result<(), EngineError> {
        let wfes = state.to_wfes(Some(actor.user_id));
        let commit = engine
            .apply(wfd, &wfes, actor, action, input, node)
            .await?;
        state.apply_commit(&commit);
        Ok(())
    }

    /// `POST /wfe/simulate/call-return` gövdesi. Bekleyen çağrı yoksa
    /// `EngineError::InvalidState` döner (route bunu 409'a çevirir).
    pub async fn call_return(
        engine: &Engine<'_>,
        wfd: &Wfd,
        state: &mut SimState,
        status: &str,
        result: Option<&Value>,
    ) -> Result<(), EngineError> {
        let awaited = state.awaited_call().cloned().ok_or_else(|| {
            EngineError::InvalidState("bu adımda çözülmeyi bekleyen bir iş akışı çağrısı yok".into())
        })?;
        let wfes = state.to_wfes(None);
        let commit = engine
            .fire_call_return(wfd, &wfes, status, None, result, &[], chrono::Utc::now())
            .await?;
        state.apply_commit(&commit);
        state.clear_awaited_call(&awaited.site_key);
        Ok(())
    }
}
```

> **Not:** `EngineError::InvalidState` yoksa `crates/wfe-core/src/error.rs`'te mevcut olan en yakın varyantı kullan (ör. `EngineError::Conflict`). Varyant adını `grep -n "pub enum EngineError" -A 40 crates/wfe-core/src/error.rs` ile doğrula ve route'taki eşlemeyi ona göre yaz.

- [ ] **Step 2: Route'ları yardımcıya bağla**

`crates/server/src/routes/simulate.rs`:

`sim_start` içinde `let new = engine.start(...)` + `let sim_state = SimState::from_new_wfe(&new);` bloğunu şununla değiştir:

```rust
    let sim_state = wf_wfe::sim::step::start(
        &engine, &wfd, &body.actor, orgtnt_id, body.action.as_deref(), &body.input,
    )
    .await
    .map_err(AppError::from)?;
```

`sim_apply` içinde `let wfes = sim_state.to_wfes(...)` + `let commit = engine.apply(...)` + `sim_state.apply_commit(&commit);` bloğunu:

```rust
    wf_wfe::sim::step::apply(
        &engine, &wfd, &mut sim_state, &body.actor, &body.action, &body.input, body.node.as_deref(),
    )
    .await
    .map_err(AppError::from)?;
```

`sim_call_return` içinde `awaited` çözümünden `clear_awaited_call`'a kadarki bloğu:

```rust
    wf_wfe::sim::step::call_return(
        &engine, &wfd, &mut sim_state, &body.status, body.result.as_ref(),
    )
    .await
    .map_err(AppError::from)?;
```

Üç yerdeki `let terminal = matches!(sim_state.status, ...)` satırlarını da `wf_wfe::sim::step::is_terminal(&sim_state)` ile değiştir.

- [ ] **Step 3: Mevcut testlerin HÂLÂ geçtiğini gör (davranış değişmedi)**

Run: `cargo test --workspace`
Expected: PASS — özellikle `crates/wfe/tests/sim_fork_join.rs` yeşil kalmalı. Kaldıysa refactor davranışı değiştirmiş demektir; geri al ve farkı bul.

- [ ] **Step 4: Commit**

```bash
git add crates/wfe/src/sim.rs crates/server/src/routes/simulate.rs
git commit -m "refactor(wfe): sim adım mantığı route'lardan sim::step'e çıktı

Senaryo koşucusu ile /wfe/simulate rotaları aynı kodu koşsun diye; ayrı
yazılsalardı simülasyonda geçen senaryo koşucuda kalabilirdi. Davranış aynı."
```

---

## Task 6: Koşucu

**Files:**
- Modify: `crates/wfe/src/scenario.rs`
- Test: `crates/wfe/tests/scenario.rs` (yeni)

- [ ] **Step 1: Failing test yaz**

`crates/wfe/tests/scenario.rs` oluştur. Mock kalıbı `crates/wfe/tests/sim_fork_join.rs`'ten kopyalanmıştır:

```rust
//! `wf_wfe::scenario::run` uçtan uca — ağsız, store'suz (mock OrgPort/AutoexecRunner).

use async_trait::async_trait;
use serde_json::json;
use uuid::Uuid;
use wf_wfe::scenario::{run, Scenario, ScenarioActor, ScenarioStep, Expect};
use wfe_core::ports::OrgPort;
use wfe_core::types::actor::{Actor, OrgUnit};
use wfe_core::types::wfd_v22::{AutoexecDef, Wfd};
use wfe_core::v22::pipeline::Engine;
use wfe_core::v22::ports::{AutoexecRunner, ExecEnv, ExecFailure};
use wfe_core::EngineError;

const GOLDEN: &str = include_str!("../../wfe-core/tests/fixtures/kredi-basvuru.golden.json");

struct MockOrg;
#[async_trait]
impl OrgPort for MockOrg {
    async fn resolve_c_orgu(&self, anchor: Uuid, _e: &str, _t: Uuid) -> Result<Vec<OrgUnit>, EngineError> {
        Ok(vec![OrgUnit { orgu_id: anchor, orgu_type: json!({"type": "branch"}), path: "1".into() }])
    }
    async fn check_user_role(&self, _: Uuid, _: Uuid, _: &str) -> Result<bool, EngineError> { Ok(true) }
    async fn orgtnt_for_orgu(&self, _: Uuid) -> Result<Uuid, EngineError> { Ok(Uuid::nil()) }
}
struct MockRunner;
#[async_trait]
impl AutoexecRunner for MockRunner {
    async fn run(&self, _d: &AutoexecDef, _e: &ExecEnv) -> Result<serde_json::Value, ExecFailure> { Ok(json!({})) }
}
static MOCK_ORG: MockOrg = MockOrg;
static MOCK_RUNNER: MockRunner = MockRunner;

fn engine() -> Engine<'static> {
    Engine { org: &MOCK_ORG, exec: &MOCK_RUNNER, env: Default::default() }
}

fn sc_actor(role: &str) -> ScenarioActor {
    ScenarioActor { orgu_id: Uuid::new_v4(), user_id: Uuid::new_v4(), role: role.into() }
}

fn fallback() -> Actor {
    Actor { orgu_id: Uuid::new_v4(), user_id: Uuid::new_v4(), role: "basvuran".into() }
}

fn base_scenario() -> Scenario {
    Scenario {
        id: "s1".into(),
        name: "test".into(),
        path: String::new(),
        description: None,
        environment: None,
        start_actor: Some(sc_actor("basvuran")),
        start_action: None,
        start_input: json!({}),
        steps: vec![],
        expect: None,
    }
}

/// Beklentisiz senaryo start atıp durur ve GEÇER — koşucu "hata yoksa ok".
#[tokio::test]
async fn scenario_without_expectations_passes_after_start() {
    let wfd = Wfd::from_json(GOLDEN).unwrap();
    let res = run(&engine(), &wfd, &serde_json::from_str(GOLDEN).unwrap(), &base_scenario(), None).await;
    assert!(res.ok, "{:?}", res.failures);
    assert_eq!(res.steps_executed, 0);
}

/// Aktörü olmayan adım, fallback verilmezse senaryoyu KALDIRIR (panik değil).
#[tokio::test]
async fn step_without_actor_and_without_fallback_fails_the_scenario() {
    let wfd = Wfd::from_json(GOLDEN).unwrap();
    let mut s = base_scenario();
    s.start_actor = None;
    let res = run(&engine(), &wfd, &serde_json::from_str(GOLDEN).unwrap(), &s, None).await;
    assert!(!res.ok);
    assert!(res.failures[0].contains("aktör"), "{:?}", res.failures);
}

/// Fallback verilirse aktörsüz senaryo koşar.
#[tokio::test]
async fn fallback_actor_is_used_when_the_scenario_has_none() {
    let wfd = Wfd::from_json(GOLDEN).unwrap();
    let mut s = base_scenario();
    s.start_actor = None;
    let res = run(&engine(), &wfd, &serde_json::from_str(GOLDEN).unwrap(), &s, Some(&fallback())).await;
    assert!(res.ok, "{:?}", res.failures);
}

/// Karşılanmayan terminal beklentisi failure üretir, koşuyu patlatmaz.
#[tokio::test]
async fn unmet_terminal_expectation_is_a_failure_not_an_error() {
    let wfd = Wfd::from_json(GOLDEN).unwrap();
    let mut s = base_scenario();
    s.expect = Some(Expect { terminal: Some("YokBoyleTerminal".into()), context_contains: None });
    let res = run(&engine(), &wfd, &serde_json::from_str(GOLDEN).unwrap(), &s, None).await;
    assert!(!res.ok);
    assert_eq!(res.failures.len(), 1);
}

/// Var olmayan aksiyon: motor hatası failure'a çevrilir, sonraki adımlar atlanır.
#[tokio::test]
async fn engine_error_stops_the_run_and_becomes_a_failure() {
    let wfd = Wfd::from_json(GOLDEN).unwrap();
    let mut s = base_scenario();
    s.steps = vec![
        ScenarioStep::Action { action: "boyle_bir_aksiyon_yok".into(), actor: Some(sc_actor("mudur")), input: json!({}), node: None },
        ScenarioStep::Action { action: "ikinci".into(), actor: Some(sc_actor("mudur")), input: json!({}), node: None },
    ];
    let res = run(&engine(), &wfd, &serde_json::from_str(GOLDEN).unwrap(), &s, None).await;
    assert!(!res.ok);
    assert_eq!(res.steps_executed, 0, "hatalı adım sayılmaz ve sonrası koşulmaz");
    assert!(res.failures[0].contains("Adım 1"), "{:?}", res.failures);
}
```

- [ ] **Step 2: Testin FAIL ettiğini gör**

Run: `cargo test -p wf-wfe --test scenario`
Expected: derleme hatası — `cannot find function 'run' in module 'wf_wfe::scenario'`

- [ ] **Step 3: Koşucuyu yaz**

`crates/wfe/src/scenario.rs` sonuna (test modülünün ÜSTÜNE):

```rust
use crate::sim::{step, SimState};
use wfe_core::types::wfd_v22::Wfd;
use wfe_core::v22::pipeline::Engine;

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
        a.as_ref().map(|x| x.to_actor()).or_else(|| fallback_actor.cloned())
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
        return fail(vec!["başlangıç aktörü çözülemedi (senaryoda yok, yedek aktör de verilmedi)".into()], 0);
    };

    let orgtnt_id = match wfe_core::ports::OrgPort::orgtnt_for_orgu(engine.org, start_actor.orgu_id).await {
        Ok(id) => id,
        Err(e) => return fail(vec![format!("aktörün tenant'ı çözülemedi: {e}")], 0),
    };

    let mut state = match step::start(
        engine, wfd, &start_actor, orgtnt_id,
        scenario.start_action.as_deref(), &scenario.start_input,
    ).await {
        Ok(s) => s,
        Err(e) => return fail(vec![format!("start: {e}")], 0),
    };

    let mut steps_executed = 0usize;
    for (i, s) in scenario.steps.iter().enumerate() {
        if step::is_terminal(&state) {
            break; // terminale ulaşıldı — kalan adımlar sessizce atlanır
        }
        let outcome = match s {
            ScenarioStep::Action { action, actor, input, node } => {
                match resolve(actor) {
                    None => Err(format!("Adım {} (\"{action}\") için aktör çözülemedi", i + 1)),
                    Some(a) => step::apply(engine, wfd, &mut state, &a, action, input, node.as_deref())
                        .await
                        .map_err(|e| format!("Adım {} (\"{action}\"): {e}", i + 1)),
                }
            }
            ScenarioStep::CallReturn { call_return } => step::call_return(
                engine, wfd, &mut state, &call_return.status, call_return.result.as_ref(),
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
    let terminal_id = if terminal { infer_terminal_id(wfd_json, &dynctx) } else { None };
    let failures = check_expectations(scenario.expect.as_ref(), terminal, terminal_id.as_deref(), &dynctx);

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
```

> **Not:** `state.dynctx`'in tipi `Value` değilse (`sim.rs`'te doğrula: `grep -n "dynctx" crates/wfe/src/sim.rs`), `serde_json::to_value(...)` ile çevir.

- [ ] **Step 4: Testlerin GEÇTİĞİNİ gör**

Run: `cargo test -p wf-wfe --test scenario`
Expected: `test result: ok. 5 passed`

- [ ] **Step 5: Commit**

```bash
git add crates/wfe/src/scenario.rs crates/wfe/tests/scenario.rs
git commit -m "feat(wfe): senaryo koşucusu — sim makinesini uçtan uca sürer

Motor hatası senaryoyu KALDIRIR (Err değil): bir senaryonun kalması normal
bir sonuçtur. Terminale ulaşılınca kalan adımlar atlanır."
```

---

## Task 7: Paralel kol, çağrı dönüşü ve `startAction` entegrasyon testleri

**Files:**
- Modify: `crates/wfe/tests/scenario.rs`

- [ ] **Step 1: Testleri yaz**

`crates/wfe/tests/scenario.rs` sonuna. Fixture'lar: paralel için `paralel-onay.json`, WFC için `akis-cagrisi.json`.

```rust
const PARALLEL: &str = include_str!("../../wfe-core/tests/fixtures/paralel-onay.json");
const CALLER: &str = include_str!("../../wfe-core/tests/fixtures/akis-cagrisi.json");

/// Paralel kolda adım, `node` ile hangi kola uygulandığını söyleyebilmeli.
/// (Kol adları ve aksiyonlar için `crates/wfe/tests/sim_fork_join.rs`'e bak —
/// fixture'ın gerçek node key'lerini ORADAN al, uydurma.)
#[tokio::test]
async fn parallel_branch_step_targets_its_branch_via_node() {
    let wfd = Wfd::from_json(PARALLEL).unwrap();
    let json: serde_json::Value = serde_json::from_str(PARALLEL).unwrap();
    let mut s = base_scenario();
    s.start_actor = Some(sc_actor("requester"));
    s.start_input = json!({"request": {"title": "Sunucu alımı", "amount": 150000}});
    s.steps = vec![
        ScenarioStep::Action { action: "start_review".into(), actor: Some(sc_actor("coordinator")), input: json!({}), node: None },
        ScenarioStep::Action { action: "approve".into(), actor: Some(sc_actor("financeApprover")), input: json!({}), node: Some("self__financeApprover".into()) },
    ];
    let res = run(&engine(), &wfd, &json, &s, None).await;
    assert!(res.ok, "{:?}", res.failures);
    assert_eq!(res.steps_executed, 2);
}

/// `node` verilmezse paralel modda aynı adım belirsizdir ve senaryo kalır —
/// koşucunun kol seçimini gerçekten ilettiğinin kanıtı.
#[tokio::test]
async fn parallel_branch_step_without_node_fails() {
    let wfd = Wfd::from_json(PARALLEL).unwrap();
    let json: serde_json::Value = serde_json::from_str(PARALLEL).unwrap();
    let mut s = base_scenario();
    s.start_actor = Some(sc_actor("requester"));
    s.start_input = json!({"request": {"title": "Sunucu alımı", "amount": 150000}});
    s.steps = vec![
        ScenarioStep::Action { action: "start_review".into(), actor: Some(sc_actor("coordinator")), input: json!({}), node: None },
        ScenarioStep::Action { action: "approve".into(), actor: Some(sc_actor("financeApprover")), input: json!({}), node: None },
    ];
    let res = run(&engine(), &wfd, &json, &s, None).await;
    assert!(!res.ok, "node'suz kol adımı geçmemeliydi");
}

/// WFC durağı: alt akış çağrısından sonra `call_return` adımı akışı ilerletir.
/// (Çağrı node'una hangi aksiyonla varıldığını `crates/wfe-core/tests/calls.rs`
/// veya `crates/wfe/tests/call_executor.rs`'ten al.)
#[tokio::test]
async fn call_return_step_resumes_a_waiting_call() {
    let wfd = Wfd::from_json(CALLER).unwrap();
    let json: serde_json::Value = serde_json::from_str(CALLER).unwrap();
    let mut s = base_scenario();
    s.steps = vec![
        // 1) çağrı node'una götüren aksiyon (fixture'dan doğrula)
        ScenarioStep::Action { action: "submit".into(), actor: Some(sc_actor("basvuran")), input: json!({}), node: None },
        // 2) çağrı dönüşü
        ScenarioStep::CallReturn { call_return: wf_wfe::scenario::CallReturn { status: "completed".into(), result: Some(json!({"skor": 82})) } },
    ];
    let res = run(&engine(), &wfd, &json, &s, None).await;
    assert!(res.ok, "{:?}", res.failures);
    assert_eq!(res.steps_executed, 2);
}

/// Bekleyen çağrı yokken `call_return` adımı senaryoyu kaldırır.
#[tokio::test]
async fn call_return_without_a_waiting_call_fails_the_scenario() {
    let wfd = Wfd::from_json(GOLDEN).unwrap();
    let json: serde_json::Value = serde_json::from_str(GOLDEN).unwrap();
    let mut s = base_scenario();
    s.steps = vec![ScenarioStep::CallReturn {
        call_return: wf_wfe::scenario::CallReturn { status: "completed".into(), result: None },
    }];
    let res = run(&engine(), &wfd, &json, &s, None).await;
    assert!(!res.ok);
    assert!(res.failures[0].contains("çağrı"), "{:?}", res.failures);
}

/// `startAction` verilen start kuralını seçer; var olmayan bir ad senaryoyu kaldırır.
#[tokio::test]
async fn unknown_start_action_fails_the_scenario() {
    let wfd = Wfd::from_json(GOLDEN).unwrap();
    let json: serde_json::Value = serde_json::from_str(GOLDEN).unwrap();
    let mut s = base_scenario();
    s.start_action = Some("boyle_bir_start_yok".into());
    let res = run(&engine(), &wfd, &json, &s, None).await;
    assert!(!res.ok);
    assert!(res.failures[0].starts_with("start:"), "{:?}", res.failures);
}
```

- [ ] **Step 2: Testleri koştur, fixture gerçekleriyle hizala**

Run: `cargo test -p wf-wfe --test scenario`

Beklenen: aksiyon/node adları fixture'la uyuşmuyorsa bazıları FAIL eder. **Fixture'ı DEĞİŞTİRME** — testteki adları `crates/wfe/tests/sim_fork_join.rs` ve `crates/wfe/tests/call_executor.rs`'teki gerçek adlarla düzelt.

- [ ] **Step 3: Yeşile al**

Run: `cargo test -p wf-wfe --test scenario`
Expected: `test result: ok. 10 passed`

- [ ] **Step 4: Commit**

```bash
git add crates/wfe/tests/scenario.rs
git commit -m "test(wfe): senaryo koşucusu — paralel kol, WFC çağrı dönüşü, startAction"
```

---

# FAZ 3 — Rotalar (crates/server)

## Task 8: `GET`/`PUT /wfd/{id}/{ver}/scenarios`

**Files:**
- Modify: `crates/server/src/routes/wfd.rs` (router ~satır 39; handler'lar `put_layout`'un altına ~satır 800)

- [ ] **Step 1: Handler'ları yaz**

`crates/server/src/routes/wfd.rs`, `put_layout`'un altına:

```rust
#[utoipa::path(get, path = "/{id}/{version}/scenarios", tag = "wfd",
    params(("id" = Uuid, Path, description = "WFD id"), ("version" = i32, Path, description = "Versiyon")),
    responses((status = 200, description = "Senaryo seti (blob yoksa boş set)", body = serde_json::Value)),
    security(("bearer_jwt" = [])))]
async fn get_scenarios(
    State(s): State<AppState>,
    auth: AppAuth,
    Path((id, ver)): Path<(Uuid, i32)>,
) -> Result<Json<Value>, AppError> {
    // Layout'un aksine GET de yetki ister: senaryolar aktör kimlikleri ve iş
    // girdileri taşır.
    require_design_on_wfd(&s, &auth, id, ver).await?;
    let set = s.wfd.fetch_scenarios(id, ver).await.map_err(map_wfd_err)?;
    Ok(Json(set.unwrap_or_else(|| json!({ "scenarios_version": "1", "scenarios": [] }))))
}

#[utoipa::path(put, path = "/{id}/{version}/scenarios", tag = "wfd",
    params(("id" = Uuid, Path, description = "WFD id"), ("version" = i32, Path, description = "Versiyon")),
    request_body = serde_json::Value,
    responses((status = 204, description = "Kaydedildi")),
    security(("bearer_jwt" = [])))]
async fn put_scenarios(
    State(s): State<AppState>,
    auth: AppAuth,
    Path((id, ver)): Path<(Uuid, i32)>,
    Json(body): Json<Value>,
) -> Result<StatusCode, AppError> {
    require_design_on_wfd(&s, &auth, id, ver).await?;
    // Şekli burada doğrula ki bozuk set koşu anına kadar saklanmasın.
    serde_json::from_value::<wf_wfe::scenario::ScenarioSet>(body.clone())
        .map_err(|e| AppError(format!("senaryo seti geçersiz: {e}"), StatusCode::UNPROCESSABLE_ENTITY))?;
    s.wfd
        .save_scenarios(id, ver, &body)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(map_wfd_err)
}
```

`json!` makrosu import edilmemişse dosya başına `use serde_json::json;` ekle.

- [ ] **Step 2: Router'a kaydet**

`crates/server/src/routes/wfd.rs`, `.routes(routes!(get_layout, put_layout))` satırının altına:

```rust
        .routes(routes!(get_scenarios, put_scenarios))
```

- [ ] **Step 3: Derlendiğini gör**

Run: `cargo test --workspace`
Expected: PASS

- [ ] **Step 4: Elle doğrula**

Sunucuyu başlat, geçerli bir JWT ile:

```bash
curl -s -X PUT "$BASE/wfd/$WFD_ID/1/scenarios" -H "Authorization: Bearer $JWT" \
  -H 'content-type: application/json' \
  -d '{"scenarios_version":"1","scenarios":[{"id":"s1","name":"Mutlu yol","startInput":{},"steps":[]}]}' -o /dev/null -w '%{http_code}\n'
# Beklenen: 204
curl -s "$BASE/wfd/$WFD_ID/1/scenarios" -H "Authorization: Bearer $JWT" | head -c 200
# Beklenen: yazdığın set
curl -s -X PUT "$BASE/wfd/$WFD_ID/1/scenarios" -H "Authorization: Bearer $JWT" \
  -H 'content-type: application/json' -d '{"scenarios":[{"name":"id yok"}]}' -o /dev/null -w '%{http_code}\n'
# Beklenen: 422
```

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/routes/wfd.rs
git commit -m "feat(server): GET/PUT /wfd/{id}/{ver}/scenarios

Layout'un aksine GET de yetki ister — senaryolar aktör kimlikleri ve iş
girdileri taşır. Yazarken şekil doğrulanır, bozuk set saklanmaz."
```

---

## Task 9: Koşu uçları

**Files:**
- Modify: `crates/server/src/routes/wfd.rs`

- [ ] **Step 1: Gövde tipini ve handler'ları yaz**

`put_scenarios`'un altına:

```rust
#[derive(Deserialize, ToSchema)]
struct RunScenariosBody {
    /// Verilirse BU doküman koşar (editördeki kaydedilmemiş hâl); verilmezse
    /// depodaki `(id, version)` dokümanı.
    #[serde(default)]
    wfd: Option<Value>,
    /// Senaryo/adım aktörü eksikse kullanılacak yedek aktör.
    #[serde(default)]
    #[schema(value_type = Option<Object>)]
    fallback_actor: Option<wfe_core::types::actor::Actor>,
    /// Yalnız bu yol önekindeki senaryolar koşar (`"Onaylar"` → `"Onaylar/..."` dahil).
    #[serde(default)]
    path_prefix: Option<String>,
}

#[derive(serde::Serialize, ToSchema)]
struct RunScenariosResponse {
    #[schema(value_type = Vec<Object>)]
    results: Vec<wf_wfe::scenario::ScenarioResult>,
}

/// Setten senaryoları yükler, dokümanı çözer ve koşar. `only` verilirse yalnız
/// o id'li senaryo koşar (tek-senaryo ucu bunu kullanır).
async fn run_scenarios_inner(
    s: &AppState,
    auth: &AppAuth,
    id: Uuid,
    ver: i32,
    body: RunScenariosBody,
    only: Option<&str>,
) -> Result<Json<RunScenariosResponse>, AppError> {
    require_design_on_wfd(s, auth, id, ver).await?;

    // Doküman: gövdeden ya da depodan. İki yol da AYNI kapıdan geçer.
    let wfd_json = match body.wfd {
        Some(v) => v,
        None => s.wfd.fetch_json_any(id, ver).await.map_err(map_wfd_err)?,
    };
    let wfd = wfe_core::types::wfd_v22::Wfd::from_value(wfd_json.clone())
        .map_err(|e| AppError(e.to_string(), StatusCode::UNPROCESSABLE_ENTITY))?;
    let report = wfe_core::validator::validate(&wfd);
    if !report.is_valid() {
        let summary = report.errors.iter()
            .map(|e| format!("[{}] {}: {}", e.code, e.path, e.message))
            .collect::<Vec<_>>().join("; ");
        return Err(AppError(format!("WFD geçersiz: {summary}"), StatusCode::UNPROCESSABLE_ENTITY));
    }

    let raw = s.wfd.fetch_scenarios(id, ver).await.map_err(map_wfd_err)?;
    let set: wf_wfe::scenario::ScenarioSet = match raw {
        Some(v) => serde_json::from_value(v)
            .map_err(|e| AppError(format!("senaryo seti geçersiz: {e}"), StatusCode::UNPROCESSABLE_ENTITY))?,
        None => Default::default(),
    };

    let selected: Vec<_> = set.scenarios.iter()
        .filter(|sc| only.map_or(true, |o| sc.id == o))
        .filter(|sc| body.path_prefix.as_ref().map_or(true, |p| sc.path == *p || sc.path.starts_with(&format!("{p}/"))))
        .collect();

    if let Some(o) = only {
        if selected.is_empty() {
            return Err(AppError(format!("senaryo bulunamadı: {o}"), StatusCode::NOT_FOUND));
        }
    }

    let org = Arc::new(wf_wfe::OrgAdapter::new(s.pool.clone()));
    let runner = wf_wfe::LiveAutoexecRunner::new(Some(s.pool.clone()));
    let mut results = Vec::with_capacity(selected.len());
    for sc in selected {
        // $env senaryo başına çözülür — her senaryo kendi ortamını söyleyebilir.
        let engine = wfe_core::v22::pipeline::Engine {
            org: &*org,
            exec: &runner,
            env: crate::routes::env::resolve_run_env(
                &s.pool, Some(auth.orgtnt_id), Some(id), sc.environment.as_deref(),
            ).await?,
        };
        results.push(
            wf_wfe::scenario::run(&engine, &wfd, &wfd_json, sc, body.fallback_actor.as_ref()).await,
        );
    }
    Ok(Json(RunScenariosResponse { results }))
}

#[utoipa::path(post, path = "/{id}/{version}/scenarios/run", tag = "wfd",
    params(("id" = Uuid, Path, description = "WFD id"), ("version" = i32, Path, description = "Versiyon")),
    request_body = RunScenariosBody,
    responses((status = 200, description = "Her senaryo için sonuç", body = RunScenariosResponse)),
    security(("bearer_jwt" = [])))]
async fn run_scenarios(
    State(s): State<AppState>,
    auth: AppAuth,
    Path((id, ver)): Path<(Uuid, i32)>,
    Json(body): Json<RunScenariosBody>,
) -> Result<Json<RunScenariosResponse>, AppError> {
    run_scenarios_inner(&s, &auth, id, ver, body, None).await
}

#[utoipa::path(post, path = "/{id}/{version}/scenarios/{sid}/run", tag = "wfd",
    params(("id" = Uuid, Path, description = "WFD id"), ("version" = i32, Path, description = "Versiyon"),
           ("sid" = String, Path, description = "Senaryo id")),
    request_body = RunScenariosBody,
    responses((status = 200, description = "Tek senaryonun sonucu", body = RunScenariosResponse)),
    security(("bearer_jwt" = [])))]
async fn run_one_scenario(
    State(s): State<AppState>,
    auth: AppAuth,
    Path((id, ver, sid)): Path<(Uuid, i32, String)>,
    Json(body): Json<RunScenariosBody>,
) -> Result<Json<RunScenariosResponse>, AppError> {
    run_scenarios_inner(&s, &auth, id, ver, body, Some(&sid)).await
}
```

`fetch_json_any` Task 2'de eklendi. `Arc`, `StatusCode`, `Deserialize`, `ToSchema` dosyada zaten import edilmiş olmalı; değilse ekle.

- [ ] **Step 2: Router'a kaydet**

```rust
        .routes(routes!(run_scenarios))
        .routes(routes!(run_one_scenario))
```

- [ ] **Step 3: Derlendiğini gör**

Run: `cargo test --workspace`
Expected: PASS

- [ ] **Step 4: Elle doğrula (kabul kriteri 2 ve 3)**

```bash
# Depodaki versiyonu koş
curl -s -X POST "$BASE/wfd/$WFD_ID/1/scenarios/run" -H "Authorization: Bearer $JWT" \
  -H 'content-type: application/json' -d '{}' | head -c 400
# Beklenen: {"results":[{"id":"s1","name":"Mutlu yol","ok":true,...}]}

# Yetkisiz kullanıcı
curl -s -X POST "$BASE/wfd/$WFD_ID/1/scenarios/run" -H "Authorization: Bearer $OTHER_JWT" \
  -H 'content-type: application/json' -d '{}' -o /dev/null -w '%{http_code}\n'
# Beklenen: 403 (ya da başka tenant ise 404)
```

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/routes/wfd.rs
git commit -m "feat(server): senaryo koşu uçları (tek + set)

Gövdedeki opsiyonel wfd editördeki kaydedilmemiş taslağı koşturur; verilmezse
depodaki versiyon koşar (CI yolu). Koşu hiçbir şey yazmaz — sim durumsuzdur."
```

---

# FAZ 4 — Editör (agnoflow-frontend)

> Bu fazın tamamı `~/Desktop/agnoflow-frontend` reposundadır. Testler: `npm test`.

## Task 10: API istemcisi + sidecar'ın sunucuya taşınması

**Files:**
- Modify: `src/api/engineApi.ts`
- Modify: `src/utils/scenarioSidecar.ts`
- Test: `src/utils/__tests__/scenarioSidecar.test.ts`

- [ ] **Step 1: API fonksiyonlarını ekle**

`src/api/engineApi.ts`'e, `getLayout`/`putLayout`'un (satır ~963-975) hemen altına. Dosyanın modül-içi `request<T>()` yardımcısı kullanılır — Authorization başlığını o ekliyor:

```ts
export interface ScenarioSet {
  scenarios_version: string;
  scenarios: WfdScenario[];
}

/** Sunucu koşucusunun senaryo başına döndürdüğü sonuç (snake_case — Rust serde). */
export interface ScenarioRunResult {
  id: string;
  name: string;
  ok: boolean;
  failures: string[];
  steps_executed: number;
  terminal: boolean;
  terminal_id: string | null;
  dynctx: Record<string, unknown>;
}

interface RunScenariosBody {
  /** Verilirse BU doküman koşar (kaydedilmemiş taslak); verilmezse depodaki versiyon. */
  wfd?: unknown;
  /** Senaryo/adım aktörü eksikse kullanılacak yedek. */
  fallback_actor?: SimActorLike | null;
  path_prefix?: string;
}

export function fetchScenarios(baseUrl: string, wfdId: string, version: number): Promise<ScenarioSet> {
  return request<ScenarioSet>(baseUrl, `/wfd/${wfdId}/${version}/scenarios`);
}

export function saveScenarioSet(baseUrl: string, wfdId: string, version: number, set: ScenarioSet): Promise<void> {
  return request<void>(baseUrl, `/wfd/${wfdId}/${version}/scenarios`, {
    method: 'PUT',
    body: JSON.stringify(set),
  });
}

export function runScenarios(
  baseUrl: string, wfdId: string, version: number, body: RunScenariosBody,
): Promise<{ results: ScenarioRunResult[] }> {
  return request(baseUrl, `/wfd/${wfdId}/${version}/scenarios/run`, {
    method: 'POST',
    body: JSON.stringify(body),
  });
}

export function runScenario(
  baseUrl: string, wfdId: string, version: number, scenarioId: string, body: RunScenariosBody,
): Promise<{ results: ScenarioRunResult[] }> {
  return request(baseUrl, `/wfd/${wfdId}/${version}/scenarios/${scenarioId}/run`, {
    method: 'POST',
    body: JSON.stringify(body),
  });
}
```

`WfdScenario` ve `SimActorLike` tiplerini `../utils/scenarioSidecar`'dan import et (dosyanın başındaki import bloğuna ekle).

- [ ] **Step 2: `scenarioSidecar.ts`'i güncelle**

`WfdScenario`'ya yeni alanları ekle ve localStorage fonksiyonlarını göç yoluna indir:

```ts
export interface ScenarioStep {
  action: string;
  input: Record<string, unknown>;
  actor?: ScenarioActor;
  /** WOR-31: paralel modda kol seçimi. */
  node?: string | null;
}

export interface ScenarioCallReturnStep {
  call_return: { status: 'completed' | 'failed' | 'terminated' | 'timeout'; result?: unknown };
}

export type AnyScenarioStep = ScenarioStep | ScenarioCallReturnStep;

export interface WfdScenario {
  id: string;
  name: string;
  /** Dizin yolu — "Onaylar/Müdür". Boş = kök. Ağaç bundan türetilir. */
  path?: string;
  description?: string;
  environment?: string;
  startInput: Record<string, unknown>;
  startActor?: ScenarioActor;
  startAction?: string;
  steps: AnyScenarioStep[];
  expect?: ScenarioExpectation;
}

/** ARTIK YALNIZ GÖÇ İÇİN — canlı okuma sunucudan yapılır (fetchScenarios).
 * Gövde eski `loadScenarios` ile birebir aynıdır, yalnız adı değişti. */
export function loadLegacyScenarios(wfdId: string): WfdScenario[] {
  if (!wfdId || !hasStorage()) return [];
  try {
    const raw = localStorage.getItem(KEY_PREFIX + wfdId);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as unknown;
    return Array.isArray(parsed) ? (parsed as WfdScenario[]) : [];
  } catch {
    return [];
  }
}
```

`saveScenarios` ve `loadScenarios` **silinir** (çağıranları Task 11/13'te değişir).

- [ ] **Step 3: Ağaç türetme yardımcısını ekle**

```ts
export interface ScenarioTreeNode {
  /** Klasörün kendi adı ("Müdür"); kökte boş. */
  name: string;
  /** Kökten buraya tam yol ("Onaylar/Müdür"); kökte boş. */
  path: string;
  children: ScenarioTreeNode[];
  /** DOĞRUDAN bu klasördeki senaryolar (alt klasörünkiler değil). */
  scenarios: WfdScenario[];
}

/** Senaryoların `path` alanlarından klasör ağacı türetir. Ayrı klasör kaydı
 * OLMADIĞI için öksüz/döngülü klasör oluşamaz; boş klasör de olamaz.
 * Yol parçalarındaki boşluklar kırpılır, boş parçalar (`"a//b"`) atlanır. */
export function buildScenarioTree(scenarios: WfdScenario[]): ScenarioTreeNode {
  const root: ScenarioTreeNode = { name: '', path: '', children: [], scenarios: [] };

  for (const scenario of scenarios) {
    const segments = (scenario.path ?? '')
      .split('/')
      .map((s) => s.trim())
      .filter((s) => s.length > 0);

    let node = root;
    for (const segment of segments) {
      let child = node.children.find((c) => c.name === segment);
      if (!child) {
        child = {
          name: segment,
          path: node.path ? `${node.path}/${segment}` : segment,
          children: [],
          scenarios: [],
        };
        node.children.push(child);
      }
      node = child;
    }
    node.scenarios.push(scenario);
  }

  return root;
}

/** Ağaçta bir yolu bulur; yol yoksa null (klasör silinmiş/senaryosu taşınmış olabilir). */
export function findTreeNode(root: ScenarioTreeNode, path: string): ScenarioTreeNode | null {
  const segments = path.split('/').map((s) => s.trim()).filter((s) => s.length > 0);
  let node: ScenarioTreeNode | undefined = root;
  for (const segment of segments) {
    node = node.children.find((c) => c.name === segment);
    if (!node) return null;
  }
  return node;
}
```

- [ ] **Step 4: Test yaz**

`src/utils/__tests__/scenarioSidecar.test.ts`'e ekle:

```ts
describe('buildScenarioTree', () => {
  const sc = (id: string, path?: string): WfdScenario =>
    ({ id, name: id, path, startInput: {}, steps: [] });

  it('path olmayan senaryolar kökte durur', () => {
    const tree = buildScenarioTree([sc('a'), sc('b', '')]);
    expect(tree.scenarios.map((s) => s.id)).toEqual(['a', 'b']);
    expect(tree.children).toHaveLength(0);
  });

  it('iç içe path klasör zinciri üretir', () => {
    const tree = buildScenarioTree([sc('a', 'Onaylar/Müdür')]);
    expect(tree.children[0].name).toBe('Onaylar');
    expect(tree.children[0].children[0].name).toBe('Müdür');
    expect(tree.children[0].children[0].scenarios[0].id).toBe('a');
    expect(tree.children[0].children[0].path).toBe('Onaylar/Müdür');
  });

  it('aynı öneki paylaşan senaryolar aynı klasörü paylaşır', () => {
    const tree = buildScenarioTree([sc('a', 'Onaylar/X'), sc('b', 'Onaylar/Y')]);
    expect(tree.children).toHaveLength(1);
    expect(tree.children[0].children.map((c) => c.name)).toEqual(['X', 'Y']);
  });

  it('boş yol parçaları ve baştaki/sondaki boşluklar temizlenir', () => {
    const tree = buildScenarioTree([sc('a', ' Onaylar // Müdür ')]);
    expect(tree.children[0].name).toBe('Onaylar');
    expect(tree.children[0].children[0].name).toBe('Müdür');
  });

  it('findTreeNode var olmayan yolda null döner', () => {
    const tree = buildScenarioTree([sc('a', 'Onaylar')]);
    expect(findTreeNode(tree, 'Onaylar')?.scenarios[0].id).toBe('a');
    expect(findTreeNode(tree, 'Yok/Boyle')).toBeNull();
  });
});
```

- [ ] **Step 5: Testleri koştur**

Run: `npm test -- scenarioSidecar`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/api/engineApi.ts src/utils/scenarioSidecar.ts src/utils/__tests__/scenarioSidecar.test.ts
git commit -m "feat(editor): senaryo API istemcisi + path'ten klasör ağacı türetme

loadScenarios/saveScenarios kaldırıldı; localStorage okuması yalnız göç
yolunda (loadLegacyScenarios) kaldı."
```

---

## Task 11: `ScenarioSection` — sunucudan oku, ağaçta gez, tek çağrıda koş

**Files:**
- Modify: `src/components/ScenarioSection.tsx`

- [ ] **Step 1: Yükleme/kaydetmeyi sunucuya bağla**

`wfdId` ve `wfdVersion` store'dan gelir — bileşen zaten `useWfdStore` kullanıyor (`TopBar.tsx:187`'deki `putLayout(engineConfig.baseUrl, wfdId, wfdVersion, layout)` aynı kaynağı kullanıyor):

```ts
const wfdId = useWfdStore((s) => s.wfdId);
const wfdVersion = useWfdStore((s) => s.wfdVersion);

const [scenarios, setScenarios] = useState<WfdScenario[]>([]);
const [loading, setLoading] = useState(false);
const [loadError, setLoadError] = useState<string | null>(null);

useEffect(() => {
  if (!wfdId || wfdVersion == null) { setScenarios([]); return; }
  const { baseUrl } = readStoredEngineConfig();
  if (!baseUrl) return;
  let cancelled = false;
  setLoading(true);
  setLoadError(null);
  fetchScenarios(baseUrl, wfdId, wfdVersion)
    .then((set) => { if (!cancelled) setScenarios(set.scenarios ?? []); })
    .catch((e: unknown) => { if (!cancelled) setLoadError(e instanceof Error ? e.message : String(e)); })
    .finally(() => { if (!cancelled) setLoading(false); });
  return () => { cancelled = true; };
}, [wfdId, wfdVersion]);

/** Tek yazma yolu — her düzenleme setin tamamını gönderir (yazma atomik). */
async function persist(next: WfdScenario[]): Promise<void> {
  setScenarios(next); // iyimser
  if (!wfdId || wfdVersion == null) return;
  const { baseUrl } = readStoredEngineConfig();
  if (!baseUrl) return;
  await saveScenarioSet(baseUrl, wfdId, wfdVersion, {
    scenarios_version: '1',
    scenarios: next,
  });
}
```

Bugün `saveScenarios(wfdId, next)` çağrılan her yeri `await persist(next)` ile değiştir. `loading` iken listenin yerine "Senaryolar yükleniyor…", `loadError` varsa hata şeridi göster.

- [ ] **Step 2: `runOne`/`runAll`'ı tek çağrıya indir**

`runOne` gövdesindeki `simulateWfe(...)` + `inferTerminalId(...)` + `checkScenarioExpectations(...)` üçlüsünü şununla değiştir:

```ts
      const { results } = await runScenario(stored.baseUrl, wfdId, wfdVersion, scenario.id, {
        wfd: serialized,
        fallback_actor: fallback,
      });
      const r = results[0];
      setResults((prev) => ({
        ...prev,
        [scenario.id]: {
          status: r.ok ? 'ok' : 'fail',
          failures: r.failures,
          terminalId: r.terminal_id,
          terminalReached: r.terminal,
          stepsExecuted: r.steps_executed,
          totalSteps: scenario.steps.length,
        },
      }));
```

`runAll` döngü yerine tek `runScenarios(...)` çağrısı yapar ve dönen `results`'ı id'ye göre `setResults`'a yazar.

- [ ] **Step 3: Ağaç görünümü + breadcrumb (T‑B1/B5)**

```tsx
const [currentPath, setCurrentPath] = useState('');

const tree = useMemo(() => buildScenarioTree(scenarios), [scenarios]);
// Bulunulan klasör silinmişse (son senaryosu taşındı) köke düş — boş ekran yerine.
const currentNode = useMemo(() => findTreeNode(tree, currentPath) ?? tree, [tree, currentPath]);
const crumbs = useMemo(() => {
  const segs = currentPath.split('/').filter(Boolean);
  return segs.map((name, i) => ({ name, path: segs.slice(0, i + 1).join('/') }));
}, [currentPath]);
```

Listenin üstüne breadcrumb, listenin başına klasör satırları:

```tsx
<nav style={{ display: 'flex', gap: 4, alignItems: 'center', fontSize: 11, marginBottom: 8 }}>
  <button type="button" style={btnGhost} onClick={() => setCurrentPath('')}>
    {t('scenarioSection.root')}
  </button>
  {crumbs.map((c) => (
    <span key={c.path} style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}>
      <span style={{ color: 'var(--app-muted)' }}>/</span>
      <button type="button" style={btnGhost} onClick={() => setCurrentPath(c.path)}>{c.name}</button>
    </span>
  ))}
</nav>

{currentNode.children.map((folder) => (
  <div key={folder.path} style={rowCardStyle}>
    <button type="button" style={btnGhost} onClick={() => setCurrentPath(folder.path)}>
      📁 {folder.name}
    </button>
    <span style={{ fontSize: 10, color: 'var(--app-muted)', marginLeft: 8 }}>
      {folder.scenarios.length + folder.children.length}
    </span>
  </div>
))}
```

Senaryo listesi artık `scenarios` yerine **`currentNode.scenarios`** üzerinde dönülür. Senaryo düzenleme formuna `path` metin kutusu eklenir — taşıma bu dizeyi değiştirmektir:

```tsx
<div>
  <div style={labelStyle}>{t('scenarioSection.pathLabel')}</div>
  <input
    style={inputStyle}
    value={scenario.path ?? ''}
    placeholder="Onaylar/Müdür"
    onChange={(e) => void persist(scenarios.map((s) =>
      s.id === scenario.id ? { ...s, path: e.target.value } : s))}
  />
</div>
```

`newScenario()` yeni senaryoyu **bulunulan klasörde** açar: `{ ..., path: currentPath }`.

i18n anahtarları (`scenarioSection.root`, `scenarioSection.pathLabel`) `src/i18n/` altındaki `editor` namespace'ine TR ve varsa diğer dillere eklenir.

- [ ] **Step 4: Testleri koştur**

Run: `npm test`
Expected: PASS (kırılan testleri yeni API'ye göre güncelle)

- [ ] **Step 5: Commit**

```bash
git add src/components/ScenarioSection.tsx
git commit -m "feat(editor): senaryolar sunucudan okunur, klasör ağacında gezilir, tek çağrıda koşar"
```

---

## Task 12: Göç düğmesi

**Files:**
- Modify: `src/components/ScenarioSection.tsx`

- [ ] **Step 1: Göç akışını ekle**

```ts
// localStorage'da senaryo VAR ve sunucuda set BOŞ ise açık bir eylem sun.
// Otomatik yükleme YOK: iki kişinin tarayıcısındaki farklı setler sessizce
// birbirinin üstüne yazabilirdi.
const legacy = useMemo(() => (wfdId ? loadLegacyScenarios(wfdId) : []), [wfdId]);
const canImport = legacy.length > 0 && scenarios.length === 0;
```

`canImport` true iken bir bilgi şeridi ve "Bu tarayıcıdaki N senaryoyu içeri aktar" düğmesi göster:

```tsx
async function importLegacy(): Promise<void> {
  await persist(legacy); // persist zaten wfdId/wfdVersion'ı store'dan alıyor (Task 11)
}
```

```tsx
{canImport && (
  <div style={{ ...rowCardStyle, borderColor: 'var(--app-accent)' }}>
    <span style={{ fontSize: 11 }}>
      {t('scenarioSection.legacyFound', { count: legacy.length })}
    </span>
    <button type="button" style={btnPrimary} onClick={() => void importLegacy()}>
      {t('scenarioSection.importLegacy')}
    </button>
  </div>
)}
```

- [ ] **Step 2: Test yaz**

`src/components/__tests__/ScenarioSection.migration.test.tsx` oluştur (mock kalıbı `smokeRender.i18n.test.tsx`'ten):

```tsx
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { ScenarioSection } from '../ScenarioSection';

const fetchScenarios = vi.fn();
const saveScenarioSet = vi.fn();

vi.mock('../../api/engineApi', async () => {
  const actual = await vi.importActual<Record<string, unknown>>('../../api/engineApi');
  return {
    ...actual,
    readStoredEngineConfig: () => ({ baseUrl: 'http://engine.test' }),
    fetchScenarios: (...a: unknown[]) => fetchScenarios(...a),
    saveScenarioSet: (...a: unknown[]) => saveScenarioSet(...a),
  };
});

// wfdId/wfdVersion store'dan okunuyor — testte sabitle.
vi.mock('../../store/wfd.store', async () => {
  const actual = await vi.importActual<Record<string, unknown>>('../../store/wfd.store');
  return {
    ...actual,
    useWfdStore: (sel: (s: Record<string, unknown>) => unknown) =>
      sel({ wfdId: 'wfd-1', wfdVersion: 1, /* bileşenin okuduğu diğer alanlar */ }),
  };
});

const LEGACY = [{ id: 's1', name: 'Eski senaryo', startInput: {}, steps: [] }];

function renderSection() {
  return render(<ScenarioSection fallbackActor={null} currentRun={null} actors={[]} />);
}

describe('ScenarioSection göç düğmesi', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
  });

  it('localStorage dolu ve sunucu boşken göç düğmesi çıkar ve seti yükler', async () => {
    localStorage.setItem('wfd-scenarios:wfd-1', JSON.stringify(LEGACY));
    fetchScenarios.mockResolvedValue({ scenarios_version: '1', scenarios: [] });
    saveScenarioSet.mockResolvedValue(undefined);

    renderSection();
    const btn = await screen.findByRole('button', { name: /içeri aktar/i });
    await userEvent.click(btn);

    await waitFor(() => expect(saveScenarioSet).toHaveBeenCalledTimes(1));
    expect(saveScenarioSet.mock.calls[0][3]).toEqual({
      scenarios_version: '1',
      scenarios: LEGACY,
    });
  });

  it('sunucuda set varken göç düğmesi ÇIKMAZ — sessizce üstüne yazma riski', async () => {
    localStorage.setItem('wfd-scenarios:wfd-1', JSON.stringify(LEGACY));
    fetchScenarios.mockResolvedValue({
      scenarios_version: '1',
      scenarios: [{ id: 'server-1', name: 'Sunucudaki', startInput: {}, steps: [] }],
    });

    renderSection();
    await screen.findByText('Sunucudaki');
    expect(screen.queryByRole('button', { name: /içeri aktar/i })).toBeNull();
  });

  it('localStorage boşken göç düğmesi çıkmaz', async () => {
    fetchScenarios.mockResolvedValue({ scenarios_version: '1', scenarios: [] });
    renderSection();
    await waitFor(() => expect(fetchScenarios).toHaveBeenCalled());
    expect(screen.queryByRole('button', { name: /içeri aktar/i })).toBeNull();
  });
});
```

- [ ] **Step 3: Testleri koştur**

Run: `npm test -- ScenarioSection`
Expected: PASS. Düğme metni i18n'den geliyorsa regex'i (`/içeri aktar/i`) gerçek metne göre düzelt; store mock'undaki alan listesini bileşenin okuduğu alanlara göre tamamla.

- [ ] **Step 4: Commit**

```bash
git add src/components/ScenarioSection.tsx src/components/__tests__/
git commit -m "feat(editor): localStorage senaryolarını sunucuya taşıyan açık göç düğmesi"
```

---

## Task 13: Publish kapısını yeni uca taşı (SESSİZ GERİLEME RİSKİ)

`TopBar.tsx` bugün yayından önce senaryoları localStorage'dan okuyup tarayıcıda koşturuyor. Taşınmazsa `loadScenarios` **boş liste** döner ve kapı hata vermeden etkisizleşir.

**Files:**
- Modify: `src/components/TopBar.tsx` (~satır 222-274)

- [ ] **Step 1: Failing test yaz**

`src/components/__tests__/TopBar.publishGate.test.tsx` oluştur:

```tsx
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { TopBar } from '../TopBar';

const runScenarios = vi.fn();
const publishDraft = vi.fn();
const saveDraft = vi.fn();

vi.mock('../../api/engineApi', async () => {
  const actual = await vi.importActual<Record<string, unknown>>('../../api/engineApi');
  return {
    ...actual,
    readStoredEngineConfig: () => ({
      baseUrl: 'http://engine.test',
      actorOrguId: 'o1', actorUserId: 'u1', actorRole: 'mudur',
    }),
    runScenarios: (...a: unknown[]) => runScenarios(...a),
    publishDraft: (...a: unknown[]) => publishDraft(...a),
    saveDraft: (...a: unknown[]) => saveDraft(...a),
    putLayout: vi.fn().mockResolvedValue(undefined),
    listEnvironments: vi.fn().mockResolvedValue([]),
  };
});

const RESULT = (ok: boolean) => ({
  results: [{
    id: 's1', name: 'Mutlu yol', ok, failures: ok ? [] : ['terminal beklendi "onaylandi", gelen "reddedildi"'],
    steps_executed: 2, terminal: true, terminal_id: 'reddedildi', dynctx: {},
  }],
});

/** Yayın düğmesine basar.
 *
 * TopBar'ın prop imzasını uygulama sırasında dosyanın başındaki `interface Props`
 * bloğundan OKU ve buradaki nesneyi ona göre doldur — prop'ları uydurma. Zorunlu
 * olan her prop için en yakın boş/no-op değeri geç (fonksiyonlar `vi.fn()`).
 * `wfdId`/`wfdVersion` store'dan geliyorsa ScenarioSection testindeki
 * `vi.mock('../../store/wfd.store', …)` kalıbını buraya da kopyala. */
async function clickPublish() {
  render(<TopBar {...(topBarProps as React.ComponentProps<typeof TopBar>)} />);
  await userEvent.click(screen.getByRole('button', { name: /yayınla/i }));
}

const topBarProps = {
  // uygulama sırasında TopBar'ın Props arayüzünden doldurulacak
};

describe('TopBar publish kapısı', () => {
  beforeEach(() => vi.clearAllMocks());

  it('kalan senaryosu olan taslak YAYINLANAMAZ', async () => {
    runScenarios.mockResolvedValue(RESULT(false));
    await clickPublish();
    await waitFor(() => expect(runScenarios).toHaveBeenCalledTimes(1));
    expect(publishDraft).not.toHaveBeenCalled();
    expect(await screen.findByText(/Mutlu yol/)).toBeTruthy();
  });

  it('senaryolar geçince yayın devam eder', async () => {
    runScenarios.mockResolvedValue(RESULT(true));
    publishDraft.mockResolvedValue(undefined);
    await clickPublish();
    await waitFor(() => expect(publishDraft).toHaveBeenCalledTimes(1));
  });

  it('kapı SUNUCUYA sorar — localStorage boş diye atlanmaz', async () => {
    // Bu testin tek işi gerilemeyi yakalamak: senaryolar sunucuda yaşıyor,
    // kapı localStorage'a bakarsa runScenarios hiç çağrılmaz ve yayın geçer.
    localStorage.clear();
    runScenarios.mockResolvedValue(RESULT(false));
    await clickPublish();
    await waitFor(() => expect(runScenarios).toHaveBeenCalledTimes(1));
    expect(publishDraft).not.toHaveBeenCalled();
  });
});
```

- [ ] **Step 2: Testin FAIL ettiğini gör**

Run: `npm test -- TopBar`
Expected: FAIL

- [ ] **Step 3: Kapıyı yeni uca bağla**

`TopBar.tsx`'te `const scenarios = wfdId ? loadScenarios(wfdId) : [];` ile başlayan bloğu (satır ~223-274) şununla değiştir:

```ts
      // Publish öncesi regresyon: senaryolar sunucuda koşar (tek çağrı).
      // Gövdede serialize edilmiş TASLAK gider — yayınlanacak olan o.
      if (wfdId && wfdVersion != null) {
        const stored = readStoredEngineConfig();
        if (!stored.baseUrl) { setMsg({ ok: false, text: t('topbar.publishNoEngineUrl') }); setBusy(false); return; }
        const fallbackActor = stored.actorOrguId && stored.actorUserId && stored.actorRole
          ? { orgu_id: stored.actorOrguId, user_id: stored.actorUserId, role: stored.actorRole }
          : null;
        try {
          const { results } = await runScenarios(stored.baseUrl, wfdId, wfdVersion, {
            wfd: serialized,
            fallback_actor: fallbackActor,
          });
          const failed = results.filter((r) => !r.ok);
          if (failed.length > 0) {
            setMsg({ ok: false, text: t('topbar.scenarioFailedCount', { count: failed.length, names: failed.map((r) => r.name).join(', ') }) });
            setBusy(false); return;
          }
        } catch (simErr) {
          setMsg({ ok: false, text: t('topbar.scenarioRunFailed', { reason: simErr instanceof Error ? simErr.message : t('topbar.unknownError') }) });
          setBusy(false); return;
        }
      }
```

Artık gereksiz kalan import'ları (`loadScenarios`, `resolveSimActor`, `checkScenarioExpectations`, `simulateWfe`) `TopBar.tsx`'ten kaldır.

- [ ] **Step 4: Testlerin GEÇTİĞİNİ gör**

Run: `npm test -- TopBar`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/components/TopBar.tsx src/components/__tests__/
git commit -m "fix(editor): publish kapısı sunucu koşucusuna bağlandı

Senaryolar localStorage'dan çıktığı için kapı boş listeye düşüp sessizce
etkisizleşecekti — kırık senaryolu WFD'ler yayınlanır hâle gelirdi."
```

---

## Task 14: TS beklenti denetimini sil

**Files:**
- Modify: `src/utils/scenarioSidecar.ts`
- Modify: `src/utils/__tests__/scenarioSidecar.test.ts`

- [ ] **Step 1: Sil**

`checkScenarioExpectations`, `deepContains`, `ScenarioCheckResult`, `ScenarioSimResult` kaldırılır — denetim artık motorda, editör `failures[]`'i gösteriyor.

**KALIR** (silme): `inferTerminalId`, `terminalCandidatesFromSerialized`, `dynctxToRecord`, `scenarioActorToSimActor`, `resolveSimActor` — `SimulationTab` bunları senaryodan bağımsız olarak, interaktif canlı koşuda ulaşılan terminali göstermek için kullanıyor.

- [ ] **Step 2: Testleri temizle**

`scenarioSidecar.test.ts`'teki `describe('checkScenarioExpectations')` bloğunu sil; `inferTerminalId` / `terminalCandidatesFromSerialized` blokları **kalır**.

- [ ] **Step 3: Kalan referans olmadığını doğrula**

Run: `grep -rn "checkScenarioExpectations\|deepContains" src/`
Expected: çıktı YOK

- [ ] **Step 4: Testleri koştur**

Run: `npm test`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/utils/scenarioSidecar.ts src/utils/__tests__/scenarioSidecar.test.ts
git commit -m "refactor(editor): beklenti denetimi silindi — kural motorda, editör sonucu gösteriyor

inferTerminalId ailesi KALIR: SimulationTab interaktif canlı koşuda kullanıyor."
```

---

# Kapanış

- [ ] **Engine tam test**

Run (agnoflow-engine): `cargo test --workspace`
Expected: tüm paketler PASS

- [ ] **Golden fixture değişmemiş**

Run: `git status --porcelain docs/spec/examples/kredi-basvuru.golden.json crates/wfe-core/tests/fixtures/`
Expected: **boş çıktı**

- [ ] **Editör tam test**

Run (agnoflow-frontend): `npm test`
Expected: PASS

- [ ] **Kabul kriterlerini elle geçir**

`docs/superpowers/specs/2026-08-07-senaryo-deposu-ve-kosucusu-design.md` §7'deki 10 maddeyi tek tek doğrula. Özellikle:
- (6) göç düğmesi: localStorage'da senaryo olan bir tarayıcıda çıkıyor, sunucuda set varken çıkmıyor
- (9) publish kapısı: kasten bozulmuş bir senaryo yayını durduruyor

- [ ] **Push**

`main` HER İKİ remote'ta senkron tutulur:

```bash
git push origin main && git push gitlab main
```

Frontend reposunda da aynısı. **`staging`'e push YAPMA** — kullanıcı açıkça "deploy" demedikçe.
