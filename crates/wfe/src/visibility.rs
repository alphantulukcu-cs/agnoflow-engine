//! Görünürlüğün TEK kaynağı: "bu aktör bu WFE'yi görebilir mi?" sorusunun SQL hâli.
//!
//! Bu modül var olma sebebini bir hatadan alıyor: aynı soru üç yerde üç farklı
//! şekilde cevaplanıyordu — detay ucu `can_view` ile (belge + org portu, satır
//! satır), havuz `current_c_a` containment'ı ile (yaklaşık: `listable[].when`
//! yok sayılıyor, `wf_admin` hiç bilinmiyordu), liste ucu ise hiç cevaplamayıp
//! işi istemciye bırakıyordu (satır başına `GET /wfe/:id` probu, N+1). Üç cevap
//! birbirinden ayrı düştüğünde kullanıcı ya göremediği satırı listede görüyor ya
//! da görebileceği satırı hiç göremiyordu.
//!
//! Artık kural TEK bir `WHERE` parçasıdır ve üç tüketici de onu kullanır:
//! liste (`GET /wfe?viewable=true`), detay (`GET /wfe/:id`) ve portal havuzu.
//! Ayrışma yapısal olarak imkânsız: parça değişirse üçü birden değişir.
//!
//! ## Kural (ürün kararı, 2026-08-13)
//! ```text
//! görünür(WFE, viewer) :=
//!      view_c_a @> viewer                    -- listable ∪ wf_admin, KALICI
//!   OR (status='active' AND (
//!          current_c_a @> viewer             -- node adayları (tek-kol)
//!       OR claimed_by  @> viewer             -- iş onun elinde
//!       OR EXISTS(aktif kol: c_a @> viewer VEYA claimed_by @> viewer)))
//! ```
//! İş bitince `current_c_a` boşaltılır (adapter) → geriye yalnız `view_c_a`
//! kalır. Yani bitmiş işi görme yetkisi tamamen `listable`/`wf_admin`
//! tasarımına bağlıdır; "işe dokunmuş olmak" (eski `can_view` kriteri (b))
//! ARTIK yetki üretmez.
//!
//! ## Neden containment (`@>`)
//! Kolonlar kuralın ÇÖZÜLMÜŞ hâlini tutuyor (`CandidateActor[]`), yani soru
//! "aktör bu listede var mı"ya iniyor ve GIN index'inden okunuyor. Aktörün
//! filtreleri istek başına BİR kez üretilir (`ViewerFilters`), satır başına
//! değil.

use serde_json::{json, Value};
use uuid::Uuid;
use wfe_core::types::actor::Actor;
use wfe_core::{EngineError, OrgPort};


/// Görünürlük `WHERE` parçası. Tablo takma adı `e` (wf.wfe) olmalıdır.
///
/// Bağlanacak parametreler `$1..$5` OFFSETLİ verilir: çağıran kendi
/// parametrelerini saydıktan sonra `sql(n)` ile kaydırır (bkz. `filters()`).
/// Sıra: rol · kullanıcı · kimlik(ident, NULL olabilir) · çapasız-kullanıcı ·
/// çapasız-kimlik(NULL olabilir) · sahiplik.
pub fn sql(offset: usize) -> String {
    let p = |i: usize| format!("${}", offset + i);
    let (role, user, ident, any_user, any_ident, owner) =
        (p(1), p(2), p(3), p(4), p(5), p(6));
    format!(
        "(
             e.view_c_a @> {role}::jsonb
          OR e.view_c_a @> {user}::jsonb
          OR ({ident}::jsonb IS NOT NULL AND e.view_c_a @> {ident}::jsonb)
          OR e.view_c_a @> {any_user}::jsonb
          OR ({any_ident}::jsonb IS NOT NULL AND e.view_c_a @> {any_ident}::jsonb)
          OR (e.status = 'active' AND (
                 e.current_c_a @> {role}::jsonb
              OR e.current_c_a @> {user}::jsonb
              OR ({ident}::jsonb IS NOT NULL AND e.current_c_a @> {ident}::jsonb)
              OR e.current_c_a @> {any_user}::jsonb
              OR ({any_ident}::jsonb IS NOT NULL AND e.current_c_a @> {any_ident}::jsonb)
              OR e.claimed_by @> {owner}::jsonb
              OR EXISTS (
                   SELECT 1 FROM wf.wfe_branch b
                    WHERE b.wfe_id = e.wfe_id
                      AND b.status = 'active'
                      AND (   b.c_a @> {role}::jsonb
                           OR b.c_a @> {user}::jsonb
                           OR ({ident}::jsonb IS NOT NULL AND b.c_a @> {ident}::jsonb)
                           OR b.c_a @> {any_user}::jsonb
                           OR ({any_ident}::jsonb IS NOT NULL AND b.c_a @> {any_ident}::jsonb)
                           OR b.claimed_by @> {owner}::jsonb))))
        )"
    )
}

