//! Ortam konfigürasyonu API'si (`$env`) — tasarım:
//! `docs/superpowers/specs/2026-08-04-env-config-design.md`.
//!
//! İki kaynak:
//!
//! * `/env/environments` — tenant düzeyinde ortam kaydı (test/prod/uat). Ayarlar sayfası.
//! * `/env/vars` — mantıksal WFD başına değerler. WFD ayarları sekmesi.
//!
//! `GET`/`PUT /env/vars/{ortam}` çifti, depolama DB satırı olmasına rağmen **dosya**
//! yüzeyi verir: `env.prod.json` indirilip başka bir kuruluma yüklenebilir.
//!
//! Secret kuralları (GitLab CI/CD değişkenlerinin karşılığı):
//! * Secret değerler hiçbir `GET` yanıtında DÖNMEZ (GitLab "hidden").
//! * `is_secret` yalnız OLUŞTURMADA işaretlenir; mevcut bir değişken secret'a çevrilemez —
//!   çevrilebilseydi önce okunur sonra çevrilirdi.
//! * Secret değer tek satır, ≥8 karakter, boşluksuz olmalı; aksi hâlde maskeleme log'u
//!   kullanılamaz hâle getirirdi.

use crate::{error::AppError, state::AppState};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;
use wf_wfe::db::crypto;

pub fn router(state: AppState) -> OpenApiRouter {
    OpenApiRouter::new()
        .routes(routes!(list_environments, create_environment))
        .routes(routes!(patch_environment, delete_environment))
        .routes(routes!(list_vars))
        .routes(routes!(get_env_file, put_env_file, patch_env_file))
        .with_state(state)
}

/// Editör/simülasyon uçları için ortam çözümü.
///
/// `orgtnt_id` ya da `wfd_id` verilmezse BOŞ ortam döner — `$env` kullanmayan çağrılar
/// (eski istemciler dahil) etkilenmesin. Ortam adı verilmezse tenant varsayılanı.
///
/// Secret'lar DAHİL çözülür: tasarımcı anahtar isteyen bir ucu editörde deneyebilmeli
/// (2026-08-04 kararı — gerekçe `wf_wfe::env_adapter`'da). Değerler ekrana dönmez;
/// `resolved_config()` ve hata metinleri maskelenir.
pub(crate) async fn resolve_run_env(
    pool: &PgPool,
    orgtnt_id: Option<Uuid>,
    wfd_id: Option<Uuid>,
    environment: Option<&str>,
) -> Result<wfe_core::v22::env::RunEnv, AppError> {
    let (Some(orgtnt_id), Some(wfd_id)) = (orgtnt_id, wfd_id) else {
        return Ok(Default::default());
    };
    let env_id = match environment {
        Some(name) => Some(
            wf_wfe::repo::env::resolve_environment(pool, orgtnt_id, Some(name))
                .await
                .map_err(|e| AppError(e.to_string(), StatusCode::UNPROCESSABLE_ENTITY))?
                .id,
        ),
        None => None,
    };
    wfe_core::v22::ports::EnvPort::load_run_env(
        &wf_wfe::env_adapter::EnvAdapter::new(pool.clone()),
        orgtnt_id,
        wfd_id,
        env_id,
    )
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::UNPROCESSABLE_ENTITY))
}

/// Secret değer maskelenebilir mi? GitLab'ın kuralı: tek satır, ≥8 karakter, boşluksuz.
/// Kısa ya da boşluklu bir değeri log'da aramak log'u kullanılamaz hâle getirir.
fn assert_maskable(key: &str, value: &str) -> Result<(), AppError> {
    if value.len() < 8 || value.chars().any(char::is_whitespace) {
        return Err(AppError(
            format!(
                "'{key}' secret değeri maskelenemez: tek satır, en az 8 karakter ve \
                 boşluksuz olmalı"
            ),
            StatusCode::UNPROCESSABLE_ENTITY,
        ));
    }
    Ok(())
}

