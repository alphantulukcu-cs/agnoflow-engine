//! Portal havuz endpoint'leri (WOR-44 — v2.2 uyumu).
//! Listeleme denormalize current_c_a cache'i ile SQL'de; can-claim/claim
//! kararı engine matcher'ı ile verilir (c_u kuralları dahil), yazım CAS'tır.

use utoipa_axum::router::OpenApiRouter;
use super::jwt::PortalActor;
use crate::{error::AppError, state::AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::routes;
use uuid::Uuid;
use wfe_core::types::actor::Actor;
use wfe_core::v22::matcher::{authorize, MatchEnv};
use wfe_core::v22::ports::WfdStore;

pub fn router(state: AppState) -> OpenApiRouter {
    OpenApiRouter::new()
        .routes(routes!(list_pool))
        .routes(routes!(can_claim))
        .routes(routes!(claim))
        .with_state(state)
}

fn to_actor(actor: &PortalActor) -> Actor {
    Actor {
        orgu_id: actor.orgu_id,
        user_id: actor.user_id,
        role: actor.role.clone(),
    }
}

/// Havuz satırının node `Ref`i. WFD çözülemediyse etiket anahtarın okunur hâline
/// düşer — `label`ın DAİMA dolu olması sözleşmedir, tek bozuk satır onu bozmamalı.
fn node_ref(wfd: Option<&wfe_core::types::wfd_v22::Wfd>, key: &str) -> wf_wfe::executor::Ref {
    match wfd {
        Some(w) => wf_wfe::executor::Ref::node(w, key),
        None => wf_wfe::executor::Ref {
            id: key.to_string(),
            label: wfe_core::v22::display::humanize_key(key),
        },
    }
}

/// One item in the pool list. Bir SQL row'u — `wfd_version` yalnızca
/// `claim_deadline` hesabı için tutulur, response'a serialize edilmez.
#[derive(Debug, sqlx::FromRow)]
struct PoolRow {
    id: Uuid,
    title: String,
    workflow_id: Uuid,
    wfd_version: i32,
    status: String,
    current_node: Option<String>,
    created_at: DateTime<Utc>,
    claimed_by: Option<Value>,
    deadline: Option<DateTime<Utc>>,
    claimed_at: Option<DateTime<Utc>>,
}

/// WOR-31 T4: paralel modda aktif bir kol satırı (`wf.wfe_branch` × `wf.wfe`).
#[derive(Debug, sqlx::FromRow)]
struct BranchPoolRow {
    id: Uuid,
    title: String,
    workflow_id: Uuid,
    wfd_version: i32,
    branch_node: String,
    created_at: DateTime<Utc>,
    claimed_by: Option<Value>,
    deadline: Option<DateTime<Utc>>,
    claimed_at: Option<DateTime<Utc>>,
}

/// Response şekli — SLA görünüm alanları (2026-07-16): `priority` (1-10,
/// otomatik) ve `claim_deadline` (claimed_at + node.claim_timeout).
#[derive(Debug, Serialize, ToSchema)]
pub struct PoolTask {
    pub id: Uuid,
    pub title: String,
    pub workflow_id: Uuid,
    pub status: String,
    /// Anahtar + gösterim (`{id, label}`) — istemci `id`yi geri gönderir, `label`ı basar.
    #[schema(value_type = Object)]
    pub current_node: Option<wf_wfe::executor::Ref>,
    pub created_at: DateTime<Utc>,
    pub claimed_by: Option<Value>,
    pub deadline: Option<DateTime<Utc>>,
    pub claimed_at: Option<DateTime<Utc>>,
    pub claim_deadline: Option<DateTime<Utc>>,
    pub priority: i32,
    /// WOR-31 T4: paralel mod kolu — `Some` ise bu satır belirli bir aktif kolu
    /// temsil eder (claim `node`, aksiyon `branch` olarak bu `id`yi geri geçirir).
    /// Paralel değilse alan hiç çıkmaz.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Object)]
    pub node: Option<wf_wfe::executor::Ref>,
    /// WOR-65: WFE revizyon token'ı. Havuz listesinde AÇIKÇA döner çünkü burada
    /// `wfah` YOKTUR — portal'ın revizyonu türetebileceği başka alan yok.
    /// **WFE-seviyesidir:** aynı paralel WFE'nin farklı kolları için üretilen
    /// satırlar AYNI `rev`'i taşır (kol-bazlı revizyon yoktur).
    pub rev: i32,
    /// WFE not tasarımı Faz 1 (K9): görünür (published + gizlenmemiş + bu
    /// aktöre `audience` açık) not sayısı — `rev` gibi TEK toplu sorguyla, kol
    /// satırları için de aynı WFE değerini taşır.
    pub note_count: i64,
    /// Faz 3 (K9 okundu takibi): bu aktör için OKUNMAMIŞ not sayısı —
    /// `note_count`'un YANINA eklendi, onu YERİNE geçmedi.
    pub unread_note_count: i64,
}

#[utoipa::path(get, path = "/", tag = "portal",
    responses((status = 200, description = "Aktörün havuzundaki görevler (öncelik sıralı)", body = Vec<PoolTask>)),
    security(("bearer_jwt" = [])))]
