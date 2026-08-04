//! DB bağlantı yönetimi. Kapsam (scope) iki türlüdür:
//!
//! * `global` — tenant genelinde; her projedeki her WFD'de görünür/kullanılabilir.
//!   Ayarlar sayfasından yönetilir.
//! * `local`  — yalnızca TEK bir WFD'de görünür/kullanılabilir; sahiplik anahtarı
//!   mantıksal WFD kimliğidir: `(project_id, wfd_name)`. WFD ayarları sekmesinden
//!   yönetilir, başka WFD'nin listesinde çıkmaz.
//!
//! Ayrıntı: `migrations/wf/20260804000001_db_connection_scope.sql`.

use utoipa_axum::router::OpenApiRouter;
use crate::{error::AppError, state::AppState};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::routes;
use uuid::Uuid;
use wf_wfe::db::{self, crypto, DbConfig, DbDriver};

pub fn router(state: AppState) -> OpenApiRouter {
    OpenApiRouter::new()
        .routes(routes!(list, create))
        .routes(routes!(test_draft))
        .routes(routes!(update, delete))
        .routes(routes!(test_saved))
        .with_state(state)
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct ListQuery {
    orgtnt_id: Uuid,
    /// Verilirse global bağlantıların yanına BU WFD'nin lokal bağlantıları da eklenir.
    /// Verilmezse (ayarlar sayfası) yalnızca global'ler döner.
    #[serde(default)]
    wfd_id: Option<Uuid>,
}

#[derive(Deserialize, ToSchema)]
struct ConnBody {
    orgtnt_id: Option<Uuid>,
    name: Option<String>,
    driver: String,
    #[serde(default = "default_mode")]
    mode: String,
    host: Option<String>,
    /// Port artık METİN: sayı ("5432") ya da tam bir `$env.KEY` şablonu olabilir
    /// (migration 20260804000002). DB CHECK ikisinden birini zorunlu kılar.
    port: Option<String>,
    database: Option<String>,
    username: Option<String>,
    #[serde(default)]
    options: Value,
    /// Parola/dizedeki gizli — verilmezse (update) mevcut korunur.
    secret: Option<String>,
    /// `global` (varsayılan) | `local`. Yalnızca create'te anlamlıdır: kapsam
    /// oluşturulduktan sonra değişmez (update kapsamı görmezden gelir).
    #[serde(default)]
    scope: Option<String>,
    /// `scope="local"` için ZORUNLU — sahip WFD. Grup kimliği bundan çözülür.
    #[serde(default)]
    wfd_id: Option<Uuid>,
}
fn default_mode() -> String {
    "fields".into()
}

/// Mantıksal WFD kimliği: (project_id, name). wfd_meta'da her versiyon ayrı satır
/// olduğundan lokal sahiplik versiyona DEĞİL bu çifte bağlanır.
async fn wfd_owner(pool: &PgPool, wfd_id: Uuid) -> Result<(Uuid, String), AppError> {
    sqlx::query_as::<_, (Uuid, String)>(
        "SELECT project_id, name FROM wf.wfd_meta WHERE wfd_id = $1",
    )
    .bind(wfd_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?
    .ok_or_else(|| AppError("WFD bulunamadı".into(), StatusCode::NOT_FOUND))
}

/// Unique ihlalini SQL metnini sızdırmadan 409'a çevirir (kısıt ADINDAN).
fn map_write_err(e: sqlx::Error) -> AppError {
    match e.as_database_error().and_then(|d| d.constraint()) {
        Some("db_connection_global_name") | Some("db_connection_local_name") => AppError(
            "duplicate name: bu isimde bir bağlantı zaten var".into(),
            StatusCode::CONFLICT,
        ),
        _ => AppError(e.to_string(), StatusCode::UNPROCESSABLE_ENTITY),
    }
}

/// Metin port'u bağlantı için sayıya çevirir.
///
/// Kolon `text` çünkü `$env.PG_PORT` gibi bir şablon tutabilir (tek bağlantı satırı tüm
/// ortamlara hizmet etsin diye). Şablonun ÇÖZÜMÜ bir koşum ortamı gerektirir; bağlantı
/// testi henüz ortam almadığı için burada açık bir hata veririz — sessizce varsayılan
/// porta düşmek yanlış bir hedefe bağlanmak demek olurdu.
fn parse_port(raw: Option<&str>) -> Result<Option<i32>, AppError> {
    let Some(s) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if s.starts_with("$env.") {
        return Err(AppError(
            format!("port bir $env şablonu ('{s}') — bu uç henüz ortam almıyor"),
            StatusCode::UNPROCESSABLE_ENTITY,
        ));
    }
    s.parse::<i32>()
        .map(Some)
        .map_err(|_| AppError(format!("geçersiz port: '{s}'"), StatusCode::BAD_REQUEST))
}

fn to_config(b: &ConnBody, secret: Option<String>) -> Result<DbConfig, AppError> {
    let driver = DbDriver::parse(&b.driver)
        .ok_or_else(|| AppError("geçersiz driver".into(), StatusCode::BAD_REQUEST))?;
    Ok(DbConfig {
        driver,
        mode: b.mode.clone(),
        host: b.host.clone(),
        port: parse_port(b.port.as_deref())?,
        database: b.database.clone(),
        username: b.username.clone(),
        secret,
        options: if b.options.is_null() {
            json!({})
        } else {
            b.options.clone()
        },
    })
}

#[utoipa::path(get, path = "/connections", tag = "db", params(ListQuery),
    responses((status = 200, description = "DB bağlantı listesi (secret hariç)", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn list(
    State(s): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Value>, AppError> {
    // wfd_id verilmediyse owner çifti NULL kalır → `scope='local'` dalı hiç eşleşmez.
    let owner = match q.wfd_id {
        Some(id) => Some(wfd_owner(&s.pool, id).await?),
        None => None,
    };
    let (owner_project, owner_name) = match &owner {
        Some((p, n)) => (Some(*p), Some(n.as_str())),
        None => (None, None),
    };
    let rows = sqlx::query_as::<_, (Uuid, String, String, String, Option<String>, Option<String>, Option<String>, Option<String>, Value, bool, Option<bool>, Option<chrono::DateTime<chrono::Utc>>, String)>(
        "SELECT id, name, driver, mode, host, port, database, username, options, is_active, last_test_ok, last_test_at, scope \
         FROM wf.db_connection \
         WHERE orgtnt_id=$1 AND is_active=true \
           AND (scope='global' OR (scope='local' AND project_id=$2 AND wfd_name=$3)) \
         ORDER BY scope, name")
        .bind(q.orgtnt_id).bind(owner_project).bind(owner_name).fetch_all(&s.pool).await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    // secret ASLA dönmez
    let items: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            json!({
                "id": r.0, "name": r.1, "driver": r.2, "mode": r.3, "host": r.4, "port": r.5,
                "database": r.6, "username": r.7, "options": r.8, "is_active": r.9,
                "last_test_ok": r.10, "last_test_at": r.11, "scope": r.12,
            })
        })
        .collect();
    Ok(Json(json!(items)))
}

#[utoipa::path(post, path = "/connections", tag = "db",
    request_body = ConnBody,
    responses((status = 200, description = "Oluşturulan bağlantı id", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn create(
    State(s): State<AppState>,
    Json(b): Json<ConnBody>,
) -> Result<Json<Value>, AppError> {
    let orgtnt = b
        .orgtnt_id
        .ok_or_else(|| AppError("orgtnt_id gerekli".into(), StatusCode::BAD_REQUEST))?;
    let name = b
        .name
        .clone()
        .ok_or_else(|| AppError("name gerekli".into(), StatusCode::BAD_REQUEST))?;
    let scope = b.scope.as_deref().unwrap_or("global");
    let owner = match scope {
        "global" => None,
        "local" => {
            let wfd_id = b.wfd_id.ok_or_else(|| {
                AppError(
                    "lokal bağlantı için wfd_id gerekli".into(),
                    StatusCode::BAD_REQUEST,
                )
            })?;
            Some(wfd_owner(&s.pool, wfd_id).await?)
        }
        other => {
            return Err(AppError(
                format!("geçersiz scope: {other}"),
                StatusCode::BAD_REQUEST,
            ))
        }
    };
    let enc = match &b.secret {
        Some(sec) => Some(
            crypto::encrypt(sec).map_err(|e| AppError(e.to_string(), StatusCode::BAD_REQUEST))?,
        ),
        None => None,
    };
    let id = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO wf.db_connection (orgtnt_id,name,driver,mode,host,port,database,username,options,secret_enc,scope,project_id,wfd_name) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13) RETURNING id")
        .bind(orgtnt).bind(&name).bind(&b.driver).bind(&b.mode)
        .bind(&b.host).bind(b.port).bind(&b.database).bind(&b.username)
        .bind(&b.options).bind(enc)
        .bind(scope)
        .bind(owner.as_ref().map(|(p, _)| *p))
        .bind(owner.as_ref().map(|(_, n)| n.as_str()))
        .fetch_one(&s.pool).await
        .map_err(map_write_err)?;
    Ok(Json(json!({ "id": id })))
}

#[utoipa::path(put, path = "/connections/{id}", tag = "db",
    params(("id" = Uuid, Path, description = "Bağlantı id")), request_body = ConnBody,
    responses((status = 204, description = "Güncellendi")),
    security(("x_admin_key" = [])))]
async fn update(
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(b): Json<ConnBody>,
) -> Result<StatusCode, AppError> {
    // Kapsam ve sahiplik update'te DEĞİŞMEZ: global bir bağlantı lokale (ya da tersi)
    // dönüştürülemez — dönüşse ona referans veren WFD'lerin görünürlüğü sessizce kayardı.
    // secret verilmezse mevcut korunur (COALESCE): None → NULL bind → COALESCE(NULL, secret_enc)
    let enc: Option<Vec<u8>> = match &b.secret {
        Some(sec) => Some(
            crypto::encrypt(sec).map_err(|e| AppError(e.to_string(), StatusCode::BAD_REQUEST))?,
        ),
        None => None,
    };
    let n = sqlx::query(
        "UPDATE wf.db_connection SET name=$2, driver=$3, mode=$4, host=$5, port=$6, database=$7, \
         username=$8, options=$9, secret_enc=COALESCE($10, secret_enc), updated_at=now() WHERE id=$1")
        .bind(id).bind(&b.name).bind(&b.driver).bind(&b.mode).bind(&b.host).bind(b.port)
        .bind(&b.database).bind(&b.username).bind(&b.options).bind(enc)
        .execute(&s.pool).await
        .map_err(map_write_err)?.rows_affected();
    if n == 0 {
        return Err(AppError(
            "bağlantı bulunamadı".into(),
            StatusCode::NOT_FOUND,
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(delete, path = "/connections/{id}", tag = "db",
    params(("id" = Uuid, Path, description = "Bağlantı id")),
    responses((status = 204, description = "Silindi")),
    security(("x_admin_key" = [])))]
async fn delete(State(s): State<AppState>, Path(id): Path<Uuid>) -> Result<StatusCode, AppError> {
    sqlx::query("DELETE FROM wf.db_connection WHERE id=$1")
        .bind(id)
        .execute(&s.pool)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/connections/test", tag = "db",
    request_body = ConnBody,
    responses((status = 200, description = "Bağlantı testi sonucu (ok/message)", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn test_draft(
    State(_s): State<AppState>,
    Json(b): Json<ConnBody>,
) -> Result<Json<Value>, AppError> {
    let cfg = to_config(&b, b.secret.clone())?;
    Ok(Json(run_test(&cfg).await))
}

#[utoipa::path(post, path = "/connections/{id}/test", tag = "db",
    params(("id" = Uuid, Path, description = "Bağlantı id")),
    responses((status = 200, description = "Kayıtlı bağlantı testi sonucu (ok/message)", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn test_saved(
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let row = sqlx::query_as::<
        _,
        (
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Value,
            Option<Vec<u8>>,
        ),
    >(
        "SELECT driver, mode, host, port, database, username, options, secret_enc \
         FROM wf.db_connection WHERE id=$1",
    )
    .bind(id)
    .fetch_optional(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?
    .ok_or_else(|| AppError("bağlantı bulunamadı".into(), StatusCode::NOT_FOUND))?;
    let driver = DbDriver::parse(&row.0)
        .ok_or_else(|| AppError("geçersiz driver".into(), StatusCode::INTERNAL_SERVER_ERROR))?;
    let secret = match row.7 {
        Some(bytes) => Some(
            crypto::decrypt(&bytes)
                .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?,
        ),
        None => None,
    };
    let cfg = DbConfig {
        driver,
        mode: row.1,
        host: row.2,
        port: parse_port(row.3.as_deref())?,
        database: row.4,
        username: row.5,
        secret,
        options: row.6,
    };
    let result = run_test(&cfg).await;
    let ok = result.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    let _ =
        sqlx::query("UPDATE wf.db_connection SET last_test_at=now(), last_test_ok=$2 WHERE id=$1")
            .bind(id)
            .bind(ok)
            .execute(&s.pool)
            .await;
    Ok(Json(result))
}

async fn run_test(cfg: &DbConfig) -> Value {
    match db::drivers::test(cfg).await {
        Ok(()) => json!({ "ok": true }),
        Err(e) => json!({ "ok": false, "message": e.to_string() }),
    }
}

/// WFD dokümanındaki autoexec SQL bağlantı referanslarını toplar
/// (`autoexec.<key>.config.connection`). Sıra + tekrarlar korunmaz; küme semantiği.
fn referenced_connection_ids(wfd: &Value) -> Vec<Uuid> {
    let Some(map) = wfd.get("autoexec").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut ids: Vec<Uuid> = map
        .values()
        .filter_map(|def| def.get("config")?.get("connection")?.as_str())
        .filter_map(|s| Uuid::parse_str(s).ok())
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// WFD yazma kapısı: doküman BAŞKA bir WFD'nin lokal bağlantısına referans veremez.
///
/// Editör listesi zaten kapsamla filtreli; bu kapı elle düzenlenmiş/kopyalanmış JSON
/// içindir. Bilinmeyen (silinmiş) id'ler burada HATA DEĞİLDİR — eskiden de kaydedilebilir
/// oldukları için engellenmesi mevcut taslakları kaydedilemez hale getirirdi; onlar
/// çalışma anında "connection bulunamadı" ile düşer.
pub async fn assert_no_foreign_local_connections(
    pool: &PgPool,
    project_id: Uuid,
    wfd_name: &str,
    wfd: &Value,
) -> Result<(), AppError> {
    let ids = referenced_connection_ids(wfd);
    if ids.is_empty() {
        return Ok(());
    }
    let foreign: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM wf.db_connection \
         WHERE id = ANY($1) AND scope='local' \
           AND NOT (project_id = $2 AND wfd_name = $3) \
         ORDER BY name",
    )
    .bind(&ids)
    .bind(project_id)
    .bind(wfd_name)
    .fetch_all(pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    if foreign.is_empty() {
        return Ok(());
    }
    Err(AppError(
        format!(
            "başka bir WFD'ye ait lokal veritabanı bağlantısı kullanılamaz: {}",
            foreign.join(", ")
        ),
        StatusCode::UNPROCESSABLE_ENTITY,
    ))
}

#[cfg(test)]
mod tests {
    use super::referenced_connection_ids;
    use serde_json::json;

    #[test]
    fn collects_sql_connection_ids_once() {
        let a = "11111111-1111-4111-8111-111111111111";
        let wfd = json!({
            "autoexec": {
                "q1": { "type": "sql", "config": { "connection": a, "query": "select 1" } },
                "q2": { "type": "sql", "config": { "connection": a, "query": "select 2" } },
                "r1": { "type": "rest", "config": { "url": "http://x" } },
                "q3": { "type": "sql", "config": { "query": "select 3" } },
                "q4": { "type": "sql", "config": { "connection": "" } },
            }
        });
        let ids = referenced_connection_ids(&wfd);
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0].to_string(), a);
    }

    #[test]
    fn no_autoexec_is_empty() {
        assert!(referenced_connection_ids(&json!({ "nodes": {} })).is_empty());
    }
}
