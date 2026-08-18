//! Faz 3 — görünürlük projeksiyonunun MEVCUT WFE'ler için üretilmesi.
//!
//! `wf.wfe.view_c_a` / `origin_orgu_id` / `wf.wfe_branch.c_a` kolonları
//! 2026-08-13'te eklendi; bu tarihten ÖNCE yaratılmış satırlarda boştur. SQL
//! görünürlük süzgeci bu kolonlara dayandığı için, backfill koşmadan o satırlar
//! süzgeçten SESSİZCE düşerdi — bu yüzden liste ucu `grants_built_at IS NULL`
//! satırları ayrıca sayar ve backfill bitmeden SQL yoluna geçilmez.
//!
//! AYNI GÜN eklenen node listable kolonları (`wf.wfe.current_view_c_a` +
//! `wf.wfe_branch.view_c_a`, migration `20260813000004_node_listable.sql`) de
//! bu komutla dolar: kapı hâlâ `grants_built_at`tir ve projeksiyonun üretimi tek
//! yerdedir (`wf_wfe::reproject::reproject_wfe`), yani yeni bir kolon eklemek
//! bu dosyada bir satır değiştirmeyi gerektirmez. Kolon eksik kalırsa `[]` olur
//! ve `can_view` (f) ile eşleşen aktör satırı süzgeçte GÖREMEZ — sessiz kayıp
//! bu yüzden hep aynı damgaya bağlanır.
//!
//! Çapa (`origin_orgu_id`) eski satırlarda İLK WFAH kaydının aktöründen
//! türetilir: akışı başlatan odur. Sistem aktörlü (nil user) kayıtlar atlanır —
//! motorun kendi yazdığı marker'lar bir birimi temsil etmez.
//!
//! ## ÖN GEÇİŞ: `end_terminal` kurtarma (2026-08-17)
//!
//! Terminal `listable[]` "WFE BU bitişte sonlandıysa görsün" der ve hangi bitiş olduğu
//! `wf.wfe.end_terminal` kolonunda durur. Kolon 2026-08-17'de eklendiği için ondan önce
//! sonlanmış satırlarda NULL'dır → `reproject` `end_view_c_a`'yı ÜRETEMEZ.
//!
//! Bu yüzden reprojeksiyondan ÖNCE bir kurtarma geçişi koşar: bitmiş satırın kanıtlarından
//! (`end_response` + WFAH'ın son gerçek aksiyonu + değişmez belge) hangi terminal olduğunu
//! ÇIKARIR — `wfe_core::v22::end_terminal::infer_end_terminal`, saf ve birim testli.
//! **ASLA TAHMİN ETMEZ:** kanıtlar tek bir terminal'e indirgenmiyorsa kolon NULL bırakılır
//! ve satır eski davranışında kalır. Yanlış yazmak, görmemesi gereken kişiye bitmiş işi
//! göstermek olurdu.
//!
//! Sıra ÖNEMLİDİR ve bu yüzden ayrı bir komut DEĞİL, aynı komutun ön geçişidir: kolon
//! dolmadan koşan `reproject` o satırı atlar ve ikinci bir çağrı gerektirirdi.
//!
//! VARSAYILAN KURU KOŞUMDUR. Yazmak için `--apply` verin:
//!   `DATABASE_URL=... cargo run -p wf-server --bin visibility_backfill -- --apply`

use std::sync::Arc;

use serde_json::Value;
use sqlx::{postgres::PgPoolOptions, Executor, PgPool};
use uuid::Uuid;
use wfe_core::v22::end_terminal::{infer_end_terminal, EndTerminalGuess, LastAction};
use wfe_core::v22::pipeline::Engine;
use wfe_core::v22::ports::WfdStore;
use wfe_core::OrgPort;

#[derive(sqlx::FromRow)]
struct Row {
    wfe_id: Uuid,
}

/// `end_terminal` kurtarma ön geçişinin aday satırı.
#[derive(sqlx::FromRow)]
struct TerminalRow {
    wfe_id: Uuid,
    wfd_id: Uuid,
    wfd_version: i32,
    end_response: Option<Value>,
}

/// Kurtarma geçişinin sayaçları — rapor bunları basar.
#[derive(Default)]
struct RecoveryStats {
    certain: usize,
    ambiguous: usize,
    no_match: usize,
    wfd_missing: usize,
}

