# DB Bağlantı Entegrasyonu — Faz 2 (SQL Node Runtime Bağlama) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** SQL autoexec node'u seçilen bir DB bağlantısına gerçekten bağlanıp (Postgres/MySQL/MSSQL) sorguyu o veritabanında çalıştırsın; editörde SQL node düzenleme ekranından bağlantı seçilebilsin.

**Architecture:** Engine `db` modülüne çalıştırılabilir bağlantı (`RunHandle` + `run_query`, sürücüye göre param + satır→JSON) ve runner'da bir kayıt önbelleği eklenir. `run_sql`, sql config'te `connection` (db_connection id) varsa registry'den çözüp o sürücüyle çalıştırır; yoksa mevcut default Postgres havuzuyla. Editörde `AutoexecSqlConfig.connection` alanı + modalda bağlantı dropdown'u + serialize/import.

**Tech Stack:** Rust (sqlx postgres+mysql, tiberius mssql), Axum; React+TS editör. İki repo: `workflow-engine` (Task 1-2, 5) ve `WFD-EDITOR` (Task 3-4). Migration psql docker. Spec: `docs/superpowers/specs/2026-07-10-wfd-db-connections-design.md` (Faz 2).

**Ön koşul:** Faz 1a+1b merged (`wf.db_connection`, `wf_wfe::db::{crypto,drivers,DbConfig,DbDriver}`, `/db/connections` API, editör DB yönetim UI). `DB_CONN_SECRET` env set.

**Not — sürücü zorluğu:** Postgres/MySQL sqlx ile benzer; MSSQL (tiberius) satır-çıkarma/param farklı ve canlı MSSQL olmadan yalnızca derleme doğrulanır — Task 5'te not düşülür.

---

## Dosya Haritası

