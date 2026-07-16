//! §7 Transition Runtime Pipeline (M8 — atomik, staged diff'ler tek commit'te).
//!
//! ```text
//! 1. WFE assigned mi? Actor owner mı? Değilse ACT reddedilir.
//! 2. transition.c_a varsa: owner bu EK kurala da match etmeli (§3).
//! 3. current_node ∈ transition.from? Değilse aday değildir.
//! 4. Adaylar array sırasıyla; when'i true olan İLK transition seçilir.
//! 5. Action input validate edilir.
//! 6. transition.wfes_effects STAGED.
//! 7. trigger[] sırayla: when → execute (timeout) → fail'de retry → catch.
//! 8. transition.wft staged DynCtx üzerinden evaluate edilir.
//! 9. COMMIT (atomik) — store'a TransitionCommit olarak devredilir.
//! ```
//!
//! Linear: WOR-36 (current_node + ilk-match), WOR-39 (WFT formları),
//! WOR-41/45 (trigger + retry/catch), WOR-42 (terminal), WOR-43 (atomik commit),
//! WOR-46 (timeout), WOR-47 (escalation).

use crate::error::EngineError;
use crate::ports::OrgPort;
use crate::types::actor::{Actor, CandidateActor as ResolvedCandidate};
use crate::types::wfah::{Wfah, WfahEntry};
use crate::types::wfd_v22::{
    ActionDef, AutoexecDef, CandidateActor, EscalationStep, TriggerInvocation, Wfd, Wft,
    WftTarget,
};
use crate::types::wfe::WfeStatus;
use crate::v22::duration::parse_iso8601_duration;
use crate::v22::effects::{apply_effects, get_path, resolve_value, set_path, EffectEnv};
use crate::v22::eval::{evaluate_bool, EvalEnv};
use crate::v22::matcher::{authorize, MatchEnv};
use crate::v22::ports::{
    AutoexecRunner, CommitOutcome, ExecEnv, ExecFailure, NewWfe, TransitionCommit, Wfes,
};
use crate::v22::resolver::resolve_c_orgu;
use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};
use uuid::Uuid;

pub struct Engine<'a> {
    pub org: &'a dyn OrgPort,
    pub exec: &'a dyn AutoexecRunner,
}

/// Claim uygunluk sonucu — portal'a neden bilgisi taşır.
#[derive(Debug, PartialEq)]
pub enum ClaimCheck {
    Ok,
    Terminal,
    AlreadyClaimed,
    NotEligible,
}

/// Node'un ilk ateşlenmemiş escalation adımı için giriş/vade bilgisi.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EscalationForecast {
    pub step_idx: usize,
    pub entered_at: DateTime<Utc>,
    pub deadline: DateTime<Utc>,
    pub overdue: bool,
}

/// `fire_claim_timeout` sonucu — `wft` verilmişse node/terminal taşıması
/// (`TransitionCommit` — normal `commit()` yolu), verilmemişse yalnızca
/// claimed_by/claimed_at temizliği (`release_claim` yolu, node DEĞİŞMEZ).
#[derive(Debug, Clone)]
pub enum ClaimTimeoutOutcome {
    Move(TransitionCommit),
    Release(WfahEntry),
}

/// `Terminal` ve `Terminated` her ikisi de "aktif değil" sınıfıdır: yeni
/// aksiyon/claim/escalation kabul etmezler (2026-07-16 SLA sözleşmesi).
fn is_terminal_class(status: &WfeStatus) -> bool {
    matches!(status, WfeStatus::Terminal | WfeStatus::Terminated)
}

impl<'a> Engine<'a> {
    // ---------------------------------------------------------------- start