/// `end_terminal` ON GEÇİŞİ: NULL kalmış bitmiş satırları kanıtlardan doldurur.
///
/// Yalnız `status = 'terminal'` satırlara dokunur — `error` (motor hatası) ve
/// `terminated` (SLA ihlali) yollarında VARILMIŞ bir bitiş yoktur, oraya bir terminal
/// id yazmak kolonun anlamını bozardı.
async fn recover_end_terminals(
    pool: &PgPool,
    wfd_store: &dyn WfdStore,
    apply: bool,
) -> RecoveryStats {
    let mut st = RecoveryStats::default();
    let rows: Vec<TerminalRow> = sqlx::query_as(
        "SELECT wfe_id, wfd_id, wfd_version, end_response
           FROM wf.wfe
          WHERE status = 'terminal' AND end_terminal IS NULL
          ORDER BY created_at",
    )
    .fetch_all(pool)
    .await
    .expect("end_terminal adayları okunamadı");

    if rows.is_empty() {
        println!("--- end_terminal kurtarma: kurtarılacak satır YOK ---\n");
        return st;
    }
    println!("--- end_terminal kurtarma ({} aday) ---", rows.len());

    for row in &rows {
        let Ok(wfd) = wfd_store.fetch(row.wfd_id, row.wfd_version).await else {
            st.wfd_missing += 1;
            println!("  ATLANDI {} — WFD çözülemedi", &row.wfe_id.to_string()[..8]);
            continue;
        };

        // WFAH'ın SON GERÇEK aksiyonu: `seq` azalan sırada ilk kez belgedeki bir
        // `actions` anahtarına denk gelen kayıt. Marker'lar (`escalate:…`,
        // `_branch_cancelled`, `call:…/…`, `unclaim`) atlanır — onlar geçiş üretmez,
        // dolayısıyla bir `wft`e de karşılık gelmezler.
        let history: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT action, from_node FROM wf.wfah WHERE wfe_id = $1 ORDER BY seq DESC",
        )
        .bind(row.wfe_id)
        .fetch_all(pool)
        .await
        .expect("wfah okunamadı");
        let last = history
            .iter()
            .find(|(action, _)| wfd.actions.contains_key(action))
            .map(|(action, from_node)| LastAction {
                from_node: from_node.as_deref(),
                action: action.as_str(),
            });

        match infer_end_terminal(&wfd, row.end_response.as_ref(), last) {
            EndTerminalGuess::Certain(id) => {
                st.certain += 1;
                println!("  KESİN   {} → {id}", &row.wfe_id.to_string()[..8]);
                if apply {
                    sqlx::query("UPDATE wf.wfe SET end_terminal = $1 WHERE wfe_id = $2")
                        .bind(&id)
                        .bind(row.wfe_id)
                        .execute(pool)
                        .await
                        .expect("end_terminal yazılamadı");
                }
            }
            EndTerminalGuess::Ambiguous(ids) => {
                st.ambiguous += 1;
                println!(
                    "  BELİRSİZ {} — adaylar: {} (kolon NULL bırakıldı)",
                    &row.wfe_id.to_string()[..8],
                    ids.join(", ")
                );
            }
            EndTerminalGuess::NoMatch => {
                st.no_match += 1;
                println!(
                    "  EŞLEŞMEDİ {} — kanıtlar hiçbir bitişe indirgenmedi (kolon NULL bırakıldı)",
                    &row.wfe_id.to_string()[..8]
                );
            }
        }
    }
    println!();
    st
}