fn assert_key_shape(key: &str) -> Result<(), AppError> {
    let ok = key
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_uppercase())
        && key
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    if !ok {
        return Err(AppError(
            format!("geçersiz anahtar '{key}' — [A-Z][A-Z0-9_]* olmalı"),
            StatusCode::UNPROCESSABLE_ENTITY,
        ));
    }
    Ok(())
}

/// `wfd_id` → mantıksal WFD kimliği `(project_id, wfd_name)`. Değerlerin sahibi budur,
/// `wfd_id` değil: her versiyon ayrı bir `wfd_id` satırıdır.
async fn owner_of(pool: &PgPool, wfd_id: Uuid) -> Result<(Uuid, String), AppError> {
    sqlx::query_as::<_, (Uuid, String)>(
        "SELECT project_id, name FROM wf.wfd_meta WHERE wfd_id = $1",
    )
    .bind(wfd_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?
    .ok_or_else(|| AppError("WFD bulunamadı".into(), StatusCode::NOT_FOUND))
}

async fn env_id_by_name(pool: &PgPool, orgtnt_id: Uuid, name: &str) -> Result<Uuid, AppError> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM wf.environment WHERE orgtnt_id = $1 AND name = $2",
    )
    .bind(orgtnt_id)
    .bind(name)
    .fetch_optional(pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?
    .ok_or_else(|| {
        AppError(
            format!("bilinmeyen ortam: '{name}'"),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
    })
}

// ---- Ortam kaydı ----

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct TenantQuery {
    orgtnt_id: Uuid,
}

#[utoipa::path(get, path = "/environments", tag = "env", params(TenantQuery),
    responses((status = 200, description = "Tenant'ın ortamları")))]
async fn list_environments(
    State(s): State<AppState>,
    Query(q): Query<TenantQuery>,
) -> Result<Json<Value>, AppError> {
    let envs = wf_wfe::repo::env::list_environments(&s.pool, q.orgtnt_id)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    Ok(Json(json!({
        "items": envs.iter().map(|e| json!({
            "id": e.id, "name": e.name, "label": e.label, "is_default": e.is_default,
        })).collect::<Vec<_>>()
    })))
}

#[derive(Deserialize, ToSchema)]
struct EnvironmentBody {
    orgtnt_id: Option<Uuid>,
    name: Option<String>,
    label: Option<String>,
    #[serde(default)]
    is_default: bool,
}

#[utoipa::path(post, path = "/environments", tag = "env", request_body = EnvironmentBody,
    responses((status = 200, description = "Oluşturuldu")))]
async fn create_environment(
    State(s): State<AppState>,
    Json(b): Json<EnvironmentBody>,
) -> Result<Json<Value>, AppError> {
    let orgtnt_id = b.orgtnt_id.ok_or_else(|| {
        AppError("orgtnt_id zorunlu".into(), StatusCode::UNPROCESSABLE_ENTITY)
    })?;
    let name = b
        .name
        .as_deref()
        .filter(|n| !n.is_empty())
        .ok_or_else(|| AppError("name zorunlu".into(), StatusCode::UNPROCESSABLE_ENTITY))?;

    let mut tx = s
        .pool
        .begin()
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    // Varsayılan tenant başına TEK — kısmi unique indeks bunu zorlar, o yüzden yenisini
    // işaretlemeden önce eskisini düşürürüz (aynı transaction'da).
    if b.is_default {
        sqlx::query("UPDATE wf.environment SET is_default = false WHERE orgtnt_id = $1")
            .bind(orgtnt_id)
            .execute(&mut *tx)
            .await
            .map_err(map_write_err)?;
    }
    let id = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO wf.environment (orgtnt_id, name, label, is_default) \
         VALUES ($1,$2,$3,$4) RETURNING id",
    )
    .bind(orgtnt_id)
    .bind(name)
    .bind(&b.label)
    .bind(b.is_default)
    .fetch_one(&mut *tx)
    .await
    .map_err(map_write_err)?;
    tx.commit()
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    Ok(Json(json!({ "id": id })))
}

