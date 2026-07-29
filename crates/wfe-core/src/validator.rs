//! WFD v2.2 custom validator — şemanın yakalayamadığı kurallar.
//! Spec: docs/spec/runtime-semantics.md §1, §2b, §5, §6.
//! Linear: WOR-32 (cross-ref, slug/uniqueness), WOR-33 (graf), WOR-34 (context/expression/retry).

use crate::types::wfd_v22::{ParallelSpec, Wfd, WfesEffects, Wft, WftCondition, WftTarget};
use crate::v22::duration::parse_iso8601_duration;
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, Clone)]
pub struct ValidationIssue {
    pub code: String,
    pub path: String,
    pub message: String,
}

#[derive(Debug, Default)]
pub struct ValidationReport {
    pub errors: Vec<ValidationIssue>,
    pub warnings: Vec<ValidationIssue>,
}

impl ValidationReport {
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }

    fn error(&mut self, code: &str, path: String, message: String) {
        self.errors.push(ValidationIssue {
            code: code.into(),
            path,
            message,
        });
    }

    fn warn(&mut self, code: &str, path: String, message: String) {
        self.warnings.push(ValidationIssue {
            code: code.into(),
            path,
            message,
        });
    }
}

pub fn validate(wfd: &Wfd) -> ValidationReport {
    let mut report = ValidationReport::default();
    check_uniqueness(wfd, &mut report);
    check_slugs(wfd, &mut report);
    check_cross_refs(wfd, &mut report);
    check_start_rules(wfd, &mut report);
    check_wft_conditions(wfd, &mut report);
    check_graph(wfd, &mut report);
    check_parallel(wfd, &mut report);
    check_expressions(wfd, &mut report);
    check_action_inputs(wfd, &mut report);
    check_context_required_removed(wfd, &mut report);
    check_context_field_writers(wfd, &mut report);
    check_action_input_consumed(wfd, &mut report);
    check_attachments(wfd, &mut report);
    check_effect_paths(wfd, &mut report);
    check_retries(wfd, &mut report);
    check_string_namespaces(wfd, &mut report);
    check_sla(wfd, &mut report);
    report
}

// ---- §1: uniqueness ----

fn check_uniqueness(wfd: &Wfd, report: &mut ValidationReport) {
    let mut seen = HashSet::new();
    for (i, t) in wfd.transitions.iter().enumerate() {
        if !seen.insert(t.id.clone()) {
            report.error(
                "unique",
                format!("transitions[{i}]"),
                format!("transition id '{}' birden fazla kez tanımlı", t.id),
            );
        }
    }
    let mut seen = HashSet::new();
    for (i, s) in wfd.start.iter().enumerate() {
        if !seen.insert(s.id.clone()) {
            report.error(
                "unique",
                format!("start[{i}]"),
                format!("start id '{}' birden fazla kez tanımlı", s.id),
            );
        }
    }
    let mut terminal_ids = HashSet::new();
    let mut terminal_ids_ci: HashMap<String, &str> = HashMap::new();
    for (i, t) in wfd.terminals.iter().enumerate() {
        if !terminal_ids.insert(t.id.clone()) {
            report.error(
                "unique",
                format!("terminals[{i}]"),
                format!("terminal id '{}' birden fazla kez tanımlı", t.id),
            );
        }
        // Terminal id'leri case-insensitive unique olmak zorunda (editor id = kullanıcı
        // adı; "Start" ile "sTaRT" aynı isim sayılır).
        let lower = t.id.to_lowercase();
        if let Some(prev) = terminal_ids_ci.insert(lower, &t.id) {
            if prev != t.id {
                report.error(
                    "unique",
                    format!("terminals[{i}]"),
                    format!(
                        "terminal id '{}' ile '{}' büyük/küçük harf farkı hariç aynı",
                        t.id, prev
                    ),
                );
            }
        }
    }
    // node ve terminal id'leri global namespace'te çakışamaz
    for key in wfd.nodes.keys() {
        if terminal_ids.contains(key) {
            report.error(
                "unique",
                format!("nodes[{key}]"),
                format!("'{key}' hem node key hem terminal id — global namespace çakışması"),
            );
        }
    }
}

// ---- §2b: slug + canonical c_a uniqueness ----

fn check_slugs(wfd: &Wfd, report: &mut ValidationReport) {
    for (key, node) in &wfd.nodes {
        let slug = node.c_a.slug();
        if key != &slug && !is_collision_suffixed(key, &slug) {
            report.error(
                "slug",
                format!("nodes[{key}]"),
                format!("node key '{key}' != slug(c_a) '{slug}'"),
            );
        }
    }
    let mut seen: HashMap<String, &String> = HashMap::new();
    for (key, node) in &wfd.nodes {
        let canonical = node.c_a.canonical();
        if let Some(prev) = seen.insert(canonical, key) {
            report.error(
                "duplicate_c_a",
                format!("nodes[{key}]"),
                format!("aynı canonical c_a iki node'da: '{prev}' ve '{key}'"),
            );
        }
    }
}

/// Collision durumunda editör `_<fnv1a16>` (4 hex) son eki ekler; validator kabul eder.
fn is_collision_suffixed(key: &str, slug: &str) -> bool {
    key.strip_prefix(slug)
        .and_then(|rest| rest.strip_prefix('_'))
        .map(|hex| hex.len() == 4 && hex.chars().all(|c| c.is_ascii_hexdigit()))
        .unwrap_or(false)
}

// ---- §1: cross-reference ----

