# workflow-engine

Çok-tenant'lı, deterministik **workflow engine** — WFD v2.2 modelini (Named Nodes,
Single-Rule C_A) çalıştırır. İnsan adımları, otomasyonlar ve koşullar tek bir runtime
üzerinde birleşir; her geçiş audit izine yazılır.

> Kanonik model spesifikasyonu [`docs/spec/`](docs/spec/) altındadır.
> Spec ile kod çelişirse **spec kazanır**.

## Mimari — crate haritası

| Crate | Sorumluluk |
|---|---|
| `wfe-core` | Saf engine (I/O yok): tip modeli + slug, validator (cross-ref/graf/expression), v2.2 runtime (`eval`, `resolver`, `matcher`, `visibility`, `effects`, `pipeline`) ve port arayüzleri. |
| `org` | Organizasyon deposu (ORGT/ORGU/ORGTNT/UR) + ORGTRVLANG parser/executor (ltree SQL). |
| `wfd` | WFD depolama: meta PostgreSQL, JSON OpenDAL; upload/fetch'te v2.2 kapısı + validator; `(wfd_id, version)` immutable cache. |
| `wfe` | Adapter'lar: `WfeAdapter` (atomik transaction, claim CAS), `OrgAdapter`, `LiveAutoexecRunner` (rest/sql/calc), `WfeExecutor` + timer sweep, `sim`. |
| `server` | Axum API: `/wfd`, `/wfe`, `/wfe/simulate`, `/autoexec/test`, `/portal` (JWT), `/org` (admin key). 60s timer sweeper. |

Bağımlılık yönü: `server → {wfd, wfe, org} → wfe-core`. `wfe-core` saftır (I/O yok);
tüm veritabanı/HTTP erişimi dış crate'lerdeki adapter'larda yapılır.

## Proje yapısı

```
workflow-engine/                 # Cargo workspace kökü
├── Cargo.toml / Cargo.lock      # Workspace tanımı
├── crates/                      # Tüm Rust crate'leri (yukarıdaki harita)
│   ├── wfe-core/                #   saf engine
│   ├── org/  wfd/  wfe/         #   depolama + adapter'lar
│   └── server/                  #   Axum HTTP API
├── migrations/                  # SQL migration'lar (psql ile manuel, sırayla)
│   ├── org/                     #   önce uygulanır
│   └── wf/                      #   sonra uygulanır
├── data/                        # Seed verisi (ör. seed_qnb_users.sql)
├── storage/                     # Yerel WFD JSON / dosya deposu (local backend)
├── public/                      # Statik varlıklar
├── init.sql                     # Bootstrap şema/seed (org DSL testleri için)
│
├── docs/                        # Tüm dokümantasyon (tek çatı)
│   ├── spec/                    #   KANONİK WFD v2.2 spec — gerçeğin kaynağı
│   ├── AUTOEXEC_GUIDE.md        #   autoexec (rest/sql/calc) rehberi
│   ├── superpowers/             #   tasarım spec'leri, plan'lar, notlar
│   └── legacy/                  #   eski doküman snapshot'ları (arşiv)
│
└── CLAUDE.md                    # Geliştirici/agent çalışma kuralları + değişmezler
```

> Frontend (WFD editörü + portal) ayrı repodadır: [`agnoflow-frontend`](../agnoflow-frontend).
> `docs/spec/` onun `docs/spec/`'i ile senkron tutulur; **gerçeğin kaynağı burasıdır.**

## Başlangıç

**Gereksinimler:** Rust (stable), PostgreSQL.

**Ortam değişkenleri** (bkz. [.env.example](.env.example)):
`DATABASE_URL`, `PORT`, `JWT_SECRET` (zorunlu); `STORAGE_BACKEND=local|s3` (+`STORAGE_PATH`);
`CORS_ORIGINS`; `ADMIN_API_KEY`; ek-belge deposu için `ATTACHMENT_STORAGE_*`.

```bash
# Migration'lar psql ile manuel, sırasıyla uygulanır (sqlx migrate kullanılmaz)
psql "$DATABASE_URL" -f migrations/org/<...>.sql   # önce org
psql "$DATABASE_URL" -f migrations/wf/<...>.sql     # sonra wf

cargo build
cargo test --workspace        # her değişiklikten sonra
cargo run -p server
```

## Dokümantasyon

| Yol | İçerik |
|---|---|
| [`docs/spec/`](docs/spec/) | Kanonik WFD v2.2 spesifikasyonu + örnek WFD'ler + şema |
| [`docs/AUTOEXEC_GUIDE.md`](docs/AUTOEXEC_GUIDE.md) | Autoexec (rest/sql/calc) rehberi |
| [`docs/superpowers/`](docs/superpowers/) | Tasarım spec'leri, plan'lar ve notlar |
| [`docs/legacy/`](docs/legacy/) | Eski doküman snapshot'ları (arşiv) |
| [CLAUDE.md](CLAUDE.md) | Geliştirici/agent çalışma kuralları ve değişmezler |
