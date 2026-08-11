//! Tenant permission havuzunun I/O katmanı.
//!
//! Bu modül SATIR ÇEKER; etkin küme kararını `crate::permission` verir. Süzme
//! (`is_active`, timeslice) bilinçli olarak SQL'de DEĞİL: `WHERE`'e kaçan her kural
//! test edilemez hale gelir (bu repoda DB'li test koşulmuyor). SQL'in tek işi
//! kapsamı getirmektir.
//!
//! Kapsam disiplini: her sorgu `orgtnt_id` ile bağlanır. Bağlanmazsa bir tenant'ın
//! yöneticisi başka tenant'ın `p_id`'sini yol parametresiyle düzenleyebilirdi
//! (`notes::find_note`'un `wfe_id`+`note_id` disiplininin aynısı).

use crate::{
    error::OrgError,
    models::{Permission, PermissionException, PermissionRoleUsage, TenantApiKey},
    permission::{
        effective_permissions, EffectivePermission, OrguRRow, PermissionRows, RpRow, UpRow, UrRow,
    },
};
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

const SEL: &str =
    "p_id, orgtnt_id, code, display_name, description, is_active, created_at, updated_at";

// ── Havuz CRUD ──────────────────────────────────────────────────────────────

/// Havuzu listeler. `q` verilirse kod VE görünen ad üzerinde arar.
pub async fn list(
    pool: &PgPool,
    orgtnt_id: Uuid,
    q: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<Permission>, OrgError> {
    let pattern = q.map(|s| format!("%{}%", escape_like(s)));
    sqlx::query_as::<_, Permission>(&format!(
        "SELECT {SEL} FROM org.p
         WHERE orgtnt_id = $1
           AND ($2::text IS NULL OR code ILIKE $2 ESCAPE '\\' OR display_name ILIKE $2 ESCAPE '\\')
         ORDER BY code
         LIMIT $3 OFFSET $4"
    ))
    .bind(orgtnt_id)
    .bind(pattern)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(OrgError::Database)
}

pub async fn get(pool: &PgPool, orgtnt_id: Uuid, p_id: Uuid) -> Result<Permission, OrgError> {
    sqlx::query_as::<_, Permission>(&format!(
        "SELECT {SEL} FROM org.p WHERE orgtnt_id = $1 AND p_id = $2"
    ))
    .bind(orgtnt_id)
    .bind(p_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| OrgError::NotFound("permission".into()))
}

/// Yeni yetki. Biçim kapısı DB'deki `p_code_format` CHECK'idir — tek kaynak;
/// ihlal `error.rs`'te kısıt ADINDAN 400'e çevrilir.
pub async fn create(
    pool: &PgPool,
    orgtnt_id: Uuid,
    code: &str,
    display_name: &str,
    description: Option<&str>,
) -> Result<Permission, OrgError> {
    let code = code.trim();
    let display_name = display_name.trim();
    if code.is_empty() || display_name.is_empty() {
        return Err(OrgError::BadRequest("kod ve görünen ad boş olamaz".into()));
    }
    sqlx::query_as::<_, Permission>(&format!(
        "INSERT INTO org.p (orgtnt_id, code, display_name, description)
         VALUES ($1, $2, $3, $4) RETURNING {SEL}"
    ))
    .bind(orgtnt_id)
    .bind(code)
    .bind(display_name)
    .bind(description.map(str::trim).filter(|s| !s.is_empty()))
    .fetch_one(pool)
    .await
    .map_err(OrgError::Database)
}

/// PATCH semantiği (`PATCH /org/orgtnt/{id}` ile aynı): alan `None` ise DEĞİŞMEZ,
/// `description` boş string ile TEMİZLENİR (NULL). Zorunlu alanlar boş gönderilirse
/// 400 — sessizce eski değeri korumak, kullanıcının sildiğini sandığı adı bırakırdı.
///
/// Okuma+yazma tek transaction'da `FOR UPDATE` ile (`orgtnt::patch` deseni).
pub async fn patch(
    pool: &PgPool,
    orgtnt_id: Uuid,
    p_id: Uuid,
    code: Option<&str>,
    display_name: Option<&str>,
    description: Option<&str>,
    is_active: Option<bool>,
) -> Result<Permission, OrgError> {
    let mut tx = pool.begin().await?;
    let current = sqlx::query_as::<_, Permission>(&format!(
        "SELECT {SEL} FROM org.p WHERE orgtnt_id = $1 AND p_id = $2 FOR UPDATE"
    ))
    .bind(orgtnt_id)
    .bind(p_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| OrgError::NotFound("permission".into()))?;

    let next_code = match code.map(str::trim) {
        Some("") => return Err(OrgError::BadRequest("kod boş olamaz".into())),
        Some(c) => c.to_string(),
        None => current.code,
    };
    let next_display = match display_name.map(str::trim) {
        Some("") => return Err(OrgError::BadRequest("görünen ad boş olamaz".into())),
        Some(d) => d.to_string(),
        None => current.display_name,
    };
    let next_description = match description.map(str::trim) {
        Some("") => None,
        Some(d) => Some(d.to_string()),
        None => current.description,
    };

    let updated = sqlx::query_as::<_, Permission>(&format!(
        "UPDATE org.p
         SET code = $3, display_name = $4, description = $5,
             is_active = COALESCE($6, is_active), updated_at = now()
         WHERE orgtnt_id = $1 AND p_id = $2
         RETURNING {SEL}"
    ))
    .bind(orgtnt_id)
    .bind(p_id)
    .bind(next_code)
    .bind(next_display)
    .bind(next_description)
    .bind(is_active)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(updated)
}

/// Kullanımdaki yetki SİLİNMEZ (`is_active = false` kullanılır): dış uygulama bir gün
/// `granted:true` alıp ertesi gün sessizce `false` almasın. Referans varsa
/// `BadRequest`, yoksa gerçek silme (havuz temiz kalsın).
pub async fn delete(pool: &PgPool, orgtnt_id: Uuid, p_id: Uuid) -> Result<(), OrgError> {
    let mut tx = pool.begin().await?;
    let in_use: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM org.rp WHERE p_id = $1)
             OR EXISTS (SELECT 1 FROM org.up WHERE p_id = $1)",
    )
    .bind(p_id)
    .fetch_one(&mut *tx)
    .await?;
    if in_use {
        return Err(OrgError::Conflict("permission.in_use".into()));
    }
    let affected = sqlx::query("DELETE FROM org.p WHERE orgtnt_id = $1 AND p_id = $2")
        .bind(orgtnt_id)
        .bind(p_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if affected == 0 {
        return Err(OrgError::NotFound("permission".into()));
    }
    tx.commit().await?;
    Ok(())
}