fn check_cross_refs(wfd: &Wfd, report: &mut ValidationReport) {
    for (i, t) in wfd.transitions.iter().enumerate() {
        let path = format!("transitions[{}]", t.id);
        for node in t.from.iter() {
            if !wfd.nodes.contains_key(node) {
                report.error(
                    "cross_ref",
                    format!("{path}.from"),
                    format!("bilinmeyen node '{node}'"),
                );
            }
        }
        if !wfd.actions.contains_key(&t.action) {
            report.error(
                "cross_ref",
                format!("{path}.action"),
                format!("bilinmeyen action '{}'", t.action),
            );
        }
        for (j, trig) in t.trigger.iter().enumerate() {
            if !wfd.autoexec.contains_key(&trig.use_) {
                report.error(
                    "cross_ref",
                    format!("{path}.trigger[{j}]"),
                    format!("bilinmeyen autoexec '{}'", trig.use_),
                );
            }
        }
        check_wft_refs(wfd, &t.wft, &format!("{path}.wft"), report);
        let _ = i;
    }
    for s in &wfd.start {
        let path = format!("start[{}]", s.id);
        for (j, trig) in s.trigger.iter().enumerate() {
            if !wfd.autoexec.contains_key(&trig.use_) {
                report.error(
                    "cross_ref",
                    format!("{path}.trigger[{j}]"),
                    format!("bilinmeyen autoexec '{}'", trig.use_),
                );
            }
        }
        check_wft_refs(wfd, &s.wft, &format!("{path}.wft"), report);
    }
    for (key, node) in &wfd.nodes {
        for (j, esc) in node.escalation.iter().enumerate() {
            if let Some(wft) = &esc.wft {
                check_wft_refs(
                    wfd,
                    wft,
                    &format!("nodes[{key}].escalation[{j}].wft"),
                    report,
                );
            }
        }
    }
}

