//! `R03` rapor sorgusunun KAPISI — canlı Postgres'e ihtiyaç duyduğu için `cargo test`
//! değil, elle koşulan bir örnek:
//!
//! ```text
//! cargo run -p wf-server --example r03_sql_gate
//! ```
//!
//! ## Neden `tests/` değil
//!
//! Depoda veritabanına bağlanan TEK bir test yok (ölçüldü: `sqlx::test` sıfır kullanım).
//! `unit-workload`un düzeltilen üç kusuru (`DISTINCT`, çapasız kova, kaynak kırılımı)
//! SAF SQL'de yaşıyor ve Rust tarafında sınanamaz — bu yüzden kapı burada. `cargo test`
//! bu dosyayı DERLER, koşmaz; DB'siz bir ortamda hiçbir şey kırılmaz.
//!
//! ## Neden sorgu kopyası ÇÜRÜMÜYOR
//!
//! Kopya kaçınılmaz (`wf-server` bir binary crate; `lib` hedefi yok, örnek onun içinden
//! bir şey `use` edemez). Bu yüzden ilk iş, buradaki metnin `wfd.rs`'te GEÇTİĞİNİ
//! doğrulamaktır: sorgu üretimde değişip burada değişmezse çalıştıran kişi ilk satırda
//! durur. Aynı hile `reference_types_parity.rs`'te de kullanılıyor.
//!
//! Her şey TEK transaction'da kurulur ve sonunda ROLLBACK edilir — veritabanında iz
//! kalmaz, tenant kimliği de her koşumda rastgeledir.
use serde_json::json;
use sqlx::{Connection, PgConnection, Row};
use uuid::Uuid;

const QUERY: &str = "WITH cand AS (
             SELECT w.wfe_id,
                    w.claimed_by,
                    (ca.elem->>'orgu_id')::uuid AS orgu_id,
                    coalesce(ca.elem->>'authority', 'c_a') AS authority
             FROM wf.wfe w
             CROSS JOIN LATERAL jsonb_array_elements(w.current_c_a) AS ca(elem)
             WHERE w.orgtnt_id = $1 AND w.status = 'active'
         ), per_unit AS (
             SELECT orgu_id,
                    wfe_id,
                    bool_or(claimed_by IS NULL) AS unclaimed,
                    bool_or(authority = 'c_a') AS via_c_a
             FROM cand
             GROUP BY orgu_id, wfe_id
         )
         SELECT p.orgu_id,
                ou.name AS orgu_name,
                count(*) FILTER (WHERE p.via_c_a)::bigint AS active,
                count(*) FILTER (WHERE NOT p.via_c_a)::bigint AS also_eligible,
                count(*) FILTER (WHERE p.via_c_a AND p.unclaimed)::bigint AS unclaimed
         FROM per_unit p
         LEFT JOIN org.orgu ou ON ou.orgu_id = p.orgu_id
         GROUP BY p.orgu_id, ou.name
         ORDER BY active DESC, also_eligible DESC
         LIMIT $2";

