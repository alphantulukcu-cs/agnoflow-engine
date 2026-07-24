# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Status

This is a **Workflow Engine (WF Engine)** project currently in the conceptual/design phase. No implementation exists yet — the repo contains only design documentation. `Terminology.MD` is the canonical reference for all domain concepts.

## Domain Architecture

The engine is built around these core relationships:

**Organization layer:**
- `ORGT` — hierarchical tree; every node has exactly one parent except the root
- `ORGTNT` — root of an organization (one per organization)
- `ORGU` — a node in ORGT; can exist under multiple ORGTNTs; every node has a type tag `ORGU_T` in the form `{"key": value | *}`
- `ORGTRVLANG` — ORGT Traversal Language (**custom minimal DSL, selected**). Used to define C_ORGU. Tokens: `self`, `parent`, `siblings`, `siblings[T]`, `children`, `children[T]`, `up[T]`, `up[T].children`, `up[T].children[T]`, `children[T].children[T]`. Each token compiles to one PostgreSQL ltree SQL template (anchor param `$p`). See Terminology.MD for full SQL mappings.
- `U` (User) — belongs to one or more ORGUs; has a unique ID in ORGT
- `R` (Role) — a keyword with a unique ID in ORGT; full definition TBD
- `UR` (User Role) — the full role assignment tuple: `(U, [{ ORGU_scope, timeslice }, R] ..)` where:
  - `ORGU_scope` — the ORGUs in which this role applies; expressed via ORGTRVLANG or a specific ORGU_T; if omitted the role applies regardless of ORGU
  - `timeslice` — optional validity period for this role assignment
  - A user can hold multiple UR entries: same R in different ORGUs, different Rs globally, different Rs per ORGU type, all with or without time slices

**Workflow layer:**
- `WFD` — JSON document that fully defines a workflow (definition)
- `WFE` — a unique runtime instance of a WFD
- `DynCtx` — the mutable variable state of a WFE; **immutable by design** (new copies or diffs are stored, never in-place mutation)
- `WFAH` — append-only history of `(ACT, Actor)` tuples applied to a WFE
- `WFES` — full execution state = union of `DynCtx` + `WFAH`

**Actor & permission layer:**
- `Actor (A)` — exact tuple `(ORGU, (U, R))` — who performed an action
- `ACT` — an action that, when applied, may change WFES
- `P(WFES, A, ACT) → bool` — permission function; exact evaluation, no ambiguity
- `WFT(WFES, A, ACT) → (new WFES, new C_A)` — transition function; produces next state and next eligible actor set

**Candidate actor resolution:**
- `C_ORGU`, `C_R`, `C_U` — sets of eligible ORGUs, Roles, Users for a given WFES
- `C_A` — union of the three above

**Query functions:**
- `P_ACT(WFES) → {A, ACTs}` — all possible (actor, action) pairs for a state
- `P_ACT_A(WFES, A) → {ACTs}` — actions available to a specific actor
- `V(DynCtx, A) → bool` — visibility of a DynCtx field for an actor; default true

**WFE lifecycle:**
- Every WFE starts with `WFE-SDynCtx` (starting context), which must capture the initiating Actor and any mandatory fields declared in the WFD.

## Key Design Invariants

1. `DynCtx` is **immutable** — persist full copies or diffs, reconstruct by merging
2. `Actor` is an **exact** `(ORGU, (U, R))` triple — no partial matching
3. `Permission` and `Visibility` are **exact** evaluations against the current `WFES`
4. `WFAH` is **append-only** — forms a complete audit trail
5. `WFD` must always be available alongside its `WFE` for any operation

## WFD JSON Structure (decided)

A WFD has five top-level sections:

```
{ "id", "name", "version", "description", "context", "start", "actions", "transitions", "terminal_when" }
```

