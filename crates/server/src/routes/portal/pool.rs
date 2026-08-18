//! Portal havuz endpoint'leri (WOR-44 — v2.2 uyumu).
//!
//! **Görünürlük burada ÜRETİLMEZ, ÖDÜNÇ ALINIR** (2026-08-14): iki havuz sorgusu
//! da `wf_wfe::visibility::sql`i koşar — liste ucunun (`GET /wfe?viewable=true`)
//! ve detay kapısının (`GET /wfe/:id`) koştuğu parçanın TA KENDİSİ. Havuzun kendi
//! `WHERE`'i vardı ve node-seviyesi listable kolonlarını (`wfe.current_view_c_a`,
//! `wfe_branch.view_c_a`) tanımıyordu; sonuç, 2026-08-13 kararının önlemek için
//! yazıldığı "aynı soruya üç farklı cevap" durumuydu — node listable'a uyan aktör
//! WFE'yi listede ve detayda görüyor, havuzda göremiyordu.
//!
//! **Görünmek ≠ claim edebilmek.** Havuz satırı üretmek görünürlük sorusudur;
//! claim kapısı AYRIDIR ve node `c_a`'sına bakar (`WfeExecutor::can_claim`/
//! `claim` → matcher, hiçbir projeksiyon kolonu okumaz). Havuzda görünen ama
//! claim edilemeyen satır olabilir — kök `listable` için bu zaten böyleydi.
//! can-claim/claim kararı engine matcher'ı ile verilir (c_u kuralları dahil),
//! yazım CAS'tır.
//!
//! **Cevap bu ayrımı TAŞIR** (2026-08-14): `PoolTask.can_claim`. Görünürlük
//! kapsamı genişleyince (node `listable`, `wf_admin`) kullanıcı claim
//! edemeyeceği satırı diğerlerinden ayırt edemiyor, düğmeye basıp `403`
//! yiyordu. Alan kararı ÜRETMEZ, ÖDÜNÇ ALIR: `WfeExecutor::can_claim_many` →
//! `can_claim_loaded`, yani `can_claim`/`claim` uçlarının gövdesi. Hesap
//! TOPLUdur (tek `load_many` + sürüm başına bir WFD), satır başına sorgu YOK.
//! Tekil uç (`GET /portal/pool/{wfe_id}/can-claim`) gerekçesiyle DURUYOR.

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
    /// **Bu satırı claim edebilir miyim?** (2026-08-14, EKLENEN alan — mevcut
    /// alanların hiçbiri değişmedi.)
    ///
    /// Havuz görünürlüğü tek SQL predicate'ine bağlandığında kapsam genişledi:
    /// kök `listable`, node `listable` ve `wf_admin` de satır üretiyor. Claim
    /// kapısı ise AYRI ve node `c_a`'sına bakıyor — kullanıcı claim edemeyeceği
    /// satırı diğerlerinden ayırt edemiyor, düğmeye basıp `403` yiyordu.
    ///
    /// Değer `WfeExecutor::can_claim_loaded`'dan gelir; yani `can_claim`/`claim`
    /// uçlarının kullandığı GÖVDENİN kendisi (`Engine::can_claim` → matcher →
    /// node `c_a`). Havuzda ikinci bir claim kuralı YOKTUR; bu alan kararı
    /// TAŞIR, ÜRETMEZ. Paralel kol satırlarında karar KOLUN node'una göredir.
    ///
    /// Zaten claim edilmiş satır: başkasındaysa `false` (`already_claimed`),
    /// sahibi çağıran ise `true` (idempotent re-claim semantiği). Durumu/WFD'si
    /// okunamayan satırda `false` (fail-closed).
    ///
    /// Tekil uç (`GET /portal/pool/{wfe_id}/can-claim`) DURUYOR — gerekçe
    /// (`reason`) hâlâ oradan okunur; istemcinin satır başına çağırmasına artık
    /// gerek yok.
    pub can_claim: bool,
}

/// Havuz sorgularının KENDİ parametre sayısı: yalnız `$1` = tenant.
/// Görünürlük filtreleri bunun ARDINDAN gelir → `visibility::sql(TENANT_PARAMS)`.
const TENANT_PARAMS: usize = 1;

