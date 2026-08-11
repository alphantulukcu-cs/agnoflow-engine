//! Ek-belge (attachment) endpoint'leri — DİREKT `/wfe/*` route ağacı (X-Actor
//! header auth). work-pool-portal bu ağacı kullanır (bkz. MEMORY: portal /wfe/*
//! direkt rotaları kullanır). JWT `/portal/wfe/*` ağacının kendi kopyası
//! `routes/portal/attachments.rs`'dedir; ikisi de `crate::attachments` paylaşımlı
//! store + durum yardımcılarını kullanır.
//!
//! Mimari: engine core dosya I/O yapmaz. Varlık kontrolü + yükleme burada
//! (opendal `AttachmentStore`) yürür; yetki `executor.query` (görünürlük) ile
//! doğrulanır. Aksiyon gate'i `routes/wfe.rs::apply_action` içinde uygulanır.

use utoipa_axum::router::OpenApiRouter;
use crate::attachments::{status_for_node, AttachmentGroupStatus};
use crate::{error::AppError, state::AppState};
use axum::{
    body::Bytes,
    extract::{Multipart, Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use utoipa::ToSchema;
use utoipa_axum::routes;
use uuid::Uuid;
use wfe_core::types::actor::Actor;
use wfe_core::types::wfd_v22::{AttachmentItem, Wfd};
use wfe_core::v22::ports::WfdStore;

/// `/wfe` router'ına merge edilir.
pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(status))
        .routes(routes!(download, remove))
        .routes(routes!(upload_multi))
}

#[derive(Serialize, ToSchema)]
struct NodeAttachmentStatus {
    satisfied: bool,
    #[schema(value_type = Vec<Object>)]
    groups: Vec<AttachmentGroupStatus>,
}

#[derive(Serialize, ToSchema)]
struct AttachmentsResponse {
    /// node slug → o node'un attachment durumu. Tek-node modda tek anahtar; paralel
    /// modda her aktif kol node'u için bir anahtar. İstemci, aksiyonu uygularken
    /// kullanacağı node'un durumunu buradan okur.
    attachments: BTreeMap<String, NodeAttachmentStatus>,
}