#[utoipa::path(patch, path = "/environments/{id}", tag = "env", request_body = EnvironmentBody,
    responses((status = 200, description = "Güncellendi")))]
async fn patch_environment(
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(b): Json<EnvironmentBody>,
) -> Result<Json<Value>, AppError> {
    let orgtnt_id = sqlx::query_scalar::<_, Uuid>("SELECT orgtnt_id FROM wf.environment WHERE id=$1")
        .bind(id)
        .fetch_optional(&s.pool)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?
        .ok_or_else(|| AppError("ortam bulunamadı".into(), StatusCode::NOT_FOUND))?;

    let mut tx = s
        .pool
        .begin()
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    if b.is_default {
        sqlx::query("UPDATE wf.environment SET is_default = false WHERE orgtnt_id = $1")
            .bind(orgtnt_id)
            .execute(&mut *tx)
            .await
            .map_err(map_write_err)?;
    }
    sqlx::query(
        "UPDATE wf.environment \
            SET name = COALESCE($2, name), label = COALESCE($3, label), \
                is_default = $4, updated_at = now() \
          WHERE id = $1",
    )
    .bind(id)
    .bind(&b.name)
    .bind(&b.label)
    .bind(b.is_default)
    .execute(&mut *tx)
    .await
    .map_err(map_write_err)?;
    tx.commit()
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    Ok(Json(json!({ "ok": true })))
}

#[utoipa::path(delete, path = "/environments/{id}", tag = "env",
    responses((status = 200, description = "Silindi")))]
async fn delete_environment(
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    // Son ortam silinemez: `wfe.environment_id` NOT NULL, ortamsız tenant WFE başlatamaz.
    let orgtnt_id = sqlx::query_scalar::<_, Uuid>("SELECT orgtnt_id FROM wf.environment WHERE id=$1")
        .bind(id)
        .fetch_optional(&s.pool)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?
        .ok_or_else(|| AppError("ortam bulunamadı".into(), StatusCode::NOT_FOUND))?;
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM wf.environment WHERE orgtnt_id = $1",
    )
    .bind(orgtnt_id)
    .fetch_one(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    if count <= 1 {
        return Err(AppError(
            "son ortam silinemez".into(),
            StatusCode::UNPROCESSABLE_ENTITY,
        ));
    }
    sqlx::query("DELETE FROM wf.environment WHERE id = $1")
        .bind(id)
        .execute(&s.pool)
        .await
        .map_err(map_write_err)?;
    Ok(Json(json!({ "ok": true })))
}

// ---- Değişkenler ----

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct VarsQuery {
    orgtnt_id: Uuid,
    /// Değerlerin sahibi WFD. `(project_id, wfd_name)`'e çözülür.
    wfd_id: Uuid,
}

/// Matris görünümü: satır = anahtar, sütun = ortam (`*` joker dahil).
/// Secret değerler `value: null, is_secret: true` olarak döner — asla açılmaz.
#[utoipa::path(get, path = "/vars", tag = "env", params(VarsQuery),
    responses((status = 200, description = "Anahtar × ortam matrisi")))]