#[tokio::main]
async fn main() {
    let apply = std::env::args().any(|a| a == "--apply");
    let db = std::env::var("DATABASE_URL").expect("DATABASE_URL gerekli");
    let pool: PgPool = PgPoolOptions::new()
        .max_connections(5)
        .after_connect(|c, _| {
            Box::pin(async move {
                c.execute("SET search_path TO org, public").await?;
                Ok(())
            })
        })
        .connect(&db)
        .await
        .expect("db connect");

    let storage =
        wf_wfd::build_operator(&wf_wfd::StorageConfig::from_env()).expect("storage init");
    let wfd_store = Arc::new(wf_wfd::WfdAdapter::new(pool.clone(), storage));
    let wfe_store = wf_wfe::WfeAdapter::new(pool.clone());
    let org: Arc<dyn OrgPort> = Arc::new(wf_wfe::OrgAdapter::new(pool.clone()));
    // Grant çözümü autoexec ÇALIŞTIRMAZ (yalnız c_a + when okur) — runner yalnız
    // `Engine`in alan sözleşmesini karşılamak için var.
    let runner = wf_wfe::LiveAutoexecRunner::new(None);
    let engine = Engine {
        org: &*org,
        exec: &runner,
        env: Default::default(),
    };

    let rows: Vec<Row> =
        sqlx::query_as("SELECT wfe_id FROM wf.wfe ORDER BY created_at")
    .fetch_all(&pool)
    .await
    .expect("wfe list");

    println!(
        "=== GÖRÜNÜRLÜK BACKFILL === ({} satır, mod: {})\n",
        rows.len(),
        if apply { "YAZ" } else { "KURU KOŞUM" }
    );

    // ÖN GEÇİŞ: `end_terminal` kurtarma. Reprojeksiyondan ÖNCE koşar — `reproject`
    // `end_view_c_a`'yı yalnız kolon doluysa üretebiliyor (bkz. modül başındaki not).
    let recovery = recover_end_terminals(&pool, &*wfd_store, apply).await;

    let (mut ok, mut skipped_wfd, mut skipped_origin, mut empty_grant) = (0, 0, 0, 0);
    // Node listable kolonu DOLU çıkan aktif satır sayısı. Operatör için tek
    // doğrulama noktası: kolon eklendi ama hiçbir satırda dolmadıysa ya belgelerde
    // node listable yok ya da projeksiyon yolu kopmuştur — ikisi ayırt edilebilsin.
    let mut node_grant = 0;
    // 2026-08-17 terminal listable: aynı doğrulama noktasının bitmiş-iş karşılığı.
    // Ayrıca `end_terminal` kolonundan ÖNCE bitmiş satırlar geri kazanılamaz —
    // hangi terminal'e varıldığı hiçbir yerde saklanmıyordu ve WFAH'tan güvenilir
    // biçimde türetilemez (`wft.conditions` o anki ctx ile çözülüyordu). O satırlar
    // eski davranışta kalır: yalnız kök `listable`/`wf_admin` gösterir. Sayısı
    // basılır ki operatör "kolon boş" ile "kolon dolmadı" durumlarını ayırabilsin.
    let mut terminal_grant = 0;
    let mut legacy_terminal = 0;

    for row in &rows {
        // Projeksiyonun üretimi TEK yerde: `wf_wfe::reproject` (worker da aynı
        // fonksiyonu çağırır). Bu komut yalnız sürücü + rapor katmanıdır.
        let outcome = wf_wfe::reproject::reproject_wfe(
            &pool,
            &*wfd_store,
            &wfe_store,
            &engine,
            row.wfe_id,
            !apply,
        )
        .await
        .expect("reproject");

        match outcome {
            wf_wfe::reproject::Outcome::WfdMissing => {
                skipped_wfd += 1;
                println!("  ATLANDI {} — WFD çözülemedi", &row.wfe_id.to_string()[..8]);
                continue;
            }
            wf_wfe::reproject::Outcome::NoAnchor => {
                skipped_origin += 1;
                println!(
                    "  ATLANDI {} — çapa türetilemedi (insan WFAH kaydı yok)",
                    &row.wfe_id.to_string()[..8]
                );
                continue;
            }
            wf_wfe::reproject::Outcome::Written { .. } => {}
        }

        // Kararın faturası: grant'ı olmayan BİTMİŞ iş bundan sonra hiç kimsenin
        // listesinde görünmez. Hata değil (WFD'de listable yoksa beklenen), ama
        // sessiz de kalmamalı — bu yüzden kolon DOĞRUDAN okunur (projeksiyonun
        // gerçeği DB'dedir; kuru koşumda kolon henüz yazılmamış olabilir).
        let (status, view_len, node_view_len, end_view_len, end_terminal): (
            String,
            i64,
            i64,
            i64,
            Option<String>,
        ) = sqlx::query_as(
            "SELECT status, jsonb_array_length(view_c_a),
                    jsonb_array_length(current_view_c_a),
                    jsonb_array_length(end_view_c_a), end_terminal
               FROM wf.wfe WHERE wfe_id = $1",
        )
        .bind(row.wfe_id)
        .fetch_one(&pool)
        .await
        .expect("status okunamadı");
        if status == "active" && node_view_len > 0 {
            node_grant += 1;
        }
        if end_view_len > 0 {
            terminal_grant += 1;
        }
        // Terminal'de bitmiş ama `end_terminal`i olmayan satır = kolondan ÖNCE
        // bitmiş satır; terminal grant'ı onun için bir daha üretilemez.
        if status == "terminal" && end_terminal.is_none() {
            legacy_terminal += 1;
        }
        // UYARI eşiği artık İKİ kalıcı kolonu birden sorar: terminal `listable`ı
        // olan bitmiş iş görünürdür, "grant'ı yok" demek yanlış olurdu.
        if status != "active" && view_len == 0 && end_view_len == 0 {
            empty_grant += 1;
            println!(
                "  UYARI  {} [{status}] — listable/wf_admin/terminal listable grant'ı YOK, \
                 bitmiş iş artık listelenmez",
                &row.wfe_id.to_string()[..8]
            );
        }
        ok += 1;
    }

    println!("\n--- ÖZET ---");
    println!("işlenen           : {ok}");
    println!("atlandı (WFD yok) : {skipped_wfd}");
    println!("atlandı (çapa yok): {skipped_origin}");
    println!("grant'sız bitmiş  : {empty_grant}");
    println!("node listable'lı  : {node_grant} (aktif, current_view_c_a dolu)");
    println!("terminal listable : {terminal_grant} (end_view_c_a dolu)");
    println!(
        "terminal'i bilinmeyen: {legacy_terminal} (end_terminal NULL — kurtarma kanıt bulamadı)"
    );
    println!(
        "end_terminal kurtarma : {} kesin · {} belirsiz · {} eşleşmedi · {} WFD yok",
        recovery.certain, recovery.ambiguous, recovery.no_match, recovery.wfd_missing
    );
    if !apply {
        println!("\nKURU KOŞUM — hiçbir şey yazılmadı. Yazmak için: --apply");
    }
}