fn check_wft_refs(wfd: &Wfd, wft: &Wft, path: &str, report: &mut ValidationReport) {
    for (kind, target) in wft_targets(wft) {
        let known = match kind {
            TargetKind::Node => wfd.nodes.contains_key(target),
            TargetKind::Terminal => wfd.terminals.iter().any(|t| t.id == target),
        };
        if !known {
            let noun = match kind {
                TargetKind::Node => "node",
                TargetKind::Terminal => "terminal",
            };
            report.error(
                "cross_ref",
                path.to_string(),
                format!("bilinmeyen {noun} '{target}'"),
            );
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum TargetKind {
    Node,
    Terminal,
}

fn wft_targets(wft: &Wft) -> Vec<(TargetKind, &str)> {
    let mut out = Vec::new();
    match wft {
        Wft::Node { node } => out.push((TargetKind::Node, node.as_str())),
        Wft::Terminal { terminal } => out.push((TargetKind::Terminal, terminal.as_str())),
        Wft::Conditional {
            conditions,
            default,
        } => {
            for c in conditions {
                if let Some(n) = &c.node {
                    out.push((TargetKind::Node, n.as_str()));
                }
                if let Some(t) = &c.terminal {
                    out.push((TargetKind::Terminal, t.as_str()));
                }
            }
            match default {
                Some(WftTarget::Node { node }) => out.push((TargetKind::Node, node.as_str())),
                Some(WftTarget::Terminal { terminal }) => {
                    out.push((TargetKind::Terminal, terminal.as_str()))
                }
                None => {}
            }
        }
        // WOR-31: fork/join — her branch başlangıç node'u VE join hedefi birer
        // çıkış kenarıdır (cross_ref + graf BFS bunları otomatik kapsar).
        Wft::Parallel { parallel } => {
            for b in &parallel.branches {
                out.push((TargetKind::Node, b.as_str()));
            }
            match &parallel.join {
                WftTarget::Node { node } => out.push((TargetKind::Node, node.as_str())),
                WftTarget::Terminal { terminal } => {
                    out.push((TargetKind::Terminal, terminal.as_str()))
                }
            }
        }
        // WOR-56: collapse hedefi bir çıkış kenarıdır (cross_ref + graf
        // reachability kapsasın); ama branch subgraph BFS'i bu kenarı İZLEMEZ
        // (aşağıda check_parallel'de atlanır — kapsam dışına çıkar).
        Wft::Collapse { collapse } => match collapse {
            WftTarget::Node { node } => out.push((TargetKind::Node, node.as_str())),
            WftTarget::Terminal { terminal } => out.push((TargetKind::Terminal, terminal.as_str())),
        },
    }
    out
}

// ---- V1, V4, V5: start kuralları (spec runtime-semantics, M16). V2/V3 kaldırıldı
// (2026-07-16): start node yeniden girilebilir; mid-flow'da normal node gibi
// davranır, wft hedefi ve escalation geçerlidir. ----

fn check_start_rules(wfd: &Wfd, report: &mut ValidationReport) {
    // V5: en az 1 start
    if wfd.start.is_empty() {
        report.error(
            "start_required",
            "start".into(),
            "en az bir start kuralı gerekli".into(),
        );
    }
    for s in &wfd.start {
        let path = format!("start[{}]", s.id);
        // V4 (M16): start.action gerçek bir action adıdır — actions{} içinde tanımlı
        // olmalı (transition'lardaki action ile aynı kural).
        if !wfd.actions.contains_key(&s.action) {
            report.error(
                "start_action",
                format!("{path}.action"),
                format!("bilinmeyen action '{}'", s.action),
            );
        }
        // V1: from var olan bir node'a işaret etmeli
        if wfd.nodes.get(&s.from).is_none() {
            report.error(
                "cross_ref",
                format!("{path}.from"),
                format!("start.from bilinmeyen node '{}'", s.from),
            );
        }
    }
}

// ---- M3: wft.conditions hedef tekilliği ----

fn check_wft_conditions(wfd: &Wfd, report: &mut ValidationReport) {
    let visit = |wft: &Wft, path: String, report: &mut ValidationReport| {
        if let Wft::Conditional { conditions, .. } = wft {
            for (i, c) in conditions.iter().enumerate() {
                check_condition_target(c, &format!("{path}.conditions[{i}]"), report);
            }
        }
    };
    for t in &wfd.transitions {
        visit(&t.wft, format!("transitions[{}].wft", t.id), report);
    }
    for s in &wfd.start {
        visit(&s.wft, format!("start[{}].wft", s.id), report);
    }
    for (key, node) in &wfd.nodes {
        for (j, esc) in node.escalation.iter().enumerate() {
            if let Some(wft) = &esc.wft {
                visit(wft, format!("nodes[{key}].escalation[{j}].wft"), report);
            }
        }
    }
}

fn check_condition_target(c: &WftCondition, path: &str, report: &mut ValidationReport) {
    match (&c.node, &c.terminal) {
        (Some(_), Some(_)) => report.error(
            "wft_target",
            path.to_string(),
            "condition hem node hem terminal hedefliyor — tam olarak biri olmalı".into(),
        ),
        (None, None) => report.error(
            "wft_target",
            path.to_string(),
            "condition hedefsiz — node veya terminal zorunlu".into(),
        ),
        _ => {}
    }
}

// ---- §5: graf — BFS reachability (escalation DAHİL) + çıkışsız node + ilk-match belirsizliği ----

fn check_graph(wfd: &Wfd, report: &mut ValidationReport) {
    // BFS
    let mut reached_nodes: HashSet<String> = HashSet::new();
    let mut reached_terminals: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();

    fn absorb(
        targets: Vec<(TargetKind, &str)>,
        reached_nodes: &mut HashSet<String>,
        reached_terminals: &mut HashSet<String>,
        queue: &mut VecDeque<String>,
    ) {
        for (kind, target) in targets {
            match kind {
                TargetKind::Node => {
                    if reached_nodes.insert(target.to_string()) {
                        queue.push_back(target.to_string());
                    }
                }
                TargetKind::Terminal => {
                    reached_terminals.insert(target.to_string());
                }
            }
        }
    }

    for s in &wfd.start {
        // Simetrik start: `from` node bir KAYNAKtır — hiçbir wft hedefi olmasa da
        // (V2 zaten yasaklar) erişilebilir sayılır, dead-node uyarısı vermemeli.
        reached_nodes.insert(s.from.clone());
        absorb(
            wft_targets(&s.wft),
            &mut reached_nodes,
            &mut reached_terminals,
            &mut queue,
        );
    }

    while let Some(node_key) = queue.pop_front() {
        for t in &wfd.transitions {
            if t.from.contains(&node_key) {
                absorb(
                    wft_targets(&t.wft),
                    &mut reached_nodes,
                    &mut reached_terminals,
                    &mut queue,
                );
            }
        }
        if let Some(node) = wfd.nodes.get(&node_key) {
            for esc in &node.escalation {
                if let Some(wft) = &esc.wft {
                    absorb(
                        wft_targets(wft),
                        &mut reached_nodes,
                        &mut reached_terminals,
                        &mut queue,
                    );
                }
            }
            // SLA-1: claim_timeout.wft de bir çıkıştır (node/terminal hedefi
            // BFS'e dahil edilmezse hedef yanlışlıkla "unreachable" görünür).
            if let Some(ct) = &node.claim_timeout {
                if let Some(target) = &ct.wft {
                    let kind = if wfd.nodes.contains_key(target) {
                        TargetKind::Node
                    } else {
                        TargetKind::Terminal
                    };
                    absorb(
                        vec![(kind, target.as_str())],
                        &mut reached_nodes,
                        &mut reached_terminals,
                        &mut queue,
                    );
                }
            }
        }
    }

    for key in wfd.nodes.keys() {
        if !reached_nodes.contains(key.as_str()) {
            report.error(
                "unreachable",
                format!("nodes[{key}]"),
                format!("WFD.Unreachable: '{key}' start'tan erişilemiyor"),
            );
        }
    }
    for t in &wfd.terminals {
        if !reached_terminals.contains(t.id.as_str()) {
            report.error(
                "unreachable",
                format!("terminals[{}]", t.id),
                format!(
                    "WFD.Unreachable: terminal '{}' hiçbir wft'den referans almıyor",
                    t.id
                ),
            );
        }
    }

    // çıkışsız node: ne transition kaynağı ne escalation'ı var
    // (start node'unun çıkışı start kuralının wft'sidir — no_exit muaf)
    let start_from: HashSet<&str> = wfd.start.iter().map(|s| s.from.as_str()).collect();
    for (key, node) in &wfd.nodes {
        if start_from.contains(key.as_str()) {
            continue;
        }
        let has_transition = wfd.transitions.iter().any(|t| t.from.contains(key));
        if !has_transition && node.escalation.is_empty() {
            report.error(
                "no_exit",
                format!("nodes[{key}]"),
                format!("'{key}' çıkışsız — transition veya escalation gerekli"),
            );
        }
    }

    // aynı (node, action) için çoklu transition
    let mut groups: HashMap<(&str, &str), Vec<&crate::types::wfd_v22::Transition>> = HashMap::new();
    for t in &wfd.transitions {
        for node in t.from.iter() {
            groups.entry((node, t.action.as_str())).or_default().push(t);
        }
    }
    for ((node, action), group) in groups {
        if group.len() < 2 {
            continue;
        }
        let without_when = group.iter().filter(|t| t.when.is_none()).count();
        let ids: Vec<&str> = group.iter().map(|t| t.id.as_str()).collect();
        if without_when >= 2 {
            report.error(
                "ambiguous_transition",
                format!("transitions[{}]", ids.join(",")),
                format!("({node}, {action}) için birden fazla when'siz transition — belirsiz"),
            );
        } else {
            report.warn(
                "ambiguous_transition",
                format!("transitions[{}]", ids.join(",")),
                format!("({node}, {action}) için çoklu transition — runtime ilk-match uygular"),
            );
        }
    }
}

// ---- WOR-31: Parallel fork/join — branch/join şekli + subgraph kısıtları ----
// Restrictions v1 (design doc §Validation): start wft'de Parallel yasak;
// branches len>=2 + distinct + var olan node; join var olan node/terminal ve
// branches'ten biri olamaz; branch subgraph'ları (fork'tan join'e/terminale
// kadar transition wft kenarları) birbirinden ayrık; subgraph içinde iç içe
// (nested) Parallel yasak; her subgraph join'e veya bir terminal'e ulaşmalı.

fn check_parallel(wfd: &Wfd, report: &mut ValidationReport) {
    // Parallel wft start kuralında kullanılamaz.
    for s in &wfd.start {
        if matches!(&s.wft, Wft::Parallel { .. }) {
            report.error(
                "parallel_start",
                format!("start[{}].wft", s.id),
                "Parallel wft start kuralında kullanılamaz".into(),
            );
        }
        // WOR-56: collapse yalnız paralel dal içinde anlamlıdır — start'ta yasak.
        if matches!(&s.wft, Wft::Collapse { .. }) {
            report.error(
                "collapse_start",
                format!("start[{}].wft", s.id),
                "Collapse wft start kuralında kullanılamaz (WOR-56)".into(),
            );
        }
    }

    // Fork noktalarını topla (yalnızca transitions.wft — start zaten yasak;
    // nested fork da ayrıca aşağıda yasaklanıyor).
    struct Fork<'a> {
        path: String,
        spec: &'a ParallelSpec,
    }
    let mut forks: Vec<Fork> = Vec::new();
    for t in &wfd.transitions {
        if let Wft::Parallel { parallel } = &t.wft {
            forks.push(Fork {
                path: format!("transitions[{}].wft", t.id),
                spec: parallel,
            });
        }
    }

    for fork in &forks {
        let path = &fork.path;
        let spec = fork.spec;

        if spec.branches.len() < 2 {
            report.error(
                "parallel_branches",
                format!("{path}.parallel.branches"),
                "parallel.branches en az 2 kol içermeli".into(),
            );
        }
        let mut seen_names = HashSet::new();
        for b in &spec.branches {
            if !seen_names.insert(b.as_str()) {
                report.error(
                    "parallel_branches",
                    format!("{path}.parallel.branches"),
                    format!("branch '{b}' tekrarlanıyor — kollar distinct olmalı"),
                );
            }
        }
        // branch/join'in var olan node/terminal'e işaret etmesi generic
        // cross_ref (check_cross_refs → wft_targets) tarafından zaten kontrol
        // edilir; burada sadece Parallel'e özgü kısıt: join, kollardan biri
        // olamaz.
        if let WftTarget::Node { node: join_node } = &spec.join {
            if spec.branches.iter().any(|b| b == join_node) {
                report.error(
                    "parallel_join",
                    format!("{path}.parallel.join"),
                    format!(
                        "join node '{join_node}' branches listesinde de var — join kollardan biri olamaz"
                    ),
                );
            }
        }
    }

    // Branch subgraph'ları: fork'tan join'e (veya bir terminal'e) kadar,
    // transition wft node kenarları takip edilerek BFS. Join node'a
    // ulaşılınca DURULUR (ötesine geçilmez).
    for fork in &forks {
        let spec = fork.spec;
        let join_node: Option<&str> = match &spec.join {
            WftTarget::Node { node } => Some(node.as_str()),
            WftTarget::Terminal { .. } => None,
        };

        // node -> hangi branch index'inde ilk görüldü (fork içi ayrıklık için)
        let mut owner: HashMap<&str, usize> = HashMap::new();

        for (bi, start) in spec.branches.iter().enumerate() {
            let mut visited: HashSet<&str> = HashSet::new();
            let mut queue: VecDeque<&str> = VecDeque::new();
            visited.insert(start.as_str());
            queue.push_back(start.as_str());
            let mut reaches_exit = join_node == Some(start.as_str());

            while let Some(node_key) = queue.pop_front() {
                if let Some(prev_bi) = owner.get(node_key) {
                    if *prev_bi != bi {
                        report.error(
                            "parallel_disjoint",
                            format!("{}.parallel", fork.path),
                            format!(
                                "node '{node_key}' birden fazla branch subgraph'ında (branch[{prev_bi}] ve branch[{bi}]) — kollar ayrık olmalı"
                            ),
                        );
                    }
                } else {
                    owner.insert(node_key, bi);
                }

                if Some(node_key) == join_node {
                    // join'e ulaşıldı — ötesine geçme.
                    continue;
                }

                for t in &wfd.transitions {
                    if !t.from.contains(node_key) {
                        continue;
                    }
                    if matches!(&t.wft, Wft::Parallel { .. }) {
                        report.error(
                            "parallel_nested",
                            format!("transitions[{}].wft", t.id),
                            "branch subgraph içinde iç içe (nested) Parallel yasak".into(),
                        );
                        continue;
                    }
                    // WOR-56: collapse kenarı subgraph dışına çıkar (kardeşleri düşürüp
                    // WFE'yi rastgele hedefe götürür) → BFS izlemez, disjoint/dead-end
                    // kurallarından muaf. Kol subgraph'ı normal (join/terminal) kenarlarla
                    // çıkışa ulaşmalıdır; collapse tek başına reaches_exit üretmez.
                    if matches!(&t.wft, Wft::Collapse { .. }) {
                        continue;
                    }
                    for (kind, target) in wft_targets(&t.wft) {
                        match kind {
                            TargetKind::Terminal => reaches_exit = true,
                            TargetKind::Node => {
                                if Some(target) == join_node {
                                    reaches_exit = true;
                                    // join node'u da ayrıklık defterine düş
                                    // (üstte tekrar işlenecek ve durulacak).
                                    if !owner.contains_key(target) {
                                        owner.insert(target, bi);
                                    }
                                } else if visited.insert(target) {
                                    queue.push_back(target);
                                }
                            }
                        }
                    }
                }
            }

            if !reaches_exit {
                report.error(
                    "parallel_dead_end",
                    format!("{}.parallel.branches[{}]", fork.path, bi),
                    format!("branch '{start}' join node'a veya bir terminal'e ulaşamıyor"),
                );
            }
        }
    }
}

// ---- §6: ZEN parse ----

fn check_expressions(wfd: &Wfd, report: &mut ValidationReport) {
    let check = |expr: &str, path: String, report: &mut ValidationReport| {
        if let Err(e) = zen_expression::validate::validate_expression(expr) {
            report.error(
                "zen_parse",
                path,
                format!("ZEN ifadesi parse edilemedi: {e}"),
            );
        }
    };

    let visit_wft = |wft: &Wft, path: &str, report: &mut ValidationReport| {
        if let Wft::Conditional { conditions, .. } = wft {
            for (i, c) in conditions.iter().enumerate() {
                check(&c.when, format!("{path}.conditions[{i}].when"), report);
            }
        }
    };

    for t in &wfd.transitions {
        let path = format!("transitions[{}]", t.id);
        if let Some(when) = &t.when {
            check(when, format!("{path}.when"), report);
        }
        for (j, trig) in t.trigger.iter().enumerate() {
            if let Some(when) = &trig.when {
                check(when, format!("{path}.trigger[{j}].when"), report);
            }
        }
        visit_wft(&t.wft, &format!("{path}.wft"), report);
    }
    for s in &wfd.start {
        let path = format!("start[{}]", s.id);
        for (j, trig) in s.trigger.iter().enumerate() {
            if let Some(when) = &trig.when {
                check(when, format!("{path}.trigger[{j}].when"), report);
            }
        }
        visit_wft(&s.wft, &format!("{path}.wft"), report);
    }
    for (key, node) in &wfd.nodes {
        for (j, esc) in node.escalation.iter().enumerate() {
            if let Some(wft) = &esc.wft {
                visit_wft(wft, &format!("nodes[{key}].escalation[{j}].wft"), report);
            }
        }
    }
    for (i, l) in wfd.listable.iter().enumerate() {
        if let Some(when) = &l.when {
            check(when, format!("listable[{i}].when"), report);
        }
    }
    if let Some(tw) = &wfd.terminal_when {
        check(tw, "terminal_when".into(), report);
    }
}

// ---- §6: action input yolları ----

fn check_action_inputs(wfd: &Wfd, report: &mut ValidationReport) {
    for (name, action) in &wfd.actions {
        for path in action.input.required.iter().chain(&action.input.optional) {
            match resolve_schema_path(&wfd.context, path) {
                PathResolution::Missing => report.error(
                    "input_path",
                    format!("actions[{name}].input"),
                    format!("input yolu '{path}' context şemasında yok"),
                ),
                PathResolution::Readonly => report.error(
                    "readonly_input",
                    format!("actions[{name}].input"),
                    format!("input yolu '{path}' x-wf-readonly — kullanıcı yazamaz"),
                ),
                PathResolution::Found | PathResolution::Opaque => {}
            }
        }
    }
}

// ---- WOR-70: context yazma sözleşmesi ----
//
// Kural seti üç parçadır ve birlikte "context.required"ın yerini alır:
//   1. `context.required` / `properties.*.required` YASAK  (context_required_removed)
//   2. Her context alanı en az bir `wfes_effects.set` tarafından yazılmalı
//      (context_field_never_written) — hiç dolmayacak alan tutulamaz.
//   3. Bir aksiyonun bildirdiği her input, o aksiyonu kullanan kuralın effects'inde
//      `$action.input.<yol>` ile tüketilmeli (unused_action_input) — istekten alınan
//      değer sessizce düşmesin.
// Çalışma anında ctx doluluk denetimi YOKTUR; her şey tasarım zamanında yakalanır.

/// Kural 1 — kaldırılan `required` bildirimleri hard reject.
fn check_context_required_removed(wfd: &Wfd, report: &mut ValidationReport) {
    if wfd.context.get("required").is_some() {
        report.error(
            "context_required_removed",
            "context.required".into(),
            "`context.required` kaldırıldı (WOR-70) — zorunluluk artık aksiyonun \
             `input.required` listesinde bildirilir. Bu listeyi context şemasından silin."
                .into(),
        );
    }
    check_nested_required(&wfd.context, "context", report);
}

fn check_nested_required(schema: &Value, path: &str, report: &mut ValidationReport) {
    let Some(props) = schema.get("properties").and_then(Value::as_object) else {
        return;
    };
    for (name, node) in props {
        let node_path = format!("{path}.properties.{name}");
        if node.get("required").is_some() {
            report.error(
                "context_required_removed",
                format!("{node_path}.required"),
                format!(
                    "'{name}' içindeki `required` listesi kaldırıldı (WOR-70) — motor bunu hiç \
                     okumuyordu. Zorunluluk aksiyonun `input.required` listesinde bildirilir."
                ),
            );
        }
        check_nested_required(node, &node_path, report);
    }
}

/// Bir yolun diğerini kapsayıp kapsamadığı: eşit, ata veya torun.
/// (`credit_info` ↔ `credit_info.amount_requested` her iki yönde de kapsar.)
fn paths_overlap(a: &str, b: &str) -> bool {
    a == b || a.starts_with(&format!("{b}.")) || b.starts_with(&format!("{a}."))
}

/// WFD'deki TÜM `wfes_effects.set` hedef yollarını toplar.
fn collect_effect_targets(wfd: &Wfd) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |effects: &WfesEffects| out.extend(effects.set.keys().cloned());

    for s in &wfd.start {
        if let Some(e) = &s.wfes_effects {
            push(e);
        }
        for trig in &s.trigger {
            if let Some(c) = &trig.catch {
                push(&c.wfes_effects);
            }
        }
    }
    for t in &wfd.transitions {
        if let Some(e) = &t.wfes_effects {
            push(e);
        }
        for trig in &t.trigger {
            if let Some(c) = &trig.catch {
                push(&c.wfes_effects);
            }
        }
    }
    for node in wfd.nodes.values() {
        for esc in &node.escalation {
            if let Some(e) = &esc.wfes_effects {
                push(e);
            }
        }
        if let Some(ct) = &node.claim_timeout {
            if let Some(e) = &ct.wfes_effects {
                push(e);
            }
        }
    }
    for t in &wfd.terminals {
        if let Some(e) = &t.wfes_effects {
            push(e);
        }
    }
    for ax in wfd.autoexec.values() {
        if let Some(e) = &ax.wfes_effects {
            push(e);
        }
    }
    out
}

/// Context şemasının yazılabilir yaprak yolları. Yaprak = altında `properties` olmayan
/// düğüm (`$ref` opaktır, yaprak sayılır).
fn collect_context_leaves(schema: &Value, prefix: &str, out: &mut Vec<String>) {
    let props = schema
        .get("properties")
        .and_then(Value::as_object)
        .filter(|p| !p.is_empty());
    let Some(props) = props else {
        if !prefix.is_empty() {
            out.push(prefix.to_string());
        }
        return;
    };
    if schema.get("$ref").is_some() && !prefix.is_empty() {
        out.push(prefix.to_string());
        return;
    }
    for (name, node) in props {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };
        collect_context_leaves(node, &path, out);
    }
}