async fn list_pool(
    State(s): State<AppState>,
    actor: PortalActor,
) -> Result<Json<Vec<PoolTask>>, AppError> {
    // WOR-44: pool = role match OR user match (c_u-resolved candidates,
    // including listable[] grants folded into current_c_a) OR owner match
    // (already claimed by this actor). role/user checks are containment on
    // the denormalized current_c_a cache; owner check is on claimed_by.
    let role_filter = serde_json::json!([{
        "orgu_id": actor.orgu_id.to_string(),
        "role":    actor.role
    }]);
    let user_filter = serde_json::json!([{
        "orgu_id": actor.orgu_id.to_string(),
        "user_id": actor.user_id.to_string()
    }]);
    // c_u can also be a non-UUID identifier (username) — mirror matcher.rs's
    // identity channel by resolving the actor's own ident, if any exists.
    let ident = s
        .executor
        .org
        .user_ident(actor.user_id)
        .await
        .map_err(AppError::from)?;
    let ident_filter = ident.as_ref().map(|ident| {
        serde_json::json!([{
            "orgu_id":    actor.orgu_id.to_string(),
            "user_ident": ident
        }])
    });
    // Çapasız (c_orgu'suz) kural girdileri birim TAŞIMAZ ve `any_orgu: true` ile
    // işaretlidir — bu yüzden orgu'lu filtreler onları YAKALAMAZ, ayrı containment gerekir.
    // Aksi halde havuz listesi ile claim kapısı ayrı düşerdi: kişi başka birimdeyken
    // matcher onu yetkilendirir ama görev listesinde hiç görünmezdi.
    // `any_orgu` işareti filtreye DAHİL: çıplak `[{"user_id": U}]` sorgusu aynı kişinin
    // BAŞKA bir birimdeki scope'lu grantını da kapsardı (jsonb containment alt küme sorar).
    let any_orgu_user_filter = serde_json::json!([{
        "any_orgu": true,
        "user_id":  actor.user_id.to_string()
    }]);
    let any_orgu_ident_filter = ident.as_ref().map(|ident| {
        serde_json::json!([{
            "any_orgu":   true,
            "user_ident": ident
        }])
    });
    let owner_filter = serde_json::json!({ "user_id": actor.user_id.to_string() });

    let rows = sqlx::query_as::<_, PoolRow>(
        "SELECT e.wfe_id       AS id,
                m.name         AS title,
                e.wfd_id       AS workflow_id,
                e.wfd_version,
                e.status,
                e.current_node,
                e.created_at,
                e.claimed_by,
                e.deadline,
                e.claimed_at
         FROM wf.wfe e
         JOIN wf.wfd_meta m
           ON m.wfd_id = e.wfd_id AND m.version = e.wfd_version
         WHERE e.status     = 'active'
           AND e.orgtnt_id  = $1
           AND (e.deadline IS NULL OR e.deadline > now())
           AND (
                 e.current_c_a @> $2::jsonb
              OR e.current_c_a @> $3::jsonb
              OR ($4::jsonb IS NOT NULL AND e.current_c_a @> $4::jsonb)
              OR e.claimed_by @> $5::jsonb
              OR e.current_c_a @> $6::jsonb
              OR ($7::jsonb IS NOT NULL AND e.current_c_a @> $7::jsonb)
           )
         ORDER BY e.created_at ASC",
    )
    .bind(actor.orgtnt_id)
    .bind(role_filter)
    .bind(user_filter)
    .bind(ident_filter)
    .bind(owner_filter)
    .bind(any_orgu_user_filter)
    .bind(any_orgu_ident_filter)
    .fetch_all(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    // priority kolon DEĞİL — okuma anında hesaplanır (2026-07-16 sözleşmesi).
    // Basitlik için Rust'ta hesaplanır ve sıralanır (SQL CASE/window ifadesi
    // yerine): pool boyutu tek aktörle sınırlı, N+1 sıralama maliyeti ihmal
    // edilebilir düzeyde.
    let now = Utc::now();
    let mut wfd_cache: std::collections::HashMap<(Uuid, i32), wfe_core::types::wfd_v22::Wfd> =
        std::collections::HashMap::new();
    let mut tasks = Vec::with_capacity(rows.len());
    for row in rows {
        let priority = wf_wfe::priority::compute_priority(row.created_at, row.deadline, now);
        // WFD artık HER satır için çözülür: `current_node` etiketi de ondan gelir.
        // Çözülemezse (silinmiş/bozuk sürüm) etiket anahtarın okunur hâline düşer,
        // satır havuzdan DÜŞMEZ.
        let key = (row.workflow_id, row.wfd_version);
        if !wfd_cache.contains_key(&key) {
            if let Ok(w) = s.wfd.fetch(row.workflow_id, row.wfd_version).await {
                wfd_cache.insert(key, w);
            }
        }
        let wfd = wfd_cache.get(&key);
        let claim_deadline = if row.claimed_at.is_some() && row.current_node.is_some() {
            wfd.and_then(|wfd| {
                wf_wfe::executor::compute_claim_deadline(
                    wfd,
                    row.current_node.as_deref(),
                    row.claimed_at,
                )
            })
        } else {
            None
        };
        let current_node = row.current_node.as_deref().map(|n| node_ref(wfd, n));
        tasks.push(PoolTask {
            id: row.id,
            title: row.title,
            workflow_id: row.workflow_id,
            status: row.status,
            current_node,
            created_at: row.created_at,
            claimed_by: row.claimed_by,
            deadline: row.deadline,
            claimed_at: row.claimed_at,
            claim_deadline,
            priority,
            node: None,
            rev: 0, // WOR-65: iki döngü de bittikten sonra tek toplu sorguyla doldurulur
            note_count: 0, // aşağıda doldurulur
            unread_note_count: 0, // aşağıda doldurulur
        });
    }

    // WOR-31 T4: paralel WFE'ler yukarıdaki sorguda GÖRÜNMEZ — fork'ta wfe-seviyesi
    // current_c_a/current_node/claimed_by temizlenir (current_c_a '[]' olur, hiçbir
    // containment filtresi eşleşmez). Paralel her AKTİF kol ayrı bir satır olarak
    // eklenir; kolun kendi `current_c_a` cache'i YOKTUR (v1 — minimal query
    // uzantısı: schema değişikliği yerine adaylık burada matcher ile hesaplanır).
    let branch_rows = sqlx::query_as::<_, BranchPoolRow>(
        "SELECT e.wfe_id       AS id,
                m.name         AS title,
                e.wfd_id       AS workflow_id,
                e.wfd_version,
                b.branch_node,
                e.created_at,
                b.claimed_by,
                e.deadline,
                b.claimed_at
         FROM wf.wfe_branch b
         JOIN wf.wfe e
           ON e.wfe_id = b.wfe_id
         JOIN wf.wfd_meta m
           ON m.wfd_id = e.wfd_id AND m.version = e.wfd_version
         WHERE b.status     = 'active'
           AND e.status     = 'active'
           AND e.orgtnt_id  = $1
           AND (e.deadline IS NULL OR e.deadline > now())
         ORDER BY e.created_at ASC, b.branch_node ASC",
    )
    .bind(actor.orgtnt_id)
    .fetch_all(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    let portal_as_actor = to_actor(&actor);
    // WFE başına TEK `load` (kol sayısınca DEĞİL) — dynctx/wfah adaylık matcher'ının
    // (c_orgu ifadesi $ctx/$wfah'a bakabilir) ihtiyacı olan bağlamdır.
    let mut wfes_cache: std::collections::HashMap<Uuid, wfe_core::v22::ports::Wfes> =
        std::collections::HashMap::new();
    for row in branch_rows {
        if !wfes_cache.contains_key(&row.id) {
            match s.executor.wfe.load(row.id).await {
                Ok(w) => {
                    wfes_cache.insert(row.id, w);
                }
                // silinmiş/tutarsız tek bir WFE tüm listeyi düşürmesin.
                Err(_) => continue,
            }
        }
        let wfes = wfes_cache.get(&row.id).unwrap();

        let key = (row.workflow_id, row.wfd_version);
        let wfd = match wfd_cache.get(&key) {
            Some(w) => w,
            None => match s.wfd.fetch(row.workflow_id, row.wfd_version).await {
                Ok(w) => {
                    wfd_cache.insert(key, w);
                    wfd_cache.get(&key).unwrap()
                }
                Err(_) => continue,
            },
        };
        let Some(node_def) = wfd.nodes.get(&row.branch_node) else {
            continue;
        };

        let owner_match = row
            .claimed_by
            .as_ref()
            .and_then(|cb| cb.get("user_id"))
            .and_then(|u| u.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())
            == Some(actor.user_id);
        // Rol/user eşleşmesi node.c_a üzerinden ANLIK hesaplanır (bkz. yukarıdaki
        // yorum — kol-seviyesi denormalize cache yok); zaten claim edilmiş
        // OLMASI eşleşmeyi ENGELLEMEZ (tek-node WFE'lerdeki mevcut davranışla
        // aynı — pool "görünürlük", "claim edilebilirlik" değildir).
        // listable[] grant'leri de tek-node yolundakiyle AYNI semantikle dahildir
        // (pipeline::node_candidates fold'u — `when` guard'ı pool'da yok sayılır,
        // over-inclusive cache kabulü; detay VIEW'ı can_view'da when'i değerlendirir).
        let env = MatchEnv {
            ctx: wfes.dynctx.as_value(),
            wfah: &wfes.wfah,
            orgtnt_id: wfes.orgtnt_id,
        };
        let mut candidate_match = owner_match
            || authorize(&node_def.c_a, &portal_as_actor, env, &*s.executor.org)
                .await
                .unwrap_or(false);
        if !candidate_match {
            for rule in &wfd.listable {
                if authorize(&rule.c_a, &portal_as_actor, env, &*s.executor.org)
                    .await
                    .unwrap_or(false)
                {
                    candidate_match = true;
                    break;
                }
            }
        }
        if !candidate_match {
            continue;
        }

        let priority = wf_wfe::priority::compute_priority(row.created_at, row.deadline, now);
        let claim_deadline =
            wf_wfe::executor::compute_claim_deadline(wfd, Some(&row.branch_node), row.claimed_at);
        tasks.push(PoolTask {
            id: row.id,
            title: row.title,
            workflow_id: row.workflow_id,
            status: "active".into(),
            current_node: Some(node_ref(Some(wfd), &row.branch_node)),
            created_at: row.created_at,
            claimed_by: row.claimed_by,
            deadline: row.deadline,
            claimed_at: row.claimed_at,
            claim_deadline,
            priority,
            node: Some(node_ref(Some(wfd), &row.branch_node)),
            rev: 0,         // aşağıda doldurulur
            note_count: 0,  // aşağıda doldurulur
            unread_note_count: 0, // aşağıda doldurulur
        });
    }

    // WOR-65: revizyon token'ları TEK toplu sorguda — iki döngü de aynı WFE'yi
    // (paralel modda kol başına bir satır) üretebildiği için doldurma işlemi
    // döngülerin İÇİNDE değil, birleşik liste üzerinde yapılır; böylece ne N+1
    // sorgu ne de aynı WFE için tekrarlanan sorgu olur. Not sayaçları (Faz 1)
    // AYNI id kümesini kullanır — aynı gerekçe, ikinci toplu sorgu.
    let rev_ids: Vec<Uuid> = {
        let mut ids: Vec<Uuid> = tasks.iter().map(|t| t.id).collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    };
    let revs = wf_wfe::repo::wfah::max_seq_by_wfe(&s.pool, &rev_ids)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    let note_counts = crate::notes::count_by_wfe(&s.pool, &rev_ids, &portal_as_actor).await?;
    let unread_counts =
        crate::notes::unread_count_by_wfe(&s.pool, &rev_ids, &portal_as_actor).await?;
    for task in &mut tasks {
        task.rev = revs.get(&task.id).copied().unwrap_or(0);
        task.note_count = note_counts.get(&task.id).copied().unwrap_or(0);
        task.unread_note_count = unread_counts.get(&task.id).copied().unwrap_or(0);
    }

    // priority DESC, deadline ASC NULLS LAST, created_at ASC (sözleşme sırası).
    tasks.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| match (a.deadline, b.deadline) {
                (Some(x), Some(y)) => x.cmp(&y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
            .then_with(|| a.created_at.cmp(&b.created_at))
    });

    Ok(Json(tasks))
}

#[derive(Serialize, ToSchema)]
struct CanClaimResponse {
    can_claim: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

/// WOR-31 T4: paralel modda kol seçimi — `?node=<branch_node>`; birden fazla
/// aktif kol varsa hangi kolun sorulduğunu belirtmek için gereklidir.
#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct NodeQuery {
    #[serde(default)]
    node: Option<String>,
}

#[utoipa::path(get, path = "/{wfe_id}/can-claim", tag = "portal",
    params(("wfe_id" = Uuid, Path, description = "WFE id"), NodeQuery),
    responses((status = 200, description = "Bu görev claim edilebilir mi", body = CanClaimResponse)),
    security(("bearer_jwt" = [])))]