// ── Rol = permission grubu ──────────────────────────────────────────────────

pub async fn role_permissions(
    pool: &PgPool,
    orgtnt_id: Uuid,
    r_id: Uuid,
) -> Result<Vec<Permission>, OrgError> {
    sqlx::query_as::<_, Permission>(&format!(
        "SELECT {} FROM org.p p
         JOIN org.rp rp ON rp.p_id = p.p_id
         WHERE p.orgtnt_id = $1 AND rp.r_id = $2
         ORDER BY p.code",
        SEL.split(", ")
            .map(|c| format!("p.{c}"))
            .collect::<Vec<_>>()
            .join(", ")
    ))
    .bind(orgtnt_id)
    .bind(r_id)
    .fetch_all(pool)
    .await
    .map_err(OrgError::Database)
}

/// Rolün permission KÜMESİNİ ayarlar (PUT semantiği): verilmeyenler silinir.
///
/// Tek transaction — yönetim ekranı "kutucukları işaretle → kaydet" akışıdır; tek tek
/// çağrı iki yöneticinin aynı rolü düzenlemesinde yarım uygulanmış küme bırakırdı.
/// Kapsam dışı `p_id` (başka tenant / olmayan) sessizce atlanmaz, HATA verir: yönetici
/// işaretlediği kutucuğun kaydedilmediğini fark etmeli.
pub async fn set_role_permissions(
    pool: &PgPool,
    orgtnt_id: Uuid,
    r_id: Uuid,
    p_ids: &[Uuid],
) -> Result<Vec<Permission>, OrgError> {
    let mut tx = pool.begin().await?;
    assert_role_in_tenant(&mut tx, orgtnt_id, r_id).await?;
    assert_perms_in_tenant(&mut tx, orgtnt_id, p_ids).await?;

    sqlx::query("DELETE FROM org.rp WHERE r_id = $1 AND NOT (p_id = ANY($2))")
        .bind(r_id)
        .bind(p_ids)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO org.rp (orgtnt_id, r_id, p_id)
         SELECT $1, $2, unnest($3::uuid[])
         ON CONFLICT (r_id, p_id) DO NOTHING",
    )
    .bind(orgtnt_id)
    .bind(r_id)
    .bind(p_ids)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    role_permissions(pool, orgtnt_id, r_id).await
}