/// Kural 2 — hiçbir effect tarafından yazılmayan context alanı reddedilir.
fn check_context_field_writers(wfd: &Wfd, report: &mut ValidationReport) {
    let targets = collect_effect_targets(wfd);
    let mut leaves = Vec::new();
    collect_context_leaves(&wfd.context, "", &mut leaves);

    for leaf in leaves {
        if targets.iter().any(|t| paths_overlap(t, &leaf)) {
            continue;
        }
        report.error(
            "context_field_never_written",
            format!("context.properties.{}", leaf.replace('.', ".properties.")),
            format!(
                "context alanı '{leaf}' hiçbir `wfes_effects` tarafından yazılmıyor — bu alan hiç \
                 dolmayacak. Ya bu alanı yazan bir aksiyona \"{leaf}\": \"$action.input.{leaf}\" \
                 effect'i ekleyin, ya da alanı context şemasından silin."
            ),
        );
    }
}

/// Bir effect bloğundaki `$action.input.<yol>` referanslarını toplar (nested değerler dahil).
fn collect_input_refs(effects: &WfesEffects, out: &mut Vec<String>) {
    for raw in effects.set.values() {
        walk_strings(raw, "", &mut |s, _| {
            if let Some(path) = s.strip_prefix("$action.input.") {
                out.push(path.to_string());
            }
        });
    }
}

