# Orgu-Role Unification — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `special` orgu niteliğini kaldır; `role`'ü tek kavram yap — role hem kullanıcıya (`org.ur`) hem orgu'ya (yeni `org.orgu_r`) bağlanabilsin, orgu'ya bağlananı üyeler devralsın.

**Architecture:** Tam relational (org.r tek katalog). Yeni `org.orgu_r` tablosu birime rol grant'ı tutar. `check_user_role` üç kaynaktan karar verir: `(org.ur granted OR org.orgu_r) AND NOT (org.ur excluded)`. ORGTRVLANG'da `[special:X]` gider, `[role:X]` gelir (relational EXISTS); çoklu `[role:a,b]` parser'da OR'a açılır. İki `*:` global yolu (`fetch_global_type` + `resolve_global_type`) tek filtre motorunda birleşir.

**Tech Stack:** Rust, axum, sqlx (Postgres), ltree; migration'lar psql ile manuel.

**Test gerçeği:** `crates/org` yalnızca `parser.rs`'te saf unit test içerir; DB-backed test harness'ı YOKTUR. Bu yüzden parser değişikliği katı TDD ile, SQL/repo/HTTP katmanı `cargo build`/`cargo test --workspace` (derleme + parser testleri) + Task 9'daki somut psql/curl doğrulaması ile kanıtlanır. Golden fixture (WFD) `special` içermez, değişmez.

---

## Dosya Haritası

| Dosya | Sorumluluk | İşlem |
|---|---|---|
| `migrations/org/20260722000001_orgu_role.sql` | Yeni `org.orgu_r` tablosu + special→role veri taşıması + `special` anahtarını silme | Create |
| `crates/org/src/traversal/parser.rs` | Virgül-OR (`[role:a,b]`) desteği + `pub fn parse_filter` ihracı; special örnekli testleri role'e çevir | Modify |
| `crates/org/src/traversal/executor.rs` | Rol yaprağı için relational EXISTS SQL; bind plumbing `Vec<String>`'e; `fetch_global_type` → `pub(crate)` | Modify |
| `crates/org/src/repo/user_role.rs` | `check_user_role` devralma+exclusion; `resolve_global_type`'ı executor'a bağla; `grant_orgu_role`/`revoke_orgu_role` | Modify |
| `crates/server/src/routes/org.rs` | `/orgtnt/:id/orgu-roles` POST/DELETE uçları + DTO'lar | Modify |
| `data/seed_qnb_regions.sql`, `data/seed_qnb_users.sql` | Seed'i yeni modele taşı (special yazma → orgu_r grant'ı) | Modify |

---

## Task 1: Migration — `org.orgu_r` tablosu + veri taşıması

**Files:**
- Create: `migrations/org/20260722000001_orgu_role.sql`

- [ ] **Step 1: Migration dosyasını yaz**

```sql
-- ================================================================
-- special → unit-inherited role.
-- 'special' orgu niteliği kaldırılır; yerine org.orgu_r (birime rol grant'ı)
-- gelir. Bir orgu'ya verilen rol, o orgudaki tüm kullanıcılarca devralınır
-- (check_user_role). Kullanıcı-rol (org.ur) DOKUNULMAZ; tek katalog org.r.
--
-- Migration tek başına ve tekrar tekrar koşabilmeli (idempotent). gen_random_uuid()
-- (PG13+, pg_catalog) kullanılır — eklenti/search_path bağımlılığı yoktur.
-- ================================================================
CREATE SCHEMA IF NOT EXISTS org;

-- 1) Birim-rol tablosu (org.ur'ye dokunulmaz; ayrı tablo).
CREATE TABLE IF NOT EXISTS org.orgu_r (
    orgu_r_id   uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    orgtnt_id   uuid        NOT NULL REFERENCES org.orgtnt(orgtnt_id),
    orgu_id     uuid        NOT NULL REFERENCES org.orgu(orgu_id),
    r_id        uuid        NOT NULL REFERENCES org.r(r_id),
    ur_type     text        NOT NULL DEFAULT 'granted'
                CHECK (ur_type IN ('inherited','granted')),
    valid_from  timestamptz,
    valid_until timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now(),
    UNIQUE (orgu_id, r_id)
);
CREATE INDEX IF NOT EXISTS orgu_r_orgu_idx ON org.orgu_r(orgu_id);
CREATE INDEX IF NOT EXISTS orgu_r_r_idx    ON org.orgu_r(r_id);

-- 2) Mevcut orgu_type.special değerlerini role kataloğuna taşı (idempotent).
INSERT INTO org.r (orgtnt_id, name, display_name)
SELECT DISTINCT oo.orgtnt_id, s.val, s.val
FROM org.orgu o
JOIN org.orgt_orgu oo ON o.orgu_id = oo.orgu_id
CROSS JOIN LATERAL jsonb_array_elements_text(o.orgu_type->'special') AS s(val)
WHERE o.orgu_type ? 'special'
ON CONFLICT (orgtnt_id, name) DO NOTHING;

-- 3) Her special içeren birim için org.orgu_r grant'ı (idempotent).
INSERT INTO org.orgu_r (orgtnt_id, orgu_id, r_id, ur_type)
SELECT DISTINCT oo.orgtnt_id, o.orgu_id, r.r_id, 'granted'
FROM org.orgu o
JOIN org.orgt_orgu oo ON o.orgu_id = oo.orgu_id
CROSS JOIN LATERAL jsonb_array_elements_text(o.orgu_type->'special') AS s(val)
JOIN org.r r ON r.orgtnt_id = oo.orgtnt_id AND r.name = s.val
WHERE o.orgu_type ? 'special'
ON CONFLICT (orgu_id, r_id) DO NOTHING;

-- 4) orgu_type'tan 'special' anahtarını kaldır ('type' kalır).
UPDATE org.orgu
SET orgu_type = orgu_type - 'special'
WHERE orgu_type ? 'special';
```