async fn can_claim(
    State(s): State<AppState>,
    actor: PortalActor,
    Path(wfe_id): Path<Uuid>,
    axum::extract::Query(q): axum::extract::Query<NodeQuery>,
) -> Result<Json<CanClaimResponse>, AppError> {
    let (can_claim, reason) = s
        .executor
        .can_claim(wfe_id, &to_actor(&actor), q.node.as_deref())
        .await
        .map_err(AppError::from)?;
    Ok(Json(CanClaimResponse { can_claim, reason }))
}

#[derive(Serialize, ToSchema)]
struct ClaimResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

/// WOR-31 T4: gövde opsiyonel (bkz. `routes/wfe.rs::parse_claim_body`) — eski
/// istemciler hiç body göndermez, `{"node": "..."}` paralel kol ipucu taşır.
#[derive(Deserialize, Default, ToSchema)]
#[schema(as = PortalClaimBody)]
struct ClaimBody {
    #[serde(default)]
    node: Option<String>,
    /// WOR-65: `PoolTask.rev`'den okunan revizyon token'ı. OPSİYONEL —
    /// göndermeyen istemci için claim akışı HİÇ DEĞİŞMEZ. Verilirse ve havuz
    /// satırı bu arada geçersizleştiyse (collapse kolu iptal etti, WFE taşındı)
    /// yanıt yanıltıcı `already_claimed` yerine 409 + `conflict.stale_revision`.
    #[serde(default)]
    expected_rev: Option<u32>,
}