/// Kural 3 — kuralın aksiyonunun bildirdiği her input, o kuralın effects'inde tüketilmeli.
fn check_action_input_consumed(wfd: &Wfd, report: &mut ValidationReport) {
    // (kural yolu, aksiyon adı, kuralın kendi effects'i, tetiklediği trigger'lar)
    let mut rules: Vec<(String, &String, Vec<String>)> = Vec::new();

    let refs_for = |own: Option<&WfesEffects>, triggers: &[crate::types::wfd_v22::TriggerInvocation]| {
        let mut refs = Vec::new();
        if let Some(e) = own {
            collect_input_refs(e, &mut refs);
        }
        for trig in triggers {
            if let Some(c) = &trig.catch {
                collect_input_refs(&c.wfes_effects, &mut refs);
            }
            // Tetiklenen autoexec'in kendi effects'i de aksiyon girdisini görebilir.
            if let Some(ax) = wfd.autoexec.get(&trig.use_) {
                if let Some(e) = &ax.wfes_effects {
                    collect_input_refs(e, &mut refs);
                }
            }
        }
        refs
    };

    for s in &wfd.start {
        rules.push((
            format!("start[{}]", s.id),
            &s.action,
            refs_for(s.wfes_effects.as_ref(), &s.trigger),
        ));
    }
    for t in &wfd.transitions {
        rules.push((
            format!("transitions[{}]", t.id),
            &t.action,
            refs_for(t.wfes_effects.as_ref(), &t.trigger),
        ));
    }

    for (path, action_name, refs) in rules {
        let Some(action) = wfd.actions.get(action_name) else {
            continue; // tanımsız aksiyon check_cross_refs'in işi
        };
        for declared in action.input.required.iter().chain(&action.input.optional) {
            if refs.iter().any(|r| paths_overlap(r, declared)) {
                continue;
            }
            report.error(
                "unused_action_input",
                path.clone(),
                format!(
                    "'{action_name}' aksiyonu '{declared}' girdisini istiyor ama bu kuralın \
                     `wfes_effects` bloğu onu hiçbir yere yazmıyor — istekten gelen değer \
                     kayboluyor. Şunu ekleyin: \"{declared}\": \"$action.input.{declared}\"."
                ),
            );
        }
    }
}

