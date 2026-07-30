//! WFC (iş akışı çağrısı) satırları — `wf.wfe_call`.
//!
//! Tablo hem **outbox** (commit ile aynı tx'te `queued` yazılır, gerçek start ayrı
//! tx'te koşar) hem **çağıran↔çağrılan bağı**dır. Üç mod (`wait` / `detached` /
//! `terminal`) aynı tabloyu paylaşır; tarama sorguları moda göre süzer.
//!
//! Bkz. `migrations/wf/20260730000001_wfe_call.sql` ve `docs/plans/workflow-call.md`.

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{Postgres, Row};
use uuid::Uuid;
use wfe_core::types::wfd_v22::{CallMode, StartAs};
use wfe_core::v22::ports::{CallSite, PendingCall, StagedCall};

fn mode_str(mode: CallMode) -> &'static str {
    mode.as_str()
}

fn parse_mode(s: &str) -> CallMode {
    match s {
        "detached" => CallMode::Detached,
        "terminal" => CallMode::Terminal,
        _ => CallMode::Wait,
    }
}

fn parse_site(kind: &str, key: String) -> CallSite {
    match kind {
        "terminal" => CallSite::Terminal(key),
        _ => CallSite::Node(key),
    }
}

/// Commit/create ile AYNI transaction'da outbox satırlarını yazar.
///
/// `ON CONFLICT DO NOTHING`: `UNIQUE (caller_wfe_id, site_kind, site_key)` çift start'ı
/// engeller. Bu, executor'ın `apply` retry döngüsüyle (WOR-62 conflict retry) birlikte
/// zorunludur — aynı transition ikinci kez koşarsa çağrı İKİ KEZ başlatılmamalıdır.
pub async fn stage(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    orgtnt_id: Uuid,
    caller_wfe_id: Uuid,
    calls: &[StagedCall],
    depth: i32,
    next_depth: i32,
) -> Result<(), sqlx::Error> {
    for call in calls {
        // Yuvalanma ve ardıl AYRI sayılır: alt akış çağrısı `depth`'i, ardıl çağrı
        // `next_depth`'i artırır. Frenleri de ayrıdır (cap 8 / cap 16).
        let (d, nd) = match call.mode {
            CallMode::Terminal => (depth, next_depth + 1),
            _ => (depth + 1, next_depth),
        };
        sqlx::query(
            "INSERT INTO wf.wfe_call
               (orgtnt_id, caller_wfe_id, site_kind, site_key, call_key, mode, status,
                input, deadline, start_as, max_next, depth, next_depth)
             VALUES ($1,$2,$3,$4,$5,$6,'queued',$7,$8,$9,$10,$11,$12)
             ON CONFLICT (caller_wfe_id, site_kind, site_key) DO NOTHING",
        )
        .bind(orgtnt_id)
        .bind(caller_wfe_id)
        .bind(call.site.kind())
        .bind(call.site.key())
        .bind(&call.call_key)
        .bind(mode_str(call.mode))
        .bind(&call.input)
        .bind(call.deadline)
        .bind(match call.start_as {
            StartAs::System => "system",
            StartAs::Actor => "actor",
        })
        .bind(call.max_next.map(|v| v as i32))
        .bind(d)
        .bind(nd)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

fn row_to_pending(row: &sqlx::postgres::PgRow) -> PendingCall {
    PendingCall {
        id: row.get("id"),
        orgtnt_id: row.get("orgtnt_id"),
        caller_wfe_id: row.get("caller_wfe_id"),
        site: parse_site(
            row.get::<String, _>("site_kind").as_str(),
            row.get("site_key"),
        ),
        call_key: row.get("call_key"),
        mode: parse_mode(row.get::<String, _>("mode").as_str()),
        input: row.get("input"),
        deadline: row.get("deadline"),
        start_as: match row.get::<String, _>("start_as").as_str() {
            "system" => StartAs::System,
            _ => StartAs::Actor,
        },
        max_next: row.get::<Option<i32>, _>("max_next").map(|v| v as u32),
        depth: row.get("depth"),
        next_depth: row.get("next_depth"),
        callee_wfe_id: row.get("callee_wfe_id"),
        end_response: row.get("end_response"),
        call_status: row.get("call_status"),
    }
}

const COLS: &str = "id, orgtnt_id, caller_wfe_id, site_kind, site_key, call_key, mode, input, \
                    deadline, start_as, max_next, depth, next_depth, callee_wfe_id, \
                    end_response, call_status";

/// Başlatılmayı bekleyen çağrılar (FIFO — eski çağrı önce başlar).
pub async fn pending_starts(
    pool: &sqlx::PgPool,
    limit: i64,
) -> Result<Vec<PendingCall>, sqlx::Error> {
    let rows = sqlx::query(&format!(
        "SELECT {COLS} FROM wf.wfe_call WHERE status = 'queued' ORDER BY created_at LIMIT $1"
    ))
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_pending).collect())
}