/// WFE'nin WFD'sini getirir. Yetki `executor.query` ile ayrıca doğrulanır — bu
/// yalnız wfd_id/version çözümüdür (orgtnt filtresi yok; direkt route deseni).
pub(crate) async fn load_wfd(s: &AppState, wfe_id: Uuid) -> Result<Wfd, AppError> {
    #[derive(sqlx::FromRow)]
    struct Row {
        wfd_id: Uuid,
        wfd_version: i32,
    }
    let row = sqlx::query_as::<_, Row>(
        "SELECT wfd_id, wfd_version FROM wf.wfe WHERE wfe_id = $1",
    )
    .bind(wfe_id)
    .fetch_optional(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?
    .ok_or_else(|| AppError("WFE bulunamadı.".into(), StatusCode::NOT_FOUND))?;

    s.wfd
        .fetch(row.wfd_id, row.wfd_version)
        .await
        .map_err(AppError::from)
}

/// Yükleme/indirme/silme için ortak çözüm: WFE VAR MI, yoksa REZERVE Mİ EDİLMİŞ?
///
/// Başlatma öncesi belgeler henüz var olmayan bir WFE'nin id'sine yazılır (bkz.
/// `crate::reservation`). O aşamada `executor.query` çağrılamaz — ortada durum yok;
/// yetki rezervasyonun sahipliğiyle verilir. WFE gerçekten varsa eski yol işler
/// (görünürlük + katalog).
///
/// Döner: (doğrulanacak WFD, o WFE/rezervasyon için çözülmüş depo).
/// `for_write`: YAZMA yolunda depo çözümü KATIDIR — `$env`de depo tanımlı değilse
/// deployment varsayılanına düşmek yerine `422 attachment_storage.missing_env` döner.
/// Sessiz fallback, müşterinin bucket'ı yerine SUNUCU DİSKİNE yazmak demekti: farklı
/// tenant'ların belgeleri bizim diskimizde yan yana dururdu. Publish kapısı
/// (`routes::wfd::assert_attachment_storage_env`) bunu önden yakalıyor ama tek savunma
/// olamaz — kapıdan önce yayınlanmış akışlar, sonradan silinen `$env` satırları ve
/// anahtarları eksik yeni ortamlar o kapının arkasından geçer.
///
/// Okuma ve silme yollarında fallback KORUNUR (`for_write: false`): eski davranışla
/// deployment deposuna yazılmış dosyalar hâlâ okunabilmeli ve temizlenebilmeli. Katılık
/// yeni yanlış yazımı durdurur, geçmişi erişilemez yapmaz.
async fn resolve_target(
    s: &AppState,
    actor: &Actor,
    wfe_id: Uuid,
    for_write: bool,
) -> Result<(Wfd, std::sync::Arc<crate::attachments::AttachmentStore>), AppError> {
    if let Some(r) = crate::reservation::get(&s.pool, wfe_id).await? {
        let orgtnt_id = s
            .executor
            .org
            .orgtnt_for_orgu(actor.orgu_id)
            .await
            .map_err(AppError::from)?;
        if !crate::reservation::owned_by(&r, orgtnt_id, actor) {
            return Err(AppError(
                "bu rezervasyon size ait değil".into(),
                StatusCode::FORBIDDEN,
            ));
        }
        let wfd = s
            .wfd
            .fetch(r.wfd_id, r.wfd_version)
            .await
            .map_err(AppError::from)?;
        let store = if for_write {
            crate::attachment_store::store_for_wfd_strict(s, r.wfd_id, orgtnt_id, r.environment_id)
                .await?
        } else {
            crate::attachment_store::store_for_wfd(s, r.wfd_id, orgtnt_id, r.environment_id).await?
        };
        return Ok((wfd, store));
    }
    authorized_nodes(s, actor, wfe_id).await?;
    let wfd = load_wfd(s, wfe_id).await?;
    let store = if for_write {
        crate::attachment_store::store_for_wfe_strict(s, wfe_id).await?
    } else {
        crate::attachment_store::store_for_wfe(s, wfe_id).await?
    };
    Ok((wfd, store))
}

/// Aktörün bu WFE'yi görebildiğini doğrular (executor.query authz), aktif node
/// listesini döndürür (tek-node → current_node; paralel → kol node'ları).
async fn authorized_nodes(
    s: &AppState,
    actor: &Actor,
    wfe_id: Uuid,
) -> Result<Vec<String>, AppError> {
    let view = s
        .executor
        .query(wfe_id, actor)
        .await
        .map_err(AppError::from)?;
    let nodes = match view.current_node {
        Some(n) => vec![n],
        None => view.branches.iter().map(|b| b.state.branch_node.clone()).collect(),
    };
    Ok(nodes)
}

fn find_item<'a>(wfd: &'a Wfd, group: &str, item: &str) -> Result<&'a AttachmentItem, AppError> {
    wfd.attachments
        .get(group)
        .and_then(|g| g.items.iter().find(|i| i.id == item))
        .ok_or_else(|| {
            AppError(
                format!("attachment slotu bulunamadı: {group}/{item}"),
                StatusCode::NOT_FOUND,
            )
        })
}

