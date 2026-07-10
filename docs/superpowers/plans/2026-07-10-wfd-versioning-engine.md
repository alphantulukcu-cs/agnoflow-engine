# WFD Versiyonlama — Engine Backend Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `wf.wfd_meta`'ya draft/published yaşam döngüsü ekle — draft'lar validasyonsuz kaydedilip sürdürülebilir, publish'te tam v2.2 validator kapısından geçip immutable published'a döner.

**Architecture:** Versiyon soyağacı mevcut `(orgtnt_id, name)` grup modeliyle korunur. `wf.wfd_meta`'ya `status` + metadata kolonları eklenir; isim başına tek açık draft kısmi-unique index ile garanti edilir. Draft satırlarının JSON'u OpenDAL'da mutable, published satırlar immutable. Draft operasyonları `WfdAdapter` üzerinde inherent metod olarak eklenir (`WfdStore` trait değişmez); server `/wfd` router'ı bunları `s.wfd.<method>` ile çağırır — mevcut `upload` deseniyle aynı.

**Tech Stack:** Rust, Axum, sqlx (PostgreSQL), OpenDAL, uuid, serde_json. Migration psql ile manuel uygulanır.

**Test notu:** Bu repoda `wfd` crate'inin DB katmanı için otomatik test harness'ı YOK (saf testler `wfe-core`'da). Bu plana sadık kalarak: derleme + saf mantık `cargo test --workspace` ile; DB/endpoint katmanı çalışan sunucuya curl ile manuel doğrulanır. Golden fixture (`docs/spec/example-wfd_kredi-basvuru_v2_2.json`) DEĞİŞTİRİLMEZ.

**Ön koşul:** `DATABASE_URL` set; org+wf migration'ları uygulanmış; en az bir `orgtnt_id` mevcut. Aşağıda `$TNT` = geçerli bir tenant UUID.

---

## Dosya Haritası

- **Create:** `migrations/wf/20260710000001_wfd_draft_status.sql` — şema değişikliği (kolonlar + kısmi-unique index).
- **Modify:** `crates/wfd/src/models.rs` — `WfdMeta`'ya `status/description/tags/owner/updated_at`.
- **Modify:** `crates/wfd/src/repo.rs` — SELECT'ler genişler; `insert` imzası büyür; yeni `get_meta_any/update_draft/set_published/delete/single_draft_of`.
- **Modify:** `crates/wfd/src/adapter.rs` — `create_draft/fetch_draft_json/save_draft/publish_draft/new_draft_from/delete_draft`; `upload` yeni `insert` imzasına uyar.
- **Modify:** `crates/wfd/src/error.rs` — `Conflict(String)` varyantı (tek-draft 409).
- **Modify:** `crates/server/src/routes/wfd.rs` — yeni route'lar + handler'lar; `AppError` map'leri.
- **Modify:** `crates/server/src/error.rs` (gerekirse) — `WfdError::Conflict` → 409 eşlemesi.

---

## Task 1: Migration — status + metadata kolonları

**Files:**
- Create: `migrations/wf/20260710000001_wfd_draft_status.sql`

- [ ] **Step 1: Migration dosyasını yaz**

```sql
-- WFD draft/published yaşam döngüsü (2026-07-10 tasarımı).
-- status='draft' satırlar mutable ve validate edilmemiştir; publish'te 'published' olur.
ALTER TABLE wf.wfd_meta
  ADD COLUMN status      text        NOT NULL DEFAULT 'published'
      CHECK (status IN ('draft','published')),
  ADD COLUMN description text,
  ADD COLUMN tags        text[]      NOT NULL DEFAULT '{}',
  ADD COLUMN owner       text        NOT NULL DEFAULT 'admin',
  ADD COLUMN updated_at  timestamptz NOT NULL DEFAULT now();

-- Bir (tenant, isim) için aynı anda en fazla tek açık draft.
CREATE UNIQUE INDEX wfd_single_draft
  ON wf.wfd_meta (orgtnt_id, name)
  WHERE status = 'draft';
```

- [ ] **Step 2: Migration'ı uygula**

Run: `psql "$DATABASE_URL" -f migrations/wf/20260710000001_wfd_draft_status.sql`
Expected: `ALTER TABLE` ve `CREATE INDEX` çıktısı, hata yok.

- [ ] **Step 3: Şemayı doğrula**

Run: `psql "$DATABASE_URL" -c "\d wf.wfd_meta"`
Expected: `status`, `description`, `tags`, `owner`, `updated_at` kolonları listelenir; mevcut satırların hepsi `status='published'` (default).

- [ ] **Step 4: Commit**

```bash
git add migrations/wf/20260710000001_wfd_draft_status.sql
git commit -m "feat(wf): wfd_meta draft/published status + metadata kolonları"
```

---

## Task 2: `WfdMeta` modelini genişlet

**Files:**
- Modify: `crates/wfd/src/models.rs`

- [ ] **Step 1: Yeni alanları ekle**

`crates/wfd/src/models.rs` içeriğini şununla değiştir:

```rust
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Serialize, FromRow)]
pub struct WfdMeta {
    pub wfd_id:      Uuid,
    pub orgtnt_id:   Uuid,
    pub name:        String,
    pub version:     i32,
    pub s3_key:      String,
    pub is_active:   bool,
    pub created_at:  DateTime<Utc>,
    pub status:      String,
    pub description: Option<String>,
    pub tags:        Vec<String>,
    pub owner:       String,
    pub updated_at:  DateTime<Utc>,
}
```

- [ ] **Step 2: Derleme kontrolü (kırık SELECT'ler beklenir)**

Run: `cargo build -p wf-wfd 2>&1 | head -30`
Expected: `repo.rs`'teki `query_as::<_, WfdMeta>` satırlarında "columns" uyuşmazlığı DEĞİL — sqlx runtime map'i olduğundan derleme geçer ama SELECT sütun listeleri Task 3'te güncellenecek. Bu adımda derleme PASS olmalı (alanlar eklendi, henüz kullanılmıyor).

Not: `FromRow` çalışma zamanında kolon adıyla eşler; eksik kolon SELECT'te olursa runtime hata verir. Bu yüzden Task 3 zorunlu.

- [ ] **Step 3: Commit**

```bash
git add crates/wfd/src/models.rs
git commit -m "feat(wf): WfdMeta'ya status/description/tags/owner/updated_at"
```

---

## Task 3: `repo.rs` — SELECT'ler, genişletilmiş insert ve draft CRUD

**Files:**
- Modify: `crates/wfd/src/repo.rs`
- Modify: `crates/wfd/src/error.rs`

- [ ] **Step 1: `error.rs`'e Conflict varyantı ekle**

`crates/wfd/src/error.rs` içindeki enum'a ekle (mevcut `Database` satırından önce):

```rust
    #[error("conflict: {0}")]
    Conflict(String),
```

- [ ] **Step 2: `repo.rs`'i yeniden yaz**

`crates/wfd/src/repo.rs` içeriğini şununla değiştir:

```rust
use sqlx::PgPool;
use uuid::Uuid;
use crate::{error::WfdError, models::WfdMeta};

const COLS: &str = "wfd_id, orgtnt_id, name, version, s3_key, is_active, created_at, \
                    status, description, tags, owner, updated_at";

/// Yeni satır ekler (published veya draft). status/description/tags/owner verilir.
#[allow(clippy::too_many_arguments)]
pub async fn insert(
    pool:        &PgPool,
    wfd_id:      Uuid,
    orgtnt_id:   Uuid,
    name:        &str,
    version:     i32,
    s3_key:      &str,
    status:      &str,
    description: Option<&str>,
    tags:        &[String],
    owner:       &str,
) -> Result<Uuid, WfdError> {
    let id = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO wf.wfd_meta \
         (wfd_id, orgtnt_id, name, version, s3_key, status, description, tags, owner) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) RETURNING wfd_id"
    )
    .bind(wfd_id).bind(orgtnt_id).bind(name).bind(version).bind(s3_key)
    .bind(status).bind(description).bind(tags).bind(owner)
    .fetch_one(pool)
    .await
    .map_err(|e| match e.as_database_error().and_then(|d| d.code()) {
        // 23505 = unique_violation → tek-draft index ihlali
        Some(code) if code == "23505" =>
            WfdError::Conflict(format!("{name}: açık draft zaten var")),
        _ => WfdError::Database(e),
    })?;
    Ok(id)
}

/// Yalnızca published (is_active) satırı döner — mevcut çalıştırma yolu.
pub async fn get_meta(pool: &PgPool, wfd_id: Uuid, version: i32) -> Result<WfdMeta, WfdError> {
    sqlx::query_as::<_, WfdMeta>(
        &format!("SELECT {COLS} FROM wf.wfd_meta \
                  WHERE wfd_id=$1 AND version=$2 AND is_active=true")
    )
    .bind(wfd_id).bind(version)
    .fetch_optional(pool).await?
    .ok_or_else(|| WfdError::NotFound(format!("{wfd_id} v{version}")))
}

/// Draft dahil herhangi bir satırı döner (is_active filtresi yok).
pub async fn get_meta_any(pool: &PgPool, wfd_id: Uuid, version: i32) -> Result<WfdMeta, WfdError> {
    sqlx::query_as::<_, WfdMeta>(
        &format!("SELECT {COLS} FROM wf.wfd_meta WHERE wfd_id=$1 AND version=$2")
    )
    .bind(wfd_id).bind(version)
    .fetch_optional(pool).await?
    .ok_or_else(|| WfdError::NotFound(format!("{wfd_id} v{version}")))
}

/// Liste — draft ve published birlikte döner (UI ayırır).
pub async fn list(pool: &PgPool, orgtnt_id: Uuid, limit: i64, offset: i64)
    -> Result<Vec<WfdMeta>, WfdError>
{
    sqlx::query_as::<_, WfdMeta>(
        &format!("SELECT {COLS} FROM wf.wfd_meta \
                  WHERE orgtnt_id=$1 AND is_active=true \
                  ORDER BY name, version DESC LIMIT $2 OFFSET $3")
    )
    .bind(orgtnt_id).bind(limit).bind(offset)
    .fetch_all(pool).await
    .map_err(WfdError::Database)
}

pub async fn next_version(pool: &PgPool, orgtnt_id: Uuid, name: &str) -> Result<i32, WfdError> {
    let max: Option<i32> = sqlx::query_scalar(
        "SELECT MAX(version) FROM wf.wfd_meta WHERE orgtnt_id=$1 AND name=$2"
    )
    .bind(orgtnt_id).bind(name)
    .fetch_one(pool).await?;
    Ok(max.unwrap_or(0) + 1)
}

/// Draft metadata günceller (JSON storage'da; burada sadece meta + updated_at).
pub async fn update_draft(
    pool: &PgPool, wfd_id: Uuid, version: i32,
    description: Option<&str>, tags: &[String],
) -> Result<(), WfdError> {
    let n = sqlx::query(
        "UPDATE wf.wfd_meta SET description=$3, tags=$4, updated_at=now() \
         WHERE wfd_id=$1 AND version=$2 AND status='draft'"
    )
    .bind(wfd_id).bind(version).bind(description).bind(tags)
    .execute(pool).await?.rows_affected();
    if n == 0 { return Err(WfdError::NotFound(format!("draft {wfd_id} v{version}"))); }
    Ok(())
}

/// Draft'ı published yapar (publish sonrası). status flip + updated_at.
pub async fn set_published(pool: &PgPool, wfd_id: Uuid, version: i32) -> Result<(), WfdError> {
    let n = sqlx::query(
        "UPDATE wf.wfd_meta SET status='published', updated_at=now() \
         WHERE wfd_id=$1 AND version=$2 AND status='draft'"
    )
    .bind(wfd_id).bind(version)
    .execute(pool).await?.rows_affected();
    if n == 0 { return Err(WfdError::NotFound(format!("draft {wfd_id} v{version}"))); }
    Ok(())
}

/// Draft satırını siler (published silinemez).
pub async fn delete_draft(pool: &PgPool, wfd_id: Uuid, version: i32) -> Result<(), WfdError> {
    let n = sqlx::query(
        "DELETE FROM wf.wfd_meta WHERE wfd_id=$1 AND version=$2 AND status='draft'"
    )
    .bind(wfd_id).bind(version)
    .execute(pool).await?.rows_affected();
    if n == 0 { return Err(WfdError::NotFound(format!("draft {wfd_id} v{version}"))); }
    Ok(())
}
```

- [ ] **Step 3: Derleme**

Run: `cargo build -p wf-wfd 2>&1 | tail -20`
Expected: HATA — `adapter.rs`'teki `repo::insert(...)` çağrısı eski 5-argümanlı imzayla uyuşmaz. Bu beklenir; Task 4'te düzeltilecek.

- [ ] **Step 4: Commit**

```bash
git add crates/wfd/src/repo.rs crates/wfd/src/error.rs
git commit -m "feat(wf): repo draft CRUD + genişletilmiş insert/SELECT"
```

---

## Task 4: `adapter.rs` — draft yaşam döngüsü metodları

**Files:**
- Modify: `crates/wfd/src/adapter.rs`

- [ ] **Step 1: `upload`'ı yeni `insert` imzasına uyarla**

`crates/wfd/src/adapter.rs` içindeki mevcut `repo::insert(&self.pool, wfd_id, orgtnt_id, name, version, &key).await?;` satırını şununla değiştir:

```rust
        repo::insert(
            &self.pool, wfd_id, orgtnt_id, name, version, &key,
            "published", None, &[], "admin",
        ).await?;
```

- [ ] **Step 2: Draft metodlarını ekle**

`impl WfdAdapter { ... }` bloğunun İÇİNE (kapanış `}`'inden önce) şunları ekle:

```rust
    /// slug: isimden basit, güvenli bir id üretir (draft iskeleti için).
    fn slug(name: &str) -> String {
        let s: String = name.trim().to_lowercase().chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        let s = s.trim_matches('_').to_string();
        if s.is_empty() { "wfd".into() } else { s }
    }

    /// Yeni draft oluşturur. İskelet JSON verilmezse minimal v2.2 taslağı yazılır.
    /// Validasyon YOK. Tek-draft ihlalinde WfdError::Conflict.
    pub async fn create_draft(
        &self,
        orgtnt_id:   Uuid,
        name:        &str,
        description: Option<&str>,
        tags:        &[String],
        wfd_json:    Option<&Value>,
    ) -> Result<(Uuid, i32), crate::error::WfdError> {
        let version = repo::next_version(&self.pool, orgtnt_id, name).await?;
        let wfd_id = Uuid::new_v4();
        let key = storage::s3_key(wfd_id, version);

        let skeleton = serde_json::json!({
            "wfd_version": "2.2",
            "id": Self::slug(name),
            "name": name,
            "description": description.unwrap_or(""),
            "nodes": [],
            "transitions": [],
        });
        let doc = wfd_json.unwrap_or(&skeleton);
        let bytes = serde_json::to_vec(doc)
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))?;
        self.storage.write(&key, bytes).await
            .map_err(|e| crate::error::WfdError::Storage(e.to_string()))?;

        repo::insert(
            &self.pool, wfd_id, orgtnt_id, name, version, &key,
            "draft", description, tags, "admin",
        ).await?;
        Ok((wfd_id, version))
    }

    /// Draft'ın ham JSON'unu döner (Wfd parse ETMEZ — eksik/geçersiz olabilir).
    pub async fn fetch_draft_json(&self, wfd_id: Uuid, version: i32)
        -> Result<Value, crate::error::WfdError>
    {
        let meta = repo::get_meta_any(&self.pool, wfd_id, version).await?;
        if meta.status != "draft" {
            return Err(crate::error::WfdError::Conflict(
                format!("{wfd_id} v{version} draft değil (status={})", meta.status)));
        }
        let bytes = self.storage.read(&meta.s3_key).await
            .map_err(|e| crate::error::WfdError::Storage(e.to_string()))?
            .to_bytes();
        serde_json::from_slice(&bytes)
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))
    }

    /// Draft JSON + metadata'yı overwrite eder. Validasyon YOK. Cache invalidate.
    pub async fn save_draft(
        &self,
        wfd_id:      Uuid,
        version:     i32,
        wfd_json:    &Value,
        description: Option<&str>,
        tags:        &[String],
    ) -> Result<(), crate::error::WfdError> {
        let meta = repo::get_meta_any(&self.pool, wfd_id, version).await?;
        if meta.status != "draft" {
            return Err(crate::error::WfdError::Conflict(
                format!("{wfd_id} v{version} draft değil")));
        }
        let bytes = serde_json::to_vec(wfd_json)
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))?;
        self.storage.write(&meta.s3_key, bytes).await
            .map_err(|e| crate::error::WfdError::Storage(e.to_string()))?;
        repo::update_draft(&self.pool, wfd_id, version, description, tags).await?;
        self.cache.write().await.remove(&(wfd_id, version));
        Ok(())
    }

    /// Draft'ı yayınlar: tam v2.2 validator, geçerse status='published'.
    /// Geçmezse InvalidJson(validator özeti) döner, draft kalır.
    pub async fn publish_draft(&self, wfd_id: Uuid, version: i32)
        -> Result<(), crate::error::WfdError>
    {
        let json = self.fetch_draft_json(wfd_id, version).await?;
        let wfd = Wfd::from_value(json)
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))?;
        let report = validator::validate(&wfd);
        if !report.is_valid() {
            let summary = report.errors.iter()
                .map(|e| format!("[{}] {}: {}", e.code, e.path, e.message))
                .collect::<Vec<_>>().join("; ");
            return Err(crate::error::WfdError::InvalidJson(format!("validator: {summary}")));
        }
        repo::set_published(&self.pool, wfd_id, version).await?;
        self.cache.write().await.remove(&(wfd_id, version));
        Ok(())
    }

    /// Published bir versiyonu edit'e açar: JSON'unu kopyalayıp yeni draft (max+1) yaratır.
    pub async fn new_draft_from(&self, src_id: Uuid, src_version: i32)
        -> Result<(Uuid, i32), crate::error::WfdError>
    {
        let src = repo::get_meta_any(&self.pool, src_id, src_version).await?;
        let bytes = self.storage.read(&src.s3_key).await
            .map_err(|e| crate::error::WfdError::Storage(e.to_string()))?
            .to_bytes();
        let json: Value = serde_json::from_slice(&bytes)
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))?;
        self.create_draft(
            src.orgtnt_id, &src.name, src.description.as_deref(), &src.tags, Some(&json),
        ).await
    }

    /// Draft'ı iskarta eder (JSON + satır). Published dokunulmaz.
    pub async fn delete_draft(&self, wfd_id: Uuid, version: i32)
        -> Result<(), crate::error::WfdError>
    {
        let meta = repo::get_meta_any(&self.pool, wfd_id, version).await?;
        if meta.status != "draft" {
            return Err(crate::error::WfdError::Conflict(
                format!("{wfd_id} v{version} draft değil")));
        }
        let _ = self.storage.delete(&meta.s3_key).await;
        repo::delete_draft(&self.pool, wfd_id, version).await?;
        self.cache.write().await.remove(&(wfd_id, version));
        Ok(())
    }
```

- [ ] **Step 3: slug için birim testi ekle**

`adapter.rs` dosyasının EN SONUNA ekle:

```rust
#[cfg(test)]
mod tests {
    use super::WfdAdapter;

    #[test]
    fn slug_lowercases_and_replaces_non_alnum() {
        assert_eq!(WfdAdapter::slug("Kredi Başvuru!"), "kredi_ba_vuru");
        assert_eq!(WfdAdapter::slug("  "), "wfd");
        assert_eq!(WfdAdapter::slug("A-B_C"), "a_b_c");
    }
}
```

Not: `slug` `is_ascii_alphanumeric` kullandığından `ş` → `_` olur (Türkçe karakterler ASCII değil). Test bu davranışı sabitler.

- [ ] **Step 4: Derleme + saf testler**

Run: `cargo test -p wf-wfd 2>&1 | tail -20`
Expected: PASS — `slug_lowercases_and_replaces_non_alnum` geçer, derleme temiz.

- [ ] **Step 5: Workspace derleme**

Run: `cargo build --workspace 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/wfd/src/adapter.rs
git commit -m "feat(wf): adapter draft create/save/publish/new-draft/delete"
```

---

## Task 5: Server `/wfd` endpoint'leri

**Files:**
- Modify: `crates/server/src/routes/wfd.rs`
- Modify: `crates/server/src/error.rs` (Conflict → 409 eşlemesi gerekiyorsa)

- [ ] **Step 1: Route'ları kaydet**

`crates/server/src/routes/wfd.rs` içindeki `router` fonksiyonunu şununla değiştir:

```rust
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", post(upload_wfd).get(list_wfd))
        .route("/validate", post(validate_wfd))
        .route("/draft", post(create_draft))
        .route("/draft/:id/:version", get(get_draft).put(save_draft).delete(delete_draft))
        .route("/draft/:id/:version/publish", post(publish_draft))
        .route("/:id/:version", get(get_wfd))
        .route("/:id/:version/new-draft", post(new_draft))
        .with_state(state)
}
```

Import satırını güncelle (`put`, `delete` ekle):

```rust
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{delete, get, post, put},
    Json, Router,
};
```

Not: `put`/`delete` route metodları `.route(path, get(..).put(..).delete(..))` zincirinde kullanılır; ayrı import gerekli değilse derleyici uyarır — kullanılmayan importları temizle.

- [ ] **Step 2: Handler'ları ekle**

`get_wfd` fonksiyonundan sonra, dosya sonuna ekle:

```rust
#[derive(Deserialize)]
struct CreateDraftBody {
    orgtnt_id:   Uuid,
    name:        String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    tags:        Vec<String>,
    /// Editörün ürettiği başlangıç dokümanı; yoksa engine iskelet yazar.
    #[serde(default)]
    wfd:         Option<Value>,
}

async fn create_draft(
    State(s): State<AppState>,
    Json(b): Json<CreateDraftBody>,
) -> Result<Json<Value>, AppError> {
    let (wfd_id, version) = s.wfd
        .create_draft(b.orgtnt_id, &b.name, b.description.as_deref(), &b.tags, b.wfd.as_ref())
        .await
        .map_err(map_wfd_err)?;
    Ok(Json(serde_json::json!({ "wfd_id": wfd_id, "version": version })))
}

async fn get_draft(
    State(s): State<AppState>,
    Path((id, ver)): Path<(Uuid, i32)>,
) -> Result<Json<Value>, AppError> {
    s.wfd.fetch_draft_json(id, ver).await.map(Json).map_err(map_wfd_err)
}

#[derive(Deserialize)]
struct SaveDraftBody {
    wfd:         Value,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    tags:        Vec<String>,
}

async fn save_draft(
    State(s): State<AppState>,
    Path((id, ver)): Path<(Uuid, i32)>,
    Json(b): Json<SaveDraftBody>,
) -> Result<StatusCode, AppError> {
    s.wfd.save_draft(id, ver, &b.wfd, b.description.as_deref(), &b.tags)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(map_wfd_err)
}

async fn publish_draft(
    State(s): State<AppState>,
    Path((id, ver)): Path<(Uuid, i32)>,
) -> Result<Json<Value>, AppError> {
    s.wfd.publish_draft(id, ver).await
        .map(|_| Json(serde_json::json!({ "wfd_id": id, "version": ver, "status": "published" })))
        .map_err(map_wfd_err)
}

async fn delete_draft(
    State(s): State<AppState>,
    Path((id, ver)): Path<(Uuid, i32)>,
) -> Result<StatusCode, AppError> {
    s.wfd.delete_draft(id, ver).await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(map_wfd_err)
}

async fn new_draft(
    State(s): State<AppState>,
    Path((id, ver)): Path<(Uuid, i32)>,
) -> Result<Json<Value>, AppError> {
    let (wfd_id, version) = s.wfd.new_draft_from(id, ver).await.map_err(map_wfd_err)?;
    Ok(Json(serde_json::json!({ "wfd_id": wfd_id, "version": version })))
}

/// WfdError → HTTP kodu eşlemesi.
fn map_wfd_err(e: wf_wfd::error::WfdError) -> AppError {
    use wf_wfd::error::WfdError as E;
    let code = match e {
        E::NotFound(_)    => StatusCode::NOT_FOUND,
        E::Conflict(_)    => StatusCode::CONFLICT,
        E::InvalidJson(_) => StatusCode::UNPROCESSABLE_ENTITY,
        _                 => StatusCode::INTERNAL_SERVER_ERROR,
    };
    AppError(e.to_string(), code)
}
```

Not: `wf_wfd::error` modülünün pub olduğunu doğrula; değilse `crates/wfd/src/lib.rs`'te `pub mod error;` olduğundan emin ol.

- [ ] **Step 3: `error` modülünün publikliğini doğrula**

Run: `rg -n "pub mod error|pub use error" crates/wfd/src/lib.rs`
Expected: `pub mod error;` görünür. Yoksa ekle ve commit'e dahil et.

- [ ] **Step 4: Workspace derleme**

Run: `cargo build --workspace 2>&1 | tail -15`
Expected: PASS. Kullanılmayan `put`/`delete` importu uyarısı çıkarsa temizle.

- [ ] **Step 5: `cargo test --workspace`**

Run: `cargo test --workspace 2>&1 | tail -15`
Expected: PASS — golden fixture testleri dahil kırılma yok.

- [ ] **Step 6: Commit**

```bash
git add crates/server/src/routes/wfd.rs crates/wfd/src/lib.rs
git commit -m "feat(server): /wfd draft create/get/save/publish/delete + new-draft"
```

---

## Task 6: Uçtan uca manuel doğrulama (curl)

**Files:** yok (çalışan sunucuya karşı doğrulama).

- [ ] **Step 1: Sunucuyu başlat**

Run: `cargo run -p server` (ayrı terminalde; `PORT`, `DATABASE_URL`, `JWT_SECRET` set)
Expected: "listening on ..." log'u.

- [ ] **Step 2: Draft oluştur**

```bash
curl -s -X POST localhost:$PORT/wfd/draft \
  -H 'content-type: application/json' \
  -d "{\"orgtnt_id\":\"$TNT\",\"name\":\"plan-test-wfd\",\"description\":\"deneme\",\"tags\":[\"a\"]}"
```
Expected: `{"wfd_id":"<uuid>","version":1}`. `WID`/`VER` değişkenlerine al.

- [ ] **Step 3: Tek-draft kısıtı (409)**

Aynı komutu tekrar çalıştır.
Expected: HTTP 409, gövdede "açık draft zaten var".

- [ ] **Step 4: Draft'ı getir**

Run: `curl -s localhost:$PORT/wfd/draft/$WID/$VER`
Expected: iskelet JSON — `{"wfd_version":"2.2","id":"plan_test_wfd","name":"plan-test-wfd",...}`.

- [ ] **Step 5: Geçersiz JSON'u kaydet (validasyonsuz başarılı)**

```bash
curl -s -o /dev/null -w '%{http_code}\n' -X PUT localhost:$PORT/wfd/draft/$WID/$VER \
  -H 'content-type: application/json' \
  -d '{"wfd":{"wfd_version":"2.2","id":"x","name":"y","nodes":[]},"description":"düzeltildi","tags":["a","b"]}'
```
Expected: `204` (eksik/geçersiz olsa da draft kaydedilir).

- [ ] **Step 6: Publish — geçersiz draft (422)**

Run: `curl -s -o /dev/null -w '%{http_code}\n' -X POST localhost:$PORT/wfd/draft/$WID/$VER/publish`
Expected: `422` — validator hatası; draft hâlâ durur. `GET /wfd/draft/$WID/$VER` hâlâ 200 döner.

- [ ] **Step 7: Geçerli WFD kaydet ve publish et**

Golden fixture'ı draft'a yaz, sonra publish:
```bash
curl -s -o /dev/null -w '%{http_code}\n' -X PUT localhost:$PORT/wfd/draft/$WID/$VER \
  -H 'content-type: application/json' \
  -d "{\"wfd\":$(cat docs/spec/example-wfd_kredi-basvuru_v2_2.json)}"
curl -s -X POST localhost:$PORT/wfd/draft/$WID/$VER/publish
```
Expected: PUT `204`; publish `{"wfd_id":...,"version":1,"status":"published"}`.

- [ ] **Step 8: Publish sonrası draft PUT reddi**

Run: `curl -s -o /dev/null -w '%{http_code}\n' -X PUT localhost:$PORT/wfd/draft/$WID/$VER -H 'content-type: application/json' -d '{"wfd":{}}'`
Expected: `409` (artık draft değil).

- [ ] **Step 9: new-draft — published'dan yeni versiyon**

Run: `curl -s -X POST localhost:$PORT/wfd/$WID/$VER/new-draft`
Expected: `{"wfd_id":"<yeni-uuid>","version":2}`. Bu YENİ `wfd_id`'yi `$WID2` değişkenine al (new-draft taze bir wfd_id üretir; `$WID` değildir).

- [ ] **Step 10: Liste yeni alanları içeriyor**

Run: `curl -s "localhost:$PORT/wfd?orgtnt_id=$TNT" | head -c 600`
Expected: her öğede `status` (`draft`/`published`), `description`, `tags`, `owner`, `updated_at` alanları.

- [ ] **Step 11: Draft iskarta**

Run: `curl -s -o /dev/null -w '%{http_code}\n' -X DELETE localhost:$PORT/wfd/draft/$WID2/2`
Expected: `204`; ardından `GET /wfd/draft/$WID2/2` → 404. (new-draft taze `wfd_id` ürettiği için `$WID` değil `$WID2` kullanılır.)

- [ ] **Step 12: Temizlik**

Test satırlarını DB'den kaldır (isteğe bağlı):
```bash
psql "$DATABASE_URL" -c "DELETE FROM wf.wfd_meta WHERE name='plan-test-wfd';"
```

Bu görevde commit yok (yalnızca doğrulama). Tüm adımlar beklenen çıktıları verirse engine backend hazır.

---

## Self-Review Notları

- **Spec kapsamı:** §2 şema (Task 1) · §3 endpoint tablosu (Task 5, tüm satırlar) · draft mutable / published immutable (Task 3-4 status guard'ları) · tam validator publish'te (Task 4 `publish_draft`) · tek-draft (Task 1 index + Task 3 Conflict) · new-draft kopya (Task 4) · liste alanları (Task 3 `COLS`) — hepsi kapsandı.
- **Editör UX (§4)** bu planın DIŞINDA — ayrı `WFD-EDITOR` planı (create ekranı, versiyon tab'ı, read-only mod) sonraki adımda yazılacak.
- **Kapsam dışı bırakılanlar:** JWT owner (şimdilik `'admin'` sabit), rename (name create'te sabit) — spec'te kabul edildi.