async fn list_vars(
    State(s): State<AppState>,
    Query(q): Query<VarsQuery>,
) -> Result<Json<Value>, AppError> {
    let (project_id, wfd_name) = owner_of(&s.pool, q.wfd_id).await?;
    let rows = sqlx::query_as::<_, (String, Option<String>, String, Option<String>, bool)>(
        "SELECT v.key, e.name, v.value_type, v.value, v.is_secret \
           FROM wf.wfd_env_var v \
           LEFT JOIN wf.environment e ON e.id = v.env_id \
          WHERE v.project_id = $1 AND v.wfd_name = $2 \
          ORDER BY v.key, e.name NULLS FIRST",
    )
    .bind(project_id)
    .bind(&wfd_name)
    .fetch_all(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    let envs = wf_wfe::repo::env::list_environments(&s.pool, q.orgtnt_id)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    let mut keys: Map<String, Value> = Map::new();
    for (key, env_name, value_type, value, is_secret) in rows {
        let scope = env_name.unwrap_or_else(|| "*".into());
        let entry = keys.entry(key).or_insert_with(|| json!({}));
        entry[scope] = json!({
            "value_type": value_type,
            "value": if is_secret { Value::Null } else { value.map(Value::String).unwrap_or(Value::Null) },
            "is_secret": is_secret,
        });
    }
    Ok(Json(json!({
        "environments": envs.iter().map(|e| e.name.clone()).collect::<Vec<_>>(),
        "keys": keys,
    })))
}

/// Tek ortamın düz JSON objesi — indirilen "dosya". Secret'lar `null` gelir.
/// `{ortam}` = ortam adı ya da `*` (joker kapsam).
#[utoipa::path(get, path = "/vars/{environment}", tag = "env", params(VarsQuery),
    responses((status = 200, description = "Ortamın değişkenleri")))]
async fn get_env_file(
    State(s): State<AppState>,
    Path(environment): Path<String>,
    Query(q): Query<VarsQuery>,
) -> Result<Json<Value>, AppError> {
    let (project_id, wfd_name) = owner_of(&s.pool, q.wfd_id).await?;
    let env_id = match environment.as_str() {
        "*" => None,
        name => Some(env_id_by_name(&s.pool, q.orgtnt_id, name).await?),
    };
    let rows = sqlx::query_as::<_, (String, String, Option<String>, bool)>(
        "SELECT key, value_type, value, is_secret FROM wf.wfd_env_var \
          WHERE project_id = $1 AND wfd_name = $2 AND env_id IS NOT DISTINCT FROM $3 \
          ORDER BY key",
    )
    .bind(project_id)
    .bind(&wfd_name)
    .bind(env_id)
    .fetch_all(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    let mut out = Map::new();
    for (key, value_type, value, is_secret) in rows {
        out.insert(
            key,
            json!({
                "type": value_type,
                "secret": is_secret,
                "value": if is_secret { Value::Null } else { value.map(Value::String).unwrap_or(Value::Null) },
            }),
        );
    }
    Ok(Json(Value::Object(out)))
}

#[derive(Deserialize, ToSchema)]
struct VarEntry {
    value: Option<String>,
    #[serde(default = "default_type", rename = "type")]
    value_type: String,
    #[serde(default)]
    secret: bool,
}

fn default_type() -> String {
    "string".into()
}

/// Yüklenen "dosya": bu ortamın değişkenlerini TAMAMEN değiştirir.
///
/// Secret satırlar için değer verilmemişse mevcut şifreli değer KORUNUR — aksi hâlde
/// dosyayı indirip (secret'lar `null` gelir) geri yüklemek tüm secret'ları silerdi.
#[utoipa::path(put, path = "/vars/{environment}", tag = "env", params(VarsQuery),
    request_body = Value, responses((status = 200, description = "Değiştirildi")))]
