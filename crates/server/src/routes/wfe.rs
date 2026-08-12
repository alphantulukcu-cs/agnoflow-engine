use utoipa_axum::router::OpenApiRouter;
use crate::{error::AppError, state::AppState};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::routes;
use uuid::Uuid;
use wf_wfe::executor::BranchView;
use wfe_core::types::actor::Actor;
use wfe_core::types::wfah::Wfah;
use wfe_core::v22::matcher::{authorize, MatchEnv};
use wfe_core::v22::ports::{BranchState, BranchStatus, WfdStore};

/// Ortam ADINI tenant'ın kayıtlı ortamlarına çözer. Bilinmeyen ad 422 — serbest metin
/// kabul edilmez, bir tipo sessizce yeni bir ortam yaratmaz. `None` = tenant varsayılanı
/// (executor tarafında çözülür).
async fn resolve_environment_id(
    s: &AppState,
    actor: &Actor,
    name: Option<&str>,
) -> Result<Option<uuid::Uuid>, AppError> {
    let Some(name) = name else { return Ok(None) };
    let orgtnt_id = s
        .executor
        .org
        .orgtnt_for_orgu(actor.orgu_id)
        .await
        .map_err(AppError::from)?;
    wf_wfe::repo::env::resolve_environment(&s.pool, orgtnt_id, Some(name))
        .await
        .map(|e| Some(e.id))
        .map_err(|e| {
            AppError(
                e.to_string(),
                axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            )
        })
}

pub fn router(state: AppState) -> OpenApiRouter {
    OpenApiRouter::new()
        .routes(routes!(start_wfe, list_wfe))
        .routes(routes!(preflight_wfe))
        .routes(routes!(apply_action))
        .routes(routes!(query_wfe))
        .routes(routes!(claim_wfe))
        .routes(routes!(reassign_wfe))
        .routes(routes!(fire_escalation))
        .routes(routes!(skip_escalation))
        .routes(routes!(possible_actions))
        .merge(super::attachments::routes())
        .merge(super::notes::routes())
        .with_state(state)
}

pub(crate) fn extract_actor(headers: &HeaderMap) -> Result<Actor, AppError> {
    let orgu_id = parse_uuid_header(headers, "x-actor-orgu")?;
    let user_id = parse_uuid_header(headers, "x-actor-user")?;
    let role = headers
        .get("x-actor-role")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            AppError(
                "X-Actor-Role header required".into(),
                StatusCode::BAD_REQUEST,
            )
        })?
        .to_string();
    Ok(Actor {
        orgu_id,
        user_id,
        role,
    })
}

fn parse_uuid_header(headers: &HeaderMap, name: &str) -> Result<Uuid, AppError> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| {
            AppError(
                format!("{name} header required (UUID)"),
                StatusCode::BAD_REQUEST,
            )
        })
}

#[derive(Deserialize, ToSchema)]
struct StartBody {
    wfd_id: Uuid,
    version: i32,
    /// M16: start aksiyonları gerçek ad taşır — verilirse yalnız bu action adını
    /// taşıyan start kuralları aday olur; verilmezse tüm start kuralları denenir.
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    input: Value,
    /// SLA-3 (2026-07-16): opsiyonel ISO 8601 duration, start anından itibaren.
    /// WFD `timeout` tanımlıysa tavan olarak uygulanır (aşarsa InvalidInput).
    #[serde(default)]
    deadline: Option<String>,
    /// Koşum ortamı ADI (`test` | `prod` | ...). Verilmezse tenant'ın varsayılanı.
    /// Örnek ömrü boyunca SABİTLENİR. Çağıran yalnız adı seçer, değerleri değil.
    #[serde(default)]
    environment: Option<String>,
}

/// Aktör bu WFD'yi (istenirse bu aksiyonla) başlatabiliyor mu? Simetrik start: initiator
/// yetkisi start kuralının `from` node'unun c_a'sında yaşar — `Engine::start`'ın kural
/// seçimindeki testin AYNISI, yalnız daha erken sorulur.
///
/// Neden rezervasyonda: yetkisiz aktörün dosyaları da depoya yazılıyordu. Kapı yalnız
/// `POST /wfe`de olduğu için sıra "rezerve → YÜKLE → 403" biçiminde işliyor, akış hiç
/// başlamıyor ama belgeler TTL dolana kadar depoda kalıyordu. Kapı yükleme anahtarının
/// verildiği yere alınır: yetkisiz aktör wfe_id ALAMAZ, dolayısıyla yazacak yeri de olmaz.
pub(crate) async fn assert_can_start(
    s: &AppState,
    wfd: &wfe_core::types::wfd_v22::Wfd,
    actor: &Actor,
    orgtnt_id: Uuid,
    action: Option<&str>,
) -> Result<(), AppError> {
    let empty_ctx = serde_json::json!({});
    let empty_wfah = Wfah::empty();
    for rule in &wfd.start {
        if let Some(a) = action {
            if rule.action != a {
                continue;
            }
        }
        let Some(node) = wfd.nodes.get(&rule.from) else {
            continue;
        };
        let env = MatchEnv {
            ctx: &empty_ctx,
            wfah: &empty_wfah,
            orgtnt_id,
        };
        if authorize(&node.c_a, actor, env, &*s.executor.org).await? {
            return Ok(());
        }
    }
    // `EngineError::StartNotEligible` ile aynı statü — istemci için ayrım yok.
    Err(AppError(
        "bu akışı başlatma yetkiniz yok".into(),
        StatusCode::FORBIDDEN,
    ))
}

/// `POST /wfe` — İKİ gövde biçimi kabul eder, ayrım `Content-Type`'tadır:
///
/// - `application/json` → belge İSTEMEYEN başlatma (rezervasyon uçları 2026-08-11'de
///   kaldırıldı; belge isteyen akış multipart göndermek ZORUNDA).
/// - `multipart/form-data` → tek istekte başlatma (2026-08-11): `payload` part'ı JSON
///   gövdedir, kalan part'lar `{grup}/{slot}` adıyla dosyalardır.
///
/// Neden aynı rota: iki biçim AYNI işi yapar (bir WFE başlatır) ve aynı cevabı döner;
/// ayrı bir yol açmak istemciyi "hangi ucu çağırayım" sorusuyla baş başa bırakırdı.
/// Eski istemciler aynen çalışır (K9).
#[utoipa::path(post, path = "/", tag = "wfe",
    request_body = StartBody,
    responses((status = 200, description = "Başlatılan WFE (WfeStartResult)", body = serde_json::Value)),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn start_wfe(
    State(s): State<AppState>,
    req: axum::extract::Request,
) -> Result<axum::response::Response, AppError> {
    use axum::extract::FromRequest;
    use axum::response::IntoResponse;

    let headers = req.headers().clone();
    let is_multipart = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.starts_with("multipart/form-data"))
        .unwrap_or(false);

    if is_multipart {
        let mp = axum::extract::Multipart::from_request(req, &s)
            .await
            .map_err(|e| AppError(format!("multipart gövde okunamadı: {e}"), StatusCode::BAD_REQUEST))?;
        return start_multipart(s, headers, mp).await;
    }

    let Json(body) = Json::<StartBody>::from_request(req, &s)
        .await
        .map_err(|e| AppError(e.body_text(), StatusCode::BAD_REQUEST))?;
    Ok(start_json(s, headers, body).await?.into_response())
}

async fn start_json(
    s: AppState,
    headers: HeaderMap,
    body: StartBody,
) -> Result<Json<wf_wfe::executor::WfeStartResult>, AppError> {
    let actor = extract_actor(&headers)?;
    let environment_id = resolve_environment_id(&s, &actor, body.environment.as_deref()).await?;

    // Başlatma aksiyonunun ek-belge kapısı. Engine'e GİTMEDEN kontrol edilir: eksikse
    // WFE hiç oluşmaz (rezervasyon durur, kullanıcı eksiği yükleyip tekrar dener).
    let wfd = s
        .wfd
        .fetch(body.wfd_id, body.version)
        .await
        .map_err(AppError::from)?;
    if let Some((node, action)) = start_gate_target(&wfd, body.action.as_deref()) {
        let gated = wfd
            .nodes
            .get(&node)
            .map(|n| n.attachments.iter().any(|a| a.gates_action(Some(&action))))
            .unwrap_or(false);
        if gated {
            // JSON gövdesi dosya TAŞIYAMAZ ve `wfe_id` dışarıdan alınamaz (rezervasyon
            // uçları 2026-08-11'de kaldırıldı, id'yi DAİMA engine üretir). Dolayısıyla
            // belge isteyen bir başlatma bu yoldan yapılamaz — tek yol multipart.
            return Err(AppError {
                message: "bu akış başlatma için belge istiyor: dosyaları multipart/form-data gövdesiyle aynı istekte gönderin".into(),
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: Some("attachment.multipart_required"),
                items: None,
            });
        }
    }

    // JSON yolunda rezervasyon YOKTUR: id engine tarafında üretilir (`start_in`).
    let result = s
        .executor
        .start_in(
            body.wfd_id,
            body.version,
            &actor,
            body.action.as_deref(),
            &body.input,
            body.deadline.as_deref(),
            environment_id,
        )
        .await
        .map_err(AppError::from)?;
    Ok(Json(result))
}

// ---------------------------------------------------------------- tek istekte başlatma

/// `payload` part'ının gövdesi — `StartBody`'nin multipart karşılığı. `wfe_id` alanı
/// YOKTUR: id bu yolda sunucuda doğar, dışarıdan verilmesinin anlamı olmaz.
#[derive(Deserialize, ToSchema)]
struct MultipartPayload {
    wfd_id: Uuid,
    version: i32,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    input: Value,
    #[serde(default)]
    deadline: Option<String>,
    #[serde(default)]
    environment: Option<String>,
    /// Dosya BİLDİRİMİ (opsiyonel). Baytlar part'larda gelir; burada yalnız bütünlük
    /// beyanı (`sha256`) ve ileride staging tutamağı (`upload_id`, Faz 3) taşınır.
    /// Dedupe parmak izine de girer — aynı girdiyle farklı belge gönderen ikinci istek
    /// böylece tekrar sayılmaz.
    #[serde(default)]
    attachments: Option<Vec<PayloadAttachment>>,
}

