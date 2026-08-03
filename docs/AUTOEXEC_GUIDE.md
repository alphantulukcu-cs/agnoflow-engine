# Autoexec Rehberi (WFD v2.2)

Autoexec'ler insan etkileşimi gerektirmeyen otomatik adımlardır: **REST**, **SQL**, **CALC**.
v2.2'de root `autoexec` kataloğunda tanımlanır, transition'lardan `trigger[].use` ile
referans edilir. Çalıştırma `crates/wfe/src/runner.rs` (`LiveAutoexecRunner`),
DB bağlantı katmanı `crates/wfe/src/db/` altındadır.

```json
"autoexec": {
  "kredi_skoru_getir": {
    "type": "rest",
    "description": "Kredi skorunu dış servisten getirir.",
    "timeout_seconds": 10,
    "config": { ... },
    "wfes_effects": { "set": { "credit_score": "$exec.result.score" } }
  }
}
```

Ortak kurallar:
- `timeout_seconds` (varsayılan 60) pipeline tarafından uygulanır; aşımı `WFD.Timeout`.
- Başarılı sonuç TEK namespace ile okunur: **`$exec.result.*`** (`wfes_effects.set` içinde).
  `$exec.response.*` v2.2'de KALDIRILDI (M7) — validator hata verir.
- Config değerlerindeki `$`-string'ler çalıştırma anında çözülür:
  `$ctx.path.to.field`, `$wfe_id`, `$actor`, `$node`, `$timestamp`.
- Hata `WFD.AutoexecFailed` üretir; trigger'daki `retry` / `catch` ile eşleşir.

## REST (`type: "rest"`)

```json
"config": {
  "method": "GET|POST|PUT|PATCH|DELETE",
  "url": "https://api.example.com/endpoint",
  "params":  { "tckid": "$ctx.applicant.tckid" },          // query string
  "headers": { "X-Trace-Id": "$wfe_id" },                  // opsiyonel
  "auth": { "type": "bearer", "token": "$ctx.api_token" }, // opsiyonel, aşağıya bak
  "body":  { "alan": "$ctx.x" },                           // JSON gövde
  "form":  { "grant_type": "client_credentials" }          // x-www-form-urlencoded
}
```