// ---- §6b: attachments katalogu + node referansları ----

fn check_attachments(wfd: &Wfd, report: &mut ValidationReport) {
    // Katalog içi: item.id grup içinde tekil olmalı.
    for (group, def) in &wfd.attachments {
        let mut seen_ids = HashSet::new();
        for item in &def.items {
            if !seen_ids.insert(item.id.clone()) {
                report.error(
                    "attachment_item_dup",
                    format!("attachments[{group}].items"),
                    format!("attachment item id '{}' grup içinde birden fazla tanımlı", item.id),
                );
            }
        }
    }
    // Node referansları: katalogda var olmalı; aynı grup bir node'da tekrar edilmemeli.
    for (node_key, node) in &wfd.nodes {
        let mut seen_refs = HashSet::new();
        for group_ref in &node.attachments {
            if !seen_refs.insert(group_ref.clone()) {
                report.error(
                    "attachment_ref_dup",
                    format!("nodes[{node_key}].attachments"),
                    format!("attachment grubu '{group_ref}' bu node'da birden fazla referanslı"),
                );
            }
            if !wfd.attachments.contains_key(group_ref) {
                report.error(
                    "attachment_ref",
                    format!("nodes[{node_key}].attachments"),
                    format!("attachment grubu '{group_ref}' root attachments katalogunda yok"),
                );
            }
        }
    }
}

