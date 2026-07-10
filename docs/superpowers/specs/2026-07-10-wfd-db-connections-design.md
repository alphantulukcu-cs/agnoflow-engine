# WFD Veritabanı Bağlantı Entegrasyonu — Tasarım

**Tarih:** 2026-07-10
**Kapsam:** `workflow-engine` (çok-sürücülü DB registry + `wf.db_connection` deposu + `/db/connections` API + SQL node runtime bağlama) ve `WFD-EDITOR` (Çalıştırma sekmesinde "Veritabanı" yönetim UI'ı + SQL node bağlantı seçimi).

## Amaç

Tasarımcı, VSCode DB extension'ı gibi editörden **birden fazla veritabanı bağlantısı** (MySQL, PostgreSQL, Microsoft SQL) tanımlayabilsin, her DB tipine göre şekillenen formla girsin (host/port ya da bağlantı dizesi/JDBC), **bağlantıyı test edebilsin**, ve tanımlı bağlantıları **SQL autoexec node** içinde kullanabilsin. Konfigürasyonlar (özellikle parolalar) DB'de **şifreli** saklanır.

## Onaylanan kararlar

1. **Tam uçtan uca:** engine gerçekten seçilen bağlantıya bağlanıp SQL çalıştırır (sadece UI değil).
2. **Üç sürücü de baştan:** Postgres + MySQL (`sqlx`), Microsoft SQL (`tiberius` — sqlx MSSQL desteklemez).
3. **Şifreli saklama:** bağlantı konfigürasyonu `wf.db_connection` tablosunda; parola/gizli alanlar **AES-256-GCM** ile şifreli (anahtar env `DB_CONN_SECRET`, base64 32 byte). İstemciye parola **asla** dönmez (write-only).
4. **İki faz:** Faz 1 = bağlantı yönetimi + test + UI. Faz 2 = SQL node'a bağlama + çok-sürücülü runtime çalıştırma.

## Mevcut durum (kısıt)

Bugün SQL autoexec node TEK bir PostgreSQL `PgPool` ile çalışıyor (`crates/wfe/src/runner.rs::run_sql`, sqlx). Çoklu-sürücü ve isimli bağlantı yok. Bu tasarım bunu bir **bağlantı registry**'siyle genişletir.

## Mimari

### Sürücü soyutlaması (engine)

Yeni bir `db` modülü (öneri: `crates/wfe` içinde `db/` veya ayrı `crates/db` krati):

```
enum DbDriver { Postgres, Mysql, Mssql }

trait DbConn: Send + Sync {
    async fn test(&self) -> Result<(), DbError>;                 // SELECT 1
    async fn run_query(&self, sql: &str, params: &[Value])       // sürücü-bağımsız satır→JSON
        -> Result<Vec<Map<String,Value>>, DbError>;
}
```

- Postgres/MySQL implementasyonu `sqlx` (`PgPool` / `MySqlPool`).
- MSSQL implementasyonu `tiberius` (+ `tokio-util` compat) — `Config`'ten TCP.
- **Registry:** `HashMap<connection_id, Arc<dyn DbConn>>`, lazy açılır; connection güncellenince/silinince invalidate.
- Satır→JSON: her sürücü kendi kolon tiplerini serde `Value`'ya map eder (mevcut Postgres map'i `runner.rs`'ten taşınır).

### Depo — `wf.db_connection` (yeni migration)

```sql
CREATE TABLE wf.db_connection (
    id           uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgtnt_id    uuid        NOT NULL,
    name         text        NOT NULL,
    driver       text        NOT NULL CHECK (driver IN ('postgres','mysql','mssql')),
    -- Alan modu VEYA bağlantı-dizesi modu (mode ile ayrılır)
    mode         text        NOT NULL DEFAULT 'fields' CHECK (mode IN ('fields','uri')),
    host         text,
    port         integer,
    database     text,
    username     text,
    options      jsonb       NOT NULL DEFAULT '{}',   -- ssl, instance, encrypt, connection_string...
    secret_enc   bytea,                                -- AES-GCM(nonce || ciphertext): parola / dizedeki gizli
    is_active    boolean     NOT NULL DEFAULT true,
    last_test_at timestamptz,
    last_test_ok boolean,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (orgtnt_id, name)
);
CREATE INDEX db_connection_orgtnt_idx ON wf.db_connection(orgtnt_id);
```

- `secret_enc`: AES-256-GCM ile şifreli parola (fields modu) veya bağlantı dizesindeki gizli kısım (uri modu). Format: `nonce(12) || ciphertext`. Anahtar env'den.
- Şifreleme yardımcı: `encrypt(plaintext) -> bytea`, `decrypt(bytea) -> String`. Anahtar yoksa (dev) açık uyarı + red.

### Bağlantı tipleri & dinamik form alanları

| Sürücü | Alanlar (fields modu) |
|---|---|
| Postgres | host, port(5432), database, username, password, options.sslmode |
| MySQL | host, port(3306), database, username, password, options.ssl |
| MSSQL | host, port(1433), database, username, password, options.instance, options.encrypt |

- **uri modu:** her tipte tek `connection_string` alanı (ör. `postgres://…`, `jdbc:…`, MSSQL `Server=…`). Gizli kısım `secret_enc`'e, gerisi `options.connection_string`'e (parolasız) — ya da tümü şifreli tutulur (basitlik için uri modunda tüm string `secret_enc`).

### API — `/db/connections` (server crate, X-Admin-Key veya portal JWT ile korunacak)

| Endpoint | İş |
|---|---|
| `GET /db/connections?orgtnt_id=` | Bağlantıları listeler — **parola/secret DÖNMEZ**, `last_test_ok` döner |
| `POST /db/connections` | Yeni bağlantı; body'deki secret şifrelenip saklanır. Döner `{id}` |
| `PUT /db/connections/:id` | Günceller; secret verilmezse mevcut korunur (COALESCE deseni) |
| `DELETE /db/connections/:id` | Siler + registry invalidate |
| `POST /db/connections/:id/test` | Kayıtlı bağlantıyı test eder → `{ok, message?}`, `last_test_*` günceller |
| `POST /db/connections/test` | Kaydetmeden test (form verisi + secret body'de) → `{ok, message?}` |

### Faz 2 — SQL node bağlama & runtime

- **WFD şeması:** SQL autoexec config'e `connection` alanı (bağlantı id/name). Golden fixture DEĞİŞMEZ; alan opsiyonel — yoksa mevcut varsayılan Postgres havuzu (geri uyumluluk).
- **Runner:** `run_sql` bağlantı çözümlemesini registry'den yapar; `connection` verilmişse o sürücüyle, verilmemişse mevcut default pool ile çalışır. Satır→JSON sürücü-bağımsız.
- **Editör:** SQL node property panelinde **bağlantı dropdown'u** (`/db/connections` listesinden).

### Editör UI (Faz 1)

- Çalıştırma sekmesine **"Veritabanı"** bölümü: bağlantı kartları listesi (isim + sürücü rozeti + son test durumu ●), **Ekle** butonu → sürücü seçimi → tipe göre şekillenen form (fields/uri toggle) → **Test et** (anlık sonuç) → Kaydet. Kart üzerinde Test/Düzenle/Sil. Çoklu bağlantı.
- Parola alanı write-only (düzenlemede boş = değişme).
- `engineApi`: `listDbConnections/createDbConnection/updateDbConnection/deleteDbConnection/testDbConnection/testDbConnectionDraft`.

## Güvenlik

- Parola/secret yalnızca `secret_enc` (AES-256-GCM) olarak diskte; API yanıtlarında hiç yer almaz.
- `DB_CONN_SECRET` env zorunlu (yoksa create/test reddi + dev uyarısı). Nonce her yazımda rastgele.
- Test/çalıştırma hataları kullanıcıya sürücü mesajıyla döner (kimlik bilgisi sızdırmadan).

## Test stratejisi

- **Engine:** şifreleme round-trip birim testi (saf). Sürücü soyutlaması test'i — Postgres/MySQL için docker'daki test DB'lerine karşı `test()`+`run_query()` (varsa), MSSQL için en azından config-parse. CRUD + test endpoint'leri manuel/curl. Golden fixture değişmez.
- **Editör:** engineApi sarmalayıcı testleri (mock fetch), form-şekillenme (driver→alanlar) birim testi, store testleri.

## Faz sırası (uygulama)

1. **Faz 1a (engine):** migration + şifreleme + `wf.db_connection` repo + CRUD/test API + Postgres/MySQL/MSSQL sürücü `test()`.
2. **Faz 1b (editör):** Veritabanı yönetim UI + engineApi + testler.
3. **Faz 2 (engine+editör):** SQL node `connection` alanı + runner çok-sürücülü çalıştırma + node property dropdown.

## Açık/kabul edilen varsayımlar

- MSSQL için `tiberius` bağımlılığı eklenecek (kabul edildi).
- Bağlantılar tenant-scoped (`orgtnt_id`).
- Şifreleme anahtarı env'de tek anahtar (KMS/rotation kapsam dışı, YAGNI).