/// Tek-kol havuz sorgusu. `$1` tenant, `$2..$7` görünürlük filtreleri.
///
/// Havuzun kendi süzgeçleri (`status`, `deadline`, `current_node`) görünürlüğün
/// DIŞINDADIR: onlar "bu satır bir havuz görevi mi" sorusunu sorar, "bu aktör bu
/// WFE'yi görebilir mi" sorusunu değil. İkincisinin tek cevabı `visibility::sql`.
fn pool_sql() -> String {
    format!(
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
           -- Paralel WFE'nin WFE-SEVİYESİ satırı havuza girmez: fork'ta
           -- `current_node` NULL'lanır ve o WFE'yi kol satırları temsil eder
           -- (aşağıdaki ikinci sorgu). Eskiden bunu boşalan `current_c_a`
           -- kendiliğinden sağlıyordu; görünürlük parçası kalıcı `view_c_a`yı ve
           -- kol EXISTS'ini de sorduğundan kural artık AÇIKÇA yazılmalı — yoksa
           -- aynı WFE hem node'suz bir satır hem de kol satırları olarak iki kez
           -- listelenirdi.
           AND e.current_node IS NOT NULL
           AND {vis}
         ORDER BY e.created_at ASC",
        vis = crate::visibility::sql(TENANT_PARAMS)
    )
}

/// WOR-31 T4: paralel modda aktif kol satırları. `$1` tenant, `$2..$7` görünürlük.
///
/// Kol tablosunun takma adı `br`'dir, `b` DEĞİL: `visibility::sql` kendi kol
/// EXISTS'ini `wf.wfe_branch b` ile açar, aynı harf dıştan da kullanılsaydı iç
/// sorgu dış adı gölgelerdi (Postgres'te geçerli ama okuyanı yanıltır).
///
/// Satır süzgeci WFE-SEVİYESİDİR: WFE görünüyorsa AKTİF KOLLARININ HEPSİ listelenir.
/// Kol bazında daraltmak ikinci bir görünürlük kuralı yazmak olurdu — 2026-08-13
/// kararının yasakladığı şey tam bu. Kolu claim edebilmek ayrı bir sorudur ve
/// `WfeExecutor::can_claim` node `c_a`'sını sorarak cevaplar.
fn branch_pool_sql() -> String {
    format!(
        "SELECT e.wfe_id       AS id,
                m.name         AS title,
                e.wfd_id       AS workflow_id,
                e.wfd_version,
                br.branch_node,
                e.created_at,
                br.claimed_by,
                e.deadline,
                br.claimed_at
         FROM wf.wfe_branch br
         JOIN wf.wfe e
           ON e.wfe_id = br.wfe_id
         JOIN wf.wfd_meta m
           ON m.wfd_id = e.wfd_id AND m.version = e.wfd_version
         WHERE br.status    = 'active'
           AND e.status     = 'active'
           AND e.orgtnt_id  = $1
           AND (e.deadline IS NULL OR e.deadline > now())
           AND {vis}
         ORDER BY e.created_at ASC, br.branch_node ASC",
        vis = crate::visibility::sql(TENANT_PARAMS)
    )
}

#[utoipa::path(get, path = "/", tag = "portal",
    responses((status = 200, description = "Aktörün havuzundaki görevler (öncelik sıralı)", body = Vec<PoolTask>)),
    security(("bearer_jwt" = [])))]