- **`context`** — JSON Schema 2020-12 document describing the DynCtx object. Extended with two custom keywords: `x-visibility` (V function per field/object) and `x-wf-readonly` (auto-set fields users cannot write). See `WFD-Design.md` for full spec.
- **`start`** — array of start rules; each has `c_a` (who may initiate), `wfes_effects` (initial DynCtx values), `wft` (first C_A after start).
- **`actions`** — dictionary of all ACTs; each has `name`, `description`, `input.required`, `input.optional`.
- **`transitions`** — ordered array of rules; each has `id`, `when` (predicate on WFES), `action`, `c_a` (C_A / P definition), `wfes_effects`, `wft`.
- **`terminal_when`** — predicate; when true the WFE is closed and no further actions are accepted.

## JSON Key ↔ Terminology Mapping (decided)

| JSON key | Terminology | Meaning |
|---|---|---|
| `c_a` | C_A | Candidate Actor set — array of rules (OR across rules, AND within a rule) |
| `c_orgu` | C_ORGU | Candidate ORGUs — defined using ORGTRVLANG expression, optionally anchored to a DynCtx actor field |
| `c_r` | C_R | Candidate Roles |
| `c_u` | C_U | Candidate Users |
| `wfes_effects` | WFES mutation | Fields written into DynCtx when the action fires |
| `wft` | WFT result | New C_A produced after the transition — same structure as `c_a` |

## Expression Syntax (decided)

**Predicates** (`when`, `terminal_when`):
```json
{ "field": "ctx.status", "op": "eq", "value": "submitted" }
{ "and": [ ... ] }  { "or": [ ... ] }  { "not": { ... } }
{ "history": { "action": "X", "count": { "op": "lt", "value": 2 } } }
```
Operators: `eq`, `ne`, `gt`, `gte`, `lt`, `lte`, `in`, `not_in`, `is_null`, `is_not_null`

**Effect/auto values**: `"$actor"`, `"$timestamp"`, `"$wfe_id"`, `"$action.input.field_name"`

**Dynamic references** (`c_orgu`, `c_u`, `wfes_effects`): `{ "ref": "$ctx.field_path" }`
- `$ctx.actor_field.orgu` / `.user` / `.role` — decompose a stored Actor

## `c_a` Structure (decided — universal)

`c_a` is the standard candidate actor block used in **three positions**: `start` (who may initiate), transitions (who may act), and `wft` (who acts next). Shape is always the same:

```json
"c_a": [
  {
    "c_orgu": "<ORGTRVLANG expr or *:[type:T]>",
    "c_r":    [["self", "roleName"]]
  }
]
```

- Array of rules — OR across rules, AND within a rule
- `c_orgu` — ORGTRVLANG expression. Two forms:
  - Absolute (no anchor): `"*:[type:branch]"` — any ORGU of type branch
  - Relative (anchored to DynCtx actor): `{ "from": "$ctx.field.orgu", "traverse": "self" }`
- `c_r` — list of `["orgu_scope", "role"]` pairs; `self` means the same ORGU as identified by `c_orgu`

## `start` / Transition Structure (decided)

Every action block — whether in `start` or `transitions` — has exactly three keys:

```json
{
  "c_a": [
    {
      "c_orgu": "*:[type:branch]",
      "c_r":    [["self", "clerk"]]
    }
  ],
  "wfes_effects": {
    "set": {
      "initiated_by": "$actor",
      "initiated_at": "$timestamp",
      "status":       "pending_review"
    }
  },
  "wft": {
    "c_a": [
      {
        "c_orgu": "self",
        "c_r": [
          ["self", "creditDeptManager"],
          ["self", "branchManager"]
        ]
      }
    ]
  }
}
```

`start` is an array of such blocks (one per entry path). `transitions` entries additionally have `id`, `when`, and `action`. No `when` in start blocks — there is no existing WFES to evaluate.

## DynCtx Schema Convention (decided)

`context` is a standard **JSON Schema 2020-12** object schema. Two WF Engine extensions:
- `x-visibility: { "c_r": [...], "c_orgu": [...], "c_u": [...] }` — placed on a property; OR logic across criteria.
- `x-wf-readonly: true` — marks fields set by `start.wfes_effects` or transition `wfes_effects`; users cannot supply these.

Use `$defs` + `$ref` for reusable types (e.g., the `actor` type).
Use `"pattern": "^[0-9]{11}$"` style constraints for domain-specific string formats.

Full user manual and worked examples: `WFD-Design.md`