    /// Yeni WFE başlatır. `wfe_id` çağıran tarafından üretilir ve effects
    /// GERÇEK id ile çözülür (WOR-6).
    ///
    /// `action`: M16 sonrası start aksiyonları gerçek ad taşır; verilirse yalnız o
    /// action adını taşıyan start kuralları aday olur (spec runtime resolution —
    /// "actor, start[].action ile adlandırılmış aksiyonu çağırır"). `None` = tüm
    /// start kuralları aday (tek start aksiyonlu WFD'ler ve eski istemciler).
    /// `deadline`: SLA-3 — başlatan kullanıcının opsiyonel ISO 8601 duration'ı
    /// (start anından itibaren). `wfd.timeout` tanımlıysa `deadline ≤ timeout`
    /// olmalı (aksi InvalidInput); resolved mutlak deadline `NewWfe.deadline`'a
    /// yazılır (2026-07-16 SLA sözleşmesi).
    #[allow(clippy::too_many_arguments)]
    pub async fn start(
        &self,
        wfd: &Wfd,
        actor: &Actor,
        orgtnt_id: Uuid,
        action: Option<&str>,
        input: &Value,
        wfe_id: Uuid,
        deadline: Option<&str>,
    ) -> Result<NewWfe, EngineError> {
        let empty_ctx = json!({});
        let empty_wfah = Wfah::empty();

        // Actor'ün başlatabildiği ilk kural (istenirse action adına daraltılmış)
        let mut rule = None;
        for r in &wfd.start {
            if let Some(a) = action {
                if r.action != a {
                    continue;
                }
            }
            // Simetrik start: initiator yetkisi `from` node'unun c_a'sında yaşar.
            let node = wfd.nodes.get(&r.from).ok_or_else(|| {
                EngineError::InvalidWfd(format!("start.from bilinmeyen node: '{}'", r.from))
            })?;
            let env = MatchEnv { ctx: &empty_ctx, wfah: &empty_wfah, orgtnt_id };
            if authorize(&node.c_a, actor, env, self.org).await? {
                rule = Some(r);
                break;
            }
        }
        let rule = rule.ok_or(EngineError::StartNotEligible)?;

        // §7.5 simetrisi: start input'u da transition input'ları gibi doğrulanır —
        // action.input.required mevcut olmalı, bildirilmemiş yol reddedilir; başlangıç
        // ctx'i YALNIZ bildirilen yollardan + effects'ten tohumlanır (serbest-form
        // context enjeksiyonu kapalı).
        let input_norm = match input {
            Value::Object(_) => input.clone(),
            Value::Null => json!({}),
            _ => return Err(EngineError::InvalidInput("start input obje olmalı".into())),
        };
        let input = &input_norm;
        let action_def = wfd.actions.get(&rule.action).ok_or_else(|| {
            EngineError::InvalidWfd(format!(
                "start action '{}' actions içinde tanımsız",
                rule.action
            ))
        })?;
        validate_readonly_paths(action_def, input, &wfd.context)?;
        let mut staged = merge_action_input(&json!({}), action_def, input)?;

        let now = Utc::now();
        let mut wfah_entries: Vec<WfahEntry> = Vec::new();
        let mut seq = 1u32;

        // SLA-3: efektif deadline çözümü — deadline verildi → now+parse(deadline)
        // (timeout tavanına tabi); verilmedi ve wfd.timeout var → now+parse(timeout);
        // ikisi de yok → NULL.
        let resolved_deadline: Option<DateTime<Utc>> = match (deadline, &wfd.timeout) {
            (Some(d), Some(t)) => {
                let dur = parse_iso8601_duration(d)?;
                let ceiling = parse_iso8601_duration(t)?;
                if dur > ceiling {
                    return Err(EngineError::InvalidInput(format!(
                        "deadline '{d}' WFD timeout '{t}' değerini aşamaz"
                    )));
                }
                Some(now + dur)
            }
            (Some(d), None) => Some(now + parse_iso8601_duration(d)?),
            (None, Some(t)) => Some(now + parse_iso8601_duration(t)?),
            (None, None) => None,
        };

        if let Some(effects) = &rule.wfes_effects {
            let env = EffectEnv {
                actor,
                wfe_id,
                node: None,
                action_input: Some(input),
                exec_result: None,
                now,
            };
            staged = apply_effects(&staged, effects, &env)?;
        }

        wfah_entries.push(WfahEntry {
            seq,
            // Transition'lar gibi düz action adını yazar (rule.id DEĞİL). M16: start
            // aksiyonu gerçek adını taşır; `c_orgu` WFAH anchor'ları da aynı gerçek
            // adı referans alır (from.wfah = "<start action adı>").
            action: rule.action.clone(),
            actor: actor.clone(),
            input: Some(input.clone()),
            applied_at: now,
        });
        seq += 1;

        self.run_triggers(
            &rule.trigger,
            wfd,
            &mut staged,
            &mut wfah_entries,
            &mut seq,
            actor,
            wfe_id,
            None,
            Some(input),
            &empty_wfah,
            orgtnt_id,
        )
        .await?;

        let (outcome, resolved_c_a, final_ctx) = self
            .resolve_wft(
                &rule.wft,
                wfd,
                staged,
                &empty_wfah,
                actor,
                wfe_id,
                Some(input),
                orgtnt_id,
            )
            .await?;

        // context.required, start zinciri (input merge + effects + wft effects)
        // tamamlandıktan SONRA denetlenir: alanlar input'tan değil effect'lerden de
        // yazılabilir ($action.input.X → ctx).
        validate_context_required(&final_ctx, &wfd.context)?;

        Ok(NewWfe {
            wfe_id,
            orgtnt_id,
            wfd_id: parse_wfd_uuid(wfd)?,
            wfd_version: 0, // store katmanı gerçek versiyon satırını bilir; executor doldurur
            initial_dynctx: final_ctx,
            wfah_entries,
            outcome,
            resolved_c_a,
            deadline: resolved_deadline,
        })
    }

    // ---------------------------------------------------------------- apply

    pub async fn apply(
        &self,
        wfd: &Wfd,
        wfes: &Wfes,
        actor: &Actor,
        action: &str,
        input: &Value,
    ) -> Result<TransitionCommit, EngineError> {
        if is_terminal_class(&wfes.status) {
            return Err(EngineError::WfeTerminal);
        }
        let current_node = wfes
            .current_node
            .as_deref()
            .ok_or_else(|| EngineError::InvalidWfd("aktif WFE'nin current_node'u yok".into()))?;

        // §7.1 — assignment / owner kontrolü
        match wfes.assigned_to {
            None => return Err(EngineError::NotClaimed),
            Some(owner) if owner != actor.user_id => return Err(EngineError::NotOwner),
            _ => {}
        }

        // §7.3–7.4 — aday transitions, ilk when-match
        let ctx = wfes.dynctx.as_value().clone();
        let mut selected = None;
        for t in &wfd.transitions {
            if t.action != action || !t.from.contains(current_node) {
                continue;
            }
            let matches = match &t.when {
                None => true,
                Some(expr) => {
                    let env = EvalEnv::new(&ctx)
                        .with_wfah(&wfes.wfah)
                        .with_node(Some(current_node))
                        .with_actor(actor)
                        .with_wfe_id(wfes.wfe_id)
                        .with_action_input(input);
                    evaluate_bool(expr, &env)?
                }
            };
            if matches {
                selected = Some(t);
                break;
            }
        }
        let transition =
            selected.ok_or_else(|| EngineError::TransitionNotFound(action.to_string()))?;

        // §7.2 — ek yetki kısıtı
        if let Some(extra_rule) = &transition.c_a {
            let env = MatchEnv { ctx: &ctx, wfah: &wfes.wfah, orgtnt_id: wfes.orgtnt_id };
            if !authorize(extra_rule, actor, env, self.org).await? {
                return Err(EngineError::PermissionDenied(action.to_string()));
            }
        }

        // §7.5 — input validation + declared path'lerin ctx'e yazımı
        let action_def = wfd
            .actions
            .get(action)
            .ok_or_else(|| EngineError::InvalidWfd(format!("action '{action}' tanımsız")))?;
        let mut staged = merge_action_input(&ctx, action_def, input)?;

        let now = Utc::now();
        let mut seq = wfes.wfah.entries().last().map(|e| e.seq + 1).unwrap_or(1);
        let mut wfah_entries: Vec<WfahEntry> = Vec::new();

        // §7.6 — transition effects STAGED
        if let Some(effects) = &transition.wfes_effects {
            let env = EffectEnv {
                actor,
                wfe_id: wfes.wfe_id,
                node: Some(current_node),
                action_input: Some(input),
                exec_result: None,
                now,
            };
            staged = apply_effects(&staged, effects, &env)?;
        }

        wfah_entries.push(WfahEntry {
            seq,
            action: action.to_string(),
            actor: actor.clone(),
            input: Some(input.clone()),
            applied_at: now,
        });
        seq += 1;

        // §7.7 — trigger'lar
        self.run_triggers(
            &transition.trigger,
            wfd,
            &mut staged,
            &mut wfah_entries,
            &mut seq,
            actor,
            wfes.wfe_id,
            Some(current_node),
            Some(input),
            &wfes.wfah,
            wfes.orgtnt_id,
        )
        .await?;

        // §7.8 — wft staged ctx üzerinden
        let (outcome, resolved_c_a, final_ctx) = self
            .resolve_wft(
                &transition.wft,
                wfd,
                staged,
                &wfes.wfah,
                actor,
                wfes.wfe_id,
                Some(input),
                wfes.orgtnt_id,
            )
            .await?;

        Ok(TransitionCommit {
            wfe_id: wfes.wfe_id,
            orgtnt_id: wfes.orgtnt_id,
            new_dynctx: final_ctx,
            wfah_entries,
            outcome,
            resolved_c_a,
        })
    }