/// Ters sorgu: bu yetki hangi rollerde, her rol kaç kullanıcıya ulaşıyor.
///
/// Kullanıcı sayısı `org.ur` doğrudan grant'ı VEYA `org.orgu_r` birim devralması
/// üzerinden sayılır — biri sayılmasa yetkinin yayılımı olduğundan küçük görünürdü.
pub async fn permission_roles(
    pool: &PgPool,
    orgtnt_id: Uuid,
    p_id: Uuid,
) -> Result<Vec<PermissionRoleUsage>, OrgError> {
    sqlx::query_as::<_, PermissionRoleUsage>(
        "SELECT r.r_id, r.name AS role_name,
                ( SELECT count(DISTINCT u_id) FROM (
                      SELECT ur.u_id FROM org.ur ur
                      WHERE ur.r_id = r.r_id AND ur.orgu_id IS NOT NULL
                        AND ur.ur_type <> 'excluded'
                      UNION
                      SELECT uo.u_id FROM org.orgu_r orr
                      JOIN org.u_orgu uo ON uo.orgu_id = orr.orgu_id
                      WHERE orr.r_id = r.r_id
                  ) reach ) AS user_count
         FROM org.r r
         JOIN org.rp rp ON rp.r_id = r.r_id
         WHERE rp.p_id = $2 AND r.orgtnt_id = $1
         ORDER BY r.name",
    )
    .bind(orgtnt_id)
    .bind(p_id)
    .fetch_all(pool)
    .await
    .map_err(OrgError::Database)
}

// ── Kişisel ıskarta (T‑A2) ──────────────────────────────────────────────────

pub async fn user_exceptions(
    pool: &PgPool,
    orgtnt_id: Uuid,
    u_id: Uuid,
) -> Result<Vec<PermissionException>, OrgError> {
    sqlx::query_as::<_, PermissionException>(
        "SELECT up.up_id, p.p_id, p.code, p.display_name, up.valid_from, up.valid_until
         FROM org.up up JOIN org.p p ON p.p_id = up.p_id
         WHERE up.orgtnt_id = $1 AND up.u_id = $2 AND up.up_type = 'excluded'
         ORDER BY p.code",
    )
    .bind(orgtnt_id)
    .bind(u_id)
    .fetch_all(pool)
    .await
    .map_err(OrgError::Database)
}

/// Ayarlanacak tek ıskarta: yetki + (isteğe bağlı) geçerlilik penceresi.
///
/// Pencere `None` ise ıskarta SÜRESİZDİR. Dolu verilirse istisna geçicidir
/// ("bu ay onaylamasın") ve süresi geçince yetki kendiliğinden geri gelir —
/// etkin küme hesabı `org.up` timeslice'ına saygı duyar.
#[derive(Debug, Clone)]
pub struct ExceptionInput {
    pub p_id: Uuid,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
}

