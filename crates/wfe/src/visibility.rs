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
//! liste (`GET /wfe?viewable=true`), detay (`GET /wfe/:id`) ve portal havuzu
//! (`routes::portal::pool` — İKİ sorgusu da, tek-kol ve kol satırları).
//! Ayrışma yapısal olarak imkânsız: parça değişirse üçü birden değişir.
//! Havuz bu parçaya en son bağlandı (2026-08-14); o güne kadar kendi
//! `WHERE`'ini taşıyordu ve node listable kolonlarını tanımıyordu — kararın
//! önlemek için yazıldığı ayrışmanın son bacağı buydu.
//!
//! ## Kural (ürün kararı, 2026-08-13)
//! ```text
//! görünür(WFE, viewer) :=
//!      view_c_a @> viewer                    -- listable ∪ wf_admin, KALICI
//!   OR (status='active' AND (
//!          current_c_a      @> viewer        -- node adayları (tek-kol)
//!       OR current_view_c_a @> viewer        -- node listable'ı (tek-kol)
//!       OR claimed_by       @> viewer        -- iş onun elinde
//!       OR EXISTS(aktif kol: c_a @> viewer VEYA view_c_a @> viewer
//!                            VEYA claimed_by @> viewer)))
//! ```
//! İş bitince `current_c_a` boşaltılır (adapter) → geriye yalnız `view_c_a`
//! kalır. Yani bitmiş işi görme yetkisi tamamen `listable`/`wf_admin`
//! tasarımına bağlıdır; "işe dokunmuş olmak" (eski `can_view` kriteri (b))
//! ARTIK yetki üretmez.
//!
//! **Node listable (`can_view` kriteri (f), 2026-08-13)** iki YENİ kolonla
//! geldi ve `status='active'` KOLUNUN İÇİNDEDİR — kalıcı `view_c_a`nın yanına
//! DEĞİL. Ayrım kolonların ömrüdür: `nodes.<key>.listable[]` "WFE bu node'da
//! İKEN görsün" der, o yüzden `current_c_a` ile aynı anda yazılır ve terminal'de
//! onunla birlikte boşaltılır (`current_view_c_a`in kalıcı kola konması, node'dan
//! çıkmış — hatta bitmiş — işi hâlâ görünür kılardı). Kol karşılığı
//! `wfe_branch.view_c_a`: paralel modda "aktif node" kümesi kol satırlarıdır.
//!
//! Bu kolonlar ACT VERMEZ (yalnız görme) ama havuz sorgusuna DA girerler
//! (2026-08-14, önceden girmiyorlardı): havuz da bu parçayı koştuğu için
//! görünürlük üç tüketicide TEK cevaptır. Havuzda satırı görmek onu claim
//! edebilmek DEĞİLDİR — claim kapısı ayrıdır ve node `c_a`'sına bakar
//! (`WfeExecutor::can_claim`/`claim`, hiçbir projeksiyon kolonu okumaz);
//! kök `listable` için bu ayrım zaten böyleydi.
//!
//! Yeni kolonlar AYNI viewer filtrelerini yeniden kullanır → `PARAM_COUNT`
//! DEĞİŞMEDİ. Kolon eklemek yeni bir parametre gerektiriyorsa filtre şekli
//! ayrışmış demektir; o zaman soru "aktör bu listede var mı" olmaktan çıkar.
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
              OR e.current_view_c_a @> {role}::jsonb
              OR e.current_view_c_a @> {user}::jsonb
              OR ({ident}::jsonb IS NOT NULL AND e.current_view_c_a @> {ident}::jsonb)
              OR e.current_view_c_a @> {any_user}::jsonb
              OR ({any_ident}::jsonb IS NOT NULL AND e.current_view_c_a @> {any_ident}::jsonb)
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
                           OR b.view_c_a @> {role}::jsonb
                           OR b.view_c_a @> {user}::jsonb
                           OR ({ident}::jsonb IS NOT NULL AND b.view_c_a @> {ident}::jsonb)
                           OR b.view_c_a @> {any_user}::jsonb
                           OR ({any_ident}::jsonb IS NOT NULL AND b.view_c_a @> {any_ident}::jsonb)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `sql()`in `status = 'active'` KOLU — parantez sayarak kesilir.
    ///
    /// Kolonun "aktif kolun içinde mi" sorusunu metin araması (`contains`) ile
    /// cevaplamak yetmez: kalıcı kolun içinde de aynı isim geçebilir. Bu yüzden
    /// alt ifade gerçekten ayıklanır.
    fn active_arm(stmt: &str) -> String {
        let start = stmt
            .find("e.status = 'active'")
            .expect("aktif kol bulunamadı");
        // `(e.status = 'active' AND (...))` — açılış parantezi hemen öncededir.
        let open = stmt[..start].rfind('(').expect("açılış parantezi");
        let mut depth = 0i32;
        for (i, ch) in stmt[open..].char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return stmt[open..open + i + 1].to_string();
                    }
                }
                _ => {}
            }
        }
        panic!("aktif kol kapanmıyor");
    }

    /// Node listable kolonları DURUMA BAĞLIDIR → `status='active'` kolunun
    /// İÇİNDE olmalı. Kalıcı kola (`view_c_a`ın yanına) kaçarsalar node'dan
    /// çıkmış, hatta BİTMİŞ iş görünür kalır — tasarımın reddettiği tam bu.
    #[test]
    fn node_listable_columns_live_inside_the_active_arm() {
        let stmt = sql(0);
        let arm = active_arm(&stmt);

        assert!(
            arm.contains("e.current_view_c_a"),
            "current_view_c_a aktif kolun içinde değil:\n{arm}"
        );
        assert!(
            arm.contains("b.view_c_a"),
            "kol view_c_a'sı aktif kolun içinde değil:\n{arm}"
        );

        // Kalıcı kolda (aktif kolun DIŞI) node listable kolonu geçmemeli.
        let permanent = stmt.replace(&arm, "");
        assert!(
            !permanent.contains("current_view_c_a"),
            "current_view_c_a kalıcı kola sızmış:\n{permanent}"
        );
        assert!(
            !permanent.contains("b.view_c_a"),
            "kol view_c_a'sı kalıcı kola sızmış:\n{permanent}"
        );
        // Buna karşılık KALICI grant kolonu kalıcı kolda DURMALI (kontrolün
        // kendisi bozulmuş olmasın).
        assert!(permanent.contains("e.view_c_a"), "kalıcı grant kolu kaybolmuş");
    }

    /// Kol EXISTS'i içinde node listable kanalı var ve `b.status='active'`
    /// süzgecinin ARKASINDA — iptal edilmiş kolun grant'ı satır getirmemeli.
    #[test]
    fn branch_view_channel_is_gated_by_active_branch() {
        let stmt = sql(0);
        let exists = stmt
            .find("FROM wf.wfe_branch b")
            .expect("kol EXISTS'i yok");
        let gate = stmt[exists..]
            .find("b.status = 'active'")
            .expect("kol status süzgeci yok");
        let view = stmt[exists..]
            .find("b.view_c_a")
            .expect("kol view_c_a kanalı yok");
        assert!(gate < view, "kol view_c_a'sı status süzgecinden ÖNCE geliyor");
    }

    /// Yeni kolonlar AYNI viewer filtrelerini yeniden kullanır → parametre
    /// sayısı DEĞİŞMEZ. Değişirse `filters()`/`as_binds()` sözleşmesi ve tüm
    /// çağıranların offset hesabı birlikte bozulur.
    #[test]
    fn param_count_is_unchanged_by_node_listable() {
        assert_eq!(PARAM_COUNT, 6);
        let stmt = sql(0);
        // Üretilen metinde $1..$6 var, $7 YOK (çağıran kendi parametrelerini
        // ondan sonra bağlar — bkz. `can_view_sql`).
        for i in 1..=PARAM_COUNT {
            assert!(stmt.contains(&format!("${i}")), "${i} parametresi yok");
        }
        assert!(
            !stmt.contains(&format!("${}", PARAM_COUNT + 1)),
            "sql() fazladan parametre üretmiş"
        );
    }

    /// Offset kaydırması TÜM kanallara uygulanır — yeni kolonlar da dahil.
    #[test]
    fn offset_shifts_every_channel() {
        let stmt = sql(10);
        assert!(!stmt.contains("$1:"), "kaydırılmamış parametre kalmış");
        for i in 11..=(10 + PARAM_COUNT) {
            assert!(stmt.contains(&format!("${i}")), "${i} parametresi yok");
        }
        // Node listable kanalları da kaydırılmış hâlde görünmeli.
        assert!(stmt.contains("e.current_view_c_a @> $11::jsonb"));
        assert!(stmt.contains("b.view_c_a @> $11::jsonb"));
    }
}