- [ ] **Step 2: Scratch DB'ye uygula ve doğrula**

Run:
```bash
psql "$DATABASE_URL" -f migrations/org/20260722000001_orgu_role.sql
psql "$DATABASE_URL" -c "\d org.orgu_r"
psql "$DATABASE_URL" -c "SELECT count(*) FROM org.orgu WHERE orgu_type ? 'special';"
psql "$DATABASE_URL" -c "SELECT count(*) FROM org.orgu_r;"
```
Expected: `org.orgu_r` tablosu listelenir; `special` içeren orgu sayısı **0**; `org.orgu_r` satır sayısı **> 0** (döviz/kredi birimleri taşındı).

- [ ] **Step 3: İdempotentliği doğrula (ikinci kez koştur)**

Run: `psql "$DATABASE_URL" -f migrations/org/20260722000001_orgu_role.sql`
Expected: Hata yok; satır sayıları değişmez (yeni ekleme olmaz).

- [ ] **Step 4: Commit**

```bash
git add migrations/org/20260722000001_orgu_role.sql
git commit -m "feat(org): org.orgu_r tablosu + special→role veri taşıması"
```

---

## Task 2: Parser — virgül-OR (`[role:a,b]`) + `parse_filter` ihracı

**Files:**
- Modify: `crates/org/src/traversal/parser.rs`
- Test: aynı dosyanın `#[cfg(test)]` bloğu

- [ ] **Step 1: Başarısız testleri yaz**

`crates/org/src/traversal/parser.rs` test bloğuna ekle (mevcut `test_and_filter` yakınına). Yardımcılar `leaf`/`kleaf`/`steps` zaten var:

```rust
    #[test]
    fn test_role_comma_expands_to_or() {
        assert_eq!(
            steps("self.children[role:doviz,kredi]"),
            vec![Step::ChildrenT(FilterExpr::Or(vec![
                kleaf("role", "doviz"),
                kleaf("role", "kredi"),
            ]))]
        );
    }

    #[test]
    fn test_role_comma_single_value_stays_leaf() {
        assert_eq!(
            steps("self.children[role:doviz]"),
            vec![Step::ChildrenT(kleaf("role", "doviz"))]
        );
    }

    #[test]
    fn test_role_comma_combines_with_and() {
        // (doviz OR kredi) AND type=sube
        assert_eq!(
            steps("self.children[role:doviz,kredi && type:sube]"),
            vec![Step::ChildrenT(FilterExpr::And(vec![
                FilterExpr::Or(vec![kleaf("role", "doviz"), kleaf("role", "kredi")]),
                leaf("sube"),
            ]))]
        );
    }
```

- [ ] **Step 2: Testin başarısız olduğunu gör**

Run: `cargo test -p wf-org parser::tests::test_role_comma -- --nocapture`
Expected: FAIL — `tokenize_filter` virgülde `InvalidFilter("unexpected ','")` döner (virgül ident karakteri değil).

- [ ] **Step 3: Tokenizer'a virgülü ident karakteri olarak ekle**

`crates/org/src/traversal/parser.rs`, `tokenize_filter` içindeki ident tarama kolunda (mevcut satır ~163-176) virgülü kabul et. İki `c.is_alphanumeric() || c == '_' || c == '-'` / `... || c == ':'` koşuluna `|| c == ','` ekle:

```rust
            c if c.is_alphanumeric() || c == '_' || c == '-' || c == ',' => {
                let mut ident = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_alphanumeric() || c == '_' || c == '-' || c == ':' || c == ',' {
                        ident.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                out.push(FTok::Ident(ident));
            }
```

- [ ] **Step 4: `parse_atom`'da virgülü OR'a aç**