- `auth` kısayolları (en son uygulanır, aynı header'ı ezer):
  - `{"type":"bearer","token":"..."}` → `Authorization: Bearer ...`
  - `{"type":"basic","username":"...","password":"..."}` → `Authorization: Basic ...`
  - `{"type":"api_key","header":"X-API-Key","value":"..."}` (header varsayılanı `X-API-Key`)
- `form` verilirse `body` yok sayılır; `form` yoksa `body` JSON olarak gönderilir.
- 2xx dışı yanıt: hata mesajına yanıt gövdesinin ilk 500 karakteri eklenir.
- JSON olmayan 2xx yanıt: `{"body": "<ham metin>"}` olarak döner
  (`$exec.result.body`); boş gövde `{}` döner.

## SQL (`type: "sql"`)

```json
"config": {
  "connection": "<db_connection uuid>",   // opsiyonel; yoksa engine'in kendi Postgres'i
  "query": "SELECT ad, skor FROM musteri WHERE id = :kim",
  "params": { "kim": "$ctx.musteri_id" }
}
```

- **Parametreler**: `:ad` yer tutucuları metin sırasında sürücüye özel işarete çevrilir
  (`$1` pg / `?` mysql-sqlite / `@P1` mssql). String literal içindeki `:x` ve
  Postgres `::cast` dokunulmaz. Aynı isim birden çok kez kullanılabilir.
- **Sonuç**: tek satır → düz obje (`$exec.result.kolon`), 0 veya çoklu satır →
  `{"rows": [...]}` (`$exec.result.rows`).
- **Tip haritası**: int2/4/8, unsigned, float4/8, numeric/decimal, bool, uuid,
  date/time/timestamp(tz) (RFC3339/ISO string), json(b), text → JSON karşılıkları.
- **Bağlantılar** (`wf.db_connection`, AES-256-GCM şifreli secret; `/db/connections` API):

| Driver adı | Wire protokolü | Varsayılan port |
|---|---|---|
| `postgres` | Postgres | 5432 |
| `cockroachdb` | Postgres | 26257 |
| `redshift` | Postgres | 5439 |
| `timescaledb` | Postgres | 5432 |
| `mysql` | MySQL | 3306 |
| `mariadb` | MySQL | 3306 |
| `tidb` | MySQL | 4000 |
| `mssql` / `sqlserver` | TDS (tiberius) | 1433 |
| `sqlite` | dosya | — (`database` = dosya yolu) |

  `mode: "fields"` (host/port/database/username/parola) veya `mode: "uri"`
  (secret = tam bağlantı dizesi; mssql'de ADO formatı). SQLite'ta `fields` modunda
  yalnızca `database` (engine'in erişebildiği dosya yolu) gerekir.
- Bağlantı handle'ları `(id, updated_at)` anahtarıyla önbelleklenir; bağlantı
  güncellenince otomatik tazelenir.

## CALC (`type: "calc"`)

```json
"config": {
  "expressions": {
    "within_limit": "$ctx.credit_score >= 700 and $ctx.credit_info.amount_requested <= 50000"
  }
}
```

- zen-expression sözdizimi. Bağlı namespace'ler:
  `$ctx.* $wfah $prev $first $node $actor $timestamp $wfe_id $action.input.*`.
- `$wfah` kapsamı, aynı trigger'ın `when` guard'ıyla AYNIdır: **bu aksiyondan ÖNCEKİ**
  geçmiş. Tetikleyen aksiyonun kendi girdisi `$action.input.*` ile okunur.
- `$prev` = geçmişin SON girdisi, `$first` = ilk girdi; alanları
  `{seq, action, actor, input, at}`. Geçmiş boşsa hepsi `null` — ifade patlamaz.
- `$exec.result.*` calc içinde bağlı DEĞİLDİR (aynı zincirdeki önceki autoexec'in
  sonucu okunamaz; ara değeri `wfes_effects` ile ctx'e yaz, `$ctx` üzerinden oku).
- Fonksiyonlar: `count`/`some`/`all`/`none`/`one`/`filter`/`map`/`flatMap` **2
  argümanlıdır** — `count($wfah, #.action == "x")`. Tek argümanlı `count(filter(...))`
  parse HATASI verir. `every` diye bir fonksiyon YOKTUR, karşılığı `all`'dır.
- Negatif indeks (`$wfah[-1]`) parse edilir ama runtime'da patlar — `$prev` kullan.
- Her anahtar sonucu `$exec.result.<anahtar>` olarak okunur.

## Editörde (WFD-EDITOR)

- Auto step → **Config** modalı: REST'te method/url/auth/headers/params/body(JSON|form),
  SQL'de bağlantı seçimi + query + params, CALC'ta expressions.
- **Sonuç → ctx** tablosu üç tipte de `wfes_effects.set`'i düzenler
  (`ctx_field` ← `$exec.result.alan`). Eski `config.result` alanı engine tarafından
  OKUNMAZ; yalnızca eski dokümanların round-trip'i için korunur.
- **Test** modalı `/autoexec/test`'e gönderir (baseUrl Çalıştırma ayarlarından);
  yanıt `request_info` (çözülmüş config) + `result`/`error` gösterir.
- DB bağlantıları Çalıştırma sekmesi → "Veritabanı bağlantıları" bölümünden yönetilir.

## /autoexec/test

```
POST /autoexec/test
{ "autoexec": { "type": "...", "config": { ... }, "timeout_seconds"?: n }, "dynctx": { ... } }
→ { "success": bool, "result"?: ..., "error"?: "WFD.X: mesaj", "request_info": <çözülmüş config> }
```

SQL/REST çalıştırma hatası `success:false` ile HTTP 200 döner; bozuk tanım 422.

## Testler

```bash
cargo test -p wf-wfe --lib          # bind_params, sqlite in-memory, auth, calc birimleri
cargo test --workspace              # tamamı
```

Canlı doğrulama: `docs/superpowers/notes/2026-07-11-autoexec-completion.md`.

## Bilinen sınırlar / gelecek

- `python` / `lambda` tipleri tanımda var, çalıştırma desteklenmiyor (açık hata döner).
- Oracle bilinçli olarak eklenmedi (native client bağımlılığı); MongoDB/Redis SQL değil.
- MSSQL canlı ortamda smoke-test edilmedi (tiberius yolu birim + tip seviyesinde hazır).
- OAuth2 client-credentials akışı, yanıt cache'i, GraphQL executor — gelecek.