const OLD_QUERY: &str = "SELECT ou.orgu_id, ou.name,
                count(*)::bigint AS active,
                count(*) FILTER (WHERE w.claimed_by IS NULL)::bigint AS unclaimed
         FROM wf.wfe w
         CROSS JOIN LATERAL jsonb_array_elements(w.current_c_a) AS ca(elem)
         JOIN org.orgu ou ON ou.orgu_id = (ca.elem->>'orgu_id')::uuid
         WHERE w.orgtnt_id = $1 AND w.status = 'active'
         GROUP BY ou.orgu_id, ou.name
         ORDER BY active DESC
         LIMIT $2";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ÇÜRÜME KAPISI — bkz. dosya başlığı.
    const WFD_SRC: &str = include_str!("../src/routes/wfd.rs");
    assert!(
        WFD_SRC.contains(QUERY),
        "sorgu `wfd.rs` ile AYRIŞMIŞ: bu örnekteki metin üretimde geçmiyor. \
         Kapı ölçtüğü şeyi ölçmüyor demektir — kopyayı güncelle."
    );

    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL")?;
    let mut conn = PgConnection::connect(&url).await?;
    let mut tx = conn.begin().await?;

    let orgtnt = Uuid::new_v4();
    let unit_a = Uuid::new_v4();
    let unit_b = Uuid::new_v4();

    sqlx::query("INSERT INTO org.orgu (orgu_id, orgu_type, name) VALUES ($1,'{\"type\":\"sube\"}',$2), ($3,'{\"type\":\"sube\"}',$4)")
        .bind(unit_a).bind("A Şube").bind(unit_b).bind("B Şube")
        .execute(&mut *tx).await?;

    // `wf.wfe.environment_id` NOT NULL; ortam satırı tenant'a bağlı.
    sqlx::query("INSERT INTO org.orgtnt (orgtnt_id, name, code) VALUES ($1, $2, $2)")
        .bind(orgtnt)
        .bind("r03-gate")
        .execute(&mut *tx)
        .await?;
    let env_id: Uuid = sqlx::query_scalar(
        "INSERT INTO wf.environment (orgtnt_id, name, label, is_default) VALUES ($1,'default','Varsayılan',true) RETURNING id")
        .bind(orgtnt).fetch_one(&mut *tx).await?;

    let project_id: Uuid = sqlx::query_scalar(
        "INSERT INTO wf.project (orgtnt_id, name) VALUES ($1,'r03-gate') RETURNING project_id",
    )
    .bind(orgtnt)
    .fetch_one(&mut *tx)
    .await?;
    let wfd_id: Uuid = sqlx::query_scalar(
        "INSERT INTO wf.wfd_meta (orgtnt_id, name, s3_key, project_id) VALUES ($1,'r03-gate','r03/gate.json',$2) RETURNING wfd_id")
        .bind(orgtnt).bind(project_id).fetch_one(&mut *tx).await?;

    let ca = |orgu: Option<Uuid>, role: &str, authority: Option<&str>| {
        let mut m = serde_json::Map::new();
        if let Some(o) = orgu {
            m.insert("orgu_id".into(), json!(o));
        } else {
            m.insert("any_orgu".into(), json!(true));
        }
        m.insert("role".into(), json!(role));
        if let Some(a) = authority {
            m.insert("authority".into(), json!(a));
        }
        serde_json::Value::Object(m)
    };

    let cases: Vec<(&str, serde_json::Value, bool)> = vec![
        // W1 — AYNI birimde İKİ rol (birim × rol çarpımı). Rapor 1 saymalı.
        (
            "W1",
            json!([
                ca(Some(unit_a), "memur", Some("c_a")),
                ca(Some(unit_a), "sef", Some("c_a"))
            ]),
            false,
        ),
        // W2 — taban A'da, grant B'yi AÇMIŞ. Sahiplenilmiş.
        (
            "W2",
            json!([
                ca(Some(unit_a), "memur", Some("c_a")),
                ca(Some(unit_b), "denetci", Some("grant"))
            ]),
            true,
        ),
        // W3 — ÇAPASIZ aday: eski sorguda sessizce DÜŞÜYORDU.
        ("W3", json!([ca(None, "", Some("c_a"))]), false),
        // W4 — gölgeleme (R05): aynı birimde hem taban hem grant. active'te TEK.
        (
            "W4",
            json!([
                ca(Some(unit_a), "memur", Some("c_a")),
                ca(Some(unit_a), "memur2", Some("grant"))
            ]),
            false,
        ),
        // W5 — ESKİ satır: `authority` YOK. `active` kovasına girer (backfill yok).
        ("W5", json!([ca(Some(unit_b), "memur", None)]), false),
    ];
    for (name, c_a, claimed) in &cases {
        sqlx::query("INSERT INTO wf.wfe (orgtnt_id, wfd_id, wfd_version, status, current_c_a, claimed_by, environment_id) VALUES ($1,$2,1,'active',$3,$4,$5)")
            .bind(orgtnt).bind(wfd_id).bind(c_a)
            .bind(if *claimed { Some(json!({"user_id": Uuid::new_v4()})) } else { None })
            .bind(env_id)
            .execute(&mut *tx).await?;
        let _ = name;
    }

    println!("=== ESKİ sorgu (bugünkü davranış) ===");
    for r in sqlx::query(OLD_QUERY)
        .bind(orgtnt)
        .bind(50i64)
        .fetch_all(&mut *tx)
        .await?
    {
        println!(
            "  orgu={:?} ad={:?} active={} unclaimed={}",
            r.get::<Uuid, _>(0),
            r.get::<String, _>(1),
            r.get::<i64, _>(2),
            r.get::<i64, _>(3)
        );
    }

    println!("=== YENİ sorgu (R03) ===");
    let mut got = Vec::new();
    for r in sqlx::query(QUERY)
        .bind(orgtnt)
        .bind(50i64)
        .fetch_all(&mut *tx)
        .await?
    {
        let o: Option<Uuid> = r.get(0);
        let n: Option<String> = r.get(1);
        let (a, e, u): (i64, i64, i64) = (r.get(2), r.get(3), r.get(4));
        println!("  orgu={o:?} ad={n:?} active={a} also_eligible={e} unclaimed={u}");
        got.push((o, n, a, e, u));
    }

    let expect = vec![
        (Some(unit_a), Some("A Şube".to_string()), 3i64, 0i64, 2i64),
        (Some(unit_b), Some("B Şube".to_string()), 1, 1, 1),
        (None, None, 1, 0, 1),
    ];
    let mut g = got.clone();
    g.sort_by_key(|r| r.0);
    let mut x = expect.clone();
    x.sort_by_key(|r| r.0);
    assert_eq!(g, x, "\nBEKLENEN {x:#?}\nGELEN    {g:#?}");
    println!("✅ sayaçlar beklenen");

    // Gate 6 — containment REGRESYONU: eleman `authority` kazandı, `@>` AYNEN eşleşiyor.
    let hit: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM wf.wfe WHERE orgtnt_id = $1 AND current_c_a @> $2",
    )
    .bind(orgtnt)
    .bind(json!([{"orgu_id": unit_b, "role": "denetci"}]))
    .fetch_one(&mut *tx)
    .await?;
    assert_eq!(hit, 1, "containment alt küme semantiği kırılmış");
    println!("✅ containment (@>) yeni alanla AYNEN eşleşiyor");

    tx.rollback().await?;
    println!("↩︎ ROLLBACK — veritabanında iz yok");
    println!(
        "(ESKİ sorgu ile karşılaştır: A 5 sayıyordu — 3 iş, 5 aday satırı; \
              çapasız kova HİÇ görünmüyordu.)"
    );
    Ok(())
}