// ---- §6: wfes_effects.set yolları (catch ve escalation dahil) ----

fn check_effect_paths(wfd: &Wfd, report: &mut ValidationReport) {
    let check_effects = |effects: &Option<crate::types::wfd_v22::WfesEffects>,
                         path: &str,
                         report: &mut ValidationReport| {
        let Some(effects) = effects else { return };
        for key in effects.set.keys() {
            if let PathResolution::Missing = resolve_schema_path(&wfd.context, key) {
                report.error(
                    "effect_path",
                    path.to_string(),
                    format!("effect yolu '{key}' context şemasında yok"),
                );
            }
        }
    };

    for s in &wfd.start {
        check_effects(&s.wfes_effects, &format!("start[{}]", s.id), report);
        for (j, trig) in s.trigger.iter().enumerate() {
            if let Some(c) = &trig.catch {
                check_effects(
                    &Some(c.wfes_effects.clone()),
                    &format!("start[{}].trigger[{j}].catch", s.id),
                    report,
                );
            }
        }
    }
    for t in &wfd.transitions {
        check_effects(&t.wfes_effects, &format!("transitions[{}]", t.id), report);
        for (j, trig) in t.trigger.iter().enumerate() {
            if let Some(c) = &trig.catch {
                check_effects(
                    &Some(c.wfes_effects.clone()),
                    &format!("transitions[{}].trigger[{j}].catch", t.id),
                    report,
                );
            }
        }
    }
    for (key, node) in &wfd.nodes {
        for (j, esc) in node.escalation.iter().enumerate() {
            check_effects(
                &esc.wfes_effects,
                &format!("nodes[{key}].escalation[{j}]"),
                report,
            );
        }
    }
    for t in &wfd.terminals {
        check_effects(&t.wfes_effects, &format!("terminals[{}]", t.id), report);
    }
    for (name, ax) in &wfd.autoexec {
        check_effects(&ax.wfes_effects, &format!("autoexec[{name}]"), report);
    }
}

// ---- §6: retry — WFD.ALL tek başına ve yalnızca son retrier'da ----

fn check_retries(wfd: &Wfd, report: &mut ValidationReport) {
    let check_triggers = |triggers: &[crate::types::wfd_v22::TriggerInvocation],
                          path: &str,
                          report: &mut ValidationReport| {
        for (j, trig) in triggers.iter().enumerate() {
            let last = trig.retry.len().saturating_sub(1);
            for (k, r) in trig.retry.iter().enumerate() {
                if r.error_equals.iter().any(|e| e == "WFD.ALL") {
                    if r.error_equals.len() > 1 {
                        report.error(
                            "retry_wfd_all",
                            format!("{path}.trigger[{j}].retry[{k}]"),
                            "WFD.ALL yalnızca tek başına kullanılabilir".into(),
                        );
                    }
                    if k != last {
                        report.error(
                            "retry_wfd_all",
                            format!("{path}.trigger[{j}].retry[{k}]"),
                            "WFD.ALL yalnızca son retrier'da kullanılabilir".into(),
                        );
                    }
                }
            }
        }
    };

    for s in &wfd.start {
        check_triggers(&s.trigger, &format!("start[{}]", s.id), report);
    }
    for t in &wfd.transitions {
        check_triggers(&t.trigger, &format!("transitions[{}]", t.id), report);
    }
}

// ---- 2026-07-16 SLA sözleşmesi: escalation + claim_timeout ----
// ---- 2026-07-28: SLA-1/SLA-2 YALNIZ bir node'a devreder. Akışı bitiremez
//      (`terminate` kaldırıldı, terminal hedef yasak — bitirme yalnız SLA-3'ün işi) ve
//      dallanma/fork/collapse kararı veremez (`wft` yalnız `{node}` formu).
//      + SLA effects namespace kısıtı ----

/// `Wft`'in wire formunun kullanıcıya gösterilecek adı — SLA hedef formu hatasında
/// hangi biçimin kullanıldığını söylemek için.
fn wft_form_name(wft: &Wft) -> &'static str {
    match wft {
        Wft::Node { .. } => "node",
        Wft::Terminal { .. } => "terminal",
        Wft::Conditional { .. } => "conditions (koşullu dallanma)",
        Wft::Parallel { .. } => "parallel (fork/join)",
        Wft::Collapse { .. } => "collapse (kolları düşür)",
    }
}

/// SLA bağlamında `$action.input.*` ve `$exec.result.*` YOKTUR (tetikleyici system
/// aktörü; ne aksiyon girdisi ne autoexec sonucu vardır) — sessizce `null` yazmak
/// yerine WFD reddedilir. `$ctx.*`, `$actor`, `$node`, `$timestamp`, `$wfe_id` geçerli.
fn check_sla_effect_namespaces(effects: &WfesEffects, path: &str, report: &mut ValidationReport) {
    for (target, raw) in &effects.set {
        walk_strings(raw, &format!("{path}.set[{target}]"), &mut |s, p| {
            for bad in ["$action.input.", "$exec.result."] {
                if s.contains(bad) {
                    report.error(
                        "sla_effect_namespace",
                        p.to_string(),
                        format!(
                            "SLA effects'inde '{bad}*' kullanılamaz (system tetikler — aksiyon girdisi/autoexec sonucu yok): '{s}'"
                        ),
                    );
                }
            }
        });
    }
}

