//! v2.2 WfeStore implementasyonu — TransitionCommit TEK PostgreSQL
//! transaction'ında uygulanır (M8 / WOR-43; WOR-7 fix).

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;
use wfe_core::types::wfd_v22::{JoinRule, WftTarget};
use wfe_core::types::{
    actor::Actor,
    dynctx::DynCtx,
    wfah::{Wfah, WfahEntry},
    wfe::WfeStatus,
};
use wfe_core::v22::ports::{
    BranchState, BranchStatus, CallView, CommitOutcome, NewWfe, PendingCall, TransitionCommit,
    WfeStore, Wfes,
};
use wfe_core::{ConflictKind, EngineError};

use crate::repo;

/// jsonb `{"user_id": "<uuid>"}` → Uuid (wfe.claimed_by / wfe_branch.claimed_by
/// ortak biçimi). Bozuk/eksik değer None döner.
fn parse_claimed_by(v: Option<&serde_json::Value>) -> Option<Uuid> {
    v.and_then(|cb| cb.get("user_id"))
        .and_then(|u| u.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
}

pub struct WfeAdapter {
    pub pool: PgPool,
}

impl WfeAdapter {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn db_err(e: impl std::fmt::Display) -> EngineError {
    EngineError::WfePort(e.to_string())
}

/// WOR-65: `wf.wfah` ve `wf.wfe_dynctx`'in `UNIQUE (wfe_id, seq)` kısıtı ZATEN bir
/// optimistic lock'tur — engine seq'i yüklediği snapshot'tan hesaplar, araya başka
/// bir commit girerse aynı seq ikinci kez yazılmak istenir ve Postgres 23505
/// (unique_violation) döner. Bu, tekil (paralel-olmayan) modda TEK yarış
/// korumasıdır: `CommitOutcome::MoveTo` yolunda ne `FOR UPDATE` ne de CAS vardır.
///
/// Önceden bu ihlal `WfePort` (→ HTTP 500) olarak sızıyordu; artık `StaleRevision`
/// conflict'ine eşlenir — böylece hem doğru HTTP kodu (409 + `conflict.stale_revision`)
/// hem de executor'ın retry döngüsü (reload → taze seq) devreye girer.
///
/// 23505 DIŞINDAKİ tüm DB hataları eskisi gibi `WfePort`'tur.
fn insert_err(e: sqlx::Error) -> EngineError {
    if let sqlx::Error::Database(db) = &e {
        if db.code().as_deref() == Some("23505") {
            return EngineError::Conflict(ConflictKind::StaleRevision);
        }
    }
    EngineError::WfePort(e.to_string())
}

async fn insert_wfah_entries(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    wfe_id: Uuid,
    entries: &[WfahEntry],
) -> Result<(), EngineError> {
    for entry in entries {
        let actor_json = serde_json::to_value(&entry.actor).map_err(db_err)?;
        sqlx::query(
            "INSERT INTO wf.wfah (wfe_id, seq, action, actor, input, applied_at)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(wfe_id)
        .bind(entry.seq as i32)
        .bind(&entry.action)
        .bind(&actor_json)
        .bind(entry.input.as_ref())
        .bind(entry.applied_at)
        .execute(&mut **tx)
        .await
        // WOR-65: seq çakışması = eşzamanlı commit (bkz. `insert_err`).
        .map_err(insert_err)?;
    }
    Ok(())
}

/// WOR-59: kol claim'ini düşüren TEK SET fragmanı. `cancel_active_branches`,
/// `mark_branch_arrived`, `BranchMoveTo` ve `release_claim` aynı ifadeyi paylaşır —
/// "kol claim'i nasıl düşer" kuralının tek doğru yeri burasıdır.
const CLEAR_BRANCH_CLAIM: &str = "claimed_by = NULL, claimed_at = NULL";

/// WOR-31: WFE terminal/terminated/failed olurken paralel modda aktif TÜM kolları
/// `cancelled` işaretler (audit için satırlar kalır) — çağıran ayrıca wfe satırında
/// `join_target = NULL` yapar. Paralel modda değilse 0 satır etkiler (no-op).
///
/// WOR-59: statü ile BİRLİKTE claim de düşürülür. Ayrı iki UPDATE olamaz — statü
/// `cancelled` olduktan sonra `status = 'active'` filtresi artık eşleşmez. Düşen
/// claim'in sahibi engine'in `_branch_cancelled` marker'ında zaten kayıtlıdır.
async fn cancel_active_branches(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    wfe_id: Uuid,
) -> Result<(), EngineError> {
    sqlx::query(&format!(
        "UPDATE wf.wfe_branch SET status = 'cancelled', {CLEAR_BRANCH_CLAIM}, updated_at = now()
         WHERE wfe_id = $1 AND status = 'active'"
    ))
    .bind(wfe_id)
    .execute(&mut **tx)
    .await
    .map_err(db_err)?;
    Ok(())
}

/// WOR-31: bir kolu `from` node'undan `arrived`'a taşır (CAS). status='active'
/// olan tam 1 satır güncellenmezse yarış olmuştur → Conflict (executor reload +
/// engine'i yeniden koşar). Çağıran, sayımdan ÖNCE wfe satırını FOR UPDATE ile
/// kilitlemiş olmalıdır.
async fn mark_branch_arrived(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    wfe_id: Uuid,
    from_node: &str,
) -> Result<(), EngineError> {
    let res = sqlx::query(&format!(
        "UPDATE wf.wfe_branch SET status = 'arrived', {CLEAR_BRANCH_CLAIM}, updated_at = now()
         WHERE wfe_id = $1 AND branch_node = $2 AND status = 'active'"
    ))
    .bind(wfe_id)
    .bind(from_node)
    .execute(&mut **tx)
    .await
    .map_err(db_err)?;
    if res.rows_affected() != 1 {
        return Err(EngineError::Conflict(ConflictKind::BranchMoved));
    }
    Ok(())
}

/// WOR-62: **kilit sırası sözleşmesi** — kol satırlarına dokunan HER commit yolu
/// önce bu fonksiyonla `wf.wfe` satırını `FOR UPDATE` alır, SONRA `wf.wfe_branch`
/// satırlarını günceller. Sıra daima **wfe → wfe_branch**'tir; ters sıra iki
/// eşzamanlı commit arasında deadlock üretir (collapse kolları kilitleyip wfe'yi
/// beklerken join wfe'yi kilitleyip kolları bekler).
///
/// Kilit ALTINDA okunan "hâlâ paralel modda mı" (`join_target IS NOT NULL`)
/// bilgisini döner. WFE satırı yoksa/tenant uymuyorsa `Conflict(WfeGone)`.
async fn lock_wfe(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    wfe_id: Uuid,
    orgtnt_id: Uuid,
) -> Result<bool, EngineError> {
    sqlx::query_scalar::<_, bool>(
        "SELECT join_target IS NOT NULL FROM wf.wfe
         WHERE wfe_id = $1 AND orgtnt_id = $2 FOR UPDATE",
    )
    .bind(wfe_id)
    .bind(orgtnt_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(db_err)?
    .ok_or(EngineError::Conflict(ConflictKind::WfeGone))
}

/// WOR-62: `lock_wfe` + "hâlâ paralel modda olmalı" kapısı. Engine'in görüşü
/// (bir kolda duruyorum) kilit altında doğrulanır; paralel mod bu arada bittiyse
/// bir kardeş (collapse/join/terminal) kazanmıştır → kaybeden aksiyon
/// `Conflict(Collapsed)` alır. Bu KALICI bir verdikttir: executor retry ETMEZ,
/// çağrı doğrudan 409 `conflict.collapsed` olarak döner.
async fn lock_wfe_parallel(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    wfe_id: Uuid,
    orgtnt_id: Uuid,
) -> Result<(), EngineError> {
    if lock_wfe(tx, wfe_id, orgtnt_id).await? {
        Ok(())
    } else {
        Err(EngineError::Conflict(ConflictKind::Collapsed))
    }
}

/// WOR-72/WOR-73: kilit ALTINDA okunan kol tablosu — varış commit'lerinin
/// doğrulama girdisi.
struct JoinState {
    /// Bu commit'ten ÖNCE `active` olan kol sayısı (acting kol dahil).
    active: i64,
    /// Bu commit'ten ÖNCE join'e varmış kolların KİMLİKLERİ (`entry_node`), sıralı.
    arrived_entries: Vec<String>,
}

impl JoinState {
    /// WOR-73: engine kararını hangi varış kümesi üzerinde verdiyse (`arrived_entries`,
    /// acting kol dahil) kilit altındaki gerçek küme de o olmalıdır.
    ///
    /// Neden sayı DEĞİL küme: ZEN join koşulu ("finans VE hukuk") sayıyla ifade
    /// edilemez, dolayısıyla adapter "kaç kol vardı" ile engine'in kararını
    /// doğrulayamaz. Kümeyi doğrulamak üç modun HEPSİ için yeterlidir: küme aynıysa
    /// engine'in (saf) kararı da aynıdır. Adapter ZEN çalıştırmaz — I/O katmanı
    /// motorun mantığını ikinci kez yazmaz.
    ///
    /// `acting_entry`: bu commit'te varan kolun kimliği; DB kümesine eklenerek
    /// karşılaştırılır (satır bu tx'te `arrived`'a alınıyor).
    fn arrival_matches(&self, acting_entry: &str, expected: &[String]) -> bool {
        let mut actual = self.arrived_entries.clone();
        actual.push(acting_entry.to_string());
        actual.sort();
        actual.dedup();
        let mut expected = expected.to_vec();
        expected.sort();
        expected.dedup();
        actual == expected
    }
}

/// Paralel-mod kilidi (bkz. `lock_wfe_parallel`) + kilit ALTINDA kol durumları —
/// eşzamanlı varış commit'lerini serialize eder.
async fn lock_and_read_join_state(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    wfe_id: Uuid,
    orgtnt_id: Uuid,
) -> Result<JoinState, EngineError> {
    lock_wfe_parallel(tx, wfe_id, orgtnt_id).await?;
    let rows = sqlx::query_as::<_, (String, Option<String>, String)>(
        "SELECT status, entry_node, branch_node FROM wf.wfe_branch WHERE wfe_id = $1",
    )
    .bind(wfe_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(db_err)?;
    let active = rows.iter().filter(|(st, _, _)| st == "active").count() as i64;
    let mut arrived_entries: Vec<String> = rows
        .iter()
        .filter(|(st, _, _)| st == "arrived")
        // WOR-73 öncesi satırlarda entry_node NULL olabilir → branch_node'a düşülür
        // (migration backfill'iyle aynı kural).
        .map(|(_, entry, node)| entry.clone().unwrap_or_else(|| node.clone()))
        .collect();
    arrived_entries.sort();
    Ok(JoinState {
        active,
        arrived_entries,
    })
}

/// WOR-73: bir kolun kimliği (`entry_node`), kilit altında okunur.
async fn branch_entry_node(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    wfe_id: Uuid,
    branch_node: &str,
) -> Result<String, EngineError> {
    let row = sqlx::query_as::<_, (Option<String>, String)>(
        "SELECT entry_node, branch_node FROM wf.wfe_branch
         WHERE wfe_id = $1 AND branch_node = $2",
    )
    .bind(wfe_id)
    .bind(branch_node)
    .fetch_optional(&mut **tx)
    .await
    .map_err(db_err)?
    .ok_or(EngineError::Conflict(ConflictKind::BranchMoved))?;
    Ok(row.0.unwrap_or(row.1))
}

#[async_trait]
impl WfeStore for WfeAdapter {
    async fn load(&self, wfe_id: Uuid) -> Result<Wfes, EngineError> {
        let row = repo::wfe::get(&self.pool, wfe_id).await.map_err(db_err)?;
        let ctx = repo::dynctx::load_latest(&self.pool, wfe_id)
            .await
            .map_err(db_err)?;
        let wfah_rows = repo::wfah::load_all(&self.pool, wfe_id)
            .await
            .map_err(db_err)?;

        let entries: Vec<WfahEntry> = wfah_rows
            .into_iter()
            .map(|r| {
                let actor: Actor = serde_json::from_value(r.actor).unwrap_or_else(|e| {
                    // WOR-19: bozuk kayıt sessiz kalmasın — audit izi log'a düşer
                    tracing::warn!("wfe {wfe_id} wfah seq {} actor parse edilemedi: {e}", r.seq);
                    Actor {
                        orgu_id: Uuid::nil(),
                        user_id: Uuid::nil(),
                        role: "unknown".into(),
                    }
                });
                WfahEntry {
                    seq: r.seq as u32,
                    action: r.action,
                    actor,
                    input: r.input,
                    applied_at: r.applied_at,
                }
            })
            .collect();

        let status = match row.status.as_str() {
            "terminal" => WfeStatus::Terminal,
            "error" => WfeStatus::Error,
            "terminated" => WfeStatus::Terminated,
            _ => WfeStatus::Active,
        };

        let assigned_to = parse_claimed_by(row.claimed_by.as_ref());

        // WOR-31: paralel mod kol satırları + join hedefi.
        let branch_rows = repo::branch::load_all(&self.pool, wfe_id)
            .await
            .map_err(db_err)?;
        let branches: Vec<BranchState> = branch_rows
            .into_iter()
            .map(|b| BranchState {
                entry_node: b.entry_node.unwrap_or_else(|| b.branch_node.clone()),
                branch_node: b.branch_node,
                status: match b.status.as_str() {
                    "arrived" => BranchStatus::Arrived,
                    "cancelled" => BranchStatus::Cancelled,
                    _ => BranchStatus::Active,
                },
                claimed_by: parse_claimed_by(b.claimed_by.as_ref()),
                claimed_at: b.claimed_at,
                entered_at: b.entered_at,
            })
            .collect();
        let join_target: Option<WftTarget> = row
            .join_target
            .as_ref()
            .and_then(|jt| serde_json::from_value(jt.clone()).ok());

        Ok(Wfes {
            wfe_id,
            orgtnt_id: row.orgtnt_id,
            environment_id: row.environment_id,
            wfd_id: row.wfd_id,
            wfd_version: row.wfd_version,
            dynctx: DynCtx(ctx),
            wfah: Wfah(entries),
            status,
            current_node: row.current_node,
            assigned_to,
            end_response: row.end_response,
            deadline: row.deadline,
            claimed_at: row.claimed_at,
            created_at: row.created_at,
            branches,
            join_target,
            // WOR-72/WOR-73: iki kolon → tek çözülmüş kural. Negatif/0 eşik DB
            // CHECK'iyle, "ikisi birden dolu" hâli `wfe_join_rule_single` ile
            // engellenir; yine de eşik önce okunur (deterministik).
            join_rule: match (row.join_threshold, row.join_when) {
                (Some(k), _) => JoinRule::Quorum(k.max(1) as u32),
                (None, Some(expr)) => JoinRule::Expr(expr),
                (None, None) => JoinRule::All,
            },
        })
    }

    async fn create(&self, new: &NewWfe) -> Result<(), EngineError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;

        let (status, current_node, end_response) = match &new.outcome {
            CommitOutcome::MoveTo { node } => ("active", Some(node.as_str()), None),
            CommitOutcome::Terminal { end_response } => ("terminal", None, Some(end_response)),
            CommitOutcome::Failed { end_response } => ("error", None, Some(end_response)),
            CommitOutcome::Terminated { end_response } => ("terminated", None, Some(end_response)),
            // WOR-31: start wft'i Parallel olamaz (engine + validator reddeder) —
            // buraya ulaşması programlama hatasıdır.
            CommitOutcome::ForkTo { .. }
            | CommitOutcome::BranchMoveTo { .. }
            | CommitOutcome::BranchArrived { .. }
            | CommitOutcome::JoinComplete { .. }
            | CommitOutcome::CollapseTo { .. } => {
                return Err(EngineError::WfePort(
                    "create paralel outcome alamaz (WOR-31: start'ta fork yasak)".into(),
                ))
            }
        };
        let c_a_json = serde_json::to_value(&new.resolved_c_a).map_err(db_err)?;

        sqlx::query(
            "INSERT INTO wf.wfe
               (wfe_id, orgtnt_id, environment_id, wfd_id, wfd_version, status, current_node, current_c_a, end_response, deadline)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(new.wfe_id)
        .bind(new.orgtnt_id)
        .bind(new.environment_id)
        .bind(new.wfd_id)
        .bind(new.wfd_version)
        .bind(status)
        .bind(current_node)
        .bind(&c_a_json)
        .bind(end_response)
        .bind(new.deadline)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;

        sqlx::query("INSERT INTO wf.wfe_dynctx (wfe_id, seq, ctx) VALUES ($1, 1, $2)")
            .bind(new.wfe_id)
            .bind(&new.initial_dynctx)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;

        insert_wfah_entries(&mut tx, new.wfe_id, &new.wfah_entries).await?;

        // WFC outbox — start pipeline'ında stage edilen çağrılar AYNI tx'te yazılır:
        // "çağrı yapılacak" niyeti, çağıranın durumu ile atomik olur.
        if !new.staged_calls.is_empty() {
            repo::call::stage(
                &mut tx,
                new.orgtnt_id,
                new.wfe_id,
                &new.staged_calls,
                new.caller.as_ref().map(|c| c.depth).unwrap_or(0),
                new.caller.as_ref().map(|c| c.next_depth).unwrap_or(0),
            )
            .await
            .map_err(db_err)?;
        }

        tx.commit().await.map_err(db_err)
    }

    async fn commit(&self, commit: &TransitionCommit) -> Result<(), EngineError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;

        let dynctx_seq = commit
            .wfah_entries
            .last()
            .map(|e| e.seq as i32)
            .unwrap_or(1);
        sqlx::query("INSERT INTO wf.wfe_dynctx (wfe_id, seq, ctx) VALUES ($1, $2, $3)")
            .bind(commit.wfe_id)
            .bind(dynctx_seq)
            .bind(&commit.new_dynctx)
            .execute(&mut *tx)
            .await
            // WOR-65: seq çakışması = eşzamanlı commit (bkz. `insert_err`).
            .map_err(insert_err)?;

        insert_wfah_entries(&mut tx, commit.wfe_id, &commit.wfah_entries).await?;

        match &commit.outcome {
            CommitOutcome::MoveTo { node } => {
                let c_a_json = serde_json::to_value(&commit.resolved_c_a).map_err(db_err)?;
                // M8: yeni node'a UNASSIGNED giriş — claimed_by/claimed_at temizlenir
                sqlx::query(
                    "UPDATE wf.wfe
                     SET current_node = $1, current_c_a = $2, claimed_by = NULL,
                         claimed_at = NULL, updated_at = now()
                     WHERE wfe_id = $3 AND orgtnt_id = $4",
                )
                .bind(node)
                .bind(&c_a_json)
                .bind(commit.wfe_id)
                .bind(commit.orgtnt_id)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
            }
            CommitOutcome::Terminal { end_response } => {
                // WOR-31: paralel modda WFE-terminal aktif kolları da iptal eder
                // (`_branch_cancelled` marker'ları engine tarafından staged edildi).
                // WOR-62: kol satırlarına dokunmadan ÖNCE wfe kilidi — kilit sırası
                // wfe → wfe_branch (bkz. `lock_wfe`). Paralel modda olmak ŞART
                // değil (tekil modda terminal de bu yoldan geçer), yalnız sıra
                // korunur. Terminal otoriterdir: eşzamanlı kardeşler kilit
                // arkasında bekler, sonra kendi CAS'larında Conflict alır.
                lock_wfe(&mut tx, commit.wfe_id, commit.orgtnt_id).await?;
                cancel_active_branches(&mut tx, commit.wfe_id).await?;
                sqlx::query(
                    "UPDATE wf.wfe
                     SET status = 'terminal', current_node = NULL, current_c_a = '[]'::jsonb,
                         claimed_by = NULL, claimed_at = NULL, join_target = NULL,
                         join_threshold = NULL, join_when = NULL,
                         end_response = $1, updated_at = now()
                     WHERE wfe_id = $2 AND orgtnt_id = $3",
                )
                .bind(end_response)
                .bind(commit.wfe_id)
                .bind(commit.orgtnt_id)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
            }
            CommitOutcome::Failed { end_response } => {
                // Engine-defined fail (§5): terminal DEĞİL, `error` durumu.
                // WOR-62: kilit sırası wfe → wfe_branch.
                lock_wfe(&mut tx, commit.wfe_id, commit.orgtnt_id).await?;
                cancel_active_branches(&mut tx, commit.wfe_id).await?;
                sqlx::query(
                    "UPDATE wf.wfe
                     SET status = 'error', current_node = NULL, current_c_a = '[]'::jsonb,
                         claimed_by = NULL, claimed_at = NULL, join_target = NULL,
                         join_threshold = NULL, join_when = NULL,
                         end_response = $1, updated_at = now()
                     WHERE wfe_id = $2 AND orgtnt_id = $3",
                )
                .bind(end_response)
                .bind(commit.wfe_id)
                .bind(commit.orgtnt_id)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
            }
            CommitOutcome::Terminated { end_response } => {
                // SLA ihlali (§5 2026-07-16): `error` DEĞİL, `terminated` durumu.
                // WOR-62: kilit sırası wfe → wfe_branch.
                lock_wfe(&mut tx, commit.wfe_id, commit.orgtnt_id).await?;
                cancel_active_branches(&mut tx, commit.wfe_id).await?;
                sqlx::query(
                    "UPDATE wf.wfe
                     SET status = 'terminated', current_node = NULL, current_c_a = '[]'::jsonb,
                         claimed_by = NULL, claimed_at = NULL, join_target = NULL,
                         join_threshold = NULL, join_when = NULL,
                         end_response = $1, updated_at = now()
                     WHERE wfe_id = $2 AND orgtnt_id = $3",
                )
                .bind(end_response)
                .bind(commit.wfe_id)
                .bind(commit.orgtnt_id)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
            }
            // WOR-31: paralel moda GİRİŞ — her kol için satır, current_node=NULL,
            // join_target persist. Fork tekil modda gerçekleştiği için CAS gerekmez.
            CommitOutcome::ForkTo {
                branches,
                join,
                join_rule,
            } => {
                let join_json = serde_json::to_value(join).map_err(db_err)?;
                // WOR-72/WOR-73: çözülmüş kural fork anında persist edilir.
                // İkisi de NULL = AND-join.
                let (threshold, when) = match join_rule {
                    JoinRule::All => (None, None),
                    JoinRule::Quorum(k) => (Some(*k as i32), None),
                    JoinRule::Expr(e) => (None, Some(e.clone())),
                };
                sqlx::query(
                    "UPDATE wf.wfe
                     SET current_node = NULL, current_c_a = '[]'::jsonb, claimed_by = NULL,
                         claimed_at = NULL, join_target = $1, join_threshold = $4,
                         join_when = $5, updated_at = now()
                     WHERE wfe_id = $2 AND orgtnt_id = $3",
                )
                .bind(&join_json)
                .bind(commit.wfe_id)
                .bind(commit.orgtnt_id)
                .bind(threshold)
                .bind(when)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
                for branch_node in branches {
                    // WOR-73: entry_node = giriş node'u ve BİR DAHA DEĞİŞMEZ
                    // (BranchMoveTo yalnız branch_node'u günceller) — kol kimliği.
                    sqlx::query(
                        "INSERT INTO wf.wfe_branch
                             (wfe_id, branch_node, entry_node, status, entered_at)
                         VALUES ($1, $2, $2, 'active', now())",
                    )
                    .bind(commit.wfe_id)
                    .bind(branch_node)
                    .execute(&mut *tx)
                    .await
                    .map_err(db_err)?;
                }
            }
            // WOR-31: tek kol token hareketi — kol CAS'ı (status='active'), claim +
            // entered_at sıfırlanır. Eşleşme yoksa yarış → Conflict.
            // WOR-62: kol CAS'ından ÖNCE wfe kilidi + paralel-mod doğrulaması.
            // Böylece "eşzamanlı kardeş collapse" senaryosu kol CAS'ının belirsiz
            // 0-satır sonucuna değil, NET `Conflict(Collapsed)`'a düşer.
            CommitOutcome::BranchMoveTo { from_node, node } => {
                lock_wfe_parallel(&mut tx, commit.wfe_id, commit.orgtnt_id).await?;
                let res = sqlx::query(&format!(
                    "UPDATE wf.wfe_branch
                     SET branch_node = $1, {CLEAR_BRANCH_CLAIM},
                         entered_at = now(), updated_at = now()
                     WHERE wfe_id = $2 AND branch_node = $3 AND status = 'active'"
                ))
                .bind(node)
                .bind(commit.wfe_id)
                .bind(from_node)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
                if res.rows_affected() != 1 {
                    return Err(EngineError::Conflict(ConflictKind::BranchMoved));
                }
            }
            // WOR-31: kol join'e vardı, engine'in görüşüne göre join HENÜZ dolmadı.
            // FOR UPDATE ile serialize et; kol CAS + kilit altında aynı ölçüt
            // (WOR-72: AND → kalan aktif kol, quorum → varış sayısı) yeniden
            // hesaplanır. Doldu çıkarsa engine'in görüşü eskimiştir (bu kol aslında
            // join'i tamamlıyor) → Conflict, executor reload edip JoinComplete emit eder.
            CommitOutcome::BranchArrived {
                from_node,
                arrived_entries,
            } => {
                let join = lock_and_read_join_state(&mut tx, commit.wfe_id, commit.orgtnt_id)
                    .await?;
                let acting = branch_entry_node(&mut tx, commit.wfe_id, from_node).await?;
                mark_branch_arrived(&mut tx, commit.wfe_id, from_node).await?;
                if !join.arrival_matches(&acting, arrived_entries) {
                    return Err(EngineError::Conflict(ConflictKind::BranchArrival));
                }
            }
            // WOR-31: varan kol engine'in görüşüne göre join'i TAMAMLIYOR — paralel
            // mod biter. FOR UPDATE serialize + kol CAS; kilit altında tamamlanma
            // ölçütü doğrulanır (AND: bu kol düşünce 0 aktif kalmalı; WOR-72 quorum:
            // varış sayısı eşiğe ulaşmalı) — tutmazsa Conflict. Ardından `next`
            // (join node MoveTo / join terminal) uygulanır + `_join` marker'ı.
            CommitOutcome::JoinComplete {
                from_node,
                quorum_collapse,
                arrived_entries,
                next,
            } => {
                let join = lock_and_read_join_state(&mut tx, commit.wfe_id, commit.orgtnt_id)
                    .await?;
                let acting = branch_entry_node(&mut tx, commit.wfe_id, from_node).await?;
                mark_branch_arrived(&mut tx, commit.wfe_id, from_node).await?;
                if !join.arrival_matches(&acting, arrived_entries) {
                    return Err(EngineError::Conflict(ConflictKind::BranchArrival));
                }
                // WOR-72: engine'in "eşik dolarken geride aktif kol kaldı" görüşü de
                // kilit altında doğrulanır — uyuşmazsa marker'lar (kimin işi iptal
                // edildi) gerçekle çelişirdi, bu yüzden yeniden koşulur.
                let leftover_active = join.active - 1;
                if *quorum_collapse != (leftover_active > 0) {
                    return Err(EngineError::Conflict(ConflictKind::BranchArrival));
                }
                if *quorum_collapse {
                    // Kalan kollar `cancelled` + claim'leri düşer (engine
                    // `_branch_cancelled` marker'larını zaten stage etti). Satırlar
                    // AND yolunun aksine SİLİNMEZ (aşağıya bak) — "hangi kol quorum
                    // yüzünden düştü" portal tarafında görünür kalsın.
                    cancel_active_branches(&mut tx, commit.wfe_id).await?;
                }
                // `_join` sistem marker'ı (dokümante istisna: adapter ekler) —
                // seq = son staged wfah seq + 1.
                let join_seq = commit.wfah_entries.last().map(|e| e.seq + 1).unwrap_or(1);
                let join_entry = WfahEntry {
                    seq: join_seq,
                    action: "_join".into(),
                    actor: Actor {
                        orgu_id: Uuid::nil(),
                        user_id: Uuid::nil(),
                        role: "system".into(),
                    },
                    input: None,
                    applied_at: chrono::Utc::now(),
                };
                insert_wfah_entries(&mut tx, commit.wfe_id, std::slice::from_ref(&join_entry))
                    .await?;

                // WOR-31: AND-join'de kol satırları silinir (audit WFAH'ta durur).
                // WOR-72: quorum join'de SİLİNMEZ — iptal edilen kolların satırı
                // `cancelled` olarak kalır (collapse yolundaki davranış).
                let drop_branch_rows = !*quorum_collapse;
                match next.as_ref() {
                    CommitOutcome::MoveTo { node } => {
                        // Join node'a UNASSIGNED giriş; paralel mod biter
                        // (join_target + join_threshold temizlenir).
                        let c_a_json =
                            serde_json::to_value(&commit.resolved_c_a).map_err(db_err)?;
                        sqlx::query(
                            "UPDATE wf.wfe
                             SET current_node = $1, current_c_a = $2, claimed_by = NULL,
                                 claimed_at = NULL, join_target = NULL,
                                 join_threshold = NULL, join_when = NULL, updated_at = now()
                             WHERE wfe_id = $3 AND orgtnt_id = $4",
                        )
                        .bind(node)
                        .bind(&c_a_json)
                        .bind(commit.wfe_id)
                        .bind(commit.orgtnt_id)
                        .execute(&mut *tx)
                        .await
                        .map_err(db_err)?;
                        if drop_branch_rows {
                            sqlx::query("DELETE FROM wf.wfe_branch WHERE wfe_id = $1")
                                .bind(commit.wfe_id)
                                .execute(&mut *tx)
                                .await
                                .map_err(db_err)?;
                        }
                    }
                    CommitOutcome::Terminal { end_response } => {
                        // Join hedefi terminal → WFE burada başarıyla biter.
                        sqlx::query(
                            "UPDATE wf.wfe
                             SET status = 'terminal', current_node = NULL,
                                 current_c_a = '[]'::jsonb, claimed_by = NULL, claimed_at = NULL,
                                 join_target = NULL, join_threshold = NULL, join_when = NULL,
                                 end_response = $1, updated_at = now()
                             WHERE wfe_id = $2 AND orgtnt_id = $3",
                        )
                        .bind(end_response)
                        .bind(commit.wfe_id)
                        .bind(commit.orgtnt_id)
                        .execute(&mut *tx)
                        .await
                        .map_err(db_err)?;
                        if drop_branch_rows {
                            sqlx::query("DELETE FROM wf.wfe_branch WHERE wfe_id = $1")
                                .bind(commit.wfe_id)
                                .execute(&mut *tx)
                                .await
                                .map_err(db_err)?;
                        }
                    }
                    other => {
                        return Err(EngineError::WfePort(format!(
                            "JoinComplete.next beklenmeyen outcome: {other:?}"
                        )))
                    }
                }
            }
            // WOR-56: node hedefli collapse — paralel mod biter, WFE `node`'a
            // UNASSIGNED girer. Aktif kollar `cancelled` (audit için satır kalır;
            // engine `_branch_cancelled` marker'larını zaten wfah'a staged etti),
            // join_target temizlenir.
            //
            // WOR-62: collapse "otoriter"dir ama SERİLEŞTİRİLMEK zorundadır.
            // Kol-arrival SAYIMI hâlâ yok (JoinComplete'in aksine kalan aktif
            // kolları beklemez) — eklenen şey kilit + paralel-mod doğrulaması:
            //   - `FOR UPDATE`: eşzamanlı kardeş commit'leri (BranchMoveTo /
            //     BranchArrived / JoinComplete / diğer collapse) bu tx'in
            //     arkasında sıraya girer; iki taraf da kolları yarı yolda
            //     görmez, tutarsız ara durum oluşmaz.
            //   - paralel-mod kapısı: bu tx kilidi aldığında `join_target` NULL
            //     ise bir kardeş ÖNCE davranmıştır (o da collapse/join/terminal
            //     yapmıştır) → bu collapse kaybeder, `Conflict(Collapsed)`.
            //     "İlk kilidi alan kazanır" tek ve deterministik kuraldır.
            // Kilit sırası wfe → wfe_branch (bkz. `lock_wfe`).
            CommitOutcome::CollapseTo { node, .. } => {
                lock_wfe_parallel(&mut tx, commit.wfe_id, commit.orgtnt_id).await?;
                cancel_active_branches(&mut tx, commit.wfe_id).await?;
                let c_a_json = serde_json::to_value(&commit.resolved_c_a).map_err(db_err)?;
                sqlx::query(
                    "UPDATE wf.wfe
                     SET current_node = $1, current_c_a = $2, claimed_by = NULL,
                         claimed_at = NULL, join_target = NULL, join_threshold = NULL, join_when = NULL,
                         updated_at = now()
                     WHERE wfe_id = $3 AND orgtnt_id = $4",
                )
                .bind(node)
                .bind(&c_a_json)
                .bind(commit.wfe_id)
                .bind(commit.orgtnt_id)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
            }
        }

        // WFC outbox — bu transition ile varılan sitedeki çağrı(lar) AYNI tx'te
        // `queued` yazılır. Çift start koruması `ON CONFLICT DO NOTHING` +
        // `UNIQUE (caller_wfe_id, site_kind, site_key)` ile: executor'ın conflict
        // retry döngüsü aynı transition'ı ikinci kez koşarsa çağrı tekrar başlamaz.
        if !commit.staged_calls.is_empty() {
            let (depth, next_depth) = caller_depths(&mut tx, commit.wfe_id).await?;
            repo::call::stage(
                &mut tx,
                commit.orgtnt_id,
                commit.wfe_id,
                &commit.staged_calls,
                depth,
                next_depth,
            )
            .await
            .map_err(db_err)?;
        }

        tx.commit().await.map_err(db_err)
    }

    // ---- WFC (iş akışı çağrısı) ----

    async fn pending_call_starts(&self, limit: i64) -> Result<Vec<PendingCall>, EngineError> {
        repo::call::pending_starts(&self.pool, limit)
            .await
            .map_err(db_err)
    }

    async fn pending_call_returns(&self, limit: i64) -> Result<Vec<PendingCall>, EngineError> {
        repo::call::pending_returns(&self.pool, limit)
            .await
            .map_err(db_err)
    }

    async fn overdue_calls(
        &self,
        now: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<PendingCall>, EngineError> {
        repo::call::overdue(&self.pool, now, limit)
            .await
            .map_err(db_err)
    }

    async fn set_call_status(
        &self,
        call_row_id: Uuid,
        status: &str,
        callee_wfe_id: Option<Uuid>,
    ) -> Result<(), EngineError> {
        repo::call::set_status(&self.pool, call_row_id, status, callee_wfe_id)
            .await
            .map_err(db_err)
    }

    async fn cancel_subcalls_of(&self, caller_wfe_id: Uuid) -> Result<Vec<Uuid>, EngineError> {
        repo::call::cancel_subcalls_of(&self.pool, caller_wfe_id)
            .await
            .map_err(db_err)
    }

    async fn calls_of_caller(&self, caller_wfe_id: Uuid) -> Result<Vec<CallView>, EngineError> {
        let rows = repo::call::list_of_caller(&self.pool, caller_wfe_id)
            .await
            .map_err(db_err)?;
        Ok(rows
            .into_iter()
            .map(|r| CallView {
                site_kind: r.site_kind,
                site_key: r.site_key,
                call_key: r.call_key,
                mode: r.mode,
                status: r.status,
                wfe_id: r.callee_wfe_id,
                call_status: r.call_status,
            })
            .collect())
    }

    async fn caller_of(&self, callee_wfe_id: Uuid) -> Result<Option<CallView>, EngineError> {
        let row = repo::call::caller_of(&self.pool, callee_wfe_id)
            .await
            .map_err(db_err)?;
        Ok(row.map(|(caller_wfe_id, r)| CallView {
            site_kind: r.site_kind,
            site_key: r.site_key,
            call_key: r.call_key,
            mode: r.mode,
            status: r.status,
            // Çağıran görünümünde ilgi duyulan id ÇAĞIRANdır (çağrılan zaten biz'iz).
            wfe_id: Some(caller_wfe_id),
            call_status: r.call_status,
        }))
    }

    async fn mark_callee_finished(
        &self,
        callee_wfe_id: Uuid,
        status: &str,
        end_response: Option<&Value>,
    ) -> Result<(), EngineError> {
        repo::call::mark_callee_finished(&self.pool, callee_wfe_id, status, end_response)
            .await
            .map_err(db_err)
    }

    async fn claim(
        &self,
        wfe_id: Uuid,
        orgtnt_id: Uuid,
        user_id: Uuid,
        branch: Option<&str>,
        marker: Option<&WfahEntry>,
    ) -> Result<bool, EngineError> {
        // CAS: yalnızca unassigned aktif VE deadline'ı geçmemiş WFE claim edilebilir —
        // eşzamanlı claim'lerden yalnızca biri satırı günceller (V1 stateless claim'in
        // kalıcı çözümü). `deadline` kontrolü DB seviyesinde tekrarlanır (2026-07-16 fix):
        // Engine::can_claim aynı kontrolü zaten yapar ama sweeper'ın henüz `terminated`'a
        // taşımadığı bir satırda check-then-write arasında güvenlik ağı sağlar.
        // Madde 6: tx içinde — CAS kazanılır VE `marker` verilirse (vekaleten claim)
        // audit WFAH kaydı AYNI transaction'da yazılır.
        let claimed_by = json!({ "user_id": user_id.to_string() });
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let result = match branch {
            // WOR-31: paralel modda CAS o kolun `wf.wfe_branch` satırında; deadline
            // wfe-seviyesidir (join'e sabit) → wfe satırına JOIN ile kontrol edilir.
            Some(branch_node) => sqlx::query(
                "UPDATE wf.wfe_branch b
                 SET claimed_by = $1, claimed_at = now(), updated_at = now()
                 FROM wf.wfe w
                 WHERE b.wfe_id = $2 AND b.branch_node = $3 AND b.status = 'active'
                   AND b.claimed_by IS NULL
                   AND w.wfe_id = b.wfe_id AND w.orgtnt_id = $4 AND w.status = 'active'
                   AND (w.deadline IS NULL OR w.deadline > now())",
            )
            .bind(&claimed_by)
            .bind(wfe_id)
            .bind(branch_node)
            .bind(orgtnt_id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?,
            None => sqlx::query(
                "UPDATE wf.wfe
                 SET claimed_by = $1, claimed_at = now(), updated_at = now()
                 WHERE wfe_id = $2 AND orgtnt_id = $3 AND status = 'active' AND claimed_by IS NULL
                   AND (deadline IS NULL OR deadline > now())",
            )
            .bind(&claimed_by)
            .bind(wfe_id)
            .bind(orgtnt_id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?,
        };
        let won = result.rows_affected() == 1;
        if won {
            if let Some(entry) = marker {
                insert_wfah_entries(&mut tx, wfe_id, std::slice::from_ref(entry)).await?;
            }
        }
        tx.commit().await.map_err(db_err)?;
        Ok(won)
    }

    /// SLA-1 claim timeout (wft'siz kol, bkz. `Engine::fire_claim_timeout`):
    /// node DEĞİŞMEDEN claimed_by/claimed_at temizlenir + WFAH marker eklenir.
    /// `new_dynctx` verilmişse (SLA-1 `wfes_effects`) ctx satırı da AYNI
    /// transaction'da marker'ın seq'i ile yazılır.
    async fn release_claim(
        &self,
        wfe_id: Uuid,
        orgtnt_id: Uuid,
        wfah_entry: &WfahEntry,
        branch: Option<&str>,
        new_dynctx: Option<&Value>,
    ) -> Result<(), EngineError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        match branch {
            // WOR-31: paralel modda yalnızca o kolun claim'i sıfırlanır (node
            // DEĞİŞMEZ; kol `active` kalır).
            Some(branch_node) => {
                sqlx::query(&format!(
                    "UPDATE wf.wfe_branch
                     SET {CLEAR_BRANCH_CLAIM}, updated_at = now()
                     WHERE wfe_id = $1 AND branch_node = $2 AND status = 'active'"
                ))
                .bind(wfe_id)
                .bind(branch_node)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
            }
            None => {
                sqlx::query(
                    "UPDATE wf.wfe
                     SET claimed_by = NULL, claimed_at = NULL, updated_at = now()
                     WHERE wfe_id = $1 AND orgtnt_id = $2",
                )
                .bind(wfe_id)
                .bind(orgtnt_id)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
            }
        }

        if let Some(ctx) = new_dynctx {
            sqlx::query("INSERT INTO wf.wfe_dynctx (wfe_id, seq, ctx) VALUES ($1, $2, $3)")
                .bind(wfe_id)
                .bind(wfah_entry.seq as i32)
                .bind(ctx)
                .execute(&mut *tx)
                .await
                // WOR-65: seq çakışması = eşzamanlı commit (bkz. `insert_err`).
                .map_err(insert_err)?;
        }

        insert_wfah_entries(&mut tx, wfe_id, std::slice::from_ref(wfah_entry)).await?;

        tx.commit().await.map_err(db_err)
    }

    /// Madde 7: yetkili devir. `release_claim` ile aynı desen ama `claimed_by`
    /// hedefe (veya havuza) ayarlanır. `target = Some`: claimed_by = {user_id},
    /// claimed_at = now(); `None`: her ikisi NULL (havuz). Uygunluk
    /// `Engine::reassign`'da doğrulanmıştır — burada yalnızca `status = 'active'`
    /// kapısı vardır (claim CAS'ının `claimed_by IS NULL` koşulu YOK: override).
    async fn reassign(
        &self,
        wfe_id: Uuid,
        orgtnt_id: Uuid,
        target: Option<Uuid>,
        wfah_entry: &WfahEntry,
        branch: Option<&str>,
    ) -> Result<(), EngineError> {
        let claimed_by = target.map(|user_id| json!({ "user_id": user_id.to_string() }));
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        match branch {
            // WOR-31: paralel modda yalnız o kolun sahipliği değişir (node DEĞİŞMEZ).
            Some(branch_node) => {
                sqlx::query(
                    "UPDATE wf.wfe_branch b
                     SET claimed_by = $1,
                         claimed_at = CASE WHEN $1 IS NULL THEN NULL ELSE now() END,
                         updated_at = now()
                     FROM wf.wfe w
                     WHERE b.wfe_id = $2 AND b.branch_node = $3 AND b.status = 'active'
                       AND w.wfe_id = b.wfe_id AND w.orgtnt_id = $4 AND w.status = 'active'",
                )
                .bind(&claimed_by)
                .bind(wfe_id)
                .bind(branch_node)
                .bind(orgtnt_id)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
            }
            None => {
                sqlx::query(
                    "UPDATE wf.wfe
                     SET claimed_by = $1,
                         claimed_at = CASE WHEN $1 IS NULL THEN NULL ELSE now() END,
                         updated_at = now()
                     WHERE wfe_id = $2 AND orgtnt_id = $3 AND status = 'active'",
                )
                .bind(&claimed_by)
                .bind(wfe_id)
                .bind(orgtnt_id)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
            }
        }

        insert_wfah_entries(&mut tx, wfe_id, std::slice::from_ref(wfah_entry)).await?;

        tx.commit().await.map_err(db_err)
    }
}

/// Bu WFE'nin kendisi bir çağrı ile yaratıldıysa taşınacak derinlik sayaçları.
///
/// Kök WFE'de (0, 0). Zincir uzadıkça `stage` bunları +1'ler — hangisini artıracağını
/// çağrının MODU belirler (`terminal` → `next_depth`, diğerleri → `depth`).
async fn caller_depths(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    wfe_id: Uuid,
) -> Result<(i32, i32), EngineError> {
    let row =
        sqlx::query("SELECT depth, next_depth FROM wf.wfe_call WHERE callee_wfe_id = $1 LIMIT 1")
            .bind(wfe_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(db_err)?;
    Ok(row
        .map(|r| {
            (
                sqlx::Row::get::<i32, _>(&r, "depth"),
                sqlx::Row::get::<i32, _>(&r, "next_depth"),
            )
        })
        .unwrap_or((0, 0)))
}