async fn put_env_file(
    State(s): State<AppState>,
    Path(environment): Path<String>,
    Query(q): Query<VarsQuery>,
    Json(body): Json<Map<String, Value>>,
) -> Result<Json<Value>, AppError> {
    let (project_id, wfd_name) = owner_of(&s.pool, q.wfd_id).await?;
    let env_id = match environment.as_str() {
        "*" => None,
        name => Some(env_id_by_name(&s.pool, q.orgtnt_id, name).await?),
    };

    let mut tx = s
        .pool
        .begin()
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    let keys: Vec<String> = body.keys().cloned().collect();
    sqlx::query(
        "DELETE FROM wf.wfd_env_var \
          WHERE project_id = $1 AND wfd_name = $2 AND env_id IS NOT DISTINCT FROM $3 \
            AND NOT (key = ANY($4))",
    )
    .bind(project_id)
    .bind(&wfd_name)
    .bind(env_id)
    .bind(&keys)
    .execute(&mut *tx)
    .await
    .map_err(map_write_err)?;

    for (key, raw) in &body {
        assert_key_shape(key)?;
        let entry: VarEntry = serde_json::from_value(raw.clone()).map_err(|e| {
            AppError(
                format!("'{key}' girdisi okunamadı: {e}"),
                StatusCode::UNPROCESSABLE_ENTITY,
            )
        })?;
        upsert_var(
            &mut tx, project_id, &wfd_name, env_id, key, &entry,
        )
        .await?;
    }

    tx.commit()
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    Ok(Json(json!({ "ok": true, "count": body.len() })))
}

/// Anahtar bazlı upsert/silme. Değeri `null` gönderilen anahtar SİLİNİR.
#[utoipa::path(patch, path = "/vars/{environment}", tag = "env", params(VarsQuery),
    request_body = Value, responses((status = 200, description = "Güncellendi")))]
async fn patch_env_file(
    State(s): State<AppState>,
    Path(environment): Path<String>,
    Query(q): Query<VarsQuery>,
    Json(body): Json<Map<String, Value>>,
) -> Result<Json<Value>, AppError> {
    let (project_id, wfd_name) = owner_of(&s.pool, q.wfd_id).await?;
    let env_id = match environment.as_str() {
        "*" => None,
        name => Some(env_id_by_name(&s.pool, q.orgtnt_id, name).await?),
    };

    let mut tx = s
        .pool
        .begin()
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    for (key, raw) in &body {
        assert_key_shape(key)?;
        if raw.is_null() {
            sqlx::query(
                "DELETE FROM wf.wfd_env_var \
                  WHERE project_id=$1 AND wfd_name=$2 AND env_id IS NOT DISTINCT FROM $3 \
                    AND key=$4",
            )
            .bind(project_id)
            .bind(&wfd_name)
            .bind(env_id)
            .bind(key)
            .execute(&mut *tx)
            .await
            .map_err(map_write_err)?;
            continue;
        }
        let entry: VarEntry = serde_json::from_value(raw.clone()).map_err(|e| {
            AppError(
                format!("'{key}' girdisi okunamadı: {e}"),
                StatusCode::UNPROCESSABLE_ENTITY,
            )
        })?;
        upsert_var(&mut tx, project_id, &wfd_name, env_id, key, &entry).await?;
    }
    tx.commit()
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    Ok(Json(json!({ "ok": true })))
}