/// Çağrılan bitmiş, çağıranın WFC-RETURN'ü işlemesi bekleniyor. Yalnız `wait`
/// satırları `returned` olabilir (diğer modlarda dönüş yoktur).
pub async fn pending_returns(
    pool: &sqlx::PgPool,
    limit: i64,
) -> Result<Vec<PendingCall>, sqlx::Error> {
    let rows = sqlx::query(&format!(
        "SELECT {COLS} FROM wf.wfe_call WHERE status = 'returned' ORDER BY returned_at LIMIT $1"
    ))
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_pending).collect())
}

/// Süre sınırı geçmiş, hâlâ koşan `wait` çağrıları.
pub async fn overdue(
    pool: &sqlx::PgPool,
    now: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<PendingCall>, sqlx::Error> {
    let rows = sqlx::query(&format!(
        "SELECT {COLS} FROM wf.wfe_call
         WHERE status IN ('queued','running') AND mode = 'wait'
           AND deadline IS NOT NULL AND deadline <= $1
         ORDER BY deadline LIMIT $2"
    ))
    .bind(now)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_pending).collect())
}

pub async fn set_status(
    pool: &sqlx::PgPool,
    id: Uuid,
    status: &str,
    callee_wfe_id: Option<Uuid>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE wf.wfe_call
            SET status = $2,
                callee_wfe_id = COALESCE($3, callee_wfe_id)
          WHERE id = $1",
    )
    .bind(id)
    .bind(status)
    .bind(callee_wfe_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Bir çağrılan WFE bittiğinde onu bekleyen satırı ilerletir.
///
/// `wait` → `returned` (çağıran sonucu işleyecek). Diğer modlar → `consumed`:
/// `detached` ve `terminal`'de dönüş YOKTUR, satır yalnız audit/izleme içindir.
pub async fn mark_callee_finished(
    pool: &sqlx::PgPool,
    callee_wfe_id: Uuid,
    call_status: &str,
    end_response: Option<&Value>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE wf.wfe_call
            SET status = CASE WHEN mode = 'wait' THEN 'returned' ELSE 'consumed' END,
                call_status = $2,
                end_response = $3,
                returned_at = now()
          WHERE callee_wfe_id = $1 AND status IN ('running','queued')",
    )
    .bind(callee_wfe_id)
    .bind(call_status)
    .bind(end_response)
    .execute(pool)
    .await?;
    Ok(())
}

/// WFC-CASCADE: çağıran sonlandığında koşan ALT AKIŞLARI iptal eder ve iptal
/// edilecek çağrılan WFE id'lerini döner.
///
/// **`mode = 'terminal'` KAPSAM DIŞI** — ardıl, astın aksine çağıranın ömrüne bağlı
/// değildir (bkz. decisions.md → WFC). Ardıl zaten çağıran bittikten sonra başlar;
/// onu iptal etmek özelliğin kendisini bozardı.
pub async fn cancel_subcalls_of(
    pool: &sqlx::PgPool,
    caller_wfe_id: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let rows = sqlx::query(
        "UPDATE wf.wfe_call
            SET status = 'cancelled'
          WHERE caller_wfe_id = $1
            AND mode <> 'terminal'
            AND status IN ('queued','running','returned')
          RETURNING callee_wfe_id",
    )
    .bind(caller_wfe_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .filter_map(|r| r.get::<Option<Uuid>, _>("callee_wfe_id"))
        .collect())
}

/// Bir WFE'nin çağrı geçmişi — API görünümü (`GET /wfe/:id`).
pub struct CallRow {
    pub site_kind: String,
    pub site_key: String,
    pub call_key: String,
    pub mode: String,
    pub status: String,
    pub callee_wfe_id: Option<Uuid>,
    pub call_status: Option<String>,
}

pub async fn list_of_caller(
    pool: &sqlx::PgPool,
    caller_wfe_id: Uuid,
) -> Result<Vec<CallRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT site_kind, site_key, call_key, mode, status, callee_wfe_id, call_status
           FROM wf.wfe_call WHERE caller_wfe_id = $1 ORDER BY created_at",
    )
    .bind(caller_wfe_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|r| CallRow {
            site_kind: r.get("site_kind"),
            site_key: r.get("site_key"),
            call_key: r.get("call_key"),
            mode: r.get("mode"),
            status: r.get("status"),
            callee_wfe_id: r.get("callee_wfe_id"),
            call_status: r.get("call_status"),
        })
        .collect())
}

/// Bir WFE'yi ÇAĞIRAN satır (varsa) — `GET /wfe/:id` içinde `caller` alanı.
pub async fn caller_of(
    pool: &sqlx::PgPool,
    callee_wfe_id: Uuid,
) -> Result<Option<(Uuid, CallRow)>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT caller_wfe_id, site_kind, site_key, call_key, mode, status, callee_wfe_id, call_status
           FROM wf.wfe_call WHERE callee_wfe_id = $1 LIMIT 1",
    )
    .bind(callee_wfe_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| {
        (
            r.get("caller_wfe_id"),
            CallRow {
                site_kind: r.get("site_kind"),
                site_key: r.get("site_key"),
                call_key: r.get("call_key"),
                mode: r.get("mode"),
                status: r.get("status"),
                callee_wfe_id: r.get("callee_wfe_id"),
                call_status: r.get("call_status"),
            },
        )
    }))
}