    // ---------------------------------------------------------------- claim

    pub async fn can_claim(
        &self,
        wfd: &Wfd,
        wfes: &Wfes,
        actor: &Actor,
    ) -> Result<ClaimCheck, EngineError> {
        if is_terminal_class(&wfes.status) {
            return Ok(ClaimCheck::Terminal);
        }
        if wfes.assigned_to.is_some() {
            return Ok(ClaimCheck::AlreadyClaimed);
        }
        let Some(node_key) = wfes.current_node.as_deref() else {
            return Ok(ClaimCheck::NotEligible);
        };
        let Some(node) = wfd.nodes.get(node_key) else {
            return Ok(ClaimCheck::NotEligible);
        };
        let ctx = wfes.dynctx.as_value();
        let env = MatchEnv { ctx, wfah: &wfes.wfah, orgtnt_id: wfes.orgtnt_id };
        if authorize(&node.c_a, actor, env, self.org).await? {
            Ok(ClaimCheck::Ok)
        } else {
            Ok(ClaimCheck::NotEligible)
        }
    }

    // ------------------------------------------------------ possible actions

    /// Owner'ın şu an gerçekleştirebileceği action adları.
    pub async fn possible_actions(
        &self,
        wfd: &Wfd,
        wfes: &Wfes,
        actor: &Actor,
    ) -> Result<Vec<String>, EngineError> {
        if is_terminal_class(&wfes.status) || wfes.assigned_to != Some(actor.user_id) {
            return Ok(vec![]);
        }
        let Some(current_node) = wfes.current_node.as_deref() else {
            return Ok(vec![]);
        };
        let ctx = wfes.dynctx.as_value().clone();
        let mut actions = Vec::new();
        for t in &wfd.transitions {
            if !t.from.contains(current_node) || actions.contains(&t.action) {
                continue;
            }
            let when_ok = match &t.when {
                None => true,
                Some(expr) => {
                    let env = EvalEnv::new(&ctx)
                        .with_wfah(&wfes.wfah)
                        .with_node(Some(current_node))
                        .with_actor(actor)
                        .with_wfe_id(wfes.wfe_id);
                    // input'a bağlı guard'lar input'suz değerlendirilemez — aday sayılır
                    evaluate_bool(expr, &env).unwrap_or(true)
                }
            };
            if !when_ok {
                continue;
            }
            if let Some(extra_rule) = &t.c_a {
                let env = MatchEnv { ctx: &ctx, wfah: &wfes.wfah, orgtnt_id: wfes.orgtnt_id };
                if !authorize(extra_rule, actor, env, self.org).await? {
                    continue;
                }
            }
            actions.push(t.action.clone());
        }
        Ok(actions)
    }

    // ------------------------------------------------------------ escalation

    /// Node'un henüz ateşlenmemiş ilk escalation adımı için giriş anı + vade
    /// bilgisi — dashboard insight'ları (yaklaşan/geciken escalation) ve
    /// `due_escalation` ortak temeli. Node'a giriş anı son WFAH kaydından
    /// türetilir; ateşlenen adımlar `escalate:<node>:<idx>` WFAH kayıtlarıyla
    /// izlenir.
    pub fn next_escalation(
        &self,
        wfd: &Wfd,
        wfes: &Wfes,
        now: DateTime<Utc>,
    ) -> Result<Option<EscalationForecast>, EngineError> {
        if is_terminal_class(&wfes.status) {
            return Ok(None);
        }
        let Some(node_key) = wfes.current_node.as_deref() else {
            return Ok(None);
        };
        let Some(node) = wfd.nodes.get(node_key) else {
            return Ok(None);
        };
        // Node giriş zamanı = current_node'a taşınmadan sonraki son insan/sistem eylemi;
        // escalation marker'ları HARİÇ tutulur, aksi halde her adım bir öncekinin
        // marker'ından ölçülür ve N≥1 adımların `after`'ı kayar (spec: hepsi node
        // girişinden ölçülür).
        let Some(entered_at) = wfes
            .wfah
            .entries()
            .iter()
            .filter(|e| !e.action.starts_with("escalate:"))
            .last()
            .map(|e| e.applied_at)
        else {
            return Ok(None);
        };
        for (idx, step) in node.escalation.iter().enumerate() {
            let marker = escalation_marker(node_key, idx);
            let fired = wfes
                .wfah
                .entries()
                .iter()
                .any(|e| e.action == marker && e.applied_at >= entered_at);
            if fired {
                continue;
            }
            // adımlar sıralı — ilk ateşlenmemiş adım bu turun cevabıdır
            let after = parse_iso8601_duration(&step.after)?;
            let deadline = entered_at + after;
            return Ok(Some(EscalationForecast {
                step_idx: idx,
                entered_at,
                deadline,
                overdue: now >= deadline,
            }));
        }
        Ok(None)
    }

