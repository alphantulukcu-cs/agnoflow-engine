use crate::{error::WfeError, models::WfeRow};
use sqlx::PgPool;
use uuid::Uuid;

const WFE_COLUMNS: &str = "wfe_id, orgtnt_id, environment_id, wfd_id, wfd_version, status,
                current_node, current_c_a, view_c_a, origin_orgu_id, claimed_by, end_response,
                deadline, claimed_at, join_target, join_threshold, join_when,
                created_at, updated_at";

pub async fn get(pool: &PgPool, wfe_id: Uuid) -> Result<WfeRow, WfeError> {
    sqlx::query_as::<_, WfeRow>(&format!(
        "SELECT {WFE_COLUMNS} FROM wf.wfe WHERE wfe_id = $1"
    ))
    .bind(wfe_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| WfeError::NotFound(wfe_id.to_string()))
}

/// `get`in TOPLU hâli — TEK sorgu, `WfeStore::load_many` için. Bulunamayan id
/// sonuçta YER ALMAZ (tek-WFE yolundaki `NotFound`un karşılığı); çağıran eksik
/// satırı "durumu okuyamadım" olarak yorumlar.
pub async fn get_many(pool: &PgPool, wfe_ids: &[Uuid]) -> Result<Vec<WfeRow>, WfeError> {
    if wfe_ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as::<_, WfeRow>(&format!(
        "SELECT {WFE_COLUMNS} FROM wf.wfe WHERE wfe_id = ANY($1)"
    ))
    .bind(wfe_ids)
    .fetch_all(pool)
    .await
    .map_err(WfeError::Database)
}

pub async fn list_by_tenant(
    pool: &PgPool,
    orgtnt_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<Vec<WfeRow>, WfeError> {
    sqlx::query_as::<_, WfeRow>(&format!(
        "SELECT {WFE_COLUMNS} FROM wf.wfe WHERE orgtnt_id = $1
         ORDER BY created_at DESC LIMIT $2 OFFSET $3"
    ))
    .bind(orgtnt_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(WfeError::Database)
}

/// Tenant'ın TOPLAM WFE sayısı — sayfalama göstergesi (`X-Total-Count`) için.
pub async fn count_by_tenant(pool: &PgPool, orgtnt_id: Uuid) -> Result<i64, WfeError> {
    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM wf.wfe WHERE orgtnt_id = $1")
        .bind(orgtnt_id)
        .fetch_one(pool)
        .await
        .map_err(WfeError::Database)
}

/// Görünürlük süzgeçli sayfa + TOPLAM sayı — sayfalamanın SQL'de yapılabilmesi
/// için süzgecin de SQL'de olması gerekir (2026-08-13). `where_sql` çağıranın
/// verdiği görünürlük parçasıdır (`server::visibility::sql`), parametreleri
/// `bind` closure'ı ile bağlanır: repo katmanı kuralın İÇERİĞİNİ bilmez, yalnız
/// yerini bilir — kural tek bir yerde (server/visibility.rs) yaşar.
///
/// `total` süzgeçten SONRAKİ sayıdır: UI "1-200 / 512" diyebilsin ve son sayfayı
/// hesaplayabilsin diye. Aynı `WHERE` iki kez koşar (sayfa + COUNT); tek sorguda
/// window fonksiyonuyla birleştirmek satır başına toplam taşımak demekti.
pub async fn list_viewable_page(
    pool: &PgPool,
    orgtnt_id: Uuid,
    limit: i64,
    offset: i64,
    where_sql: &str,
    binds: &[Option<serde_json::Value>],
) -> Result<(Vec<WfeRow>, i64), WfeError> {
    // $1 orgtnt, $2..$N görünürlük filtreleri, sonra limit/offset.
    let n = binds.len();
    let rows_sql = format!(
        "SELECT {WFE_COLUMNS} FROM wf.wfe e WHERE e.orgtnt_id = $1 AND {where_sql}
         ORDER BY e.created_at DESC LIMIT ${} OFFSET ${}",
        n + 2,
        n + 3
    );
    let mut q = sqlx::query_as::<_, WfeRow>(&rows_sql).bind(orgtnt_id);
    for b in binds {
        q = q.bind(b);
    }
    let rows = q
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await
        .map_err(WfeError::Database)?;

    let count_sql =
        format!("SELECT count(*) FROM wf.wfe e WHERE e.orgtnt_id = $1 AND {where_sql}");
    let mut cq = sqlx::query_scalar::<_, i64>(&count_sql).bind(orgtnt_id);
    for b in binds {
        cq = cq.bind(b);
    }
    let total = cq.fetch_one(pool).await.map_err(WfeError::Database)?;
    Ok((rows, total))
}

/// Escalation / root-timeout süpürücüsü için tüm aktif WFE id'leri.
pub async fn list_active_ids(pool: &PgPool) -> Result<Vec<Uuid>, WfeError> {
    sqlx::query_scalar::<_, Uuid>("SELECT wfe_id FROM wf.wfe WHERE status = 'active'")
        .fetch_all(pool)
        .await
        .map_err(WfeError::Database)
}

/// Tenant'ta node'u olan tüm aktif WFE'ler — dashboard escalation insight'ı için
/// (WOR: escalation-forecast). En son güncellenenler önce; makul bir tavan ile
/// N+1 escalation hesaplamasının maliyetini sınırlar.
pub async fn list_active_by_tenant(
    pool: &PgPool,
    orgtnt_id: Uuid,
    limit: i64,
) -> Result<Vec<WfeRow>, WfeError> {
    sqlx::query_as::<_, WfeRow>(&format!(
        "SELECT {WFE_COLUMNS} FROM wf.wfe
         WHERE orgtnt_id = $1 AND status = 'active' AND current_node IS NOT NULL
         ORDER BY updated_at DESC LIMIT $2"
    ))
    .bind(orgtnt_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(WfeError::Database)
}