/// `sql()`in beklediği parametre sayısı — çağıran offset hesabında kullanır.
pub const PARAM_COUNT: usize = 6;

/// Bir aktörün containment filtreleri — istek başına BİR kez üretilir.
///
/// `user_ident` org portundan çözülür (matcher'ın kimlik kanalının aynası:
/// `c_u` bir UUID yerine kullanıcı adı da olabilir). Çözülemezse ilgili iki
/// filtre NULL gider ve SQL onları atlar.
pub struct ViewerFilters {
    role: Value,
    user: Value,
    ident: Option<Value>,
    any_user: Value,
    any_ident: Option<Value>,
    owner: Value,
}

impl ViewerFilters {
    pub async fn build(actor: &Actor, org: &dyn OrgPort) -> Result<Self, EngineError> {
        let ident = org.user_ident(actor.user_id).await?;
        Ok(Self {
            role: json!([{ "orgu_id": actor.orgu_id.to_string(), "role": actor.role }]),
            user: json!([{ "orgu_id": actor.orgu_id.to_string(), "user_id": actor.user_id.to_string() }]),
            ident: ident
                .as_ref()
                .map(|i| json!([{ "orgu_id": actor.orgu_id.to_string(), "user_ident": i }])),
            // Çapasız (c_orgu'suz) kural girdileri birim TAŞIMAZ ve `any_orgu: true`
            // ile işaretlidir; birimli filtreler onları yakalamaz. İşaret filtreye
            // DAHİL — çıplak `[{"user_id": U}]` sorgusu aynı kişinin BAŞKA bir
            // birimdeki kapsamlı grant'ını da yakalardı (containment alt küme sorar).
            any_user: json!([{ "any_orgu": true, "user_id": actor.user_id.to_string() }]),
            any_ident: ident.as_ref().map(|i| json!([{ "any_orgu": true, "user_ident": i }])),
            owner: json!({ "user_id": actor.user_id.to_string() }),
        })
    }

    /// `sql()`deki parametre SIRASIYLA bağlanacak değerler. Sıra SÖZLEŞMEDİR —
    /// değişirse `sql()` de değişmeli, o yüzden ikisi aynı dosyada durur.
    /// `None` öğeler SQL NULL olarak gider (jsonb `null` DEĞİL): `sql()` onları
    /// `IS NOT NULL` ile atlar.
    pub fn as_binds(&self) -> Vec<Option<Value>> {
        vec![
            Some(self.role.clone()),
            Some(self.user.clone()),
            self.ident.clone(),
            Some(self.any_user.clone()),
            self.any_ident.clone(),
            Some(self.owner.clone()),
        ]
    }
}

/// Tek WFE için görünürlük kapısı — detay ucunun kullandığı hâli.
///
/// `can_view` (wfe-core) ile AYNI cevabı vermek zorunda olan ikinci bir
/// uygulama DEĞİLDİR: bu, kuralın ta kendisidir. Detay ucu da listeyle aynı
/// `sql()` parçasını sorar, böylece "listede gördüğüm satırı açamıyorum"
/// sınıfı hatalar yapısal olarak imkânsız hale gelir.
pub async fn can_view_sql(
    pool: &sqlx::PgPool,
    wfe_id: Uuid,
    filters: &ViewerFilters,
) -> Result<bool, EngineError> {
    let stmt = format!(
        "SELECT EXISTS (SELECT 1 FROM wf.wfe e WHERE e.wfe_id = $7 AND {})",
        sql(0)
    );
    let mut q = sqlx::query_scalar::<_, bool>(&stmt);
    for b in filters.as_binds() {
        q = q.bind(b);
    }
    q.bind(wfe_id)
        .fetch_one(pool)
        .await
        .map_err(|e| EngineError::WfePort(e.to_string()))
}