async fn list_pool(
    State(s): State<AppState>,
    actor: PortalActor,
) -> Result<Json<Vec<PoolTask>>, AppError> {
    let portal_as_actor = to_actor(&actor);
    // Görünürlük filtreleri istek başına BİR kez üretilir (satır başına değil) ve
    // İKİ sorguya da aynı sırayla bağlanır — sıra `visibility::sql`in sözleşmesidir.
    // Havuzun eski elle yazılmış filtre demeti (rol/user/ident/çapasız/owner) BURADAN
    // KALKTI: aynı demeti ikinci kez kurmak, kural değiştiğinde havuzun geride
    // kalmasının tam sebebiydi.
    let filters = crate::visibility::ViewerFilters::build(&portal_as_actor, &*s.executor.org)
        .await
        .map_err(AppError::from)?;
    let binds = filters.as_binds();

    let stmt = pool_sql();
    let mut q = sqlx::query_as::<_, PoolRow>(&stmt).bind(actor.orgtnt_id);
    for b in &binds {
        q = q.bind(b);
    }
    let rows = q
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
            can_claim: false, // aşağıda TOPLU olarak doldurulur
        });
    }

    // WOR-31 T4: paralel WFE'ler yukarıdaki sorguda GÖRÜNMEZ (fork'ta `current_node`
    // NULL'lanır, bkz. `pool_sql`); onları AKTİF KOL başına bir satır temsil eder.
    //
    // 2026-08-14: kol başına CANLI adaylık çözümü (WFE başına `wfe.load` + kol
    // başına `authorize` + kök `listable` fold'u) KALDIRILDI. Sebebi tek başına
    // "N+1 pahalıydı" değil, ikinci bir görünürlük kuralı olmasıydı: `when`
    // guard'ını yok sayıyor, `wf_admin`i ve node listable'ı hiç bilmiyordu.
    // Karşılıkları artık projeksiyon kolonlarındadır ve `visibility::sql`in kol
    // EXISTS'i onları SQL'de sorar — `wf.wfe_branch.c_a` (canlı `authorize`ın
    // cache'i, commit anında `fill_view_grants` yazar), `wf.wfe_branch.view_c_a`
    // (kolun node listable'ı), `wf.wfe.view_c_a` (kök `listable` ∪ `wf_admin`,
    // guard UYGULANMIŞ), `br.claimed_by` (eski `owner_match`). Projeksiyonu
    // olmayan eski satırlar için `visibility_backfill --apply` koşulur — liste ve
    // detay uçlarının da bağlı olduğu aynı ön koşul.
    let branch_stmt = branch_pool_sql();
    let mut q = sqlx::query_as::<_, BranchPoolRow>(&branch_stmt).bind(actor.orgtnt_id);
    for b in &binds {
        q = q.bind(b);
    }
    let branch_rows = q
        .fetch_all(&s.pool)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    for row in branch_rows {
        // WFD yalnız GÖSTERİM için çözülür (etiket + claim deadline); çözülemezse
        // satır havuzdan DÜŞMEZ — görünürlük kararı zaten SQL'de verildi.
        let key = (row.workflow_id, row.wfd_version);
        if !wfd_cache.contains_key(&key) {
            if let Ok(w) = s.wfd.fetch(row.workflow_id, row.wfd_version).await {
                wfd_cache.insert(key, w);
            }
        }
        let wfd = wfd_cache.get(&key);

        let priority = wf_wfe::priority::compute_priority(row.created_at, row.deadline, now);
        let claim_deadline = wfd.and_then(|w| {
            wf_wfe::executor::compute_claim_deadline(w, Some(&row.branch_node), row.claimed_at)
        });
        tasks.push(PoolTask {
            id: row.id,
            title: row.title,
            workflow_id: row.workflow_id,
            status: "active".into(),
            current_node: Some(node_ref(wfd, &row.branch_node)),
            created_at: row.created_at,
            claimed_by: row.claimed_by,
            deadline: row.deadline,
            claimed_at: row.claimed_at,
            claim_deadline,
            priority,
            node: Some(node_ref(wfd, &row.branch_node)),
            rev: 0,         // aşağıda doldurulur
            note_count: 0,  // aşağıda doldurulur
            unread_note_count: 0, // aşağıda doldurulur
            can_claim: false, // aşağıda TOPLU olarak doldurulur
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

    // **Görünmek ≠ claim edebilmek** — ve artık cevap bunu TAŞIR (2026-08-14).
    // Görünürlük tek predicate'e bağlandığında havuz kapsamı genişledi (kök
    // `listable`, node `listable`, `wf_admin` de satır üretiyor); claim kapısı ise
    // AYRI ve node `c_a`'sına bakıyor. Alan olmadan kullanıcı claim edemeyeceği
    // satırı ayırt edemiyor, düğmeye basıp 403 alıyordu.
    //
    // Karar ÖDÜNÇ ALINIR: `WfeExecutor::can_claim_many` satır başına
    // `can_claim_loaded`'ı — yani `can_claim`/`claim` uçlarının gövdesini —
    // çağırır. Havuzda ikinci bir claim kuralı YOK; buradaki tek iş anahtarı
    // (WFE + kol) taşımak. Kol satırlarında hedef KOLUN node'udur (`node.id`),
    // tek-kol satırlarında `None` — yani karar kolun kendi `c_a`'sına göre verilir.
    //
    // N+1 açılmaz: `rev`/not sayaçları gibi TEK toplu geçiş — durumlar tek
    // `load_many`, WFD'ler sürüm başına bir kez (adapter cache'i) okunur, karar
    // üretimi saf CPU'dur.
    let claim_targets: Vec<(Uuid, Option<String>)> = tasks
        .iter()
        .map(|t| (t.id, t.node.as_ref().map(|n| n.id.clone())))
        .collect();
    let claimable = s
        .executor
        .can_claim_many(&claim_targets, &portal_as_actor)
        .await
        .map_err(AppError::from)?;

    for task in &mut tasks {
        task.rev = revs.get(&task.id).copied().unwrap_or(0);
        task.note_count = note_counts.get(&task.id).copied().unwrap_or(0);
        task.unread_note_count = unread_counts.get(&task.id).copied().unwrap_or(0);
        task.can_claim = claimable
            .get(&(task.id, task.node.as_ref().map(|n| n.id.clone())))
            .copied()
            .unwrap_or(false);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Havuz sorguları görünürlük parçasını GÖMMEZ, ÖDÜNÇ ALIR — ve tam olarak
    /// `visibility::sql(TENANT_PARAMS)` metnini taşırlar. Metnin kendisi aranır
    /// (kolon adları değil): parça değişirse iki tüketici de birlikte değişsin,
    /// havuzun kopyası geride kalamasın. Bu repoda DB'li test koşulmuyor, o
    /// yüzden kontrat ÜRETİLEN SQL üzerinden doğrulanır.
    #[test]
    fn both_pool_queries_borrow_the_visibility_predicate() {
        let vis = crate::visibility::sql(TENANT_PARAMS);
        for (name, stmt) in [("wfe", pool_sql()), ("branch", branch_pool_sql())] {
            assert!(
                stmt.contains(&vis),
                "{name} sorgusu visibility::sql({TENANT_PARAMS}) parçasını taşımıyor:\n{stmt}"
            );
        }
    }

    /// Offset kayması: havuzun KENDİ parametresi `$1` (tenant), görünürlük
    /// `$2..$7`. Fazla/eksik parametre bind sırasını sessizce kaydırır — sqlx
    /// derleme zamanında bunu görmez, sorgu çalışma anında ya patlar ya da
    /// YANLIŞ satır döndürür.
    #[test]
    fn visibility_params_start_after_the_tenant_param() {
        let last = TENANT_PARAMS + crate::visibility::PARAM_COUNT;
        assert_eq!(last, 7, "parametre bütçesi değişmiş");
        for (name, stmt) in [("wfe", pool_sql()), ("branch", branch_pool_sql())] {
            assert!(
                stmt.contains("e.orgtnt_id  = $1"),
                "{name}: tenant parametresi $1 değil"
            );
            for i in (TENANT_PARAMS + 1)..=last {
                assert!(stmt.contains(&format!("${i}")), "{name}: ${i} parametresi yok");
            }
            assert!(
                !stmt.contains(&format!("${}", last + 1)),
                "{name}: fazladan parametre var (bind listesi eksik kalır)"
            );
        }
    }

    /// Kararın kendisi: node-seviyesi listable kolonları ARTIK havuzda. Bunlar
    /// `visibility::sql`den gelir; testin işi "havuz o parçayı gerçekten
    /// koşuyor mu"yu kolon adıyla teyit etmek (parça değişip kolon düşerse
    /// yukarıdaki metin testi hâlâ geçerdi).
    ///
    /// 2026-08-17: `e.end_view_c_a` (terminal listable) de listede. Havuzda pratik
    /// karşılığı YOKTUR — havuzun kendi süzgeci `status='active'` ister ve o kolon
    /// yalnız BİTMİŞ satırda dolar. Yine de aranıyor çünkü test "havuz görünürlük
    /// parçasının TAMAMINI koşuyor mu" sorusunu soruyor; parçanın bir kolu havuza
    /// girmezse üç tüketicinin tek cevabı sessizce ikiye ayrılmış olur.
    #[test]
    fn pool_now_sees_node_listable_columns() {
        for (name, stmt) in [("wfe", pool_sql()), ("branch", branch_pool_sql())] {
            for col in [
                "e.current_view_c_a",
                "b.view_c_a",
                "e.view_c_a",
                "e.current_c_a",
                "e.end_view_c_a",
            ] {
                assert!(stmt.contains(col), "{name}: {col} havuz sorgusunda yok");
            }
        }
    }

    /// Kol sorgusunda `wf.wfe_branch` takma adı `br`'dir: `visibility::sql` kendi
    /// EXISTS'ini `wf.wfe_branch b` ile açar, dıştan da `b` kullanılsaydı iç sorgu
    /// dış adı gölgelerdi.
    #[test]
    fn branch_query_does_not_shadow_the_predicate_alias() {
        let stmt = branch_pool_sql();
        // Görünürlük parçası çıkarılınca geriye DIŞ sorgu kalır; kol tablosuna
        // orada TEK bir referans olmalı ve o da `br` takma adıyla.
        let outer = stmt.replace(&crate::visibility::sql(TENANT_PARAMS), "");
        assert!(
            outer.contains("FROM wf.wfe_branch br"),
            "dış sorguda kol takma adı `br` değil:\n{outer}"
        );
        assert_eq!(
            outer.matches("wf.wfe_branch").count(),
            1,
            "dış sorgu kol tablosuna birden fazla kez dokunuyor:\n{outer}"
        );
    }

    /// Havuz cevabı claim edilebilirliği TAŞIR (2026-08-14) ve alan EKLENMİŞtir:
    /// mevcut alanların hiçbiri düşmemiş/yeniden adlandırılmamış olmalı
    /// (`AppError.items` deseni — geriye uyumluluk sözleşmesi). Serileşmiş şekil
    /// üzerinden ölçülür, alan adları istemci sözleşmesidir.
    #[test]
    fn pool_task_adds_can_claim_without_dropping_existing_fields() {
        let task = PoolTask {
            id: Uuid::nil(),
            title: "x".into(),
            workflow_id: Uuid::nil(),
            status: "active".into(),
            current_node: None,
            created_at: DateTime::from_timestamp_nanos(0),
            claimed_by: None,
            deadline: None,
            claimed_at: None,
            claim_deadline: None,
            priority: 1,
            node: None,
            rev: 0,
            note_count: 0,
            unread_note_count: 0,
            can_claim: false,
        };
        let v = serde_json::to_value(&task).unwrap();
        for field in [
            "id",
            "title",
            "workflow_id",
            "status",
            "current_node",
            "created_at",
            "claimed_by",
            "deadline",
            "claimed_at",
            "claim_deadline",
            "priority",
            "rev",
            "note_count",
            "unread_note_count",
        ] {
            assert!(v.get(field).is_some(), "mevcut alan düşmüş: {field}");
        }
        assert_eq!(
            v.get("can_claim"),
            Some(&Value::Bool(false)),
            "claim edilebilirlik alanı cevapta yok"
        );
    }

    /// Paralel WFE'nin WFE-seviyesi satırı havuza girmez — yoksa aynı WFE hem
    /// node'suz bir satır hem de kol satırları olarak iki kez listelenir.
    #[test]
    fn parallel_wfes_are_represented_only_by_branch_rows() {
        assert!(pool_sql().contains("e.current_node IS NOT NULL"));
        assert!(!branch_pool_sql().contains("e.current_node IS NOT NULL"));
    }
}