Aynı dosyada `parse_atom`'ın `Some(FTok::Ident(_))` kolu (mevcut satır ~277-285) `FilterExpr::Leaf(parse_type_filter(&s))` döndürüyor. Bunu virgül-farkındalıklı bir yardımcıyla değiştir. `parse_type_filter`'ın altına ekle:

```rust
/// Bir atom ident'ini FilterExpr'e çevirir. Değer virgül içeriyorsa
/// (`role:doviz,kredi`) aynı anahtarla OR'a açılır; tek değer Leaf kalır.
fn atom_to_expr(s: &str) -> FilterExpr {
    let tf = parse_type_filter(s);
    if tf.val.contains(',') {
        let leaves: Vec<FilterExpr> = tf
            .val
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(|p| FilterExpr::Leaf(TypeFilter::new(tf.key.clone(), p)))
            .collect();
        match leaves.len() {
            0 => FilterExpr::Leaf(TypeFilter::new(tf.key, "")),
            1 => leaves.into_iter().next().unwrap(),
            _ => FilterExpr::Or(leaves),
        }
    } else {
        FilterExpr::Leaf(tf)
    }
}
```

`parse_atom`'daki ilgili satırı değiştir:

```rust
                self.advance();
                Ok(atom_to_expr(&s))
```

- [ ] **Step 5: `TypeFilter::new`'i modül içinden erişilebilir yap (gerekiyorsa)**

`atom_to_expr` `TypeFilter::new` çağırıyor; `new` şu an `fn new` (private). Aynı modülde olduğu için erişilebilir — değişiklik gerekmez. Doğrula: `parse_type_filter` da `TypeFilter::new` kullanıyor (aynı modül). OK.

- [ ] **Step 6: Testlerin geçtiğini gör**

Run: `cargo test -p wf-org parser::tests::test_role_comma`
Expected: PASS (üç test).

- [ ] **Step 7: `parse_filter` public ihracını ekle (Task 4 için)**

`parse_filter_expr` private. Public sarmalayıcı ekle (mevcut `pub fn parse` yakınına):

```rust
/// `[...]` içindeki filtre ifadesini (köşeli parantezsiz iç metin) FilterExpr'e çevirir.
/// `*:` global selektör çözümünde executor filtre motoruyla paylaşılır.
pub fn parse_filter(inner: &str) -> Result<FilterExpr, ParseError> {
    parse_filter_expr(inner)
}
```

- [ ] **Step 8: Special örnekli mevcut testleri role'e çevir**

`special` retire edildiği için örnek testleri güncelle. `test_key_val_children` (satır ~473) ve `test_and_filter` (satır ~497):

```rust
    #[test]
    fn test_key_val_children() {
        assert_eq!(
            steps("self.children[role:doviz]"),
            vec![Step::ChildrenT(kleaf("role", "doviz"))]
        );
    }
```
```rust
    #[test]
    fn test_and_filter() {
        assert_eq!(
            steps("self.children[role:kredi && type:sube]"),
            vec![Step::ChildrenT(FilterExpr::And(vec![
                kleaf("role", "kredi"),
                leaf("sube")
            ]))]
        );
    }
```

- [ ] **Step 9: Parser testlerinin tümü geçsin + commit**

Run: `cargo test -p wf-org parser`
Expected: PASS (tüm parser testleri).

```bash
git add crates/org/src/traversal/parser.rs
git commit -m "feat(org): ORGTRVLANG [role:a,b] virgül-OR + parse_filter ihracı"
```

---

## Task 3: Executor — rol yaprağı relational EXISTS SQL

**Files:**
- Modify: `crates/org/src/traversal/executor.rs`

Rol yaprağı JSONB değil, `org.orgu_r` join'i ister. Bind plumbing bugün leaf başına 2 param'lık tuple (`Vec<(String,String)>`); rol yaprağı tek param (rol adı) kullandığından plumbing düz `Vec<String>`'e çevrilir.

- [ ] **Step 1: `filter_sql`'i düz param listesine + rol koluna çevir**

Mevcut `filter_sql` (satır 136-162) tümüyle değiştirilir:

```rust
fn filter_sql(expr: &FilterExpr, idx: &mut usize) -> (String, Vec<String>) {
    match expr {
        // Rol yaprağı: relational — birimin org.orgu_r grant'ı var mı?
        FilterExpr::Leaf(tf) if tf.key == "role" => {
            let v = *idx;
            *idx += 1;
            let sql = format!(
                "EXISTS (SELECT 1 FROM org.orgu_r orr \
                    JOIN org.r rr ON orr.r_id = rr.r_id \
                  WHERE orr.orgu_id = m.orgu_id AND rr.name = ${v} \
                    AND rr.is_active = true \
                    AND (orr.valid_from  IS NULL OR orr.valid_from  <= now()) \
                    AND (orr.valid_until IS NULL OR orr.valid_until >  now()))"
            );
            (sql, vec![tf.val.clone()])
        }
        // Tip/JSONB yaprağı: mevcut davranış.
        FilterExpr::Leaf(tf) => {
            let k = *idx;
            *idx += 1;
            let v = *idx;
            *idx += 1;
            let sql = format!(
                "(m.orgu_type->>${} = ${} OR m.orgu_type->${} @> to_jsonb(${}::text))",
                k, v, k, v
            );
            (sql, vec![tf.key.clone(), tf.val.clone()])
        }
        FilterExpr::Not(inner) => {
            let (s, b) = filter_sql(inner, idx);
            (format!("NOT {s}"), b)
        }
        FilterExpr::And(exprs) => {
            let (parts, binds) = collect_filter_parts(exprs, idx);
            (format!("({})", parts.join(" AND ")), binds)
        }
        FilterExpr::Or(exprs) => {
            let (parts, binds) = collect_filter_parts(exprs, idx);
            (format!("({})", parts.join(" OR ")), binds)
        }
    }
}
```

- [ ] **Step 2: `collect_filter_parts`'ı düz listeye çevir**

Mevcut (satır 164-176) → dönüş tipi `Vec<String>`:

```rust
fn collect_filter_parts(exprs: &[FilterExpr], idx: &mut usize) -> (Vec<String>, Vec<String>) {
    let mut parts = Vec::new();
    let mut binds = Vec::new();
    for e in exprs {
        let (s, b) = filter_sql(e, idx);
        parts.push(s);
        binds.extend(b);
    }
    (parts, binds)
}
```

- [ ] **Step 3: `fetch_global_type` bind döngüsünü düz listeye çevir + `pub(crate)` yap**

Mevcut `async fn fetch_global_type` (satır 104-134) imzasını `pub(crate)` yap ve bağlama döngüsünü güncelle:

```rust
pub(crate) async fn fetch_global_type(
    pool: &PgPool,
    orgtnt_id: Uuid,
    filter: &FilterExpr,
) -> Result<Vec<Orgu>, OrgError> {
    let mut idx = 2usize; // $1 = orgtnt_id
    let (fsql, bindings) = filter_sql(filter, &mut idx);
    let sql = format!(
        "SELECT m.orgu_id, oo.orgtnt_id, oo.orgt_id, oo.parent_orgu_id, \
             oo.path::text AS path, m.orgu_type, m.name, m.metadata, \
             (m.is_active AND oo.is_active) AS is_active, m.created_at, m.updated_at \
         FROM org.orgu m \
         JOIN org.orgt_orgu oo ON m.orgu_id = oo.orgu_id \
         WHERE oo.orgtnt_id = $1 AND m.is_active = true AND oo.is_active = true \
           AND (m.orgu_type ? '*' OR {fsql})"
    );
    let mut args = PgArguments::default();
    args.add(orgtnt_id);
    for p in bindings {
        args.add(p);
    }
    sqlx::query_as_with::<_, Orgu, _>(&sql, args)
        .fetch_all(pool)
        .await
        .map_err(OrgError::Database)
}
```

- [ ] **Step 4: `run_filtered` bind döngüsünü düz listeye çevir**

Mevcut `run_filtered` (satır 178-196) imzası `bindings: Vec<(String, String)>` → `bindings: Vec<String>`, döngü:

```rust
async fn run_filtered(
    pool: &PgPool,
    sql: String,
    ids: &[Uuid],
    orgt_id: Uuid,
    bindings: Vec<String>,
) -> Result<Vec<Orgu>, OrgError> {
    let mut args = PgArguments::default();
    args.add(ids.to_vec());
    args.add(orgt_id);
    for p in bindings {
        args.add(p);
    }
    sqlx::query_as_with::<_, Orgu, _>(&sql, args)
        .fetch_all(pool)
        .await
        .map_err(OrgError::Database)
}
```

- [ ] **Step 5: Derle**

Run: `cargo build -p wf-org`
Expected: Derleme başarılı. `execute_step` içindeki `filter_sql`/`run_filtered` çağrıları imza değişmediğinden (yalnızca bind eleman tipi) uyumlu; tip hatası çıkarsa `execute_step`'teki `let (fsql, bindings) = filter_sql(...)` kullanımını kontrol et — `bindings` artık `Vec<String>` ve `run_filtered`'a olduğu gibi geçer.

- [ ] **Step 6: Commit**

```bash
git add crates/org/src/traversal/executor.rs
git commit -m "feat(org): ORGTRVLANG [role:X] relational EXISTS (org.orgu_r) + düz bind"
```

---

## Task 4: `resolve_global_type`'ı executor filtre motoruna bağla

**Files:**
- Modify: `crates/org/src/repo/user_role.rs`

`*:` global yolu bugün elle tek `key:val` SQL'i çalıştırıyor; `&&`/`||`/virgül ve `role` desteklemiyor. Executor'ın `fetch_global_type`'ına delege ederek hepsini kazanır.