    /// Süresi dolan ilk escalation adımının index'i (M6/§8).
    pub fn due_escalation(
        &self,
        wfd: &Wfd,
        wfes: &Wfes,
        now: DateTime<Utc>,
    ) -> Result<Option<usize>, EngineError> {
        Ok(self
            .next_escalation(wfd, wfes, now)?
            .filter(|f| f.overdue)
            .map(|f| f.step_idx))
    }

    /// Vadesi gelen escalation adımını uygular; assigned WFE'de de çalışır,
    /// taşımada assignment temizlenir (store commit'i her MoveTo'da temizler).
    /// SLA-2 (2026-07-16): `terminate: true` ise wft'e bakılmaksızın instance
    /// `terminated` olur (end_response `{"reason":"SLA.Dwell","node":...}`).
    pub async fn fire_escalation(
        &self,
        wfd: &Wfd,
        wfes: &Wfes,
        step_idx: usize,
        now: DateTime<Utc>,
    ) -> Result<TransitionCommit, EngineError> {
        let node_key = wfes
            .current_node
            .as_deref()
            .ok_or_else(|| EngineError::InvalidWfd("escalation için current_node yok".into()))?;
        let step: &EscalationStep = wfd
            .nodes
            .get(node_key)
            .and_then(|n| n.escalation.get(step_idx))
            .ok_or_else(|| {
                EngineError::InvalidWfd(format!("escalation adımı yok: {node_key}[{step_idx}]"))
            })?;

        let system = system_actor();
        let mut staged = wfes.dynctx.as_value().clone();
        if let Some(effects) = &step.wfes_effects {
            let env = EffectEnv {
                actor: &system,
                wfe_id: wfes.wfe_id,
                node: Some(node_key),
                action_input: None,
                exec_result: None,
                now,
            };
            staged = apply_effects(&staged, effects, &env)?;
        }

        let seq = wfes.wfah.entries().last().map(|e| e.seq + 1).unwrap_or(1);
        let wfah_entries = vec![WfahEntry {
            seq,
            action: escalation_marker(node_key, step_idx),
            actor: system.clone(),
            input: Some(json!({"after": step.after})),
            applied_at: now,
        }];

        if step.terminate == Some(true) {
            return Ok(TransitionCommit {
                wfe_id: wfes.wfe_id,
                orgtnt_id: wfes.orgtnt_id,
                new_dynctx: staged,
                wfah_entries,
                outcome: CommitOutcome::Terminated {
                    end_response: json!({"reason": "SLA.Dwell", "node": node_key}),
                },
                resolved_c_a: vec![],
            });
        }

        // XOR validator garanti eder: terminate yoksa wft vardır.
        let wft = step.wft.as_ref().ok_or_else(|| {
            EngineError::InvalidWfd(format!(
                "escalation adımı wft veya terminate içermeli: {node_key}[{step_idx}]"
            ))
        })?;
        let (outcome, resolved_c_a, final_ctx) = self
            .resolve_wft(wft, wfd, staged, &wfes.wfah, &system, wfes.wfe_id, None, wfes.orgtnt_id)
            .await?;

        Ok(TransitionCommit {
            wfe_id: wfes.wfe_id,
            orgtnt_id: wfes.orgtnt_id,
            new_dynctx: final_ctx,
            wfah_entries,
            outcome,
            resolved_c_a,
        })
    }

    // -------------------------------------------------------- SLA-3 deadline

    /// Instance deadline (SLA-3) aşıldı mı? — `wfe.deadline` kolonundan okunur
    /// (start'ta resolve edilmiş mutlak zaman); her tick'te ISO parse ETMEZ
    /// (eski `root_timeout_due`'nun yerini alır, 2026-07-16).
    pub fn deadline_due(&self, wfes: &Wfes, now: DateTime<Utc>) -> bool {
        if is_terminal_class(&wfes.status) {
            return false;
        }
        matches!(wfes.deadline, Some(d) if now >= d)
    }

    /// Engine-defined SLA sonlanması: WFE `terminated`'a alınır (§5'teki
    /// `Failed`/`error`'dan AYRI — SLA ihlali hata değildir).
    pub fn fire_deadline_timeout(&self, wfes: &Wfes, now: DateTime<Utc>) -> TransitionCommit {
        let seq = wfes.wfah.entries().last().map(|e| e.seq + 1).unwrap_or(1);
        let system = system_actor();
        TransitionCommit {
            wfe_id: wfes.wfe_id,
            orgtnt_id: wfes.orgtnt_id,
            new_dynctx: wfes.dynctx.as_value().clone(),
            wfah_entries: vec![WfahEntry {
                seq,
                action: "timeout:deadline".into(),
                actor: system,
                input: Some(json!({"deadline": wfes.deadline})),
                applied_at: now,
            }],
            outcome: CommitOutcome::Terminated {
                end_response: json!({"reason": "SLA.Deadline"}),
            },
            resolved_c_a: vec![],
        }
    }

    // ----------------------------------------------------- SLA-1 claim timeout

    /// Claim timeout (SLA-1) süresi doldu mu? — node'un `claim_timeout.after`'ı
    /// `wfes.claimed_at`'tan itibaren ölçülür; unassigned/terminal-class/
    /// claim_timeout tanımsız node'da her zaman false.
    pub fn claim_timeout_due(
        &self,
        wfd: &Wfd,
        wfes: &Wfes,
        now: DateTime<Utc>,
    ) -> Result<bool, EngineError> {
        if is_terminal_class(&wfes.status) {
            return Ok(false);
        }
        let Some(claimed_at) = wfes.claimed_at else {
            return Ok(false);
        };
        let Some(node_key) = wfes.current_node.as_deref() else {
            return Ok(false);
        };
        let Some(node) = wfd.nodes.get(node_key) else {
            return Ok(false);
        };
        let Some(ct) = &node.claim_timeout else {
            return Ok(false);
        };
        Ok(now >= claimed_at + parse_iso8601_duration(&ct.after)?)
    }