#[utoipa::path(post, path = "/{wfe_id}/claim", tag = "portal",
    params(("wfe_id" = Uuid, Path, description = "WFE id")),
    request_body = ClaimBody,
    responses((status = 200, description = "Claim sonucu", body = ClaimResponse)),
    security(("bearer_jwt" = [])))]
async fn claim(
    State(s): State<AppState>,
    actor: PortalActor,
    Path(wfe_id): Path<Uuid>,
    body: axum::body::Bytes,
) -> Result<Json<ClaimResponse>, AppError> {
    let claim_body: ClaimBody = if body.is_empty() {
        ClaimBody::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|e| AppError(format!("invalid claim body: {e}"), StatusCode::BAD_REQUEST))?
    };
    let outcome = s
        .executor
        .claim(
            wfe_id,
            &to_actor(&actor),
            claim_body.node.as_deref(),
            claim_body.expected_rev,
        )
        .await
        .map_err(AppError::from)?;
    if !outcome.success {
        let status = match outcome.reason.as_deref() {
            Some("already_claimed") => StatusCode::CONFLICT,
            _ => StatusCode::FORBIDDEN,
        };
        return Err(AppError(
            outcome
                .reason
                .unwrap_or_else(|| "Bu görevi almak için yetkiniz yok.".into()),
            status,
        ));
    }
    Ok(Json(ClaimResponse {
        success: true,
        reason: None,
    }))
}