- [ ] **Step 1: `resolve_global_type`'ı yeniden yaz**

Mevcut (satır 142-178) tümüyle değiştirilir:

```rust
/// `*:[filter]` — tenant genelinde filtreye uyan orgu üyelikleri.
/// Executor'ın filtre motoruna delege eder (type/role, &&/||, virgül-OR hepsi desteklenir).
async fn resolve_global_type(
    pool: &PgPool,
    type_expr: &str,
    orgtnt_id: Uuid,
) -> Result<Vec<OrgUnit>, OrgError> {
    let inner = type_expr.trim().trim_start_matches('[').trim_end_matches(']');
    let filter = parser::parse_filter(inner).map_err(|e| OrgError::BadRequest(e.to_string()))?;
    let orgus = executor::fetch_global_type(pool, orgtnt_id, &filter).await?;
    Ok(orgus.into_iter().map(OrgUnit::from).collect())
}
```

- [ ] **Step 2: Derle**

Run: `cargo build -p wf-org`
Expected: Başarılı. `parser::parse_filter` (Task 2) ve `executor::fetch_global_type` `pub(crate)` (Task 3) erişilebilir. `OrgUnit::from(Orgu)` `models.rs:96`'da tanımlı. Kullanılmayan importlar (`serde_json`) için uyarı çıkarsa temizle.

- [ ] **Step 3: Commit**

```bash
git add crates/org/src/repo/user_role.rs
git commit -m "refactor(org): *: global selektör tek filtre motorunda birleşti (role dahil)"
```

---

## Task 5: `check_user_role` — orgu devralma + exclusion

**Files:**
- Modify: `crates/org/src/repo/user_role.rs`

- [ ] **Step 1: SQL'i `(A OR B) AND NOT C` olarak yeniden yaz**

Mevcut `check_user_role` (satır 94-121) gövdesindeki `sqlx::query_scalar` SQL'ini değiştir (imza aynı kalır):

```rust
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT
            ( EXISTS (
                SELECT 1 FROM org.ur u JOIN org.r r ON u.r_id = r.r_id
                WHERE u.u_id = $1 AND u.orgu_id = $2 AND r.name = $3
                  AND r.is_active = true AND u.ur_type != 'excluded'
                  AND (u.valid_from  IS NULL OR u.valid_from  <= now())
                  AND (u.valid_until IS NULL OR u.valid_until >  now()) )
              OR EXISTS (
                SELECT 1 FROM org.orgu_r orr JOIN org.r r ON orr.r_id = r.r_id
                WHERE orr.orgu_id = $2 AND r.name = $3
                  AND r.is_active = true
                  AND (orr.valid_from  IS NULL OR orr.valid_from  <= now())
                  AND (orr.valid_until IS NULL OR orr.valid_until >  now()) ) )
          AND NOT EXISTS (
                SELECT 1 FROM org.ur u JOIN org.r r ON u.r_id = r.r_id
                WHERE u.u_id = $1 AND u.orgu_id = $2 AND r.name = $3
                  AND u.ur_type = 'excluded' )",
    )
    .bind(user_id)
    .bind(orgu_id)
    .bind(role_name)
    .fetch_one(pool)
    .await?;
    Ok(exists)
```

Doküman yorumunu da güncelle (fonksiyon üstü): "org.ur doğrudan grant VEYA org.orgu_r birim grant'ı; org.ur 'excluded' satırı her ikisini de ezer."

- [ ] **Step 2: Derle + workspace testleri**

Run: `cargo test --workspace`
Expected: Derleme başarılı; mevcut testler geçer (bu SQL değişikliği DB-backed test kapsamında değil, derleme + regresyon kontrolü).

- [ ] **Step 3: Commit**

```bash
git add crates/org/src/repo/user_role.rs
git commit -m "feat(org): check_user_role — orgu-devralınan rol + org.ur excluded override"
```

---

## Task 6: Repo — `grant_orgu_role` / `revoke_orgu_role`

**Files:**
- Modify: `crates/org/src/repo/user_role.rs`

- [ ] **Step 1: İki fonksiyonu ekle**

`grant_assignment`/`revoke_assignment` yakınına ekle:

```rust
/// Bir orgu'ya rol grant'ı: org.orgu_r satırı. Rol yoksa oluşturulur (ensure_role).
/// İdempotent — aynı (orgu, rol) tekrar çağrılırsa yeni satır eklenmez.
pub async fn grant_orgu_role(
    pool: &PgPool,
    orgtnt_id: Uuid,
    orgu_id: Uuid,
    role_name: &str,
) -> Result<(), OrgError> {
    let role = ensure_role(pool, orgtnt_id, role_name).await?;
    sqlx::query(
        "INSERT INTO org.orgu_r (orgtnt_id, orgu_id, r_id, ur_type)
         VALUES ($1, $2, $3, 'granted')
         ON CONFLICT (orgu_id, r_id) DO NOTHING",
    )
    .bind(orgtnt_id)
    .bind(orgu_id)
    .bind(role.r_id)
    .execute(pool)
    .await
    .map_err(OrgError::Database)?;
    Ok(())
}

/// `grant_orgu_role`'ün tersi: bir orgu'nun rol grant'ını (org.orgu_r) kaldırır.
/// Etkilenen satır varsa true.
pub async fn revoke_orgu_role(
    pool: &PgPool,
    orgtnt_id: Uuid,
    orgu_id: Uuid,
    role_name: &str,
) -> Result<bool, OrgError> {
    let r = sqlx::query(
        "DELETE FROM org.orgu_r
         WHERE orgtnt_id = $1 AND orgu_id = $2
           AND r_id = (SELECT r_id FROM org.r WHERE orgtnt_id = $1 AND name = $3)",
    )
    .bind(orgtnt_id)
    .bind(orgu_id)
    .bind(role_name)
    .execute(pool)
    .await
    .map_err(OrgError::Database)?;
    Ok(r.rows_affected() > 0)
}
```

- [ ] **Step 2: Derle + commit**

Run: `cargo build -p wf-org`
Expected: Başarılı.

```bash
git add crates/org/src/repo/user_role.rs
git commit -m "feat(org): grant_orgu_role / revoke_orgu_role repo uçları"
```

---

## Task 7: Server API — `/orgtnt/:id/orgu-roles` uçları

**Files:**
- Modify: `crates/server/src/routes/org.rs`

Not: Mevcut `assignments` desenine paralel — POST gövdeyle, DELETE query paramlarıyla (revoke_assignment ile tutarlı).

- [ ] **Step 1: Route kaydını ekle**

`router` fn'de (satır 15-48) `assignments` route'unun ardına ekle:

```rust
        .route(
            "/orgtnt/:id/orgu-roles",
            post(create_orgu_role).delete(delete_orgu_role),
        )
```

- [ ] **Step 2: DTO + handler'ları ekle**

`create_assignment`/`revoke_assignment` yakınına ekle:

```rust
/// Orgu'ya rol grant'ı: birimdeki tüm kullanıcılar bu rolü devralır (check_user_role).
#[derive(Deserialize)]
struct OrguRoleBody {
    orgu_id: Uuid,
    role_name: String,
}

async fn create_orgu_role(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
    Json(body): Json<OrguRoleBody>,
) -> Result<axum::http::StatusCode, AppError> {
    let role_name = body.role_name.trim();
    if role_name.is_empty() {
        return Err(AppError(
            "rol adı boş olamaz".into(),
            axum::http::StatusCode::BAD_REQUEST,
        ));
    }
    repo::user_role::grant_orgu_role(&pool, orgtnt_id, body.orgu_id, role_name)
        .await
        .map_err(AppError::from)?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// Orgu rol grant'ını kaldırma: (orgu_id, role_name) query paramlarıyla.
#[derive(Deserialize)]
struct RevokeOrguRoleQuery {
    orgu_id: Uuid,
    role_name: String,
}

async fn delete_orgu_role(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
    Query(q): Query<RevokeOrguRoleQuery>,
) -> Result<axum::http::StatusCode, AppError> {
    let removed = repo::user_role::revoke_orgu_role(&pool, orgtnt_id, q.orgu_id, &q.role_name)
        .await
        .map_err(AppError::from)?;
    if removed {
        Ok(axum::http::StatusCode::NO_CONTENT)
    } else {
        Err(AppError(
            "birim-rol bulunamadı".into(),
            axum::http::StatusCode::NOT_FOUND,
        ))
    }
}
```

- [ ] **Step 3: Derle + commit**

Run: `cargo build -p wf-server`
Expected: Başarılı (`post`, `Query`, `Json` importları zaten mevcut).

```bash
git add crates/server/src/routes/org.rs
git commit -m "feat(server): /org/orgtnt/:id/orgu-roles POST/DELETE uçları"
```

---

## Task 8: Seed dosyalarını yeni modele taşı

**Files:**
- Modify: `data/seed_qnb_regions.sql`
- Modify: `data/seed_qnb_users.sql`

Seed'ler taze DB'de çalışır; migration'a güvenemez → special yerine doğrudan `org.orgu_r` yazmalı.

- [ ] **Step 1: `seed_orgu_type`'ı yalnızca `{type}` döndürecek şekilde sadeleştir**

`data/seed_qnb_regions.sql`, `seed_orgu_type` fn'inin gövdesini (satır 27-53) değiştir — special mantığını çıkar:

```sql
CREATE OR REPLACE FUNCTION seed_orgu_type(
    p_orgu_t text,
    p_code   text,
    p_name   text
) RETURNS jsonb LANGUAGE plpgsql IMMUTABLE AS $$
BEGIN
    -- special kaldırıldı; birim-rolleri seed sonunda org.orgu_r'a yazılır.
    RETURN jsonb_build_object('type', p_orgu_t);
END;
$$;
```