    /// Vadesi gelen claim timeout'u uygular. `wft` verilmişse escalation fire
    /// benzeri node/terminal taşıması (assignment zaten commit'te temizlenir);
    /// verilmemişse yalnızca claimed_by/claimed_at CAS ile temizlenir (sayaç
    /// sıfırlanır, node DEĞİŞMEZ) — `WfeStore::release_claim` ile persist edilir.
    pub async fn fire_claim_timeout(
        &self,
        wfd: &Wfd,
        wfes: &Wfes,
        now: DateTime<Utc>,
    ) -> Result<ClaimTimeoutOutcome, EngineError> {
        let node_key = wfes
            .current_node
            .as_deref()
            .ok_or_else(|| EngineError::InvalidWfd("claim timeout için current_node yok".into()))?;
        let node = wfd
            .nodes
            .get(node_key)
            .ok_or_else(|| EngineError::InvalidWfd(format!("bilinmeyen node '{node_key}'")))?;
        let ct = node.claim_timeout.as_ref().ok_or_else(|| {
            EngineError::InvalidWfd(format!("node '{node_key}' claim_timeout taşımıyor"))
        })?;
        let system = system_actor();
        let seq = wfes.wfah.entries().last().map(|e| e.seq + 1).unwrap_or(1);
        let marker = format!("claim_timeout:{node_key}");

        match &ct.wft {
            None => {
                let wfah_entry = WfahEntry {
                    seq,
                    action: marker,
                    actor: system,
                    input: Some(json!({"after": ct.after})),
                    applied_at: now,
                };
                Ok(ClaimTimeoutOutcome::Release(wfah_entry))
            }
            Some(target) => {
                // Bare string hedef — node/terminal ayrımı validator'ın garanti
                // ettiği referansa göre çözülür (wft-target-exists).
                let wft = if wfd.nodes.contains_key(target) {
                    Wft::Node { node: target.clone() }
                } else {
                    Wft::Terminal { terminal: target.clone() }
                };
                let wfah_entries = vec![WfahEntry {
                    seq,
                    action: marker,
                    actor: system.clone(),
                    input: Some(json!({"after": ct.after, "wft": target})),
                    applied_at: now,
                }];
                let staged = wfes.dynctx.as_value().clone();
                let (outcome, resolved_c_a, final_ctx) = self
                    .resolve_wft(
                        &wft,
                        wfd,
                        staged,
                        &wfes.wfah,
                        &system,
                        wfes.wfe_id,
                        None,
                        wfes.orgtnt_id,
                    )
                    .await?;
                Ok(ClaimTimeoutOutcome::Move(TransitionCommit {
                    wfe_id: wfes.wfe_id,
                    orgtnt_id: wfes.orgtnt_id,
                    new_dynctx: final_ctx,
                    wfah_entries,
                    outcome,
                    resolved_c_a,
                }))
            }
        }
    }

    // ------------------------------------------------------------- internals

    /// §7.7 — trigger zinciri: when → timeout'lu execute → retry → catch.
    /// Başarılı autoexec'in wfes_effects'i STAGED; catch effects STAGED.
    /// Unhandled + required → hata (hiçbir şey commit edilmez).
    #[allow(clippy::too_many_arguments)]
    async fn run_triggers(
        &self,
        triggers: &[TriggerInvocation],
        wfd: &Wfd,
        staged: &mut Value,
        wfah_entries: &mut Vec<WfahEntry>,
        seq: &mut u32,
        actor: &Actor,
        wfe_id: Uuid,
        node: Option<&str>,
        action_input: Option<&Value>,
        wfah: &Wfah,
        _orgtnt_id: Uuid,
    ) -> Result<(), EngineError> {
        for trig in triggers {
            // when guard — staged ctx üzerinden
            if let Some(when) = &trig.when {
                let mut env = EvalEnv::new(staged)
                    .with_wfah(wfah)
                    .with_node(node)
                    .with_actor(actor)
                    .with_wfe_id(wfe_id);
                if let Some(input) = action_input {
                    env = env.with_action_input(input);
                }
                if !evaluate_bool(when, &env)? {
                    continue;
                }
            }

            let def = wfd.autoexec.get(&trig.use_).ok_or_else(|| {
                EngineError::InvalidWfd(format!("autoexec '{}' tanımsız", trig.use_))
            })?;

            let system = Actor { role: "system".into(), ..actor.clone() };
            match self.execute_with_retry(def, trig, staged, wfe_id, node, &system).await {
                Ok(result) => {
                    if let Some(effects) = &def.wfes_effects {
                        let env = EffectEnv {
                            actor: &system,
                            wfe_id,
                            node,
                            action_input,
                            exec_result: Some(&result),
                            now: Utc::now(),
                        };
                        *staged = apply_effects(staged, effects, &env)?;
                    }
                    wfah_entries.push(WfahEntry {
                        seq: *seq,
                        action: format!("trigger:{}", trig.use_),
                        actor: system,
                        input: Some(json!({"result": result})),
                        applied_at: Utc::now(),
                    });
                    *seq += 1;
                }
                Err(failure) => {
                    // catch?
                    let caught = trig.catch.as_ref().filter(|c| {
                        c.error_equals
                            .iter()
                            .any(|e| e == "WFD.ALL" || e == &failure.error)
                    });
                    if let Some(catch) = caught {
                        let env = EffectEnv {
                            actor: &system,
                            wfe_id,
                            node,
                            action_input,
                            exec_result: None,
                            now: Utc::now(),
                        };
                        *staged = apply_effects(staged, &catch.wfes_effects, &env)?;
                        wfah_entries.push(WfahEntry {
                            seq: *seq,
                            action: format!("trigger:{}", trig.use_),
                            actor: system,
                            input: Some(json!({
                                "error": failure.error,
                                "message": failure.message,
                                "handled": true,
                            })),
                            applied_at: Utc::now(),
                        });
                        *seq += 1;
                        continue; // handled — devam (routing YOK)
                    }
                    if trig.required {
                        return Err(EngineError::Autoexec(format!(
                            "{}: {} ({})",
                            trig.use_, failure.error, failure.message
                        )));
                    }
                    // required=false → atla, kayıt düş
                    wfah_entries.push(WfahEntry {
                        seq: *seq,
                        action: format!("trigger:{}", trig.use_),
                        actor: system,
                        input: Some(json!({
                            "error": failure.error,
                            "message": failure.message,
                            "handled": false,
                            "required": false,
                        })),
                        applied_at: Utc::now(),
                    });
                    *seq += 1;
                }
            }
        }
        Ok(())
    }