fn check_sla(wfd: &Wfd, report: &mut ValidationReport) {
    for (key, node) in &wfd.nodes {
        for (j, esc) in node.escalation.iter().enumerate() {
            let path = format!("nodes[{key}].escalation[{j}]");
            // 2026-07-28: SLA-2 akışı BİTİREMEZ — `terminate` kaldırıldı, `wft` zorunlu.
            if esc.terminate.is_some() {
                report.error(
                    "escalation_terminate_removed",
                    format!("{path}.terminate"),
                    "`terminate` kaldırıldı — SLA-2 akışı bitiremez; yalnız root `timeout` (SLA-3) bitirir. Adımı bir node hedefine (`wft`) çevirin ya da adımı kaldırın".into(),
                );
            }
            if esc.wft.is_none() {
                report.error(
                    "escalation_wft_required",
                    path.clone(),
                    "escalation adımı bir node hedefi (`wft`) içermelidir".into(),
                );
            }
            // SLA-2 hedefi YALNIZ `{node}` olabilir: terminal (akışı bitirir),
            // conditions (dallanma kararı), parallel (fork) ve collapse (kolları
            // düşürür) formlarının hepsi bir AKSİYONUN verebileceği kararlardır —
            // bir zamanlayıcının değil. SLA sadece "sıradaki havuza devret" yapar.
            match &esc.wft {
                None | Some(Wft::Node { .. }) => {}
                Some(Wft::Terminal { terminal }) => report.error(
                    "sla_terminal_target",
                    format!("{path}.wft"),
                    format!(
                        "SLA-2 escalation hedefi terminal olamaz ('{terminal}') — SLA yalnız node'lar arası devirdir; akışı zaman aşımıyla bitiren tek kural root `timeout` (SLA-3)"
                    ),
                ),
                Some(other) => report.error(
                    "sla_target_not_node",
                    format!("{path}.wft"),
                    format!(
                        "SLA-2 escalation hedefi yalnız `{{node}}` olabilir — '{}' formu kullanılamaz. Dallanma/fork/collapse bir aksiyonun kararıdır; SLA yalnız sıradaki havuza devreder",
                        wft_form_name(other)
                    ),
                ),
            }
            if let Some(effects) = &esc.wfes_effects {
                check_sla_effect_namespaces(effects, &format!("{path}.wfes_effects"), report);
            }
        }
        if let Some(ct) = &node.claim_timeout {
            let path = format!("nodes[{key}].claim_timeout");
            if let Err(e) = parse_iso8601_duration(&ct.after) {
                report.error("duration_format", format!("{path}.after"), e.to_string());
            }
            if let Some(effects) = &ct.wfes_effects {
                check_sla_effect_namespaces(effects, &format!("{path}.wfes_effects"), report);
            }
            if let Some(target) = &ct.wft {
                // SLA-1 hedefi YALNIZ node olabilir (2026-07-28). Terminal referansı
                // ayrı bir hata verir; hiç bilinmiyorsa cross_ref.
                if wfd.terminals.iter().any(|t| t.id == *target) {
                    report.error(
                        "sla_terminal_target",
                        format!("{path}.wft"),
                        format!(
                            "SLA-1 claim_timeout hedefi terminal olamaz ('{target}') — bir node seçin ya da hedefi kaldırıp claim'i havuza bırakın"
                        ),
                    );
                } else if !wfd.nodes.contains_key(target) {
                    report.error(
                        "cross_ref",
                        format!("{path}.wft"),
                        format!("bilinmeyen node '{target}'"),
                    );
                }
            }
        }
    }
}

// ---- M7: $exec.response.* her yerde hata; $ctx.* referans yolları şemada olmalı ----

fn check_string_namespaces(wfd: &Wfd, report: &mut ValidationReport) {
    let value = match serde_json::to_value(wfd) {
        Ok(v) => v,
        Err(_) => return,
    };
    walk_strings(&value, "$", &mut |s, path| {
        if s.contains("$exec.response.") {
            report.error(
                "exec_response",
                path.to_string(),
                format!("'$exec.response.*' kaldırıldı (M7) — '$exec.result.*' kullanın: '{s}'"),
            );
        }
        // $ctx.<path> referansları — token'ı çıkar, şemada doğrula
        let mut rest = s;
        while let Some(idx) = rest.find("$ctx.") {
            let token: String = rest[idx + 5..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '.')
                .collect();
            let token = token.trim_end_matches('.').to_string();
            if !token.is_empty() {
                if let PathResolution::Missing = resolve_schema_path(&wfd.context, &token) {
                    report.error(
                        "ctx_ref",
                        path.to_string(),
                        format!("'$ctx.{token}' context şemasında yok"),
                    );
                }
            }
            rest = &rest[idx + 5..];
        }
    });
}

fn walk_strings<'a>(v: &'a Value, path: &str, f: &mut impl FnMut(&'a str, &str)) {
    match v {
        Value::String(s) => f(s, path),
        Value::Array(arr) => {
            for (i, item) in arr.iter().enumerate() {
                walk_strings(item, &format!("{path}[{i}]"), f);
            }
        }
        Value::Object(map) => {
            for (k, item) in map {
                // context şeması serbest metin içerebilir (description vb.) — atla
                if path == "$" && k == "context" {
                    continue;
                }
                walk_strings(item, &format!("{path}.{k}"), f);
            }
        }
        _ => {}
    }
}

// ---- context şeması yol çözümü ----

enum PathResolution {
    Found,
    Missing,
    /// Şema bu derinliği kısıtlamıyor (properties tanımsız, $ref, vs.)
    Opaque,
    Readonly,
}

fn resolve_schema_path(context: &Value, dotted: &str) -> PathResolution {
    let mut current = context;
    let mut readonly = false;
    for segment in dotted.split('.') {
        if current.get("$ref").is_some() {
            return PathResolution::Opaque;
        }
        let Some(props) = current.get("properties").and_then(Value::as_object) else {
            return PathResolution::Opaque;
        };
        let Some(next) = props.get(segment) else {
            return PathResolution::Missing;
        };
        current = next;
        if current
            .get("x-wf-readonly")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            readonly = true;
        }
    }
    if readonly {
        PathResolution::Readonly
    } else {
        PathResolution::Found
    }
}