- [ ] **Step 2: `seed_qnb_regions.sql` sonuna birim-rol grant bloğu ekle**

Orgular ve `org.orgt_orgu` bağları kurulduktan SONRA (dosyanın en sonuna) — eski regex mantığını org.orgu_r'a yazacak biçimde. Tenant id sabiti seed_qnb_users ile aynıdır (`3c1811a6-1e63-4261-a1ce-658da1fbfa6b`):

```sql
-- ================================================================
-- Birim-rolleri: eski 'special' regex'i artık org.orgu_r grant'ı üretir.
-- ================================================================
DO $$
DECLARE
    c uuid := '3c1811a6-1e63-4261-a1ce-658da1fbfa6b';
    r_kredi uuid; r_doviz uuid;
BEGIN
    INSERT INTO org.r (orgtnt_id, name, display_name) VALUES
        (c, 'kredi', 'Kredi'), (c, 'doviz', 'Döviz')
    ON CONFLICT (orgtnt_id, name) DO NOTHING;
    SELECT r_id INTO r_kredi FROM org.r WHERE orgtnt_id = c AND name = 'kredi';
    SELECT r_id INTO r_doviz FROM org.r WHERE orgtnt_id = c AND name = 'doviz';

    -- kredi birimleri
    INSERT INTO org.orgu_r (orgtnt_id, orgu_id, r_id, ur_type)
    SELECT c, o.orgu_id, r_kredi, 'granted'
    FROM org.orgu o
    JOIN org.orgt_orgu oo ON o.orgu_id = oo.orgu_id
    WHERE oo.orgtnt_id = c
      AND ( (o.metadata->>'code') ~* 'kredi'
            OR o.name ~* 'kredi|finansman|fon yonetimi|fon yönetimi' )
    ON CONFLICT (orgu_id, r_id) DO NOTHING;

    -- doviz birimleri
    INSERT INTO org.orgu_r (orgtnt_id, orgu_id, r_id, ur_type)
    SELECT c, o.orgu_id, r_doviz, 'granted'
    FROM org.orgu o
    JOIN org.orgt_orgu oo ON o.orgu_id = oo.orgu_id
    WHERE oo.orgtnt_id = c
      AND ( (o.metadata->>'code') ~* 'havalimani|pasaport|serbest-bolge|international|laleli|karakoy|sultanhamam|nisantasi|bodrum|marmaris|fethiye|kusadasi|cesme|yalikavak|ortakoy'
            OR o.name ~* 'havaliman|pasaport|serbest bolge|serbest bölge|international|laleli|karakoy|karaköy|sultanhamam|nisantasi|nişantaşı|bodrum|marmaris|fethiye|kuşadası|çeşme|yalıkavak|ortaköy' )
    ON CONFLICT (orgu_id, r_id) DO NOTHING;
END $$;
```