    /// Timeout'lu tek çalıştırma + ASL retry döngüsü.
    /// Bekleme = interval * backoff^attempt, max_delay ile kırpılır (§7.7).
    async fn execute_with_retry(
        &self,
        def: &AutoexecDef,
        trig: &TriggerInvocation,
        staged: &Value,
        wfe_id: Uuid,
        node: Option<&str>,
        system: &Actor,
    ) -> Result<Value, ExecFailure> {
        let env = ExecEnv {
            wfe_id,
            ctx: staged.clone(),
            node: node.map(String::from),
            actor: system.clone(),
        };
        let mut attempts_per_retrier: Vec<u32> = vec![0; trig.retry.len()];

        loop {
            let run = self.exec.run(def, &env);
            let outcome =
                tokio::time::timeout(std::time::Duration::from_secs(def.timeout_seconds as u64), run)
                    .await;
            let failure = match outcome {
                Ok(Ok(result)) => return Ok(result),
                Ok(Err(f)) => f,
                Err(_) => ExecFailure::timeout(),
            };

            // eşleşen ilk retrier
            let matching = trig.retry.iter().enumerate().find(|(_, r)| {
                r.error_equals
                    .iter()
                    .any(|e| e == "WFD.ALL" || e == &failure.error)
            });
            let Some((idx, retrier)) = matching else {
                return Err(failure);
            };
            // ASL semantiği: max_attempts = yeniden deneme sayısı (ilk çağrı hariç)
            let attempt = attempts_per_retrier[idx];
            if attempt >= retrier.max_attempts {
                return Err(failure);
            }
            attempts_per_retrier[idx] = attempt + 1;

            let delay =
                retrier.interval_seconds as f64 * retrier.backoff_rate.powi(attempt as i32);
            let delay = match retrier.max_delay_seconds {
                Some(max) => delay.min(max as f64),
                None => delay,
            };
            tokio::time::sleep(std::time::Duration::from_secs_f64(delay.max(0.0))).await;
        }
    }

    /// §7.8 — WFT çözümü. Terminal'de terminal.wfes_effects uygulanır ve
    /// wfe_end_response $-string'leri FINAL staged ctx ile çözülür (M9/WOR-42).
    #[allow(clippy::too_many_arguments)]
    async fn resolve_wft(
        &self,
        wft: &Wft,
        wfd: &Wfd,
        staged: Value,
        wfah: &Wfah,
        actor: &Actor,
        wfe_id: Uuid,
        action_input: Option<&Value>,
        orgtnt_id: Uuid,
    ) -> Result<(CommitOutcome, Vec<ResolvedCandidate>, Value), EngineError> {
        let target = match wft {
            Wft::Node { node } => Target::Node(node.clone()),
            Wft::Terminal { terminal } => Target::Terminal(terminal.clone()),
            Wft::Conditional {
                conditions,
                default,
            } => {
                let mut chosen = None;
                for cond in conditions {
                    let mut env = EvalEnv::new(&staged)
                        .with_wfah(wfah)
                        .with_actor(actor)
                        .with_wfe_id(wfe_id);
                    if let Some(input) = action_input {
                        env = env.with_action_input(input);
                    }
                    if evaluate_bool(&cond.when, &env)? {
                        chosen = Some(match (&cond.node, &cond.terminal) {
                            (Some(n), None) => Target::Node(n.clone()),
                            (None, Some(t)) => Target::Terminal(t.clone()),
                            _ => {
                                return Err(EngineError::InvalidWfd(
                                    "wft condition tam olarak bir hedef içermeli".into(),
                                ))
                            }
                        });
                        break;
                    }
                }
                match (chosen, default) {
                    (Some(t), _) => t,
                    (None, Some(WftTarget::Node { node })) => Target::Node(node.clone()),
                    (None, Some(WftTarget::Terminal { terminal })) => {
                        Target::Terminal(terminal.clone())
                    }
                    (None, None) => return Err(EngineError::NoConditionMatched),
                }
            }
        };

        match target {
            Target::Node(node_key) => {
                let node = wfd.nodes.get(&node_key).ok_or_else(|| {
                    EngineError::InvalidWfd(format!("wft hedefi bilinmeyen node '{node_key}'"))
                })?;
                let mut resolved = self
                    .resolve_candidates(&node.c_a, &staged, wfah, actor, orgtnt_id)
                    .await?;
                // WOR-44: wfd.listable[] grants union into the pool cache too (VIEW-only;
                // `when` guards are ignored here — over-inclusive cache is acceptable since
                // claim/act stay matcher-gated on the real rule).
                for listable in &wfd.listable {
                    let mut extra = self
                        .resolve_candidates(&listable.c_a, &staged, wfah, actor, orgtnt_id)
                        .await?;
                    resolved.append(&mut extra);
                }
                Ok((CommitOutcome::MoveTo { node: node_key }, resolved, staged))
            }
            Target::Terminal(terminal_id) => {
                let terminal = wfd
                    .terminals
                    .iter()
                    .find(|t| t.id == terminal_id)
                    .ok_or_else(|| {
                        EngineError::InvalidWfd(format!(
                            "wft hedefi bilinmeyen terminal '{terminal_id}'"
                        ))
                    })?;
                let now = Utc::now();
                let env = EffectEnv {
                    actor,
                    wfe_id,
                    node: None,
                    action_input,
                    exec_result: None,
                    now,
                };
                let final_ctx = match &terminal.wfes_effects {
                    Some(effects) => apply_effects(&staged, effects, &env)?,
                    None => staged,
                };
                let mut end_response = Map::new();
                for (k, raw) in &terminal.wfe_end_response {
                    end_response.insert(k.clone(), resolve_value(raw, &final_ctx, &env)?);
                }
                Ok((
                    CommitOutcome::Terminal {
                        end_response: Value::Object(end_response),
                    },
                    vec![],
                    final_ctx,
                ))
            }
        }
    }