#[utoipa::path(get, path = "/{id}/attachments", tag = "attachments",
    params(("id" = Uuid, Path, description = "WFE id")),
    responses((status = 200, description = "Aktif node'ların attachment durumu", body = AttachmentsResponse)),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn status(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(wfe_id): Path<Uuid>,
) -> Result<Json<AttachmentsResponse>, AppError> {
    let actor = super::wfe::extract_actor(&headers)?;
    let nodes = authorized_nodes(&s, &actor, wfe_id).await?;
    let wfd = load_wfd(&s, wfe_id).await?;
    let store = crate::attachment_store::store_for_wfe(&s, wfe_id).await?;

    // DB metadata'sı (ad/tip/boyut/tarih/sürüm) — depo yalnız var/yok bilir. Tek
    // sorguyla önden çekilir, node döngüsünde tekrar sorgulanmaz. Hata olursa (ör.
    // geçici DB sorunu) metadata'sız devam edilir: bu uç yalnız SÜSLEME katmanıdır,
    // `uploaded` gerçeği depodan zaten geliyor — bir gösterim ayrıntısı yüzünden
    // durum sorgusunu 500'e düşürmek istemiyoruz.
    let metas = crate::wfe_attachment::list_by_wfe(&s.pool, wfe_id)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(
                "attachment metadata sorgusu başarısız (wfe_id={wfe_id}): {}",
                e.message
            );
            vec![]
        });

    let mut map = BTreeMap::new();
    for node in nodes {
        // Node geneli liste — aksiyon sorulmadı; kapsam süzmesini istemci `actions`
        // alanından yapar (`satisfied` burada "hiçbir aksiyon engelli değil" demektir).
        let mut groups = status_for_node(&store, &wfd, wfe_id, &node, None)
            .await
            .map_err(|e| {
                AppError(
                    format!("attachment durum sorgusu başarısız: {e}"),
                    StatusCode::INTERNAL_SERVER_ERROR,
                )
            })?;
        crate::attachments::enrich_with_meta(&mut groups, &metas);
        if groups.is_empty() {
            continue;
        }
        map.insert(
            node,
            NodeAttachmentStatus {
                satisfied: crate::attachments::satisfied(&groups),
                groups,
            },
        );
    }
    Ok(Json(AttachmentsResponse { attachments: map }))
}

#[utoipa::path(get, path = "/{id}/attachments/{group}/{item}", tag = "attachments",
    params(
        ("id" = Uuid, Path, description = "WFE id"),
        ("group" = String, Path, description = "Attachment grup key"),
        ("item" = String, Path, description = "Attachment item id"),
    ),
    responses((status = 200, description = "Dosya içeriği (octet-stream)", body = Vec<u8>)),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn download(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((wfe_id, group, item)): Path<(Uuid, String, String)>,
) -> Result<(StatusCode, HeaderMap, Bytes), AppError> {
    let actor = super::wfe::extract_actor(&headers)?;
    let (wfd, store) = resolve_target(&s, &actor, wfe_id, false).await?;
    find_item(&wfd, &group, &item)?;

    let bytes = store
        .read(wfe_id, &group, &item)
        .await
        .map_err(|_| AppError("dosya bulunamadı".into(), StatusCode::NOT_FOUND))?;

    let mut h = HeaderMap::new();
    h.insert(
        axum::http::header::CONTENT_TYPE,
        "application/octet-stream".parse().unwrap(),
    );
    Ok((StatusCode::OK, h, Bytes::from(bytes)))
}