Not: Kod eşleşmesi `metadata->>'code'` üzerinden yapılır (orgu kodları `metadata.code`'da tutulur — `orgu_seed_code_unique` indeksi bunu doğrular). Eğer seed'de kod başka alanda ise, implementasyon sırasında `seed_orgu_type` çağrısına giden `p_code` kaynağını izleyip aynı ifadeyi kullan.

- [ ] **Step 3: `seed_qnb_users.sql` döviz-birim sorgusunu org.orgu_r'a çevir**

`data/seed_qnb_users.sql` satır 99-101'deki `orgu_type -> 'special'` okumasını org.orgu_r join'iyle değiştir:

```sql
    FOR rec IN
        SELECT o.orgu_id, o.name
        FROM org.orgu o
        JOIN org.orgt_orgu oo ON o.orgu_id = oo.orgu_id
        JOIN org.orgu_r orr   ON orr.orgu_id = o.orgu_id
        JOIN org.r rr         ON rr.r_id = orr.r_id AND rr.name = 'doviz'
        WHERE oo.orgtnt_id = c AND o.is_active = true AND oo.is_active = true
    LOOP
```

Blok başındaki yorumdaki `*:[special:doviz]` referansını `*:[role:doviz]` olarak güncelle.

- [ ] **Step 4: Seed'leri taze DB'de doğrula**

Run:
```bash
psql "$DATABASE_URL" -f data/seed_qnb_regions.sql
psql "$DATABASE_URL" -f data/seed_qnb_users.sql
psql "$DATABASE_URL" -c "SELECT count(*) FROM org.orgu WHERE orgu_type ? 'special';"
psql "$DATABASE_URL" -c "SELECT r.name, count(*) FROM org.orgu_r orr JOIN org.r r ON orr.r_id=r.r_id GROUP BY r.name;"
```
Expected: special içeren orgu **0**; `doviz`/`kredi` grant sayıları **> 0**.

- [ ] **Step 5: Commit**

```bash
git add data/seed_qnb_regions.sql data/seed_qnb_users.sql
git commit -m "feat(data): seed'ler special yerine org.orgu_r grant'ı üretir"
```

---

## Task 9: Uçtan uca doğrulama

**Files:** (yok — doğrulama)

- [ ] **Step 1: Tüm workspace derlensin ve testler geçsin**

Run: `cargo test --workspace`
Expected: PASS. Golden fixture testleri dahil hepsi yeşil (fixture `special` içermez).

- [ ] **Step 2: Migration + seed'i temiz bir scratch DB'de baştan uygula**

Run: (org migration'larını sırayla, sonra wf, sonra seed)
```bash
for f in migrations/org/*.sql; do psql "$DATABASE_URL" -f "$f"; done
psql "$DATABASE_URL" -f data/seed_qnb_regions.sql
psql "$DATABASE_URL" -f data/seed_qnb_users.sql
```
Expected: Hata yok.

- [ ] **Step 3: `*:[role:X]` global selektörünü doğrula (server ayakta)**

Server'ı başlat (`cargo run -p wf-server`), sonra ORGTRVLANG'ı bir orgu üzerinden test et:
```bash
curl -s "http://localhost:$PORT/org/orgu/<bir-orgu-id>/traverse?expr=*:%5Brole:doviz%5D" | jq 'length'
```
Expected: > 0 (döviz birimleri döner). `*:[role:doviz,kredi]` ile de dener → döviz VEYA kredi birimleri (OR).

- [ ] **Step 4: `check_user_role` devralmasını doğrula (psql)**

Bir döviz biriminde, o birime bireysel `org.ur` rolü OLMAYAN bir kullanıcı seç ve check'i simüle et:
```bash
psql "$DATABASE_URL" -c "
  SELECT
    ( EXISTS (SELECT 1 FROM org.orgu_r orr JOIN org.r r ON orr.r_id=r.r_id
              WHERE orr.orgu_id='<doviz-orgu-id>' AND r.name='doviz') ) AS orgu_grants_doviz;"
```
Expected: `t`. Ardından o birimdeki bir kullanıcı için `POST /org/orgtnt/:id/orgu-roles` yerine mevcut atama uçlarıyla değil, engine akışında (sim portalı) döviz rolü isteyen bir node'da bu kullanıcının **claim edebildiğini** gözle (bireysel atama olmadan). Bir `org.ur` `excluded` satırı eklendiğinde aynı kullanıcının claim edemediğini gözle.

- [ ] **Step 5: `/orgu-roles` uçlarını doğrula**

```bash
curl -s -X POST "http://localhost:$PORT/org/orgtnt/<tid>/orgu-roles" \
  -H 'content-type: application/json' \
  -d '{"orgu_id":"<orgu-id>","role_name":"doviz"}' -w '%{http_code}\n'
curl -s -X DELETE "http://localhost:$PORT/org/orgtnt/<tid>/orgu-roles?orgu_id=<orgu-id>&role_name=doviz" -w '%{http_code}\n'
```
Expected: İkisi de `204`; tekrar DELETE → `404`.

- [ ] **Step 6: Regression — `[type:X]` ve `&&`/`||` hâlâ çalışıyor**

```bash
curl -s "http://localhost:$PORT/org/orgu/<orgu-id>/traverse?expr=*:%5Btype:sube%5D" | jq 'length'
```
Expected: > 0 (şubeler). Tip filtresi bozulmamış.

- [ ] **Step 7: Docs — CLAUDE.md crate haritası notu (opsiyonel, küçük)**

`org` satırındaki "ORGTRVLANG parser/executor" açıklamasına `special`'ın kalktığını yansıtan tek kelimelik güncelleme uygunsa yap; spec zaten `docs/superpowers/specs/` altında.

---

## Self-Review Notları

- **Spec kapsamı:** org.orgu_r (§1)→T1/T6/T7; check_user_role (§2)→T5; ORGTRVLANG [role:X]+array (§3)→T2/T3/T4; migration (§4)→T1; API (§5)→T7; seed→T8; test/verify (§6)→T9. Tümü kapsandı.
- **Tip tutarlılığı:** `filter_sql`/`collect_filter_parts`/`run_filtered`/`fetch_global_type` hepsi `Vec<String>` bind'e çevrildi (T3, tek atomik değişiklik). `parse_filter` (T2) ↔ `fetch_global_type pub(crate)` (T3) ↔ `resolve_global_type` (T4) imzaları uyumlu.
- **Virgül-OR** parser'da çözülür (T2) → executor rol yaprağı tek-değerlidir (T3), `= $v` yeterli, `ANY`/`string_to_array` gerekmez.
- **DB-backed test yok:** repo gerçeğine uygun; SQL katmanı T9'da psql/curl ile somut doğrulanır.