    /// Yeni node'un c_a'sını (orgu × rol, orgu × user) aday listesine çözer —
    /// pool cache'i (WOR-44). c_r girdileri role-only ResolvedCandidate, c_u
    /// girdileri user-only ResolvedCandidate üretir (role boş string).
    /// Kimlik kanalı matcher.rs (§3.3) ile birebir: c_u önce UUID string
    /// olarak parse edilir (user_id), parse başarısızsa ident olarak saklanır
    /// (user_ident) — pool sorgusu actor'ün kendi ident'ini org.user_ident ile
    /// çözüp aynı kanaldan eşler. Claim yetkisi HER ZAMAN matcher ile
    /// runtime'da yeniden doğrulanır; bu cache yalnızca liste görünürlüğü içindir.
    async fn resolve_candidates(
        &self,
        rule: &CandidateActor,
        ctx: &Value,
        wfah: &Wfah,
        actor: &Actor,
        orgtnt_id: Uuid,
    ) -> Result<Vec<ResolvedCandidate>, EngineError> {
        let units =
            resolve_c_orgu(&rule.c_orgu, actor.orgu_id, ctx, wfah, orgtnt_id, self.org).await?;
        let mut out = Vec::new();
        if let Some(roles) = &rule.c_r {
            for unit in &units {
                for role in roles {
                    out.push(ResolvedCandidate {
                        orgu_id: unit.orgu_id,
                        role: role.clone(),
                        user_id: None,
                        user_ident: None,
                    });
                }
            }
        }
        if let Some(users) = &rule.c_u {
            for unit in &units {
                for u in users {
                    let (user_id, user_ident) = match Uuid::parse_str(u) {
                        Ok(uuid) => (Some(uuid), None),
                        Err(_) => (None, Some(u.clone())),
                    };
                    out.push(ResolvedCandidate {
                        orgu_id: unit.orgu_id,
                        role: String::new(),
                        user_id,
                        user_ident,
                    });
                }
            }
        }
        Ok(out)
    }
}

enum Target {
    Node(String),
    Terminal(String),
}

fn escalation_marker(node_key: &str, idx: usize) -> String {
    format!("escalate:{node_key}:{idx}")
}

fn system_actor() -> Actor {
    Actor {
        orgu_id: Uuid::nil(),
        user_id: Uuid::nil(),
        role: "system".into(),
    }
}

fn parse_wfd_uuid(wfd: &Wfd) -> Result<Uuid, EngineError> {
    // WFD JSON id'si insan-okur slug olabilir; store katmanı UUID satır id'sini bilir.
    // Burada parse edilebiliyorsa kullanılır, yoksa nil döner ve executor doldurur.
    Ok(Uuid::parse_str(&wfd.id).unwrap_or(Uuid::nil()))
}

/// §7.5 — required path'ler mevcut, tüm leaf'ler declared, sonra declared path'ler ctx'e yazılır.
fn merge_action_input(
    ctx: &Value,
    action: &ActionDef,
    input: &Value,
) -> Result<Value, EngineError> {
    let declared: Vec<&String> = action
        .input
        .required
        .iter()
        .chain(action.input.optional.iter())
        .collect();

    for required in &action.input.required {
        if get_path(input, required).is_none() {
            return Err(EngineError::InvalidInput(format!(
                "zorunlu input '{required}' eksik"
            )));
        }
    }

    // declared olmayan leaf path reddedilir
    let mut leaves = Vec::new();
    collect_leaf_paths(input, String::new(), &mut leaves);
    for leaf in &leaves {
        let covered = declared
            .iter()
            .any(|d| leaf == *d || leaf.starts_with(&format!("{d}.")) || d.starts_with(&format!("{leaf}.")));
        if !covered {
            return Err(EngineError::InvalidInput(format!(
                "input yolu '{leaf}' bu action'da tanımlı değil"
            )));
        }
    }

    let mut staged = match ctx {
        Value::Object(m) => m.clone(),
        _ => Map::new(),
    };
    for path in declared {
        if let Some(value) = get_path(input, path) {
            set_path(&mut staged, path, value.clone());
        }
    }
    Ok(Value::Object(staged))
}

fn collect_leaf_paths(value: &Value, prefix: String, out: &mut Vec<String>) {
    match value {
        Value::Object(map) if !map.is_empty() => {
            for (k, v) in map {
                let path = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                collect_leaf_paths(v, path, out);
            }
        }
        _ => {
            if !prefix.is_empty() {
                out.push(prefix);
            }
        }
    }
}

/// Start input'unda dolu gelen, bildirimli (declared) bir yol context şemasında
/// x-wf-readonly işaretliyse reddedilir — yol üzerindeki her segment denetlenir.
/// Bildirimsiz yollar zaten `merge_action_input`'ta reddedildiği için burada yalnız
/// action.input listesindeki yollara bakmak yeterlidir.
fn validate_readonly_paths(
    action: &ActionDef,
    input: &Value,
    context_schema: &Value,
) -> Result<(), EngineError> {
    let declared = action.input.required.iter().chain(action.input.optional.iter());
    for path in declared {
        if get_path(input, path).is_none() {
            continue;
        }
        let mut schema = context_schema;
        for seg in path.split('.') {
            let Some(prop) = schema.get("properties").and_then(|p| p.get(seg)) else {
                break;
            };
            if prop
                .get("x-wf-readonly")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                return Err(EngineError::InvalidInput(format!(
                    "'{path}' x-wf-readonly — start input'unda verilemez"
                )));
            }
            schema = prop;
        }
    }
    Ok(())
}