- **Create:** `crates/wfe/src/db/run.rs` — `RunHandle` (Pg/My/Ms), `connect(cfg)`, `run_query(handle, query, params)` (sürücüye göre param + satır→JSON).
- **Modify:** `crates/wfe/src/db/mod.rs` — `pub mod run;` + `Registry` (id+updated_at → Arc<RunHandle> önbellek).
- **Modify:** `crates/wfe/src/runner.rs` — `LiveAutoexecRunner`'a registry + `run_sql`'de `connection` dalı.
- **Modify:** `crates/wfe/src/executor.rs` (veya runner'ın kurulduğu yer) — runner'a registry/pool geçişi (zaten default pool var).
- **Modify (editör):** `src/types/wfd.types.ts` — `AutoexecSqlConfig.connection?: string`.
- **Modify (editör):** `src/hooks/useExport.ts` — sql serialize `connection` içerir.
- **Modify (editör):** `src/utils/wfdImport.ts` — sql import `connection` okur.
- **Modify (editör):** `src/api/engineApi.ts` — `readStoredEngineConfig()` helper (localStorage CONFIG_KEY).
- **Modify (editör):** `src/components/shared/AutoexecConfigModal.tsx` — sql bölümüne bağlantı dropdown'u.

---

## Task 1: Engine — çalıştırılabilir bağlantı (`run.rs`)

**Files:** Create `crates/wfe/src/db/run.rs`; Modify `crates/wfe/src/db/mod.rs`

- [ ] **Step 1: `mod.rs`'e run modülü + param yardımcısı**

`crates/wfe/src/db/mod.rs` sonuna ekle:

```rust
pub mod run;

/// `:name` yer tutucularını sıralı (marker, value) listesine çevirir.
/// marker_fn(index) sürücüye göre işaret üretir ($1 / ? / @P1).
pub fn bind_params(
    query: &str,
    params: &serde_json::Map<String, serde_json::Value>,
    marker: impl Fn(usize) -> String,
) -> (String, Vec<serde_json::Value>) {
    let mut sql = query.to_string();
    let mut bound = Vec::new();
    for (name, value) in params {
        let ph = format!(":{name}");
        if sql.contains(&ph) {
            bound.push(value.clone());
            sql = sql.replace(&ph, &marker(bound.len()));
        }
    }
    (sql, bound)
}
```

- [ ] **Step 2: `bind_params` birim testi**

`crates/wfe/src/db/mod.rs`'in `#[cfg(test)]` bölümüne (yoksa ekle):

```rust
#[cfg(test)]
mod bind_tests {
    use super::bind_params;
    use serde_json::json;

    #[test]
    fn pg_markers_positional() {
        let mut m = serde_json::Map::new();
        m.insert("a".into(), json!(1));
        let (sql, vals) = bind_params("SELECT * WHERE x = :a", &m, |i| format!("${i}"));
        assert_eq!(sql, "SELECT * WHERE x = $1");
        assert_eq!(vals, vec![json!(1)]);
    }

    #[test]
    fn mysql_marker_qmark() {
        let mut m = serde_json::Map::new();
        m.insert("a".into(), json!("x"));
        let (sql, _) = bind_params("WHERE x = :a", &m, |_| "?".to_string());
        assert_eq!(sql, "WHERE x = ?");
    }
}
```

Run: `cargo test -p wf-wfe db::bind 2>&1 | tail -6` → 2 PASS.

- [ ] **Step 3: `run.rs` — connect + run_query**

`crates/wfe/src/db/run.rs`:

```rust
//! Seçilen bağlantıya karşı sorgu çalıştırma (Faz 2). Postgres/MySQL sqlx,
//! MSSQL tiberius. Param `:name` → sürücüye özel işaret; satırlar JSON'a map edilir.
use super::{bind_params, DbConfig, DbDriver, DbError};
use serde_json::{json, Map, Value};
use sqlx::{Column, Row};

pub enum RunHandle {
    Pg(sqlx::PgPool),
    My(sqlx::MySqlPool),
    Ms(tiberius::Config),
}

fn sqlx_uri(cfg: &DbConfig, scheme: &str, default_port: i32) -> String {
    if cfg.mode == "uri" { return cfg.secret.clone().unwrap_or_default(); }
    let host = cfg.host.as_deref().unwrap_or("localhost");
    let port = cfg.port.unwrap_or(default_port);
    let db = cfg.database.as_deref().unwrap_or("");
    let user = cfg.username.as_deref().unwrap_or("");
    let pass = cfg.secret.as_deref().unwrap_or("");
    format!("{scheme}://{user}:{pass}@{host}:{port}/{db}")
}

pub async fn connect(cfg: &DbConfig) -> Result<RunHandle, DbError> {
    match cfg.driver {
        DbDriver::Postgres => {
            let pool = sqlx::postgres::PgPoolOptions::new().max_connections(2)
                .acquire_timeout(std::time::Duration::from_secs(8))
                .connect(&sqlx_uri(cfg, "postgres", 5432)).await
                .map_err(|e| DbError(e.to_string()))?;
            Ok(RunHandle::Pg(pool))
        }
        DbDriver::Mysql => {
            let pool = sqlx::mysql::MySqlPoolOptions::new().max_connections(2)
                .acquire_timeout(std::time::Duration::from_secs(8))
                .connect(&sqlx_uri(cfg, "mysql", 3306)).await
                .map_err(|e| DbError(e.to_string()))?;
            Ok(RunHandle::My(pool))
        }
        DbDriver::Mssql => {
            use tiberius::{Config, AuthMethod};
            let mut c = Config::new();
            if cfg.mode == "uri" {
                c = Config::from_ado_string(cfg.secret.as_deref().unwrap_or(""))
                    .map_err(|e| DbError(e.to_string()))?;
            } else {
                c.host(cfg.host.as_deref().unwrap_or("localhost"));
                c.port(cfg.port.unwrap_or(1433) as u16);
                if let Some(db) = &cfg.database { c.database(db); }
                c.authentication(AuthMethod::sql_server(
                    cfg.username.as_deref().unwrap_or(""),
                    cfg.secret.as_deref().unwrap_or("")));
                if cfg.options.get("encrypt").and_then(|v| v.as_str()) == Some("false") {
                    c.encryption(tiberius::EncryptionLevel::NotSupported);
                } else { c.trust_cert(); }
            }
            Ok(RunHandle::Ms(c))
        }
    }
}

/// SQL'i çalıştırır, satırları JSON'a map eder. Tek satır → obje, çoklu → {rows:[...]}.
pub async fn run_query(handle: &RunHandle, query: &str, params: &Map<String, Value>) -> Result<Value, DbError> {
    let rows = match handle {
        RunHandle::Pg(pool) => {
            let (sql, vals) = bind_params(query, params, |i| format!("${i}"));
            let mut q = sqlx::query(&sql);
            for v in &vals { q = bind_pg(q, v); }
            let rows = q.fetch_all(pool).await.map_err(|e| DbError(e.to_string()))?;
            rows.iter().map(pg_row_json).collect::<Vec<_>>()
        }
        RunHandle::My(pool) => {
            let (sql, vals) = bind_params(query, params, |_| "?".to_string());
            let mut q = sqlx::query(&sql);
            for v in &vals { q = bind_my(q, v); }
            let rows = q.fetch_all(pool).await.map_err(|e| DbError(e.to_string()))?;
            rows.iter().map(my_row_json).collect::<Vec<_>>()
        }
        RunHandle::Ms(config) => {
            use tokio::net::TcpStream;
            use tokio_util::compat::TokioAsyncWriteCompatExt;
            let (sql, vals) = bind_params(query, params, |i| format!("@P{i}"));
            let tcp = TcpStream::connect(config.get_addr()).await.map_err(|e| DbError(e.to_string()))?;
            tcp.set_nodelay(true).ok();
            let mut client = tiberius::Client::connect(config.clone(), tcp.compat_write()).await
                .map_err(|e| DbError(e.to_string()))?;
            let mut q = tiberius::Query::new(sql);
            for v in &vals { push_ms(&mut q, v); }
            let stream = q.query(&mut client).await.map_err(|e| DbError(e.to_string()))?;
            let ms_rows = stream.into_first_result().await.map_err(|e| DbError(e.to_string()))?;
            ms_rows.iter().map(ms_row_json).collect::<Vec<_>>()
        }
    };
    Ok(match rows.len() { 1 => rows.into_iter().next().unwrap(), _ => json!({ "rows": rows }) })
}

// ── sqlx bind yardımcıları ──
fn bind_pg<'a>(q: sqlx::query::Query<'a, sqlx::Postgres, sqlx::postgres::PgArguments>, v: &Value)
    -> sqlx::query::Query<'a, sqlx::Postgres, sqlx::postgres::PgArguments> {
    match v {
        Value::Number(n) if n.is_i64() => q.bind(n.as_i64()),
        Value::Number(n) => q.bind(n.as_f64()),
        Value::Bool(b) => q.bind(*b),
        Value::String(s) => q.bind(s.clone()),
        other => q.bind(other.to_string()),
    }
}
fn bind_my<'a>(q: sqlx::query::Query<'a, sqlx::MySql, sqlx::mysql::MySqlArguments>, v: &Value)
    -> sqlx::query::Query<'a, sqlx::MySql, sqlx::mysql::MySqlArguments> {
    match v {
        Value::Number(n) if n.is_i64() => q.bind(n.as_i64()),
        Value::Number(n) => q.bind(n.as_f64()),
        Value::Bool(b) => q.bind(*b),
        Value::String(s) => q.bind(s.clone()),
        other => q.bind(other.to_string()),
    }
}
fn push_ms(q: &mut tiberius::Query, v: &Value) {
    match v {
        Value::Number(n) if n.is_i64() => q.bind(n.as_i64().unwrap()),
        Value::Number(n) => q.bind(n.as_f64().unwrap()),
        Value::Bool(b) => q.bind(*b),
        Value::String(s) => q.bind(s.clone()),
        other => q.bind(other.to_string()),
    }
}

// ── satır → JSON ──
fn pg_row_json(row: &sqlx::postgres::PgRow) -> Value {
    let mut obj = Map::new();
    for col in row.columns() {
        let val: Value = row.try_get::<Value, _>(col.name())
            .or_else(|_| row.try_get::<String, _>(col.name()).map(Value::from))
            .or_else(|_| row.try_get::<i64, _>(col.name()).map(Value::from))
            .or_else(|_| row.try_get::<f64, _>(col.name()).map(Value::from))
            .or_else(|_| row.try_get::<bool, _>(col.name()).map(Value::from))
            .unwrap_or(Value::Null);
        obj.insert(col.name().to_string(), val);
    }
    Value::Object(obj)
}
fn my_row_json(row: &sqlx::mysql::MySqlRow) -> Value {
    let mut obj = Map::new();
    for col in row.columns() {
        let val: Value = row.try_get::<String, _>(col.name()).map(Value::from)
            .or_else(|_| row.try_get::<i64, _>(col.name()).map(Value::from))
            .or_else(|_| row.try_get::<f64, _>(col.name()).map(Value::from))
            .or_else(|_| row.try_get::<bool, _>(col.name()).map(Value::from))
            .unwrap_or(Value::Null);
        obj.insert(col.name().to_string(), val);
    }
    Value::Object(obj)
}
fn ms_row_json(row: &tiberius::Row) -> Value {
    let mut obj = Map::new();
    let cols: Vec<String> = row.columns().iter().map(|c| c.name().to_string()).collect();
    for (i, name) in cols.iter().enumerate() {
        let val: Value = row.try_get::<&str, _>(i).ok().flatten().map(|s| Value::from(s.to_string()))
            .or_else(|| row.try_get::<i32, _>(i).ok().flatten().map(|n| Value::from(n as i64)))
            .or_else(|| row.try_get::<i64, _>(i).ok().flatten().map(Value::from))
            .or_else(|| row.try_get::<f64, _>(i).ok().flatten().map(Value::from))
            .or_else(|| row.try_get::<bool, _>(i).ok().flatten().map(Value::from))
            .unwrap_or(Value::Null);
        obj.insert(name.clone(), val);
    }
    Value::Object(obj)
}
```

- [ ] **Step 4: Derleme**

Run: `cargo build -p wf-wfe 2>&1 | tail -12`
Expected: PASS. tiberius `Query::bind`/`into_first_result`/`try_get` API'si 0.12'ye göre; imza farkı olursa minimal uyarla (davranışı koru) ve NOT düş. sqlx `query::Query`/`PgArguments`/`MySqlArguments` import yolları sürüm 0.7'ye göre — derleyici hatası olursa yollarını düzelt.

- [ ] **Step 5: Commit**

```bash
git add crates/wfe/src/db/run.rs crates/wfe/src/db/mod.rs
git commit -m "feat(wfe): db run_query — pg/mysql/mssql sürücüye göre çalıştırma + param + satır→JSON"
```

---

## Task 2: Engine — runner'da `connection` dalı + registry

**Files:** Modify `crates/wfe/src/runner.rs`; Modify runner kurulum noktası (`crates/wfe/src/executor.rs` veya `lib.rs`)

- [ ] **Step 1: Runner'a registry alanı**

`crates/wfe/src/runner.rs` üstüne importlar + struct genişletmesi:

```rust
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;
use crate::db::{self, DbConfig, DbDriver};
use crate::db::run::RunHandle;
```

`LiveAutoexecRunner`'ı güncelle:

```rust
pub struct LiveAutoexecRunner {
    client: Client,
    pool: Option<PgPool>,
    // (connection_id, updated_at_epoch) → çalıştırılabilir handle önbelleği
    registry: Mutex<HashMap<Uuid, (i64, Arc<RunHandle>)>>,
}

impl LiveAutoexecRunner {
    pub fn new(pool: Option<PgPool>) -> Self {
        Self { client: Client::new(), pool, registry: Mutex::new(HashMap::new()) }
    }
}
```

- [ ] **Step 2: `run_sql`'de connection çözümü**

`run_sql` gövdesinin başına, mevcut `let pool = ...` satırından ÖNCE ekle:

```rust
        // Faz 2: config.connection verilmişse o bağlantıya bağlan
        if let Some(conn_id) = def.config.get("connection").and_then(Value::as_str) {
            if !conn_id.is_empty() {
                return self.run_sql_on_connection(conn_id, def, env).await;
            }
        }
```

Ve `impl LiveAutoexecRunner` içine yeni metod ekle:

```rust
    async fn run_sql_on_connection(&self, conn_id: &str, def: &AutoexecDef, env: &ExecEnv)
        -> Result<Value, ExecFailure> {
        let id = Uuid::parse_str(conn_id)
            .map_err(|_| ExecFailure::failed(format!("geçersiz connection id: {conn_id}")))?;
        let meta_pool = self.pool.as_ref()
            .ok_or_else(|| ExecFailure::failed("connection çözümü için meta havuzu yok"))?;

        // Bağlantı satırını oku (updated_at ile önbellek anahtarı)
        let row = sqlx::query_as::<_, (String, String, Option<String>, Option<i32>, Option<String>, Option<String>, Value, Option<Vec<u8>>, chrono::DateTime<chrono::Utc>)>(
            "SELECT driver, mode, host, port, database, username, options, secret_enc, updated_at \
             FROM wf.db_connection WHERE id = $1 AND is_active = true")
            .bind(id).fetch_optional(meta_pool).await
            .map_err(|e| ExecFailure::failed(format!("connection okunamadı: {e}")))?
            .ok_or_else(|| ExecFailure::failed(format!("connection bulunamadı: {conn_id}")))?;

        let updated = row.8.timestamp();
        let handle = {
            let mut reg = self.registry.lock().await;
            match reg.get(&id) {
                Some((ts, h)) if *ts == updated => h.clone(),
                _ => {
                    let driver = DbDriver::parse(&row.0)
                        .ok_or_else(|| ExecFailure::failed("geçersiz driver"))?;
                    let secret = match &row.7 {
                        Some(b) => Some(db::crypto::decrypt(b)
                            .map_err(|e| ExecFailure::failed(format!("secret çözülemedi: {e}")))?),
                        None => None,
                    };
                    let cfg = DbConfig { driver, mode: row.1.clone(), host: row.2.clone(), port: row.3,
                        database: row.4.clone(), username: row.5.clone(), secret, options: row.6.clone() };
                    let h = Arc::new(db::run::connect(&cfg).await
                        .map_err(|e| ExecFailure::failed(format!("bağlanılamadı: {e}")))?);
                    reg.insert(id, (updated, h.clone()));
                    h
                }
            }
        };

        let query = def.config.get("query").and_then(Value::as_str)
            .ok_or_else(|| ExecFailure::failed("sql config'te query yok"))?;
        let params = def.config.get("params").map(|p| resolve_config_value(p, env)).unwrap_or(json!({}));
        let empty = Map::new();
        let params_map = params.as_object().unwrap_or(&empty);
        db::run::run_query(&handle, query, params_map).await
            .map_err(|e| ExecFailure::failed(format!("sql hatası: {e}")))
    }
```

- [ ] **Step 3: Derleme + mevcut testler**

Run: `cargo build --workspace 2>&1 | tail -8`
Expected: PASS.
Run: `cargo test --workspace 2>&1 | grep -E "test result: FAILED" || echo OK`
Expected: OK (mevcut calc/golden testleri bozulmaz; run_sql'in default-pool dalı değişmedi).

- [ ] **Step 4: Commit**

```bash
git add crates/wfe/src/runner.rs
git commit -m "feat(wfe): SQL node config.connection → seçilen DB'de çalıştırma (registry önbellek)"
```

---

## Task 3: Editör — sql config `connection` + serialize + import + config helper

**Files:** Modify `src/types/wfd.types.ts`, `src/hooks/useExport.ts`, `src/utils/wfdImport.ts`, `src/api/engineApi.ts` (çalışma dizini `WFD/wfd-editor`)

- [ ] **Step 1: Tip + serialize + import**

`src/types/wfd.types.ts` `AutoexecSqlConfig`'e ekle:
```ts
export interface AutoexecSqlConfig {
  query: string;
  params: Record<string, string>;
  result: Record<string, string>;
  connection?: string; // db_connection id (Faz 2)
}
```

`src/hooks/useExport.ts` sql serialize dalını (mevcut `return { query: c.query, params: c.params, result: c.result };`) şununla değiştir:
```ts
    const entry: Record<string, unknown> = { query: c.query, params: c.params, result: c.result };
    if (c.connection) entry.connection = c.connection;
    return entry;
```

`src/utils/wfdImport.ts` sql import bloğunda (`autoexec.type === 'sql'`), dönen config objesine ekle:
```ts
      connection: typeof read('connection') === 'string' ? (read('connection') as string) : undefined,
```

- [ ] **Step 2: `readStoredEngineConfig` helper**

`src/api/engineApi.ts` sonuna ekle:
```ts
/** Editörün localStorage'daki engine config'ini okur (modal gibi prop erişimi olmayan yerler için). */
export function readStoredEngineConfig(): Partial<EngineConfig> {
  try {
    const raw = localStorage.getItem('wfd-editor.engine.config');
    return raw ? (JSON.parse(raw) as Partial<EngineConfig>) : {};
  } catch { return {}; }
}
```

- [ ] **Step 3: Build**

Run: `npm run build`
Expected: tsc temiz.

- [ ] **Step 4: Commit**

```bash
git add src/types/wfd.types.ts src/hooks/useExport.ts src/utils/wfdImport.ts src/api/engineApi.ts
git commit -m "feat(editor): SQL config connection alanı + serialize/import + engine config helper"
```

---

## Task 4: Editör — SQL node modalında bağlantı dropdown'u

**Files:** Modify `src/components/shared/AutoexecConfigModal.tsx`

- [ ] **Step 1: Bağlantıları yükle + dropdown**

`AutoexecConfigModal.tsx` üstüne importlar:
```ts
import { useEffect, useState } from 'react';
import { listDbConnections, readStoredEngineConfig } from '../../api/engineApi';
import type { DbConnection } from '../../api/engineApi';
```

Bileşen gövdesinde (sql dalı render edilmeden önce, hook kuralları için koşulsuz):
```ts
  const [dbConns, setDbConns] = useState<DbConnection[]>([]);
  useEffect(() => {
    const cfg = readStoredEngineConfig();
    if (!cfg.baseUrl || !cfg.orgtntId) return;
    listDbConnections(cfg.baseUrl, cfg.orgtntId, cfg.adminKey ?? '')
      .then(setDbConns).catch(() => setDbConns([]));
  }, []);
```

sql bölümünde (`step.subtype === 'sql'`), query textarea'sının ÜSTÜNE bağlantı seçici ekle:
```tsx
        <div style={labelStyle}>Veritabanı bağlantısı</div>
        <select
          value={(cfg?.connection as string) ?? ''}
          onChange={(e) => patchConfig({ connection: e.target.value || undefined })}
          style={{ width: '100%', marginBottom: 10, background: 'var(--app-surface)', border: '1px solid var(--app-border-strong)', borderRadius: 'var(--app-radius)', color: 'var(--app-text)', fontSize: 13, padding: '9px 11px' }}
        >
          <option value="">(varsayılan engine DB)</option>
          {dbConns.map((c) => <option key={c.id} value={c.id}>{c.name} · {c.driver}</option>)}
        </select>
```

Not: `patchConfig` `Partial<...SqlConfig...>` alıyor; `connection` alanı Task 3'te tipe eklendiği için tip-uyumlu. `cfg` tipi rest&sql&calc birleşimi olduğundan `cfg?.connection` erişimi derlenir.

- [ ] **Step 2: Build + test regresyon**

Run: `npm run build`
Expected: tsc temiz.
Run: `npm test 2>&1 | grep -E "Tests " | tail -1`
Expected: yeni kırık yok.

- [ ] **Step 3: Commit**

```bash
git add src/components/shared/AutoexecConfigModal.tsx
git commit -m "feat(editor): SQL node düzenleme ekranında DB bağlantı seçici"
```

---

## Task 5: Uçtan uca doğrulama

**Files:** yok.

- [ ] **Step 1 (engine, curl):** `DB_CONN_SECRET` set, sunucu çalışır (free port). Bir Postgres bağlantısı (kayıtlı, ör. apex_registry id'si) ile `/autoexec/test` VEYA doğrudan bir WFD sql node'u üzerinden test. En basit: `/autoexec/test` ucu varsa sql def + `config.connection=<id>` + `query:"SELECT 1 AS x"` gönder → `{"x":1}` benzeri sonuç. (Uç yoksa: küçük bir sql node içeren WFD publish edip `/wfe` start ile tetikle.)

- [ ] **Step 2 (editör, dev):** SQL node düzenleme ekranı → "Veritabanı bağlantısı" dropdown'unda `apex_registry` görünür → seç → query `SELECT 1 AS x` → WFD'yi kaydet/publish → simülasyon/çalıştırmada o bağlantıya gider.

- [ ] **Step 3 (MSSQL notu):** Canlı MSSQL yoksa yalnızca derleme + (varsa) bağlantı-hatası mesajı doğrulanır. tiberius satır-çıkarma canlı MSSQL ile ayrıca doğrulanmalı — bu task'ta not düşülür, gerçek doğrulama MSSQL erişilince yapılır.

Bu görevde commit yok.

---

## Self-Review Notları

- **Spec (Faz 2) kapsamı:** sql config `connection` alanı (Task 3) · runner registry'den çözüp seçilen sürücüyle çalıştırma (Task 1-2) · satır→JSON sürücü-bağımsız (Task 1) · node property dropdown (Task 4) · geri uyumluluk: connection yoksa mevcut default pool (Task 2 Step 2, erken dönüş yalnızca dolu connection'da) — kapsandı.
- **Tip tutarlılığı:** `RunHandle`/`connect`/`run_query`/`bind_params` Task 1'de tanımlı, Task 2'de aynı imzayla; `DbConfig`/`DbDriver`/`crypto::decrypt` Faz 1a imzaları; editör `AutoexecSqlConfig.connection` Task 3'te tanımlı, Task 4'te kullanılıyor.
- **Risk:** MSSQL tiberius `Query::bind`/`into_first_result`/`try_get` ve sqlx `query::Query` import yolları sürüme göre uyarlanabilir (Task 1 Step 4 notu). Canlı MSSQL doğrulaması ertelenir.
- **Golden fixture** değişmez; default-pool sql yolu korunur.