async fn upsert_var(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    project_id: Uuid,
    wfd_name: &str,
    env_id: Option<Uuid>,
    key: &str,
    entry: &VarEntry,
) -> Result<(), AppError> {
    // `is_secret` mevcut satırda ne ise O KALIR: sonradan secret'a çevrilebilseydi önce
    // okunur sonra çevrilirdi; secret'lıktan çıkarılabilseydi de değer açığa çıkardı.
    let existing = sqlx::query_as::<_, (bool,)>(
        "SELECT is_secret FROM wf.wfd_env_var \
          WHERE project_id=$1 AND wfd_name=$2 AND env_id IS NOT DISTINCT FROM $3 AND key=$4",
    )
    .bind(project_id)
    .bind(wfd_name)
    .bind(env_id)
    .bind(key)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    let is_secret = match existing {
        Some((was_secret,)) => {
            if was_secret != entry.secret {
                return Err(AppError(
                    format!(
                        "'{key}' için secret bayrağı değiştirilemez — anahtarı silip \
                         yeniden oluşturun"
                    ),
                    StatusCode::UNPROCESSABLE_ENTITY,
                ));
            }
            was_secret
        }
        None => entry.secret,
    };

    if is_secret {
        let Some(value) = entry.value.as_deref() else {
            // Değer verilmedi: mevcut şifreli değer korunur. Dosyayı indirip (secret'lar
            // null gelir) geri yüklemek secret'ları silmemeli.
            if existing.is_some() {
                return Ok(());
            }
            return Err(AppError(
                format!("'{key}' secret olarak oluşturuluyor ama değer verilmedi"),
                StatusCode::UNPROCESSABLE_ENTITY,
            ));
        };
        assert_maskable(key, value)?;
        if entry.value_type != "string" {
            return Err(AppError(
                format!("'{key}' secret olduğu için tipi string olmalı"),
                StatusCode::UNPROCESSABLE_ENTITY,
            ));
        }
        let enc = crypto::encrypt(value)
            .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
        sqlx::query(
            "INSERT INTO wf.wfd_env_var \
                 (project_id, wfd_name, env_id, key, value_type, value_enc, is_secret) \
             VALUES ($1,$2,$3,$4,'string',$5,true) \
             ON CONFLICT (project_id, wfd_name, key, env_id) DO UPDATE \
                 SET value_enc = EXCLUDED.value_enc, updated_at = now()",
        )
        .bind(project_id)
        .bind(wfd_name)
        .bind(env_id)
        .bind(key)
        .bind(enc)
        .execute(&mut **tx)
        .await
        .map_err(map_write_err)?;
    } else {
        sqlx::query(
            "INSERT INTO wf.wfd_env_var \
                 (project_id, wfd_name, env_id, key, value_type, value, is_secret) \
             VALUES ($1,$2,$3,$4,$5,$6,false) \
             ON CONFLICT (project_id, wfd_name, key, env_id) DO UPDATE \
                 SET value_type = EXCLUDED.value_type, value = EXCLUDED.value, \
                     updated_at = now()",
        )
        .bind(project_id)
        .bind(wfd_name)
        .bind(env_id)
        .bind(key)
        .bind(&entry.value_type)
        .bind(&entry.value)
        .execute(&mut **tx)
        .await
        .map_err(map_write_err)?;
    }
    Ok(())
}

/// Kısıt ihlalini SQL metnini sızdırmadan çevirir (kısıt ADINDAN) — `routes::db`'nin
/// yaptığının aynısı.
fn map_write_err(e: sqlx::Error) -> AppError {
    match e.as_database_error().and_then(|d| d.constraint()) {
        Some("environment_tenant_name") => AppError(
            "bu isimde bir ortam zaten var".into(),
            StatusCode::CONFLICT,
        ),
        Some("wfd_env_var_key") => AppError(
            "bu anahtar bu kapsamda zaten tanımlı".into(),
            StatusCode::CONFLICT,
        ),
        Some("wf_environment_name_check") | Some("environment_name_check") => AppError(
            "geçersiz ortam adı — [a-z][a-z0-9_-]* olmalı".into(),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        Some("wfd_env_var_key_check") => AppError(
            "geçersiz anahtar — [A-Z][A-Z0-9_]* olmalı".into(),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        Some("wfd_env_var_secret_is_string") => AppError(
            "secret değerin tipi string olmalı".into(),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        _ => AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_shape_is_enforced() {
        assert!(assert_key_shape("AUTH_API").is_ok());
        assert!(assert_key_shape("A1_B2").is_ok());
        assert!(assert_key_shape("auth_api").is_err());
        assert!(assert_key_shape("1ABC").is_err());
        assert!(assert_key_shape("AUTH-API").is_err());
        assert!(assert_key_shape("").is_err());
    }

    /// GitLab'ın maskeleme ön koşulu: kısa ya da boşluklu değer log'u kullanılamaz yapar.
    #[test]
    fn maskable_secret_rules() {
        assert!(assert_maskable("K", "sk-live-abc123").is_ok());
        assert!(assert_maskable("K", "kisa").is_err());
        assert!(assert_maskable("K", "bosluk var burada").is_err());
        assert!(assert_maskable("K", "iki\nsatir").is_err());
    }
}