#[derive(Deserialize, Serialize, ToSchema, Clone)]
struct PayloadAttachment {
    group: String,
    item: String,
    /// Verilirse yazarken hesaplanan özetle karşılaştırılır — yarım/bozuk yüklenmiş
    /// dosya sessizce kabul edilmesin.
    #[serde(default)]
    sha256: Option<String>,
    /// Faz 3 (staging handle). Bugün verilirse `501` — sözleşme şimdiden tanımlı
    /// olsun ki istemci biçimi sonradan kırılmasın.
    #[serde(default)]
    upload_id: Option<String>,
}

/// Slot bazında ret. Tek `error` metni N dosyanın hangisinin neden reddedildiğini
/// anlatamıyordu; gövdeye `items` olarak eklenir (bkz. `AppError::items`).
#[derive(Serialize)]
struct ItemError {
    group: String,
    item: String,
    code: &'static str,
    message: String,
}

fn rejected(items: Vec<ItemError>) -> AppError {
    AppError {
        message: format!("{} belge reddedildi", items.len()),
        status: StatusCode::UNPROCESSABLE_ENTITY,
        code: Some("attachment.rejected"),
        items: serde_json::to_value(items).ok(),
    }
}

/// Tek istekte başlatma (2026-08-11 tasarımı, Faz 1).
///
/// Sıra ve gerekçeleri:
/// 1. `payload` İLK part olmak zorunda — yetki kararı baytlardan ÖNCE verilebilsin
///    (aksi hâlde yetkisiz isteğin 200 MB'ını okuyup sonunda 403 demek gerekirdi).
/// 2. Dedupe parmak izi de aynı yerde sorulur: tekrar istek baytları göndermeden yanıtlanır.
/// 3. Dosyalar `attachments/{wfe_id}/…` altına AKIŞ halinde yazılır — bellek kullanımı
///    dosya sayısından ve boyutundan bağımsızdır.
/// 4. HER hata yolunda yazılanlar silinir ve rezervasyon/dedupe satırı bırakılır:
///    istemci hiçbir telafi çağrısı yapmaz (tasarım K4).
async fn start_multipart(
    s: AppState,
    headers: HeaderMap,
    mut mp: axum::extract::Multipart,
) -> Result<axum::response::Response, AppError> {
    use axum::response::IntoResponse;

    let actor = extract_actor(&headers)?;

    // --- 1. payload (İLK part) --------------------------------------------------
    let first = mp
        .next_field()
        .await
        .map_err(|e| AppError(format!("multipart okunamadı: {e}"), StatusCode::BAD_REQUEST))?
        .ok_or_else(|| AppError("multipart gövdesi boş".into(), StatusCode::BAD_REQUEST))?;
    if first.name() != Some("payload") {
        return Err(AppError {
            message: "ilk part 'payload' (JSON) olmalı — yetki ve kapı kararı dosyalardan önce verilir".into(),
            status: StatusCode::BAD_REQUEST,
            code: Some("multipart.payload_first"),
            items: None,
        });
    }
    let raw = first
        .bytes()
        .await
        .map_err(|e| AppError(format!("payload okunamadı: {e}"), StatusCode::BAD_REQUEST))?;
    let payload: MultipartPayload = serde_json::from_slice(&raw)
        .map_err(|e| AppError(format!("payload JSON'u geçersiz: {e}"), StatusCode::BAD_REQUEST))?;


    // --- 2. yetki: bayt okumadan --------------------------------------------------
    let orgtnt_id = s
        .executor
        .org
        .orgtnt_for_orgu(actor.orgu_id)
        .await
        .map_err(AppError::from)?;
    let wfd = s
        .wfd
        .fetch(payload.wfd_id, payload.version)
        .await
        .map_err(AppError::from)?;
    assert_can_start(&s, &wfd, &actor, orgtnt_id, payload.action.as_deref()).await?;

    // --- 3. dedupe: tekrar istek baytlardan önce yanıtlanır ------------------------
    let att_json = serde_json::to_value(&payload.attachments).unwrap_or(Value::Null);
    let fp = crate::start_dedupe::fingerprint(
        actor.user_id,
        payload.wfd_id,
        payload.version,
        payload.action.as_deref(),
        &payload.input,
        &att_json,
    );
    match crate::start_dedupe::claim(&s.pool, &fp, actor.user_id, s.cfg.dedupe_window_secs).await? {
        crate::start_dedupe::Claim::Replay(wfe_id) => return replay_response(&s, &actor, wfe_id).await,
        crate::start_dedupe::Claim::InProgress => {
            return Err(AppError {
                message: "aynı başlatma isteği şu anda işleniyor".into(),
                status: StatusCode::CONFLICT,
                code: Some("conflict.start_in_progress"),
                items: None,
            })
        }
        crate::start_dedupe::Claim::Fresh => {}
    }

    // Buradan SONRAKİ her çıkış yolu dedupe satırını bırakmalı — aksi hâlde başarısız
    // bir deneme, düzeltilmiş tekrar denemeyi `DEDUPE_WINDOW` boyunca bloklardı.
    let out = start_multipart_committed(&s, &actor, orgtnt_id, &wfd, payload, mp).await;
    match out {
        Ok(result) => {
            crate::start_dedupe::complete(&s.pool, &fp, result.wfe_id).await?;
            Ok(Json(result).into_response())
        }
        Err(e) => {
            if let Err(re) = crate::start_dedupe::release(&s.pool, &fp).await {
                tracing::warn!("dedupe satırı bırakılamadı: {}", re.message);
            }
            Err(e)
        }
    }
}

/// Dosyaları yazar, kapıyı kontrol eder, WFE'yi başlatır. Hata hâlinde YAZILAN HER ŞEYİ
/// geri alır (dosyalar + rezervasyon satırı) — çağıran yalnız dedupe satırını bırakır.
async fn start_multipart_committed(
    s: &AppState,
    actor: &Actor,
    orgtnt_id: Uuid,
    wfd: &wfe_core::types::wfd_v22::Wfd,
    payload: MultipartPayload,
    mp: axum::extract::Multipart,
) -> Result<wf_wfe::executor::WfeStartResult, AppError> {
    let environment_id = resolve_environment_id(s, actor, payload.environment.as_deref()).await?;

    // Rezervasyon satırı İSTEMCİYE GÖRÜNMEZ (wfe_id ancak başarıda cevaba girer). Tek
    // işlevi süreç isteğin ortasında ölürse (deploy/OOM/kill) yazılmış baytların
    // sahibini süpürücüye bildirmektir — K4 yalnız süreç yaşarken geçerlidir (K5).
    let reservation = crate::reservation::Reservation {
        wfe_id: Uuid::new_v4(),
        orgtnt_id,
        wfd_id: payload.wfd_id,
        wfd_version: payload.version,
        environment_id,
        actor_orgu_id: actor.orgu_id,
        actor_user_id: actor.user_id,
    };
    crate::reservation::create(&s.pool, &reservation).await?;

    let out = write_and_start(s, actor, orgtnt_id, wfd, &payload, mp, &reservation).await;
    if out.is_err() {
        // K4: dosyalar + defter satırı gider. Silme de patlarsa saatlik süpürücü toplar
        // (satır duruyor, yani sahipsiz kalmıyor) — asıl hatayı GÖLGELEMEYELİM.
        if let Err(ce) = crate::reservation::release(s, &reservation).await {
            tracing::warn!(wfe_id = %reservation.wfe_id, "başarısız başlatma temizlenemedi: {}", ce.message);
        }
    }
    out
}