#[utoipa::path(delete, path = "/{id}/attachments/{group}/{item}", tag = "attachments",
    params(
        ("id" = Uuid, Path, description = "WFE id"),
        ("group" = String, Path, description = "Attachment grup key"),
        ("item" = String, Path, description = "Attachment item id"),
    ),
    responses((status = 204, description = "Silindi")),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn remove(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((wfe_id, group, item)): Path<(Uuid, String, String)>,
) -> Result<StatusCode, AppError> {
    let actor = super::wfe::extract_actor(&headers)?;
    let (wfd, store) = resolve_target(&s, &actor, wfe_id, false).await?;
    find_item(&wfd, &group, &item)?;

    store
        .delete(wfe_id, &group, &item)
        .await
        .map_err(|e| {
            AppError(
                format!("silme başarısız: {e}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;
    Ok(StatusCode::NO_CONTENT)
}

// --------------------------------------------------------- çok dosyalı, AKSİYONSUZ yükleme

/// `PUT /{id}/attachments` cevabındaki tek dosya girdisi.
#[derive(Serialize, ToSchema)]
struct UploadedItem {
    group: String,
    item: String,
    size_bytes: u64,
    sha256: String,
}

/// `PUT /{id}/attachments` cevabı.
#[derive(Serialize, ToSchema)]
pub(crate) struct UploadMultiResponse {
    uploaded: Vec<UploadedItem>,
}

/// Slot bazında ret — `routes/wfe.rs::ItemError`in bu ağaçtaki eşdeğeri. O tip PRIVATE
/// olduğu için buradan kullanılamıyor, kendi kopyamız yazılıyor — ama hata gövdesinin
/// ŞEKLİ (`{error, code:"attachment.rejected", items:[{group,item,code,message}]}`)
/// BİREBİR aynı: istemci iki uçta (tekil aksiyon-içi çok dosyalı / bu aksiyonsuz uç)
/// farklı bir şekil görmemeli.
#[derive(Serialize)]
struct UploadItemError {
    group: String,
    item: String,
    code: &'static str,
    message: String,
}

fn rejected_multi(items: Vec<UploadItemError>) -> AppError {
    AppError {
        message: format!("{} belge reddedildi", items.len()),
        status: StatusCode::UNPROCESSABLE_ENTITY,
        code: Some("attachment.rejected"),
        items: serde_json::to_value(items).ok(),
    }
}

/// Aksiyonsuz çok-dosyalı yüklemenin katalog kapsamı.
///
/// Bu uç tek bir aksiyona bağlı DEĞİLDİR (bir grup "toplanır" diyorsa yüklenebilir; hangi
/// aksiyonu kapadığı ayrı bir sorudur) — bu yüzden `AttachmentRef::gates_action` süzmesi
/// UYGULANMAZ, aksiyon kapsamlı bir referans da kapsamsız biriyle aynı şekilde kataloğa girer.
///
/// WFE zaten VARSA: aktif node'ların (paralelde kol node'larının) referansladığı gruplar —
/// `authorized_nodes` bu listeyi zaten `executor.query` authz'siyle birlikte verir.
///
/// Henüz başlamamış bir REZERVASYON içinse "aktif node" kavramı yok (WFE satırı yok,
/// `executor.query` sorabileceği bir durum yok) — o aşamada WFD'nin TÜM katalog gruplarına
/// izin verilir; tek-dosyalı eski uçtaki (`find_item`, kök `wfd.attachments`e bakar) davranışla
/// aynı gerekçe: hangi node'dan geçileceği başlatmadan önce bilinmez.
async fn upload_catalog<'a>(
    s: &AppState,
    actor: &Actor,
    wfe_id: Uuid,
    wfd: &'a Wfd,
    is_reservation: bool,
) -> Result<HashMap<(String, String), &'a AttachmentItem>, AppError> {
    let mut catalog = HashMap::new();
    if is_reservation {
        for (group, def) in &wfd.attachments {
            for item in &def.items {
                catalog.insert((group.clone(), item.id.clone()), item);
            }
        }
        return Ok(catalog);
    }
    let nodes = authorized_nodes(s, actor, wfe_id).await?;
    for node_key in &nodes {
        let Some(node) = wfd.nodes.get(node_key) else { continue };
        for aref in &node.attachments {
            let Some(group) = wfd.attachments.get(aref.group()) else { continue };
            for item in &group.items {
                catalog.insert((aref.group().to_string(), item.id.clone()), item);
            }
        }
    }
    Ok(catalog)
}

/// `wf.upload_staging` satırı için gereken (wfd_id, wfd_version, orgtnt_id, environment_id)
/// dörtlüsü — staging nesnesi NİHAİ anahtarla AYNI depoda durmalı, o depo bu dörtlüyle
/// çözülür (bkz. `crate::staging` modül başlığı "Depo çözümü"). WFE zaten varsa `wf.wfe`
/// satırından okunur (`routes/wfe.rs::apply_multipart_staged`teki AYNI sorgu); henüz
/// başlamamış bir rezervasyon içinse (`wf.wfe` satırı YOK) aynı dörtlü rezervasyon
/// kaydından gelir — `crate::reservation::Reservation` birebir aynı alanları taşır.
async fn staging_context(
    s: &AppState,
    wfe_id: Uuid,
    reservation: Option<&crate::reservation::Reservation>,
) -> Result<(Uuid, i32, Uuid, Option<Uuid>), AppError> {
    if let Some(r) = reservation {
        return Ok((r.wfd_id, r.wfd_version, r.orgtnt_id, r.environment_id));
    }
    sqlx::query_as::<_, (Uuid, i32, Uuid, Option<Uuid>)>(
        "SELECT wfd_id, wfd_version, orgtnt_id, environment_id FROM wf.wfe WHERE wfe_id = $1",
    )
    .bind(wfe_id)
    .fetch_one(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))
}

/// `PUT /{id}/attachments` — çok dosyalı, AKSİYONSUZ yükleme (tek yol, 2026-08-11 sonrası).
///
/// Bugüne kadar panel dosya başına bir `PUT .../attachments/{grup}/{item}` isteği atıyordu;
/// başlatma/aksiyon yolları (Faz 1/4) ise `multipart/form-data` ile N dosyayı bir istekte
/// kabul ediyordu — portal bu yüzden iki ayrı kod yolu tutmak zorundaydı. Bu paylaşımlı
/// yardımcı (hem direkt `/wfe/*` hem JWT `/portal/wfe/*` ağacından çağrılır) panel için de
/// AYNI çok-dosyalı yolu açar. `payload` part'ı YOKTUR: aksiyon/girdi taşımaz, yetki
/// `wfe_id`den zaten çözülür — Faz 1/4'teki "ilk part `payload` olmalı" kuralı (yetki
/// kararı baytlardan önce verilsin diye) burada gereksizdir.
///
/// ATOMİKLİK: Faz 4'ün deseni izlenir (`routes/wfe.rs::apply_multipart_staged`, ONA bkz.) —
/// her dosya önce STAGING'e yazılır (nihai anahtara HİÇ dokunulmaz), hepsi doğrulandıktan
/// SONRA topluca `promote` edilir. Bir dosya reddedilirse HİÇBİRİ nihai anahtara gitmez:
/// kullanıcıya "N dosyadan k'sı yüklendi" diye anlatılacak bir ara durum bırakılmaz.
pub(crate) async fn upload_multi_shared(
    s: &AppState,
    actor: &Actor,
    wfe_id: Uuid,
    mut mp: Multipart,
) -> Result<UploadMultiResponse, AppError> {
    // Yetki + depo: mevcut çözüm — rezervasyon dalını da destekler (başlatma öncesi toplu
    // yükleme de bu uçtan yapılabilir). YAZMA yolu olduğu için KATI çözülür (`for_write:
    // true`); depo burada hiç yazılmıyor olsa da (yazma `staging`in kendi çözümüyle olur)
    // bu çağrı `$env`de depo TANIMLI mı sorusunu baytlardan önce, tek yerden sorar.
    let (wfd, _store) = resolve_target(s, actor, wfe_id, true).await?;
    let reservation = crate::reservation::get(&s.pool, wfe_id).await?;

    let catalog = upload_catalog(s, actor, wfe_id, &wfd, reservation.is_some()).await?;
    let (wfd_id, wfd_version, orgtnt_id, environment_id) =
        staging_context(s, wfe_id, reservation.as_ref()).await?;

    let max_request = s.cfg.attachment_max_request_mb as usize * 1024 * 1024;
    let mut errors: Vec<UploadItemError> = Vec::new();
    let mut parts: Vec<crate::staging::StagedPart> = Vec::new();
    let mut request_total = 0usize;

    while let Some(mut field) = mp
        .next_field()
        .await
        .map_err(|e| AppError(format!("multipart okunamadı: {e}"), StatusCode::BAD_REQUEST))?
    {
        let Some(name) = field.name().map(str::to_string) else {
            continue;
        };
        // Part adı `{grup}/{slot}` — dosya ADI anahtara KARIŞMAZ (Faz 1/4'teki aynı kural:
        // istemcinin verdiği ad depoda yol enjeksiyonu yüzeyi olmasın).
        let Some((group, item)) = name.split_once('/').map(|(g, i)| (g.to_string(), i.to_string()))
        else {
            errors.push(UploadItemError {
                group: name.clone(),
                item: String::new(),
                code: "unknown_slot",
                message: format!("part adı '{name}' '{{grup}}/{{slot}}' biçiminde değil"),
            });
            continue;
        };
        let Some(def) = catalog.get(&(group.clone(), item.clone())).copied() else {
            errors.push(UploadItemError {
                group,
                item,
                code: "unknown_slot",
                message: "bu WFE'nin aktif node'larından erişilebilir bir dosya slotu değil".into(),
            });
            continue;
        };
        // Tip kapısı baytlardan ÖNCE (uzunluk 0): reddedilecek dosya staging'e bile yazılmaz.
        let declared_ct = field.content_type().map(str::to_string);
        if let Err(crate::attachments::UploadReject::UnsupportedType(ct)) =
            crate::attachments::check_upload(def, declared_ct.as_deref(), 0)
        {
            errors.push(UploadItemError {
                group,
                item,
                code: "unsupported_type",
                message: format!(
                    "{ct} desteklenmiyor (izin verilenler: {})",
                    crate::attachments::all_accept_patterns(def).join(", ")
                ),
            });
            continue;
        }

        let staged = crate::staging::Staged {
            upload_id: Uuid::new_v4(),
            orgtnt_id,
            wfd_id,
            wfd_version,
            environment_id,
            grp: group.clone(),
            item: item.clone(),
            actor_orgu_id: actor.orgu_id,
            actor_user_id: actor.user_id,
            created_at: chrono::Utc::now(),
        };
        crate::staging::create(&s.pool, &staged).await?;
        let remaining = max_request.saturating_sub(request_total);
        let part = match crate::staging::stage_part(s, &staged, &mut field, remaining).await {
            Ok(p) => p,
            Err(e) => {
                // Yazma yarıda kaldı: o ana kadarki tüm staging silinir, nihai anahtar zaten
                // HİÇ dokunulmadı — mevcut belgeler olduğu gibi durur.
                crate::staging::discard(s, &parts).await;
                return Err(e);
            }
        };
        request_total += part.size_bytes as usize;

        if part.size_bytes == 0 {
            errors.push(UploadItemError {
                group,
                item,
                code: "empty",
                message: "boş dosya yüklenemez".into(),
            });
            // Boş dosya da staging'e YAZILDI (0 baytlık nesne) — temizlik listesine girer ki
            // `discard` onu da silsin, sahipsiz kalmasın.
            parts.push(part);
            continue;
        }
        if let Err(crate::attachments::UploadReject::TooLarge(mb)) =
            crate::attachments::check_upload(def, declared_ct.as_deref(), part.size_bytes as usize)
        {
            errors.push(UploadItemError {
                group,
                item,
                code: "too_large",
                message: format!("dosya {mb} MB sınırını aşıyor"),
            });
            parts.push(part);
            continue;
        }
        if let Some(crate::attachments::UploadReject::TypeMismatch { declared, detected }) =
            crate::attachments::detect_mismatch(declared_ct.as_deref(), &part.head)
        {
            errors.push(UploadItemError {
                group,
                item,
                code: "type_mismatch",
                message: format!("içerik beyan edilen tiple uyuşmuyor: {declared} denildi, {detected} bulundu"),
            });
            parts.push(part);
            continue;
        }
        parts.push(part);
    }

    if !errors.is_empty() {
        crate::staging::discard(s, &parts).await;
        return Err(rejected_multi(errors));
    }

    // Hepsi doğrulandı: nihai anahtara taşı + metadata satırlarını yaz. Bu ucun apply
    // yolundan (Faz 4) farkı: ÖNÜNDE geri alınamaz bir engine adımı YOKTUR — taşımanın
    // kendisi burada atomikliği taşır. `promote` birkaç kez denenir (geçici ağ/throttle
    // hatalarına karşı, Faz 4'teki AYNI desen); GERÇEKTEN tükenirse henüz denenmemiş
    // parçalar (bu parça dahil) staging'de silinir, istek 502 ile durur. Kabul edilen tek
    // boşluk: fiziksel anahtar VERSİYONSUZDUR (`attachments/{wfe_id}/{grup}/{item}`,
    // yeniden yükleme üzerine yazar) — bu parçadan ÖNCE başarıyla taşınmış olanlar geri
    // ALINAMAZ (eski bayt yedeklenmedi). Bu, doğrulama/ret yolundaki ("hiçbiri yazılmamalı")
    // atomiklik sözünden AYRI bir sınıf: burada dosyalar zaten kabul edilmiş, yalnız fiziksel
    // taşıma (server-side copy) nadir bir altyapı arızasıyla kesiliyor.
    let mut rows = Vec::new();
    for (idx, part) in parts.iter().enumerate() {
        let mut attempt = 0;
        let moved = loop {
            attempt += 1;
            match crate::staging::promote(s, part, wfe_id).await {
                Ok(()) => break Ok(()),
                Err(e) if attempt < 3 => {
                    tracing::warn!(
                        %wfe_id, grup = %part.grp, slot = %part.item, attempt,
                        "dosya taşıma denemesi başarısız, tekrar denenecek: {}", e.message
                    );
                }
                Err(e) => break Err(e),
            }
        };
        match moved {
            Ok(()) => rows.push(crate::wfe_attachment::AttachmentRow {
                wfe_id,
                grp: part.grp.clone(),
                item: part.item.clone(),
                storage_key: format!("attachments/{wfe_id}/{}/{}", part.grp, part.item),
                filename: None,
                content_type: part
                    .content_type
                    .clone()
                    .unwrap_or_else(|| "application/octet-stream".into()),
                size_bytes: part.size_bytes as i64,
                sha256: part.sha256.clone(),
                uploaded_by: actor.user_id,
            }),
            Err(e) => {
                // Bu noktaya kadar BAŞARIYLA taşınmış olanlar metadata'ya yazılır (depo
                // gerçeği zaten değişti, `uploaded` gerçeğinin kaynağı DEPOdur — bkz.
                // `crate::attachments` modül başlığı). Kalanı (bu parça dahil, henüz
                // staging'de) `discard` temizler.
                if let Err(ie) = crate::wfe_attachment::insert_many(&s.pool, &rows).await {
                    tracing::warn!(%wfe_id, "attachment metadata yazılamadı: {}", ie.message);
                }
                crate::staging::discard(s, &parts[idx..]).await;
                return Err(AppError(
                    format!(
                        "belge {}/{} depoya taşınamadı: {}",
                        part.grp, part.item, e.message
                    ),
                    StatusCode::BAD_GATEWAY,
                ));
            }
        }
    }
    if let Err(e) = crate::wfe_attachment::insert_many(&s.pool, &rows).await {
        tracing::warn!(%wfe_id, "attachment metadata yazılamadı: {}", e.message);
    }

    Ok(UploadMultiResponse {
        uploaded: rows
            .iter()
            .map(|r| UploadedItem {
                group: r.grp.clone(),
                item: r.item.clone(),
                size_bytes: r.size_bytes as u64,
                sha256: r.sha256.clone(),
            })
            .collect(),
    })
}

#[utoipa::path(put, path = "/{id}/attachments", tag = "attachments",
    params(("id" = Uuid, Path, description = "WFE id")),
    request_body(content = String, description = "multipart/form-data — alan adları `{grup}/{slot}`; `payload` part'ı YOK (aksiyonsuz)"),
    responses(
        (status = 200, description = "Yükleme sonucu", body = serde_json::Value),
        (status = 422, description = "Bir veya daha fazla dosya reddedildi (attachment.rejected)"),
    ),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn upload_multi(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(wfe_id): Path<Uuid>,
    mp: Multipart,
) -> Result<Json<UploadMultiResponse>, AppError> {
    let actor = super::wfe::extract_actor(&headers)?;
    upload_multi_shared(&s, &actor, wfe_id, mp).await.map(Json)
}