/// Kişisel ıskarta KÜMESİNİ ayarlar (PUT semantiği, tek transaction).
///
/// Var olan satırın penceresi GÜNCELLENİR (`DO UPDATE`): `DO NOTHING` olsaydı bir
/// ıskartanın süresini değiştirmek imkânsız olurdu — kullanıcı önce kaldırıp
/// yeniden eklemek zorunda kalır, arada yetki bir an açılırdı.
pub async fn set_user_exceptions(
    pool: &PgPool,
    orgtnt_id: Uuid,
    u_id: Uuid,
    items: &[ExceptionInput],
) -> Result<Vec<PermissionException>, OrgError> {
    for item in items {
        if let (Some(from), Some(until)) = (item.valid_from, item.valid_until) {
            if until <= from {
                // Ters pencere ıskartayı SESSİZCE etkisiz kılardı (hiçbir an geçerli
                // olmaz) — yönetici yetkiyi kapattığını sanardı.
                return Err(OrgError::BadRequest(
                    "ıskarta bitişi başlangıcından sonra olmalı".into(),
                ));
            }
        }
    }
    let p_ids: Vec<Uuid> = items.iter().map(|i| i.p_id).collect();
    let from: Vec<Option<DateTime<Utc>>> = items.iter().map(|i| i.valid_from).collect();
    let until: Vec<Option<DateTime<Utc>>> = items.iter().map(|i| i.valid_until).collect();

    let mut tx = pool.begin().await?;
    assert_user_in_tenant(&mut tx, orgtnt_id, u_id).await?;
    assert_perms_in_tenant(&mut tx, orgtnt_id, &p_ids).await?;

    sqlx::query(
        "DELETE FROM org.up
         WHERE u_id = $1 AND up_type = 'excluded' AND NOT (p_id = ANY($2))",
    )
    .bind(u_id)
    .bind(&p_ids)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO org.up (orgtnt_id, u_id, p_id, up_type, valid_from, valid_until)
         SELECT $1, $2, t.p_id, 'excluded', t.valid_from, t.valid_until
         FROM unnest($3::uuid[], $4::timestamptz[], $5::timestamptz[])
              AS t(p_id, valid_from, valid_until)
         ON CONFLICT (u_id, p_id, up_type) DO UPDATE
         SET valid_from = EXCLUDED.valid_from, valid_until = EXCLUDED.valid_until",
    )
    .bind(orgtnt_id)
    .bind(u_id)
    .bind(&p_ids)
    .bind(&from)
    .bind(&until)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    user_exceptions(pool, orgtnt_id, u_id).await
}

// ── Etkin küme ──────────────────────────────────────────────────────────────

/// Etkin küme hesabının girdisini çeker. Süzme YOK — kararı `crate::permission` verir.
pub async fn load_rows(
    pool: &PgPool,
    orgtnt_id: Uuid,
    u_id: Uuid,
) -> Result<PermissionRows, OrgError> {
    let ur = sqlx::query(
        "SELECT ur.orgu_id, ur.r_id, r.name AS role_name, r.is_active AS role_is_active,
                ur.ur_type, ur.valid_from, ur.valid_until
         FROM org.ur ur JOIN org.r r ON r.r_id = ur.r_id
         WHERE ur.u_id = $1 AND ur.orgtnt_id = $2",
    )
    .bind(u_id)
    .bind(orgtnt_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|row| UrRow {
        orgu_id: row.get("orgu_id"),
        r_id: row.get("r_id"),
        role_name: row.get("role_name"),
        role_is_active: row.get("role_is_active"),
        ur_type: row.get("ur_type"),
        valid_from: row.get("valid_from"),
        valid_until: row.get("valid_until"),
    })
    .collect::<Vec<_>>();

    // Yalnız kullanıcının ÜYE olduğu birimlerin grant'ları (`u_orgu` join'i kapsamdır).
    let orgu_r = sqlx::query(
        "SELECT orr.orgu_id, orr.r_id, r.name AS role_name, r.is_active AS role_is_active,
                orr.valid_from, orr.valid_until
         FROM org.orgu_r orr
         JOIN org.r r ON r.r_id = orr.r_id
         JOIN org.u_orgu uo ON uo.orgu_id = orr.orgu_id
         WHERE uo.u_id = $1 AND orr.orgtnt_id = $2",
    )
    .bind(u_id)
    .bind(orgtnt_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|row| OrguRRow {
        orgu_id: row.get("orgu_id"),
        r_id: row.get("r_id"),
        role_name: row.get("role_name"),
        role_is_active: row.get("role_is_active"),
        valid_from: row.get("valid_from"),
        valid_until: row.get("valid_until"),
    })
    .collect::<Vec<_>>();

    let role_ids: Vec<Uuid> = ur
        .iter()
        .map(|r| r.r_id)
        .chain(orgu_r.iter().map(|r| r.r_id))
        .collect();

    let rp = sqlx::query("SELECT r_id, p_id FROM org.rp WHERE r_id = ANY($1)")
        .bind(&role_ids)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|row| RpRow {
            r_id: row.get("r_id"),
            p_id: row.get("p_id"),
        })
        .collect::<Vec<_>>();

    let up = sqlx::query(
        "SELECT p_id, up_type, valid_from, valid_until
         FROM org.up WHERE u_id = $1 AND orgtnt_id = $2",
    )
    .bind(u_id)
    .bind(orgtnt_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|row| UpRow {
        p_id: row.get("p_id"),
        up_type: row.get("up_type"),
        valid_from: row.get("valid_from"),
        valid_until: row.get("valid_until"),
    })
    .collect::<Vec<_>>();

    // Katalog `is_active` DAHİL gelir: süzmeyi saf fonksiyon yapar.
    let p_ids: Vec<Uuid> = rp.iter().map(|r| r.p_id).collect();
    let perms = sqlx::query_as::<_, Permission>(&format!(
        "SELECT {SEL} FROM org.p WHERE orgtnt_id = $1 AND p_id = ANY($2)"
    ))
    .bind(orgtnt_id)
    .bind(&p_ids)
    .fetch_all(pool)
    .await?;

    Ok(PermissionRows {
        ur,
        orgu_r,
        rp,
        up,
        perms,
    })
}