async fn write_and_start(
    s: &AppState,
    actor: &Actor,
    orgtnt_id: Uuid,
    wfd: &wfe_core::types::wfd_v22::Wfd,
    payload: &MultipartPayload,
    mut mp: axum::extract::Multipart,
    reservation: &crate::reservation::Reservation,
) -> Result<wf_wfe::executor::WfeStartResult, AppError> {
    let wfe_id = reservation.wfe_id;
    let store =
        crate::attachment_store::store_for_wfd_strict(
            s,
            payload.wfd_id,
            orgtnt_id,
            reservation.environment_id,
        )
        .await?;

    // Kapı hedefi + o aksiyonu kapayan grupların katalogu. Aynı çözüm preflight'ta da
    // var; İKİSİ AYNI SORUYU SORAR (hangi slotlar bu başlatmaya ait).
    let gate = start_gate_target(wfd, payload.action.as_deref());
    let mut catalog: std::collections::HashMap<
        (String, String),
        &wfe_core::types::wfd_v22::AttachmentItem,
    > = std::collections::HashMap::new();
    if let Some((node_key, action)) = &gate {
        if let Some(node) = wfd.nodes.get(node_key) {
            for aref in &node.attachments {
                if !aref.gates_action(Some(action)) {
                    continue;
                }
                if let Some(group) = wfd.attachments.get(aref.group()) {
                    for item in &group.items {
                        catalog.insert((aref.group().to_string(), item.id.clone()), item);
                    }
                }
            }
        }
    }

    let declared: std::collections::HashMap<(String, String), &PayloadAttachment> = payload
        .attachments
        .iter()
        .flatten()
        .map(|a| ((a.group.clone(), a.item.clone()), a))
        .collect();

    let max_request = s.cfg.attachment_max_request_mb as usize * 1024 * 1024;
    let mut request_total = 0usize;
    let mut errors: Vec<ItemError> = Vec::new();
    // K7 metadata satırları: `start_reserved` BAŞARILI olduktan SONRA yazılır (FK
    // `wf.wfe`ye bağlı — WFE henüz yok, önce yazılamaz). Toplanır, en sonda tek seferde
    // `wfe_attachment::insert_many`ye verilir.
    let mut attachment_rows: Vec<crate::wfe_attachment::AttachmentRow> = Vec::new();

    while let Some(mut field) = mp
        .next_field()
        .await
        .map_err(|e| AppError(format!("multipart okunamadı: {e}"), StatusCode::BAD_REQUEST))?
    {
        let Some(name) = field.name().map(str::to_string) else {
            continue;
        };
        // Part adı `{grup}/{slot}`. Dosya ADI anahtara KARIŞMAZ — karışsaydı istemcinin
        // verdiği ad depoda yol enjeksiyonu yüzeyi olurdu.
        let Some((group, item)) = name.split_once('/').map(|(g, i)| (g.to_string(), i.to_string()))
        else {
            errors.push(ItemError {
                group: name.clone(),
                item: String::new(),
                code: "unknown_slot",
                message: format!("part adı '{name}' '{{grup}}/{{slot}}' biçiminde değil"),
            });
            continue;
        };
        let Some(def) = catalog.get(&(group.clone(), item.clone())).copied() else {
            errors.push(ItemError {
                group,
                item,
                code: "unknown_slot",
                message: "bu başlatma aksiyonu için tanımlı bir dosya slotu değil".into(),
            });
            continue;
        };

        let declared_ct = field.content_type().map(str::to_string);
        // Dosya adı yalnız METADATA'dır, storage anahtarına karışmaz (yol sabit:
        // attachments/{wfe_id}/{grup}/{item}). Yine de DB'de bir sonraki okuyucuya
        // (Content-Disposition, gösterim) çıplak gitmesin diye `notes` modülündeki aynı
        // kuralla temizlenir — iki dosya-yolu (not/ek-belge) aynı ada aynı güvenle bakar.
        let filename = field
            .file_name()
            .map(|f| crate::notes::sanitize_filename(&crate::notes::decode_filename(f)));
        // Tip kapısı baytlardan ÖNCE: uzunluk 0 verilir, `check_upload` önce tipe göre
        // kuralı seçer, boyutu SONRA denetler — böylece reddedilecek dosya hiç yazılmaz.
        if let Err(crate::attachments::UploadReject::UnsupportedType(ct)) =
            crate::attachments::check_upload(def, declared_ct.as_deref(), 0)
        {
            errors.push(ItemError {
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

        let slot_cap = slot_cap_bytes(def);
        let mut writer = store.writer(wfe_id, &group, &item).await.map_err(|e| {
            AppError(
                format!("yükleme başlatılamadı: {e}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;
        let mut hasher = crate::attachments::Sha256Stream::new();
        let mut head: Vec<u8> = Vec::with_capacity(64);
        let mut total = 0usize;
        let mut overflow: Option<&'static str> = None;

        while let Some(chunk) = field.chunk().await.map_err(|e| {
            AppError(format!("dosya akışı kesildi: {e}"), StatusCode::BAD_REQUEST)
        })? {
            total += chunk.len();
            request_total += chunk.len();
            if head.len() < 64 {
                let take = std::cmp::min(64 - head.len(), chunk.len());
                head.extend_from_slice(&chunk[..take]);
            }
            if slot_cap.is_some_and(|c| total > c) {
                overflow = Some("too_large");
                break;
            }
            if request_total > max_request {
                overflow = Some("request_too_large");
                break;
            }
            hasher.update(&chunk);
            writer.write(chunk).await.map_err(|e| {
                AppError(format!("yükleme başarısız: {e}"), StatusCode::INTERNAL_SERVER_ERROR)
            })?;
        }

        if let Some(kind) = overflow {
            // `close` ETMİYORUZ: yarım nesne tamamlanmasın (S3'te multipart upload abort edilir).
            let _ = writer.abort().await;
            if kind == "request_too_large" {
                return Err(AppError(
                    format!(
                        "istek toplamı {} MB sınırını aşıyor",
                        s.cfg.attachment_max_request_mb
                    ),
                    StatusCode::PAYLOAD_TOO_LARGE,
                ));
            }
            errors.push(ItemError {
                group,
                item,
                code: "too_large",
                message: format!("dosya {} MB sınırını aşıyor", slot_max_size_mb(def).unwrap_or(0)),
            });
            continue;
        }
        writer.close().await.map_err(|e| {
            AppError(format!("yükleme kapatılamadı: {e}"), StatusCode::INTERNAL_SERVER_ERROR)
        })?;

        if total == 0 {
            errors.push(ItemError { group, item, code: "empty", message: "boş dosya yüklenemez".into() });
            continue;
        }
        // Boyut kuralı ARTIK kesin uzunlukla: `check_upload` tipe göre doğru kuralı seçer.
        if let Err(crate::attachments::UploadReject::TooLarge(mb)) =
            crate::attachments::check_upload(def, declared_ct.as_deref(), total)
        {
            errors.push(ItemError {
                group,
                item,
                code: "too_large",
                message: format!("dosya {mb} MB sınırını aşıyor"),
            });
            continue;
        }
        // İçerik beyanı YALAN mı (magic-byte)? `.exe`nin `application/pdf` diye geçmesi
        // bu denetimin kapattığı asıl senaryodur.
        if let Some(crate::attachments::UploadReject::TypeMismatch { declared: decl, detected }) =
            crate::attachments::detect_mismatch(declared_ct.as_deref(), &head)
        {
            errors.push(ItemError {
                group,
                item,
                code: "type_mismatch",
                message: format!("içerik beyan edilen tiple uyuşmuyor: {decl} denildi, {detected} bulundu"),
            });
            continue;
        }
        let digest = hasher.finish();
        if let Some(expected) = declared
            .get(&(group.clone(), item.clone()))
            .and_then(|d| d.sha256.as_deref())
        {
            if !expected.eq_ignore_ascii_case(&digest) {
                errors.push(ItemError {
                    group,
                    item,
                    code: "checksum_mismatch",
                    message: "bildirilen sha256 ile yüklenen içerik uyuşmuyor".into(),
                });
                continue;
            }
        }
        // Dosya kesin olarak kabul edildi. Metadata satırı burada TOPLANIR, yazılmaz —
        // FK `wf.wfe`ye bağlı ve WFE bu noktada henüz yok (bkz. fonksiyon sonu).
        let storage_key = format!("attachments/{wfe_id}/{group}/{item}");
        attachment_rows.push(crate::wfe_attachment::AttachmentRow {
            wfe_id,
            grp: group,
            item,
            storage_key,
            filename,
            content_type: declared_ct.unwrap_or_else(|| "application/octet-stream".into()),
            size_bytes: total as i64,
            sha256: digest,
            uploaded_by: actor.user_id,
        });
    }

    // Staging tutamakları (Faz 3, K8): baytlar bu isteğe HİÇ girmedi, önceden
    // `POST /uploads` ile depoya kondu. Burada yalnız sahiplik+varlık doğrulanır ve
    // nesne server-side COPY ile nihai anahtara taşınır — indirip yeniden yükleme yok.
    // Part'lardan SONRA işlenir: aynı slot hem part hem handle ile gelirse taşıma
    // part'ın yazdığının üzerine yazar, son söz açıkça bildirilen handle'ındır.
    for decl in payload.attachments.iter().flatten() {
        let Some(upload_id) = decl.upload_id.as_deref() else {
            continue;
        };
        let Ok(uid) = Uuid::parse_str(upload_id) else {
            errors.push(ItemError {
                group: decl.group.clone(),
                item: decl.item.clone(),
                code: "upload_not_found",
                message: format!("geçersiz upload_id: {upload_id}"),
            });
            continue;
        };
        if !catalog.contains_key(&(decl.group.clone(), decl.item.clone())) {
            errors.push(ItemError {
                group: decl.group.clone(),
                item: decl.item.clone(),
                code: "unknown_slot",
                message: "bu başlatma aksiyonu için tanımlı bir dosya slotu değil".into(),
            });
            continue;
        }
        match crate::staging::take(s, uid, actor, wfe_id).await {
            Ok(taken) => attachment_rows.push(crate::wfe_attachment::AttachmentRow {
                wfe_id,
                grp: taken.grp.clone(),
                item: taken.item.clone(),
                storage_key: format!("attachments/{wfe_id}/{}/{}", taken.grp, taken.item),
                filename: None,
                // Staging'de tip beyanı olmayabilir (presigned PUT'ta istemci başlık
                // göndermemiş olabilir) — katalog kapısı zaten `POST /uploads`ta koştu.
                content_type: taken
                    .content_type
                    .unwrap_or_else(|| "application/octet-stream".into()),
                size_bytes: taken.size_bytes as i64,
                sha256: taken.sha256,
                uploaded_by: actor.user_id,
            }),
            Err(e) => errors.push(ItemError {
                group: decl.group.clone(),
                item: decl.item.clone(),
                code: "upload_not_found",
                message: e.message,
            }),
        }
    }

    if !errors.is_empty() {
        return Err(rejected(errors));
    }

    // Kapı: zorunlu slotlar tam mı. Eski yoldan farklı olarak burada "tekrar dene"
    // diye bir ara durum YOK — dosyalar istemcinin elinde, isteği aynen tekrarlar.
    if let Some((node, action)) = &gate {
        let gated = wfd
            .nodes
            .get(node)
            .map(|n| n.attachments.iter().any(|a| a.gates_action(Some(action))))
            .unwrap_or(false);
        if gated {
            let groups =
                crate::attachments::status_for_node(&store, wfd, wfe_id, node, Some(action))
                    .await
                    .map_err(|e| {
                        AppError(
                            format!("attachment durum sorgusu başarısız: {e}"),
                            StatusCode::INTERNAL_SERVER_ERROR,
                        )
                    })?;
            let missing = crate::attachments::missing_required(&groups);
            if !missing.is_empty() {
                return Err(AppError {
                    message: format!("Eksik zorunlu belgeler: {}", missing.join(", ")),
                    status: StatusCode::UNPROCESSABLE_ENTITY,
                    code: Some("attachment.missing"),
                    items: None,
                });
            }
        }
    }

    let result = s
        .executor
        .start_reserved(
            payload.wfd_id,
            payload.version,
            actor,
            payload.action.as_deref(),
            &payload.input,
            payload.deadline.as_deref(),
            reservation.environment_id,
            Some(wfe_id),
        )
        .await
        .map_err(AppError::from)?;

    // WFE artık gerçek — FK bu satırların yazılmasına artık izin verir. Hata burada
    // WFE'yi GERİ ALMAZ: akış zaten başarıyla başladı, dosyalar depoda gerçekten duruyor;
    // yalnızca denetim/gösterim katmanı olan metadata'nın yazılamaması yüzünden başarıyla
    // başlamış bir akışı iptal etmek (ya da istemciye hata dönmek) daha büyük zarardır.
    if let Err(e) = crate::wfe_attachment::insert_many(&s.pool, &attachment_rows).await {
        tracing::warn!(wfe_id = %wfe_id, "ek-belge metadata'sı yazılamadı: {}", e.message);
    }

    crate::reservation::delete(&s.pool, wfe_id).await?;
    Ok(result)
}

/// Dedupe penceresi içinde tekrarlanan istek: iş TEKRAR KOŞMAZ, ilk WFE'nin bugünkü
/// durumu döner. `current_c_a` ve `end_response` yeniden kurulamaz (start anının
/// çıktısıdır, sorgu görüşü onu taşımaz) — bu yüzden cevaba `Idempotent-Replay: true`
/// konur: istemci elindekinin yeni bir başlatmanın çıktısı DEĞİL, bir yansıma olduğunu bilir.
async fn replay_response(
    s: &AppState,
    actor: &Actor,
    wfe_id: Uuid,
) -> Result<axum::response::Response, AppError> {
    use axum::response::IntoResponse;
    let view = s.executor.query(wfe_id, actor).await.map_err(AppError::from)?;
    let body = wf_wfe::executor::WfeStartResult {
        wfe_id,
        terminal: !matches!(view.status, wfe_core::types::wfe::WfeStatus::Active),
        current_node: view.current_node.clone(),
        end_response: view.end_response.clone(),
        current_c_a: vec![],
    };
    let mut resp = Json(body).into_response();
    resp.headers_mut()
        .insert("idempotent-replay", axum::http::HeaderValue::from_static("true"));
    Ok(resp)
}

/// Kapının uygulanacağı (node, aksiyon) çifti. Aksiyon verilmişse onu taşıyan start
/// kuralı; verilmemişse TEK start kuralı varsa o. Birden çok kural arasından seçim
/// yapılmaz: hangisinin koşacağını engine belirler, yanlış kuralın kapısını uygulamak
/// olmayan bir belgeyi istemek ya da olan bir kapıyı atlamak olurdu.
fn start_gate_target(
    wfd: &wfe_core::types::wfd_v22::Wfd,
    action: Option<&str>,
) -> Option<(String, String)> {
    let rule = match action {
        Some(a) => wfd.start.iter().find(|r| r.action == a)?,
        None => match wfd.start.as_slice() {
            [only] => only,
            _ => return None,
        },
    };
    Some((rule.from.clone(), rule.action.clone()))
}

#[derive(Deserialize, ToSchema)]
struct PreflightItem {
    group: String,
    item: String,
    /// İstemcinin seçtiği dosyanın DEKLARE ettiği boyutu — sunucu henüz hiçbir bayt
    /// almadı, bu değer doğrulanamaz, yalnız erken bilgilendirme için kullanılır.
    #[serde(default)]
    size_bytes: Option<u64>,
    #[serde(default)]
    content_type: Option<String>,
}

/// `POST /wfe/preflight` gövdesi — `POST /wfe`'nin `payload`ıyla KASITLI olarak aynı
/// alan adlarını taşır: istemci aynı JSON'ı önce buraya, sonra (dosyalar hazırlandıktan
/// sonra) gerçek başlatmaya yollayabilsin. `input` bu yüzden burada durur ama preflight
/// onu DOĞRULAMAZ (ZEN/kural koşumu yok) — yalnız sözleşme simetrisi için kabul edilir.
#[derive(Deserialize, ToSchema)]
struct PreflightBody {
    wfd_id: Uuid,
    version: i32,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    #[allow(dead_code)] // bkz. yukarıdaki doc yorumu: kasıtlı olarak okunmuyor
    input: Option<Value>,
    #[serde(default)]
    attachments: Option<Vec<PreflightItem>>,
}

/// Bir dosya slotunun istemciye görünen yüzü: dosya seçiciyi kurmak için yeterli
/// bilgi (`accept`/`max_size_mb`) + etiketler. Katalogdaki `AttachmentItem`in
/// portala süzülmüş biçimidir — ham katalog tipini dışa vermeyiz, çünkü onun format
/// kuralları (`Vec<AttachmentFormatRule>`) istemcinin ihtiyacından fazlasını taşır.
#[derive(Serialize, ToSchema)]
struct PreflightSlot {
    group: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    group_label: Option<String>,
    item: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    required: bool,
    accept: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_size_mb: Option<u64>,
}

#[derive(Serialize, ToSchema)]
struct PreflightItemError {
    group: String,
    item: String,
    /// Sabit makine-okunur kod (`unknown_slot`/`too_large`/`unsupported_type`) —
    /// istemci METNİ değil BUNU okur (bkz. `AppError::code`daki aynı gerekçe).
    code: String,
    message: String,
}

#[derive(Serialize, ToSchema)]
struct PreflightResult {
    ok: bool,
    slots: Vec<PreflightSlot>,
    items: Vec<PreflightItemError>,
}

/// Bir item'ın format kurallarındaki EN GENİŞ boyut sınırı — istemcinin dosya seçicisi
/// TEK bir sayı ister, katalogta ise kural başına farklı sınır olabilir (örn.
/// pdf/jpg→4MB, xml/zip→20MB). Gerçek denetim yüklenen dosyanın content-type'ına göre
/// EŞLEŞEN kuralı uygular (`check_upload`); burası yalnız keşif/UI özetidir. Herhangi
/// bir kural sınırsızsa (`max_size_mb: None`) ya da hiç kural yoksa slot da sınırsız
/// sayılır — en dar sınırı göstermek istemciyi olması gerekenden erken caydırırdı.
/// Akış sırasında uygulanacak üst sınır, BAYT cinsinden. `slot_max_size_mb`ten ayrı
/// olmasının sebebi: o fonksiyon GÖSTERİM içindir ve `round()` uygular — `max_size_mb: 0.4`
/// gibi MB altı bir kural orada 0'a yuvarlanır. Sınır olarak kullanılsaydı o slota
/// yüklenen HER dosya ilk chunk'ta reddedilirdi. Burada f64 doğrudan bayta çevrilir.
///
/// `None` = sınırsız (kuralların biri bile `max_size_mb` vermiyorsa; `check_upload` da
/// tipe uyan kuralı seçtiği için nihai karar orada verilir — bu yalnız akışı korur).
fn slot_cap_bytes(item: &wfe_core::types::wfd_v22::AttachmentItem) -> Option<usize> {
    if item.formats.is_empty() {
        return None;
    }
    let mut max = 0.0_f64;
    for rule in &item.formats {
        match rule.max_size_mb {
            Some(mb) => max = max.max(mb),
            None => return None,
        }
    }
    Some((max * 1024.0 * 1024.0).ceil() as usize)
}

fn slot_max_size_mb(item: &wfe_core::types::wfd_v22::AttachmentItem) -> Option<u64> {
    if item.formats.is_empty() {
        return None;
    }
    let mut max = 0.0_f64;
    for rule in &item.formats {
        match rule.max_size_mb {
            Some(mb) => max = max.max(mb),
            None => return None,
        }
    }
    Some(max.round() as u64)
}

/// Baytlar yola çıkmadan önce yetki + slot kurallarını sorar. **KAPI DEĞİLDİR** —
/// `ok: true` dese bile gerçek denetim `POST /wfe` (ya da `.../actions`) içinde
/// YENİDEN koşar: preflight ile gerçek başlatma arasında durum değişmiş olabilir,
/// istemci preflight'ı hiç çağırmadan da doğrudan başlatabilir. Bu yüzden hata
/// bulunsa (`items` doluysa) bile HTTP durumu DAİMA 200'dür; `ok`/`items` istemcinin
/// UI'da göstereceği bir ÖN bilgidir, sunucu tarafından zorlanan bir karar değil.
///
/// Üç işi birden görür (bkz. tasarım dokümanı "POST /wfe/preflight"): (1) yetkisiz
/// aktörü dosyalar hiç yola çıkmadan TEMİZ bir 403 ile bilgilendirir — K2 sunucu
/// tarafında zaten bu korumayı veriyordu, ama tarayıcı büyük bir gövdeyi yollarken
/// gelen erken 403'ü çoğu zaman ağ hatasına çeviriyordu; burada gövde hiç yok, o
/// risk de yok. (2) istemcinin bildirdiği boyut/tip'i katalogla erkenden karşılaştırır.
/// (3) `accept`/`max_size_mb` gibi slot kurallarını TEK kaynaktan (katalog) sunar —
/// bugün portal bunu WFD dokümanını kendisi ayrıştırarak yapıyordu.
#[utoipa::path(post, path = "/preflight", tag = "wfe",
    request_body = PreflightBody,
    responses(
        (status = 200, description = "Ön kontrol sonucu (kapı DEĞİLDİR)", body = PreflightResult),
        (status = 403, description = "Aktör bu akışı başlatamaz"),
    ),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn preflight_wfe(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<PreflightBody>,
) -> Result<Json<PreflightResult>, AppError> {
    let actor = extract_actor(&headers)?;
    let orgtnt_id = s
        .executor
        .org
        .orgtnt_for_orgu(actor.orgu_id)
        .await
        .map_err(AppError::from)?;

    // start_wfe'deki desenin AYNISI: WFD gerçekten var mı, aktör bu (isteğe bağlı
    // aksiyonla daraltılmış) akışı başlatabiliyor mu. Yetkisizse burada 403 döner —
    // preflight'ın asıl amacı bu erken cevaptır (bkz. fonksiyon doc yorumu).
    let wfd = s
        .wfd
        .fetch(body.wfd_id, body.version)
        .await
        .map_err(AppError::from)?;
    assert_can_start(&s, &wfd, &actor, orgtnt_id, body.action.as_deref()).await?;

    // Kapı hedefi bulunamadıysa (aksiyon belirsiz ya da bu WFD'de start ek-belge
    // istemiyor) gösterilecek slot yok — bu bir HATA değildir, `ok: true` + boş liste.
    let Some((node_key, action)) = start_gate_target(&wfd, body.action.as_deref()) else {
        return Ok(Json(PreflightResult {
            ok: true,
            slots: vec![],
            items: vec![],
        }));
    };

    // Node'un attachment referanslarından YALNIZ bu aksiyonu kapıyanları çöz —
    // `gates_action` çapasız/kapsam dışı referansları eler (aynı süzme `status_for_node`
    // ve `apply_action`daki gate kontrolünde de var). Aynı zamanda validasyon için
    // (group,item) → katalog `AttachmentItem`'ına bir bakış tablosu tutulur; `slots`
    // listesi buradan üretilir, `items` denetimi de aynı tablodan okur — iki liste
    // birbirinden SAPAMAZ.
    let mut slots = Vec::new();
    let mut catalog: std::collections::HashMap<(String, String), &wfe_core::types::wfd_v22::AttachmentItem> =
        std::collections::HashMap::new();
    if let Some(node) = wfd.nodes.get(&node_key) {
        for aref in &node.attachments {
            if !aref.gates_action(Some(&action)) {
                continue;
            }
            let group_ref = aref.group();
            let Some(group) = wfd.attachments.get(group_ref) else {
                continue; // validator zaten yakalar; runtime'da sessiz atla (status_for_node ile aynı)
            };
            for item in &group.items {
                catalog.insert((group_ref.to_string(), item.id.clone()), item);
                slots.push(PreflightSlot {
                    group: group_ref.to_string(),
                    group_label: group.label.clone(),
                    item: item.id.clone(),
                    label: item.label.clone(),
                    required: item.required,
                    accept: crate::attachments::all_accept_patterns(item),
                    max_size_mb: slot_max_size_mb(item),
                });
            }
        }
    }

    // İstemci dosya BİLDİRDİYSE (henüz yüklemedi) her biri katalogla erkenden
    // karşılaştırılır. Eşleşme mantığı (`image/*` joker dahil) `crate::attachments::
    // check_upload`ta zaten var — burada KOPYALANMAZ, aynen çağrılır. `size_bytes`
    // verilmemişse 0 varsayılır: bu, boyut denetimini es geçer ama tip denetimini
    // (katalogla eşleşen kuralı bulma) ETKİLEMEZ — `check_upload` önce tipe göre
    // kuralı seçer, boyutu SONRA denetler.
    let mut items = Vec::new();
    if let Some(reqs) = &body.attachments {
        for pi in reqs {
            let Some(item) = catalog.get(&(pi.group.clone(), pi.item.clone())) else {
                items.push(PreflightItemError {
                    group: pi.group.clone(),
                    item: pi.item.clone(),
                    code: "unknown_slot".into(),
                    message: format!(
                        "{}/{} bu aksiyon için tanımlı bir dosya slotu değil",
                        pi.group, pi.item
                    ),
                });
                continue;
            };
            let len = pi.size_bytes.unwrap_or(0) as usize;
            if let Err(reject) =
                crate::attachments::check_upload(item, pi.content_type.as_deref(), len)
            {
                let (code, message) = match reject {
                    crate::attachments::UploadReject::UnsupportedType(ct) => (
                        "unsupported_type",
                        format!(
                            "{ct} desteklenmiyor (izin verilenler: {})",
                            crate::attachments::all_accept_patterns(item).join(", ")
                        ),
                    ),
                    crate::attachments::UploadReject::TooLarge(max_mb) => (
                        "too_large",
                        format!("dosya {max_mb} MB sınırını aşıyor"),
                    ),
                    // Preflight'ta bayt YOKTUR — magic-byte çelişkisi buradan çıkamaz;
                    // kol yalnız `check_upload`'ın reddi tam kapansın diye var. Çelişki
                    // gerçek yüklemede (`POST /wfe`) tespit edilir.
                    crate::attachments::UploadReject::TypeMismatch { declared, detected } => (
                        "type_mismatch",
                        format!("içerik beyan edilen tiple uyuşmuyor: {declared} / {detected}"),
                    ),
                };
                items.push(PreflightItemError {
                    group: pi.group.clone(),
                    item: pi.item.clone(),
                    code: code.into(),
                    message,
                });
            }
        }
    }

    let ok = items.is_empty();
    Ok(Json(PreflightResult { ok, slots, items }))
}

#[derive(Deserialize, ToSchema)]
struct ApplyBody {
    action: String,
    #[serde(default)]
    input: Value,
    /// WOR-31 T4: paralel modda kol seçimi — action ≥2 aktif kolun
    /// transition'ıyla eşleşiyorsa zorunlu (aksi halde `AmbiguousAction`/409).
    /// Değer `possible-actions`taki `branch.id`dir; istemci için OPAKTIR.
    #[serde(default)]
    branch: Option<String>,
    /// GLB (global aksiyon) hedef seçimi — değer `possible-actions`taki
    /// `target.options[].id`dir. `wft: {targets}` taşıyan aksiyonda ZORUNLU
    /// (400 `action.target_required`), diğerlerinde YASAK (400
    /// `action.target_unexpected`). Bir action input DEĞİLDİR: `$ctx`'e yazılmaz,
    /// `$wfah` izdüşümüne girmez.
    #[serde(default)]
    target: Option<String>,
    /// WOR-65: istemcinin okuduğu WFE revizyon token'ı (`GET /wfe/:id` →`rev`,
    /// `GET /wfe` → satır başına `rev`). OPSİYONEL — göndermeyen istemci bugünkü
    /// davranışı görür. Verilirse ve durum bu arada ilerlemişse hiçbir şey
    /// uygulanmaz: 409 + `code: "conflict.stale_revision"`.
    ///
    /// Neden gövde alanı, `If-Match` başlığı değil: token opak bir entity-tag
    /// değil, düz bir tamsayıdır; `If-Match`'in weak/strong karşılaştırma, `*`
    /// ve liste semantiğinin yarısını uygulamak yanıltıcı olurdu. Ayrıca bu
    /// endpoint'in gövdesinde zaten opsiyonel alanlar var (`branch`) — token da
    /// aynı yerde, aynı tipte, tek bir sözleşmede durur.
    #[serde(default)]
    expected_rev: Option<u32>,
    /// K5 (2026-08-10, WFE not tasarımı): apply BAŞARILI olduktan SONRA bu
    /// draft notu yayınlar (`wfah_seq`/`node` commit'in ürettiği geçişten
    /// türetilir — bkz. `publish_note_after_apply`). Göndermeyen istemci için
    /// davranış HİÇ değişmez.
    #[serde(default)]
    note_id: Option<Uuid>,
}

/// `WfeApplyResult` (`wf_wfe::executor`) ALAN EKLEMEDEN sarılır — o tipe alan
/// eklemek başka bir işin sözleşmesini değiştirirdi. `note_error` yalnız
/// `note_id` gönderilip yayınlama başarısız olduğunda dolar (K5).
#[derive(Serialize)]
struct ApplyResultWithNote {
    #[serde(flatten)]
    result: wf_wfe::executor::WfeApplyResult,
    #[serde(skip_serializing_if = "Option::is_none")]
    note_error: Option<String>,
    /// Faz 4: aksiyon UYGULANDI ama yüklenen dosya(lar) staging'den nihai anahtara
    /// taşınamadı. Aksiyon geri ALINMAZ (motorun defteri yazıldı) — ama hata SESSİZ de
    /// kalmaz: istemci dosyayı tekrar yükleyebilsin diye burada taşınır. Metadata satırı
    /// da yazılmadığı için sonraki kapı kontrolü dosyayı yok görür ve akışı durdurur.
    #[serde(skip_serializing_if = "Option::is_none")]
    attachment_error: Option<String>,
}

#[utoipa::path(post, path = "/{id}/actions", tag = "wfe",
    params(("id" = Uuid, Path, description = "WFE id")),
    request_body = ApplyBody,
    responses((status = 200, description = "Uygulanan aksiyon sonucu (WfeApplyResult + opsiyonel note_error)", body = serde_json::Value)),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn apply_action(
    State(s): State<AppState>,
    Path(wfe_id): Path<Uuid>,
    req: axum::extract::Request,
) -> Result<axum::response::Response, AppError> {
    use axum::extract::FromRequest;
    use axum::response::IntoResponse;

    let headers = req.headers().clone();
    let is_multipart = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.starts_with("multipart/form-data"))
        .unwrap_or(false);

    if is_multipart {
        let mp = axum::extract::Multipart::from_request(req, &s)
            .await
            .map_err(|e| AppError(format!("multipart gövde okunamadı: {e}"), StatusCode::BAD_REQUEST))?;
        return apply_multipart(s, headers, wfe_id, mp).await;
    }

    let Json(body) = Json::<ApplyBody>::from_request(req, &s)
        .await
        .map_err(|e| AppError(e.body_text(), StatusCode::BAD_REQUEST))?;
    Ok(apply_json(s, headers, wfe_id, body).await?.into_response())
}

async fn apply_json(
    s: AppState,
    headers: HeaderMap,
    wfe_id: Uuid,
    body: ApplyBody,
) -> Result<Json<ApplyResultWithNote>, AppError> {
    let actor = extract_actor(&headers)?;

    // Attachment gate: hedef node'un `required` dosyaları yüklenmeden engine'e HİÇ
    // gitmeyiz (server-side zorlama; UI-only gating'e güvenilmez). Engine core
    // dosyadan habersiz kalır — kontrol portal katmanı opendal store'undadır.
    let wfd = super::attachments::load_wfd(&s, wfe_id).await?;
    let has_attachments = wfd.nodes.values().any(|n| !n.attachments.is_empty());
    if has_attachments {
        let view = s
            .executor
            .query(wfe_id, &actor)
            .await
            .map_err(AppError::from)?;
        // Paralelde current_node None'dur; kol seçimi body.branch ile gelir.
        let target_node = body
            .branch
            .clone()
            .or_else(|| view.current_node.as_ref().map(|n| n.id.clone()));
        if let Some(node) = &target_node {
            let store = crate::attachment_store::store_for_wfe(&s, wfe_id).await?;
            let groups =
                crate::attachments::status_for_node(
                    &store,
                    &wfd,
                    wfe_id,
                    node,
                    Some(body.action.as_str()),
                )
                    .await
                    .map_err(|e| {
                        AppError(
                            format!("attachment durum sorgusu başarısız: {e}"),
                            StatusCode::INTERNAL_SERVER_ERROR,
                        )
                    })?;
            let missing = crate::attachments::missing_required(&groups);
            if !missing.is_empty() {
                return Err(AppError {
                    message: format!("Eksik zorunlu belgeler: {}", missing.join(", ")),
                    status: StatusCode::UNPROCESSABLE_ENTITY,
                    code: Some("attachment.missing"),
                    items: None,
                });
            }
        }
    }

    let result = s
        .executor
        .apply(
            wfe_id,
            &actor,
            &body.action,
            &body.input,
            body.branch.as_deref(),
            body.target.as_deref(),
            body.expected_rev,
        )
        .await
        .map_err(AppError::from)?;

    // Not, apply BAŞARILI olduktan SONRA yayınlanır (K5) — geçiş zaten
    // gerçekleşti; not yayınlama hatası apply sonucunu YUTMAZ, `note_error`
    // olarak taşınır ve not draft kalır (kullanıcı tekrar dener).
    let note_error = match body.note_id {
        Some(note_id) => crate::notes::publish_after_apply(&s.pool, wfe_id, note_id, &actor)
            .await
            .err()
            .map(|e| e.message),
        None => None,
    };

    Ok(Json(ApplyResultWithNote { result, note_error, attachment_error: None }))
}

// ------------------------------------------- akış ortasında çok dosyalı aksiyon (Faz 4)

/// `POST /wfe/{id}/actions` multipart biçimi: `payload` part'ı `ApplyBody` JSON'u,
/// kalan part'lar `{grup}/{slot}` adıyla dosyalar. Başlatmadaki desenin AYNISI (K2).
///
/// **Değişmez: aksiyon uygulanamazsa mevcut dosyalar DEĞİŞMEZ.** Bu yüzden dosyalar
/// nihai anahtara doğrudan YAZILMAZ — nihai anahtar tektir, üzerine yazılırsa eski
/// baytlar geri getirilemez (`wf.wfe_attachment` satırı sürümlenir ama NESNE sürümlenmez).
/// Sıra: staging'e yaz → kapıyı "mevcut ∪ staging" ile sor → aksiyonu uygula →
/// başarıda nihai anahtara taşı, hatada staging'i sil.
async fn apply_multipart(
    s: AppState,
    headers: HeaderMap,
    wfe_id: Uuid,
    mut mp: axum::extract::Multipart,
) -> Result<axum::response::Response, AppError> {
    use axum::response::IntoResponse;

    let actor = extract_actor(&headers)?;

    // 1. payload İLK part (K2) — aksiyon adı ve `expected_rev` dosyalardan önce bilinmeli.
    let first = mp
        .next_field()
        .await
        .map_err(|e| AppError(format!("multipart okunamadı: {e}"), StatusCode::BAD_REQUEST))?
        .ok_or_else(|| AppError("multipart gövdesi boş".into(), StatusCode::BAD_REQUEST))?;
    if first.name() != Some("payload") {
        return Err(AppError {
            message: "ilk part 'payload' (JSON) olmalı".into(),
            status: StatusCode::BAD_REQUEST,
            code: Some("multipart.payload_first"),
            items: None,
        });
    }
    let raw = first
        .bytes()
        .await
        .map_err(|e| AppError(format!("payload okunamadı: {e}"), StatusCode::BAD_REQUEST))?;
    let body: ApplyBody = serde_json::from_slice(&raw)
        .map_err(|e| AppError(format!("payload JSON'u geçersiz: {e}"), StatusCode::BAD_REQUEST))?;

    let wfd = super::attachments::load_wfd(&s, wfe_id).await?;
    let view = s.executor.query(wfe_id, &actor).await.map_err(AppError::from)?;

    // 2. Dedupe ÇAPASI `expected_rev`tir — "o anki rev" DEĞİL. Gerekçe ters çalışır:
    //    ilk apply başarılı olunca rev ilerler, tekrar isteği o anki rev'e bakarsa parmak
    //    izi DEĞİŞİR ve aksiyon ikinci kez uygulanır — tam kaçınılmak istenen şey.
    //    İstemcinin gönderdiği `expected_rev` tekrar denemede AYNI kalır, iz tutar.
    //    Gönderilmemişse dedupe HİÇ koşmaz: çapasız tahmin, aynı girdiyle meşru olarak
    //    tekrarlanan bir aksiyonu ("revizyon iste") sessizce yutardı. Başlatmadaki K6 ile
    //    çelişmez — `expected_rev` zaten var olan bir alandır (WOR-65), yeni istemci yükü değil.
    let fp = body.expected_rev.map(|rev| {
        crate::start_dedupe::fingerprint(
            actor.user_id,
            wfe_id,
            rev as i32,
            Some(body.action.as_str()),
            &body.input,
            &Value::Null,
        )
    });
    if let Some(fp) = &fp {
        match crate::start_dedupe::claim(&s.pool, fp, actor.user_id, s.cfg.dedupe_window_secs).await?
        {
            crate::start_dedupe::Claim::Replay(_) => return apply_replay_response(&s, &actor, wfe_id).await,
            crate::start_dedupe::Claim::InProgress => {
                return Err(AppError {
                    message: "aynı aksiyon isteği şu anda işleniyor".into(),
                    status: StatusCode::CONFLICT,
                    code: Some("conflict.start_in_progress"),
                    items: None,
                })
            }
            crate::start_dedupe::Claim::Fresh => {}
        }
    }

    let out = apply_multipart_staged(&s, &actor, wfe_id, &wfd, &view, &body, mp).await;
    match (out, fp) {
        (Ok(r), Some(fp)) => {
            crate::start_dedupe::complete(&s.pool, &fp, wfe_id).await?;
            Ok(Json(r).into_response())
        }
        (Ok(r), None) => Ok(Json(r).into_response()),
        (Err(e), fp) => {
            if let Some(fp) = fp {
                if let Err(re) = crate::start_dedupe::release(&s.pool, &fp).await {
                    tracing::warn!("dedupe satırı bırakılamadı: {}", re.message);
                }
            }
            Err(e)
        }
    }
}

/// Dedupe penceresinde tekrarlanan AKSİYON isteği: aksiyon tekrar UYGULANMAZ, WFE'nin
/// bugünkü durumu aksiyon cevabının şeklinde döner. `start`in replay'inden ayrı bir
/// fonksiyon olmasının sebebi ŞEKİL: bu uç `{result, note_error}` sarmalıyla cevap verir,
/// düz `WfeStartResult` döndürmek istemcinin ayrıştırmasını kırardı.
///
/// `current_c_a` yeniden kurulamaz (apply anının çıktısıdır, sorgu görüşü taşımaz) —
/// bu yüzden `Idempotent-Replay: true` konur: istemci elindekinin yeni bir aksiyonun
/// çıktısı DEĞİL, bir yansıma olduğunu bilir.
async fn apply_replay_response(
    s: &AppState,
    actor: &Actor,
    wfe_id: Uuid,
) -> Result<axum::response::Response, AppError> {
    use axum::response::IntoResponse;
    let view = s.executor.query(wfe_id, actor).await.map_err(AppError::from)?;
    let body = ApplyResultWithNote {
        result: wf_wfe::executor::WfeApplyResult {
            wfe_id,
            terminal: !matches!(view.status, wfe_core::types::wfe::WfeStatus::Active),
            current_node: view.current_node.clone(),
            end_response: view.end_response.clone(),
            current_c_a: vec![],
        },
        note_error: None,
        attachment_error: None,
    };
    let mut resp = Json(body).into_response();
    resp.headers_mut()
        .insert("idempotent-replay", axum::http::HeaderValue::from_static("true"));
    Ok(resp)
}

async fn apply_multipart_staged(
    s: &AppState,
    actor: &Actor,
    wfe_id: Uuid,
    wfd: &wfe_core::types::wfd_v22::Wfd,
    view: &wf_wfe::executor::WfeView,
    body: &ApplyBody,
    mut mp: axum::extract::Multipart,
) -> Result<ApplyResultWithNote, AppError> {
    // Paralelde `current_node` None'dur; kol seçimi `body.branch` ile gelir (JSON yolla aynı).
    let target_node = body
        .branch
        .clone()
        .or_else(|| view.current_node.as_ref().map(|n| n.id.clone()));

    // Bu aksiyonu KAPAYAN grupların katalogu — başlatmadaki çözümün aynısı.
    let mut catalog: std::collections::HashMap<
        (String, String),
        &wfe_core::types::wfd_v22::AttachmentItem,
    > = std::collections::HashMap::new();
    if let Some(node_key) = &target_node {
        if let Some(node) = wfd.nodes.get(node_key) {
            for aref in &node.attachments {
                if !aref.gates_action(Some(body.action.as_str())) {
                    continue;
                }
                if let Some(group) = wfd.attachments.get(aref.group()) {
                    for item in &group.items {
                        catalog.insert((aref.group().to_string(), item.id.clone()), item);
                    }
                }
            }
        }
    }

    // WFE'nin kendi WFD'si ve ortamı — staging satırı bunlarla açılır ki `promote`in
    // kopyası dosyanın gideceği bucket'ta olsun (depo WFD başına `$env` ile çözülür).
    let (wfd_id, wfd_version, orgtnt_id, environment_id) =
        sqlx::query_as::<_, (Uuid, i32, Uuid, Option<Uuid>)>(
            "SELECT wfd_id, wfd_version, orgtnt_id, environment_id FROM wf.wfe WHERE wfe_id = $1",
        )
        .bind(wfe_id)
        .fetch_one(&s.pool)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    let max_request = s.cfg.attachment_max_request_mb as usize * 1024 * 1024;
    let mut errors: Vec<ItemError> = Vec::new();
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
        let Some((group, item)) = name.split_once('/').map(|(g, i)| (g.to_string(), i.to_string()))
        else {
            errors.push(ItemError {
                group: name.clone(),
                item: String::new(),
                code: "unknown_slot",
                message: format!("part adı '{name}' '{{grup}}/{{slot}}' biçiminde değil"),
            });
            continue;
        };
        let Some(def) = catalog.get(&(group.clone(), item.clone())).copied() else {
            errors.push(ItemError {
                group,
                item,
                code: "unknown_slot",
                message: "bu aksiyon için tanımlı bir dosya slotu değil".into(),
            });
            continue;
        };
        // Tip kapısı baytlardan ÖNCE (uzunluk 0): reddedilecek dosya staging'e bile yazılmaz.
        let declared_ct = field.content_type().map(str::to_string);
        if let Err(crate::attachments::UploadReject::UnsupportedType(ct)) =
            crate::attachments::check_upload(def, declared_ct.as_deref(), 0)
        {
            errors.push(ItemError {
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
                // Yazma yarıda kaldı: o ana kadarki tüm staging silinir, nihai anahtar
                // zaten HİÇ dokunulmadı — mevcut belgeler olduğu gibi durur.
                crate::staging::discard(s, &parts).await;
                return Err(e);
            }
        };
        request_total += part.size_bytes as usize;

        if part.size_bytes == 0 {
            errors.push(ItemError { group, item, code: "empty", message: "boş dosya yüklenemez".into() });
            continue;
        }
        if let Err(crate::attachments::UploadReject::TooLarge(mb)) =
            crate::attachments::check_upload(def, declared_ct.as_deref(), part.size_bytes as usize)
        {
            errors.push(ItemError {
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
            errors.push(ItemError {
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
        return Err(rejected(errors));
    }

    // 3. Kapı: depoda OLAN + bu istekte staging'e yazılan birlikte sayılır. Aksi hâlde
    //    kullanıcı eksiği aynı istekte gönderse bile kapı "eksik" der ve dosya hiç
    //    yerine konmaz — çıkışsız döngü (bkz. `missing_required_with_pending`).
    if let Some(node) = &target_node {
        let store = crate::attachment_store::store_for_wfe(s, wfe_id).await?;
        let groups = crate::attachments::status_for_node(
            &store,
            wfd,
            wfe_id,
            node,
            Some(body.action.as_str()),
        )
        .await
        .map_err(|e| {
            AppError(
                format!("attachment durum sorgusu başarısız: {e}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;
        let pending: Vec<(String, String)> = parts
            .iter()
            .map(|p| (p.grp.clone(), p.item.clone()))
            .collect();
        let missing = crate::attachments::missing_required_with_pending(&groups, &pending);
        if !missing.is_empty() {
            crate::staging::discard(s, &parts).await;
            return Err(AppError {
                message: format!("Eksik zorunlu belgeler: {}", missing.join(", ")),
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: Some("attachment.missing"),
                items: None,
            });
        }
    }

    // 4. Aksiyon. Bu noktaya kadar nihai anahtarlara HİÇ dokunulmadı.
    let result = match s
        .executor
        .apply(
            wfe_id,
            actor,
            &body.action,
            &body.input,
            body.branch.as_deref(),
            body.target.as_deref(),
            body.expected_rev,
        )
        .await
    {
        Ok(r) => r,
        Err(e) => {
            crate::staging::discard(s, &parts).await;
            return Err(AppError::from(e));
        }
    };

    // 5. Aksiyon geçti — dosyalar ancak ŞİMDİ yerine taşınır. Taşıma başarısız olursa
    //    aksiyon GERİ ALINMAZ (geçiş zaten commit edildi, motorun defteri yazıldı);
    //    `warn` loglanır ve staging satırı kalır, süpürücü toplar. Kullanıcı dosyayı
    //    tekrar yükleyebilir — not yayınlama hatasının (K5) aynı gerekçesi.
    let mut rows = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    for part in &parts {
        // Taşıma aynı bucket içinde server-side copy'dir; başarısızlığı beklenmez ama
        // geçici olabilir (ağ/throttle). Aksiyon geri alınamadığı için burada PES ETMEK
        // pahalı: birkaç kez denenir.
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
                tracing::warn!(
                    %wfe_id, grup = %part.grp, slot = %part.item,
                    "aksiyon sonrası dosya taşınamadı: {}", e.message
                );
                failed.push(format!("{}/{}", part.grp, part.item));
            }
        }
    }
    if let Err(e) = crate::wfe_attachment::insert_many(&s.pool, &rows).await {
        tracing::warn!(%wfe_id, "attachment metadata yazılamadı: {}", e.message);
    }

    let note_error = match body.note_id {
        Some(note_id) => crate::notes::publish_after_apply(&s.pool, wfe_id, note_id, actor)
            .await
            .err()
            .map(|e| e.message),
        None => None,
    };
    // Taşınamayan dosya varsa cevapta TAŞINIR: aksiyon geçti ama belge yerinde değil,
    // istemci bunu bilmeli ve dosyayı tekrar yüklemeli.
    let attachment_error = (!failed.is_empty()).then(|| {
        format!(
            "aksiyon uygulandı ancak şu belgeler depoya taşınamadı, lütfen tekrar yükleyin: {}",
            failed.join(", ")
        )
    });
    Ok(ApplyResultWithNote { result, note_error, attachment_error })
}

/// WOR-31 T4: gövde opsiyonel — hiç body/`{}` gönderilirse `node = None` (eski
/// davranış), `{"node": "..."}` verilirse paralel kol ipucu. Body zorunlu bir
/// `Json<T>` extractor'ı OLMADIĞI için (geriye uyumluluk: eski istemciler hiç
/// gövde göndermez) ham `Bytes` üzerinden opsiyonel ayrıştırılır.
#[derive(Deserialize, Default, ToSchema)]
#[schema(as = WfeClaimBody)]
struct ClaimBody {
    #[serde(default)]
    node: Option<String>,
    /// WOR-65: opsiyonel revizyon token'ı (bkz. `ApplyBody::expected_rev`).
    /// Claim'in kendi CAS'ı yarışı zaten çözer; bu kapı "listede gördüğüm satır
    /// hâlâ geçerli mi" sorusunu yanıtlar. Göndermeyen istemci için claim akışı
    /// HİÇ DEĞİŞMEZ (200 + `{success:false, reason:"already_claimed"}`).
    #[serde(default)]
    expected_rev: Option<u32>,
}

fn parse_claim_body(bytes: &axum::body::Bytes) -> Result<ClaimBody, AppError> {
    if bytes.is_empty() {
        return Ok(ClaimBody::default());
    }
    serde_json::from_slice(bytes)
        .map_err(|e| AppError(format!("invalid claim body: {e}"), StatusCode::BAD_REQUEST))
}

#[utoipa::path(post, path = "/{id}/claim", tag = "wfe",
    params(("id" = Uuid, Path, description = "WFE id")),
    request_body = ClaimBody,
    responses((status = 200, description = "Claim sonucu (ClaimOutcome)", body = serde_json::Value)),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn claim_wfe(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(wfe_id): Path<Uuid>,
    body: axum::body::Bytes,
) -> Result<Json<wf_wfe::executor::ClaimOutcome>, AppError> {
    let actor = extract_actor(&headers)?;
    let claim_body = parse_claim_body(&body)?;
    s.executor
        .claim(
            wfe_id,
            &actor,
            claim_body.node.as_deref(),
            claim_body.expected_rev,
        )
        .await
        .map(Json)
        .map_err(AppError::from)
}

/// Madde 7: yetkili claim devri. `to` = devralacak tam aktör üçlüsü; `null`/atlanırsa
/// havuza bırakma (force-unclaim). `node` = WOR-31 paralel modda kol seçimi.
#[derive(Deserialize, ToSchema)]
struct ReassignBody {
    #[serde(default)]
    to: Option<TargetActor>,
    #[serde(default)]
    node: Option<String>,
}

#[derive(Deserialize, ToSchema)]
struct TargetActor {
    orgu_id: Uuid,
    user_id: Uuid,
    role: String,
}

#[utoipa::path(post, path = "/{id}/reassign", tag = "wfe",
    params(("id" = Uuid, Path, description = "WFE id")),
    request_body = ReassignBody,
    responses((status = 200, description = "Devir sonucu (ReassignOutcome)", body = serde_json::Value)),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn reassign_wfe(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(wfe_id): Path<Uuid>,
    Json(body): Json<ReassignBody>,
) -> Result<Json<wf_wfe::executor::ReassignOutcome>, AppError> {
    let reassigner = extract_actor(&headers)?;
    let target = body.to.map(|t| Actor {
        orgu_id: t.orgu_id,
        user_id: t.user_id,
        role: t.role,
    });
    s.executor
        .reassign(wfe_id, &reassigner, target.as_ref(), body.node.as_deref())
        .await
        .map(Json)
        .map_err(AppError::from)
}

/// T‑A5 WF Admin: escalation müdahalesi gövdesi. `node` yalnız paralel modda anlamlı
/// (kol başına sayaç) — `reassign` ile aynı konvansiyon.
#[derive(Deserialize, ToSchema)]
struct EscalationAdminBody {
    #[serde(default)]
    node: Option<String>,
}

/// Sıradaki escalation adımını ELLE tetikler (vade beklemeden).
///
/// Adım numarası istemciden alınmaz: sıradaki ateşlenmemiş adım uygulanır, böylece
/// escalation adımlarının sıralı olma sözleşmesi korunur.
#[utoipa::path(post, path = "/{id}/escalation/fire", tag = "wfe",
    params(("id" = Uuid, Path, description = "WFE id")),
    request_body = EscalationAdminBody,
    responses(
        (status = 200, description = "Uygulanan adım", body = serde_json::Value),
        (status = 400, description = "Paralel modda kol node'u verilmedi"),
        (status = 403, description = "Aktör wf_admin kurallarına uymuyor"),
        (status = 409, description = "Bekleyen adım yok (escalation.none_pending) veya WFE bitmiş"),
    ),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn fire_escalation(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(wfe_id): Path<Uuid>,
    Json(body): Json<EscalationAdminBody>,
) -> Result<Json<wf_wfe::executor::EscalationAdminOutcome>, AppError> {
    let admin = extract_actor(&headers)?;
    let outcome = s
        .executor
        .fire_escalation_now(wfe_id, &admin, body.node.as_deref())
        .await?;
    none_pending_to_conflict(outcome).map(Json)
}

/// Sıradaki escalation adımını ATLAR — geçiş uygulanmaz, audit satırı yazılır.
#[utoipa::path(post, path = "/{id}/escalation/skip", tag = "wfe",
    params(("id" = Uuid, Path, description = "WFE id")),
    request_body = EscalationAdminBody,
    responses(
        (status = 200, description = "Atlanan adım", body = serde_json::Value),
        (status = 400, description = "Paralel modda kol node'u verilmedi"),
        (status = 403, description = "Aktör wf_admin kurallarına uymuyor"),
        (status = 409, description = "Bekleyen adım yok (escalation.none_pending) veya WFE bitmiş"),
    ),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn skip_escalation(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(wfe_id): Path<Uuid>,
    Json(body): Json<EscalationAdminBody>,
) -> Result<Json<wf_wfe::executor::EscalationAdminOutcome>, AppError> {
    let admin = extract_actor(&headers)?;
    let outcome = s
        .executor
        .skip_escalation(wfe_id, &admin, body.node.as_deref())
        .await?;
    none_pending_to_conflict(outcome).map(Json)
}

/// "Dokunacak adım yok" çekirdekte bir CEVAPtır; HTTP'de 409 + makine kodudur.
/// Dönüşüm burada, tek yerde yapılır (portal kabuğu da bunu kullanır).
pub(crate) fn none_pending_to_conflict(
    outcome: wf_wfe::executor::EscalationAdminOutcome,
) -> Result<wf_wfe::executor::EscalationAdminOutcome, AppError> {
    if matches!(
        outcome,
        wf_wfe::executor::EscalationAdminOutcome::NonePending
    ) {
        return Err(AppError {
            message: "bu node'da bekleyen escalation adımı yok".into(),
            status: StatusCode::CONFLICT,
            code: Some("escalation.none_pending"),
            items: None,
        });
    }
    Ok(outcome)
}

#[utoipa::path(get, path = "/{id}", tag = "wfe",
    params(("id" = Uuid, Path, description = "WFE id")),
    responses((status = 200, description = "WFE görünümü (WfeView)", body = serde_json::Value)),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn query_wfe(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(wfe_id): Path<Uuid>,
) -> Result<Json<wf_wfe::executor::WfeView>, AppError> {
    let actor = extract_actor(&headers)?;
    s.executor
        .query(wfe_id, &actor)
        .await
        .map(Json)
        .map_err(AppError::from)
}

#[utoipa::path(get, path = "/{id}/possible-actions", tag = "wfe",
    params(("id" = Uuid, Path, description = "WFE id")),
    responses((status = 200, description = "Aktörün uygulayabileceği aksiyonlar", body = serde_json::Value)),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn possible_actions(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(wfe_id): Path<Uuid>,
) -> Result<Json<Vec<wf_wfe::executor::PossibleAction>>, AppError> {
    let actor = extract_actor(&headers)?;
    s.executor
        .possible_actions(wfe_id, &actor)
        .await
        .map(Json)
        .map_err(AppError::from)
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct WfeListQuery {
    limit: Option<i64>,
    offset: Option<i64>,
}

/// `WfeRow` + SLA görünüm alanları (2026-07-16): `priority` (1-10, otomatik) ve
/// `claim_deadline` (claimed_at + node.claim_timeout, hesaplanmış). `deadline` /
/// `claimed_at` / `join_target` zaten `WfeRow` kolonları — flatten ile response'a
/// dahil olur.
#[derive(serde::Serialize)]
struct WfeListItem {
    #[serde(flatten)]
    row: wf_wfe::models::WfeRow,
    /// `WfeRow.current_node`un dış yüzü: anahtar + gösterim (`Ref`). Satır tipinde
    /// `skip_serializing`dir çünkü etiket ancak WFD elde varken üretilir.
    /// WFD çözülemezse (silinmiş/bozuk sürüm) `label` anahtarın okunur hâline düşer.
    current_node: Option<wf_wfe::executor::Ref>,
    priority: i32,
    claim_deadline: Option<chrono::DateTime<chrono::Utc>>,
    /// WOR-65: WFE revizyon token'ı. Listede AÇIKÇA döner çünkü buraya `wfah`
    /// dizisi dahil değildir — portal'ın revizyonu türetebileceği başka bir alan
    /// YOK. `priority`/`claim_deadline` gibi hesaplanmış bir alandır (`wf.wfe`
    /// kolonu DEĞİL), bu yüzden `WfeRow`'a değil buraya konur.
    rev: i32,
    /// WOR-31 T4: paralel modda bu WFE'nin AKTİF kolları (`GET /wfe/:id` ile aynı
    /// şekil — `node` bir `Ref`tir; `c_a`/`claim_as` liste ucunda hesaplanmaz).
    /// Paralel değilken (join_target NULL) BOŞ dizi — liste tüketicisi kol-başına
    /// satır fan-out'u için bunu okur (current_node paralel modda NULL'dır).
    branches: Vec<BranchView>,
    /// WFE not tasarımı Faz 1 (K9): görünür (published + gizlenmemiş + bu
    /// aktöre `audience` açık) not sayısı — TEK toplu sorgu
    /// (`notes::count_by_wfe`), N+1 yok. `WfeView`'a DOKUNULMAZ; sayaç yalnız
    /// liste görünümündedir.
    note_count: i64,
    /// Faz 3 (K9 okundu takibi): bu aktör için OKUNMAMIŞ not sayısı — havuz
    /// rozeti "kaç not var" değil "kaç YENİ not var" göstersin diye
    /// `note_count`'un YANINA eklendi, onu YERİNE geçmedi.
    unread_note_count: i64,
}

/// Liste uçlarının node `Ref`i. WFD çözülemediyse (silinmiş/bozuk sürüm) etiket
/// anahtarın okunur hâline düşer — istemci `label` alanının DAİMA dolu olduğuna
/// güvenebilmeli, tek bir bozuk satır bu sözü bozmamalı.
fn node_ref(wfd: Option<&wfe_core::types::wfd_v22::Wfd>, key: &str) -> wf_wfe::executor::Ref {
    match wfd {
        Some(w) => wf_wfe::executor::Ref::node(w, key),
        None => wf_wfe::executor::Ref {
            id: key.to_string(),
            label: wfe_core::v22::display::humanize_key(key),
        },
    }
}

/// Liste ucunun kol görünümü: `c_a`/`claim_as` HESAPLANMAZ (havuz "görünürlük"
/// listesidir, claim kararı `GET /wfe/:id`de verilir) — o alanlar boş kalır ve
/// serileşmede düşer.
fn branch_list_view(
    wfd: Option<&wfe_core::types::wfd_v22::Wfd>,
    b: &BranchState,
) -> BranchView {
    BranchView {
        node: node_ref(wfd, &b.branch_node),
        entry_node: node_ref(wfd, b.entry_or_current()),
        status: b.status,
        claimed_by: b.claimed_by,
        claimed_at: b.claimed_at,
        entered_at: b.entered_at,
        c_a: vec![],
        claim_as: None,
    }
}

/// WOR-31 T4: liste kol satırı (`BranchListRow`) → kalıcı temsil (`BranchState`).
/// `claimed_by` jsonb `{"user_id": "<uuid>"}`
/// biçiminden Uuid'e çözülür (wfe_adapter `parse_claimed_by` ile aynı sözleşme);
/// `status` metni enum'a eşlenir (aktif dışı zaten sorguda süzülür).
fn branch_list_row_to_state(r: wf_wfe::models::BranchListRow) -> BranchState {
    BranchState {
        // WOR-73: liste sorgusu kol kimliğini çekmiyor (havuz görünümü kimliği
        // kullanmaz) — boş bırakılır, `entry_or_current()` branch_node'a düşer.
        entry_node: String::new(),
        branch_node: r.branch_node,
        status: match r.status.as_str() {
            "arrived" => BranchStatus::Arrived,
            "cancelled" => BranchStatus::Cancelled,
            _ => BranchStatus::Active,
        },
        claimed_by: r
            .claimed_by
            .as_ref()
            .and_then(|cb| cb.get("user_id"))
            .and_then(|u| u.as_str())
            .and_then(|s| Uuid::parse_str(s).ok()),
        claimed_at: r.claimed_at,
        entered_at: r.entered_at,
    }
}

#[utoipa::path(get, path = "/", tag = "wfe", params(WfeListQuery),
    responses((status = 200, description = "Tenant'ın WFE listesi (WfeListItem[])", body = serde_json::Value)),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn list_wfe(
    State(s): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(q): axum::extract::Query<WfeListQuery>,
) -> Result<Json<Vec<WfeListItem>>, AppError> {
    let actor = extract_actor(&headers)?;
    // WOR-5 fix: orgu_id tenant DEĞİLDİR — orgtnt_id org katmanından çözülür
    let orgtnt_id = s
        .executor
        .org
        .orgtnt_for_orgu(actor.orgu_id)
        .await
        .map_err(AppError::from)?;
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let offset = q.offset.unwrap_or(0).max(0);
    let rows = wf_wfe::repo::wfe::list_by_tenant(&s.pool, orgtnt_id, limit, offset)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    // WOR-65: revizyonlar TEK toplu sorguda (satır başına sorgu YOK).
    let wfe_ids: Vec<Uuid> = rows.iter().map(|r| r.wfe_id).collect();
    let revs = wf_wfe::repo::wfah::max_seq_by_wfe(&s.pool, &wfe_ids)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    // WFE not tasarımı Faz 1/3: not sayaçları da TEK toplu sorguda (N+1 yok).
    let note_counts = crate::notes::count_by_wfe(&s.pool, &wfe_ids, &actor).await?;
    let unread_counts = crate::notes::unread_count_by_wfe(&s.pool, &wfe_ids, &actor).await?;

    // WOR-31 T4: paralel WFE'lerin aktif kolları — TEK toplu sorgu, `wfe_id`'ye
    // göre grupla. Yalnız `join_target` dolu (paralel) satırları sorgula: tek-kol
    // WFE'ler için gereksiz yük olmasın.
    let parallel_ids: Vec<Uuid> = rows
        .iter()
        .filter(|r| r.join_target.is_some())
        .map(|r| r.wfe_id)
        .collect();
    let mut branches_by_wfe: std::collections::HashMap<Uuid, Vec<BranchState>> =
        std::collections::HashMap::new();
    for br in wf_wfe::repo::branch::load_active_for_wfes(&s.pool, &parallel_ids)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?
    {
        branches_by_wfe
            .entry(br.wfe_id)
            .or_default()
            .push(branch_list_row_to_state(br));
    }

    let now = chrono::Utc::now();
    // (wfd_id, version) immutable — aynı sürüm birden çok satırda paylaşılıyorsa
    // fetch tekrarlanmaz. Artık HER satır için çözülür (yalnız claim_deadline
    // gerektiğinde değil): `current_node`/kol etiketleri de WFD'den gelir.
    let mut wfd_cache: std::collections::HashMap<(Uuid, i32), wfe_core::types::wfd_v22::Wfd> =
        std::collections::HashMap::new();
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let priority = wf_wfe::priority::compute_priority(row.created_at, row.deadline, now);
        let key = (row.wfd_id, row.wfd_version);
        if !wfd_cache.contains_key(&key) {
            // Bozuk/silinmiş tek bir WFD tüm listeyi düşürmesin — etiketler
            // anahtarın okunur hâline düşer, satır listede kalır.
            if let Ok(w) = s.wfd.fetch(row.wfd_id, row.wfd_version).await {
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
        let current_node = row
            .current_node
            .as_deref()
            .map(|n| node_ref(wfd, n));
        let rev = revs.get(&row.wfe_id).copied().unwrap_or(0);
        let branches = branches_by_wfe
            .remove(&row.wfe_id)
            .unwrap_or_default()
            .into_iter()
            .map(|b| branch_list_view(wfd, &b))
            .collect();
        let note_count = note_counts.get(&row.wfe_id).copied().unwrap_or(0);
        let unread_note_count = unread_counts.get(&row.wfe_id).copied().unwrap_or(0);
        out.push(WfeListItem {
            current_node,
            priority,
            claim_deadline,
            rev,
            branches,
            note_count,
            unread_note_count,
            row,
        });
    }
    Ok(Json(out))
}