/// context.required alanları (noktalı yol olabilir) start zinciri bittiğinde
/// final ctx'te mevcut olmalı — kaynak fark etmez (input merge veya effects).
fn validate_context_required(ctx: &Value, context_schema: &Value) -> Result<(), EngineError> {
    if let Some(required) = context_schema.get("required").and_then(Value::as_array) {
        for field in required.iter().filter_map(Value::as_str) {
            if get_path(ctx, field).is_none() {
                return Err(EngineError::InvalidInput(format!(
                    "context zorunlu alanı '{field}' start sonrasında eksik"
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! WOR-44: resolve_candidates artık c_r YANINDA c_u için de aday üretmeli
    //! (pool cache'ine önceden hiç girmeyen c_u-only node'lar için).
    use super::*;
    use crate::types::actor::OrgUnit;
    use crate::types::wfd_v22::COrgu;
    use async_trait::async_trait;

    struct MockOrg;

    #[async_trait]
    impl OrgPort for MockOrg {
        async fn resolve_c_orgu(
            &self,
            anchor: Uuid,
            _expr: &str,
            _orgtnt: Uuid,
        ) -> Result<Vec<OrgUnit>, EngineError> {
            Ok(vec![OrgUnit {
                orgu_id: anchor,
                orgu_type: json!({"type": "branch"}),
                path: "1".into(),
            }])
        }
        async fn check_user_role(&self, _: Uuid, _: Uuid, _: &str) -> Result<bool, EngineError> {
            Ok(true)
        }
        async fn orgtnt_for_orgu(&self, _: Uuid) -> Result<Uuid, EngineError> {
            Ok(Uuid::nil())
        }
    }

    struct DummyRunner;

    #[async_trait]
    impl AutoexecRunner for DummyRunner {
        async fn run(&self, _def: &AutoexecDef, _env: &ExecEnv) -> Result<Value, ExecFailure> {
            unimplemented!("resolve_candidates testlerinde autoexec çalışmaz")
        }
    }

    fn rule(c_r: Option<Vec<&str>>, c_u: Option<Vec<&str>>) -> CandidateActor {
        CandidateActor {
            c_orgu: COrgu::Selector("self".into()),
            c_r: c_r.map(|v| v.into_iter().map(String::from).collect()),
            c_u: c_u.map(|v| v.into_iter().map(String::from).collect()),
        }
    }

    #[tokio::test]
    async fn resolve_candidates_still_emits_role_candidates() {
        let org = MockOrg;
        let runner = DummyRunner;
        let engine = Engine { org: &org, exec: &runner };
        let actor = Actor { orgu_id: Uuid::new_v4(), user_id: Uuid::new_v4(), role: "clerk".into() };
        let wfah = Wfah::empty();

        let out = engine
            .resolve_candidates(&rule(Some(vec!["branchClerk"]), None), &json!({}), &wfah, &actor, Uuid::nil())
            .await
            .unwrap();

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].orgu_id, actor.orgu_id);
        assert_eq!(out[0].role, "branchClerk");
        assert_eq!(out[0].user_id, None);
        assert_eq!(out[0].user_ident, None);
    }

    #[tokio::test]
    async fn resolve_candidates_emits_user_candidate_for_uuid_c_u() {
        let org = MockOrg;
        let runner = DummyRunner;
        let engine = Engine { org: &org, exec: &runner };
        let actor = Actor { orgu_id: Uuid::new_v4(), user_id: Uuid::new_v4(), role: "clerk".into() };
        let wfah = Wfah::empty();
        let target_user = Uuid::new_v4();
        let target_user_str = target_user.to_string();

        let out = engine
            .resolve_candidates(
                &rule(None, Some(vec![target_user_str.as_str()])),
                &json!({}),
                &wfah,
                &actor,
                Uuid::nil(),
            )
            .await
            .unwrap();

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].orgu_id, actor.orgu_id);
        assert_eq!(out[0].role, "");
        assert_eq!(out[0].user_id, Some(target_user));
        assert_eq!(out[0].user_ident, None);
    }

    #[tokio::test]
    async fn resolve_candidates_emits_ident_candidate_for_non_uuid_c_u() {
        let org = MockOrg;
        let runner = DummyRunner;
        let engine = Engine { org: &org, exec: &runner };
        let actor = Actor { orgu_id: Uuid::new_v4(), user_id: Uuid::new_v4(), role: "clerk".into() };
        let wfah = Wfah::empty();

        let out = engine
            .resolve_candidates(&rule(None, Some(vec!["jdoe"])), &json!({}), &wfah, &actor, Uuid::nil())
            .await
            .unwrap();

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].role, "");
        assert_eq!(out[0].user_id, None);
        assert_eq!(out[0].user_ident.as_deref(), Some("jdoe"));
    }

    #[tokio::test]
    async fn resolve_candidates_unions_role_and_user_entries() {
        let org = MockOrg;
        let runner = DummyRunner;
        let engine = Engine { org: &org, exec: &runner };
        let actor = Actor { orgu_id: Uuid::new_v4(), user_id: Uuid::new_v4(), role: "clerk".into() };
        let wfah = Wfah::empty();

        let out = engine
            .resolve_candidates(
                &rule(Some(vec!["creditAnalyst"]), Some(vec!["jdoe"])),
                &json!({}),
                &wfah,
                &actor,
                Uuid::nil(),
            )
            .await
            .unwrap();

        assert_eq!(out.len(), 2);
        assert!(out.iter().any(|c| c.role == "creditAnalyst" && c.user_id.is_none() && c.user_ident.is_none()));
        assert!(out.iter().any(|c| c.role.is_empty() && c.user_ident.as_deref() == Some("jdoe")));
    }
}