pub async fn effective_for_user(
    pool: &PgPool,
    orgtnt_id: Uuid,
    u_id: Uuid,
) -> Result<Vec<EffectivePermission>, OrgError> {
    let rows = load_rows(pool, orgtnt_id, u_id).await?;
    Ok(effective_permissions(&rows, Utc::now()))
}

/// `check` ucu için katalog: yalnız SORULAN kodların satırları. Tüm havuzu çekmek
/// binlerce satırlık tenant'ta boşa iş olurdu; `unknown` teşhisi için varlık yeter.
pub async fn catalog_by_codes(
    pool: &PgPool,
    orgtnt_id: Uuid,
    codes: &[String],
) -> Result<Vec<Permission>, OrgError> {
    let folded: Vec<String> = codes.iter().map(|c| c.to_ascii_lowercase()).collect();
    sqlx::query_as::<_, Permission>(&format!(
        "SELECT {SEL} FROM org.p WHERE orgtnt_id = $1 AND lower(code) = ANY($2)"
    ))
    .bind(orgtnt_id)
    .bind(&folded)
    .fetch_all(pool)
    .await
    .map_err(OrgError::Database)
}

/// Kullanıcının tenant'ı. `/org/users/{id}/...` yolları tenant taşımıyor
/// (mevcut `/org/users/{id}/roles` ile aynı biçim); kapsam kullanıcı satırından
/// çözülür ki sorgular yine `orgtnt_id` ile bağlanabilsin.
pub async fn tenant_of_user(pool: &PgPool, u_id: Uuid) -> Result<Uuid, OrgError> {
    sqlx::query_scalar::<_, Uuid>("SELECT orgtnt_id FROM org.u WHERE u_id = $1")
        .bind(u_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| OrgError::NotFound("user".into()))
}

/// Tenant içinde kullanıcı adından `u_id` — `check` ucu ikisini de kabul eder.
pub async fn user_id_by_username(
    pool: &PgPool,
    orgtnt_id: Uuid,
    username: &str,
) -> Result<Uuid, OrgError> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT u_id FROM org.u WHERE orgtnt_id = $1 AND username = $2",
    )
    .bind(orgtnt_id)
    .bind(username)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| OrgError::NotFound("user".into()))
}

// ── Tenant API anahtarı ─────────────────────────────────────────────────────

pub async fn list_api_keys(
    pool: &PgPool,
    orgtnt_id: Uuid,
) -> Result<Vec<TenantApiKey>, OrgError> {
    sqlx::query_as::<_, TenantApiKey>(
        "SELECT key_id, orgtnt_id, name, prefix, key_hash, is_active,
                expires_at, last_used_at, created_at
         FROM org.orgtnt_api_key WHERE orgtnt_id = $1 ORDER BY created_at DESC",
    )
    .bind(orgtnt_id)
    .fetch_all(pool)
    .await
    .map_err(OrgError::Database)
}

pub async fn create_api_key(
    pool: &PgPool,
    orgtnt_id: Uuid,
    name: &str,
    prefix: &str,
    key_hash: &str,
    expires_at: Option<DateTime<Utc>>,
) -> Result<TenantApiKey, OrgError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(OrgError::BadRequest("anahtar adı boş olamaz".into()));
    }
    sqlx::query_as::<_, TenantApiKey>(
        "INSERT INTO org.orgtnt_api_key (orgtnt_id, name, prefix, key_hash, expires_at)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING key_id, orgtnt_id, name, prefix, key_hash, is_active,
                   expires_at, last_used_at, created_at",
    )
    .bind(orgtnt_id)
    .bind(name)
    .bind(prefix)
    .bind(key_hash)
    .bind(expires_at)
    .fetch_one(pool)
    .await
    .map_err(OrgError::Database)
}

/// Önek ile arama — hash karşılaştırması ÇAĞIRANDA (sabit zamanlı) yapılır.
pub async fn api_key_by_prefix(
    pool: &PgPool,
    prefix: &str,
) -> Result<Option<TenantApiKey>, OrgError> {
    sqlx::query_as::<_, TenantApiKey>(
        "SELECT key_id, orgtnt_id, name, prefix, key_hash, is_active,
                expires_at, last_used_at, created_at
         FROM org.orgtnt_api_key WHERE prefix = $1",
    )
    .bind(prefix)
    .fetch_optional(pool)
    .await
    .map_err(OrgError::Database)
}

pub async fn revoke_api_key(
    pool: &PgPool,
    orgtnt_id: Uuid,
    key_id: Uuid,
) -> Result<(), OrgError> {
    let affected = sqlx::query(
        "UPDATE org.orgtnt_api_key SET is_active = false
         WHERE orgtnt_id = $1 AND key_id = $2 AND is_active = true",
    )
    .bind(orgtnt_id)
    .bind(key_id)
    .execute(pool)
    .await?
    .rows_affected();
    if affected == 0 {
        return Err(OrgError::NotFound("api key".into()));
    }
    Ok(())
}

/// `last_used_at`'ı GÜN bazında tazeler. Amaç kullanılmayan anahtarı fark etmek;
/// kesin denetim izi değil — her istekte yazmak `check` ucunu okuma yolundan
/// yazma yoluna çevirirdi.
pub async fn touch_api_key(pool: &PgPool, key_id: Uuid) -> Result<(), OrgError> {
    sqlx::query(
        "UPDATE org.orgtnt_api_key SET last_used_at = now()
         WHERE key_id = $1
           AND (last_used_at IS NULL OR last_used_at < now() - interval '1 day')",
    )
    .bind(key_id)
    .execute(pool)
    .await?;
    Ok(())
}

// ── Kapsam kapıları ─────────────────────────────────────────────────────────

async fn assert_role_in_tenant(
    tx: &mut Transaction<'_, Postgres>,
    orgtnt_id: Uuid,
    r_id: Uuid,
) -> Result<(), OrgError> {
    let ok: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM org.r WHERE r_id = $1 AND orgtnt_id = $2)",
    )
    .bind(r_id)
    .bind(orgtnt_id)
    .fetch_one(&mut **tx)
    .await?;
    if ok {
        Ok(())
    } else {
        Err(OrgError::NotFound("role".into()))
    }
}

async fn assert_user_in_tenant(
    tx: &mut Transaction<'_, Postgres>,
    orgtnt_id: Uuid,
    u_id: Uuid,
) -> Result<(), OrgError> {
    let ok: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM org.u WHERE u_id = $1 AND orgtnt_id = $2)",
    )
    .bind(u_id)
    .bind(orgtnt_id)
    .fetch_one(&mut **tx)
    .await?;
    if ok {
        Ok(())
    } else {
        Err(OrgError::NotFound("user".into()))
    }
}

/// Kapsam dışı `p_id` sessizce atlanmaz: yönetici işaretlediği kutucuğun
/// kaydedilmediğini fark etmeli.
async fn assert_perms_in_tenant(
    tx: &mut Transaction<'_, Postgres>,
    orgtnt_id: Uuid,
    p_ids: &[Uuid],
) -> Result<(), OrgError> {
    if p_ids.is_empty() {
        return Ok(());
    }
    let found: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM org.p WHERE orgtnt_id = $1 AND p_id = ANY($2)",
    )
    .bind(orgtnt_id)
    .bind(p_ids)
    .fetch_one(&mut **tx)
    .await?;
    let distinct = {
        let mut ids = p_ids.to_vec();
        ids.sort();
        ids.dedup();
        ids.len() as i64
    };
    if found == distinct {
        Ok(())
    } else {
        Err(OrgError::NotFound("permission".into()))
    }
}

/// `ILIKE` joker karakterlerini kaçırır — kullanıcının yazdığı `%`/`_` ARAMA
/// METNİDİR, desen değil (`user_role::escape_like` ile aynı gerekçe).
fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}
