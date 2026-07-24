# TERMINOLOGY

## ORGANIZATION
---
**Organization Tree (ORGT) :** A hierarchical tree structure such that

- Every node has exactly one parent except ORGT which has no parent

- Every ORGT has one or more orgazinational tenant **(ORGTNT)** which is the root of that organization

- Every node has types **(ORGU_T)** tag in the form types : {"key":value|*} whcih may be empty

**Organization Tenant (ORGTNT) :** The root of an organization

**Organization Unit (ORGU) :** A node in ORGT and may exist under more than one ORGTNT

#### ORGU TRAVERSAL
---
We have a **ORGT** traversal language **(ORGTRVLANG)** such that it is possible to define

- the same ORGU
- The ORGUs of my parent
- The ORGUs of my parent with type = T
- All of my children
- My children of type = T
- Up from me, first ancestor with type = T
- The children of Up from me, first ancestor with type = T
- The children of type = T, Up from me, first ancestor with type = T
- The children of type = T of my children of type = T

Three options were evaluated:

---

**Option 1 — XPath-Style Path Expressions:**

```
.                                        → same ORGU
..                                       → parent
../*                                     → siblings (ORGUs of my parent)
../*[type='T']                           → parent's children of type T
./*                                      → all my children
./*[type='T']                            → my children of type T
ancestor::*[type='T'][1]                 → first ancestor of type T
ancestor::*[type='T'][1]/*               → children of that ancestor
ancestor::*[type='T'][1]/*[type='T']     → typed children of typed ancestor
./*[type='T']/*[type='T']                → typed grandchildren via typed children
```

Covers all patterns. Not user-friendly — requires XML/XSLT background. `ancestor::*[type='T'][1]` is not readable by a business analyst.

---

**Option 2 — Dotted-Path Encoding (NATS.io inspired):**

```
self
self.parent:{type1, type2}
self.*
self.*:{type1, type2}
```

User-friendly for simple downward navigation. **Cannot express upward ancestor traversal** (`up[T]` and derived patterns). Eliminated.

---

**Option 3 — Custom Minimal DSL ✅ SELECTED**

A purpose-built language covering exactly the 9 required patterns, readable without prior technical knowledge:

```
self                             → same ORGU
parent                           → parent ORGU
siblings                         → all siblings (children of parent)
siblings[T]                      → siblings of type T
children                         → all direct children
children[T]                      → direct children of type T
up[T]                            → first ancestor of type T (upward search)
up[T].children                   → all children of first ancestor of type T
up[T].children[T]                → typed children of first ancestor of type T
children[T].children[T]          → typed grandchildren via typed children
```

**Why selected:**
- Covers all 9 required patterns; nothing more
- Readable by workflow designers without technical background
- Each token maps directly to one PostgreSQL ltree query template — no complex parser needed
- Extensible in a controlled way if new traversal patterns are required

---

#### ORGTRVLANG — PostgreSQL ltree Implementation

ORGT is stored in PostgreSQL using the **ltree** extension. Each ORGU stores its full root-to-node path as a dot-separated label string. A **GiST** index on the path column accelerates all hierarchical operators.

```
ORGTNT root       →  path: '1'
Division          →  path: '1.10'
Branch            →  path: '1.10.100'
Credit Dept       →  path: '1.10.100.1001'
```

Key ltree operators used:

| Operator | Meaning |
|---|---|
| `@>` | is ancestor of |
| `<@` | is descendant of |
| `~` | matches lquery pattern |
| `subpath(p, offset, len)` | extract subpath |
| `nlevel(p)` | depth of path |

**ORGU_T** (node type) is stored as a separate column and filtered with a standard B-tree index alongside ltree path queries.

Each ORGTRVLANG token compiles to one SQL template where `$p` is the anchor ORGU path:

```sql
-- self
WHERE path = $p

-- parent
WHERE path = subpath($p, 0, nlevel($p) - 1)

-- siblings
WHERE subpath(path, 0, nlevel(path) - 1) = subpath($p, 0, nlevel($p) - 1)
  AND path <> $p

-- siblings[T]
WHERE subpath(path, 0, nlevel(path) - 1) = subpath($p, 0, nlevel($p) - 1)
  AND path <> $p AND orgu_type = 'T'

-- children
WHERE path <@ $p AND nlevel(path) = nlevel($p) + 1

-- children[T]
WHERE path <@ $p AND nlevel(path) = nlevel($p) + 1 AND orgu_type = 'T'

-- up[T]  (nearest ancestor of type T)
WHERE $p <@ path AND orgu_type = 'T'
ORDER BY nlevel(path) DESC LIMIT 1

-- up[T].children
WITH anc AS (
    SELECT path FROM orgu
    WHERE $p <@ path AND orgu_type = 'T'
    ORDER BY nlevel(path) DESC LIMIT 1
)
SELECT o.* FROM orgu o
JOIN anc ON o.path <@ anc.path AND nlevel(o.path) = nlevel(anc.path) + 1

-- up[T].children[T]
WITH anc AS (
    SELECT path FROM orgu
    WHERE $p <@ path AND orgu_type = 'T'
    ORDER BY nlevel(path) DESC LIMIT 1
)
SELECT o.* FROM orgu o
JOIN anc ON o.path <@ anc.path AND nlevel(o.path) = nlevel(anc.path) + 1
WHERE o.orgu_type = 'T'

-- children[T].children[T]
SELECT o2.* FROM orgu o1
JOIN orgu o2 ON o2.path <@ o1.path AND nlevel(o2.path) = nlevel(o1.path) + 1
WHERE o1.path <@ $p AND nlevel(o1.path) = nlevel($p) + 1
  AND o1.orgu_type = 'T' AND o2.orgu_type = 'T'
```

**Index recommendation:** GiST on `path`, B-tree on `orgu_type`. GIN is not needed — hierarchical range queries (`<@`, `@>`) favour GiST; GIN would only gain on label-pattern searches (`~`) which are not the primary use case here.

## USER and ROLES
---
**User (U) :** A user which has a unique ID in **ORGT**. A **U** may belong to more than one **ORGU**

**Role (R) :** A keyword which has a uniqu ID in **ORGT**. 

**User Role (UR) :** (U, [{ORGU:type, timeslice}, R]..) where ORGU:type can be defined by using **ORGTRVLANG** or by simply indicationg ORGU:type

- more than one **R** regardless of **ORGU** with/without a time slice

- more than one **R** for some **ORGU**s with/without a time slice

- more than one **R** for some **ORGU**s with **(ORGU_T)**s with/without a time slice


## WORKFLOW
---

> All definitions below assume that **WFD** is also available for **WFE**

**Workflow Definition (WFD) :** A JSON which defines everything about a workflow

**Workflow Execution (WFE) :** An unique instance of a **WFD**. 

**Dynamic Context (DynCtx) :** A JSON which keeps the variable state of a **WFE**

**Action (ACT) :** An Action when applied may change **WFES**

**Actor (A) :** The **(ORGU, (U, R))** tuple who applied an **ACT** on a **WFE**. This is an exact tuple

**Workflow Actions History (WFAH) :** A history of **(**ACT**, **A**) tuple applied to this **WFE**

**Workflow Execution State (WFES) :** The exact state of a **WFE** which is union of **DynCtx** and **WFAH**

**Candidate ORGUs for an ACT (C_ORGU) :** A set of **ORGU**s which are eligible to apply any **ACK** on a specific **WFE** when at a specific **WFES**. This is defined by using **(ORGTRVLANG)**

**Candidate Roles for an ACT (C_R) :** A set of **(R)**  which are eligible to apply any **ACK** on a specific **WFE** when at a specific **WFES**

**Candidate U for an ACT (C_U) :** A set of **U**s which are eligible to apply any **ACK** on a specific **WFE** when at a specific **WFES**

**Candidate Actor for an ACT (C_A) :** Union of **C_ORGU**, **C_R** and **C_U**

**Permission (P) :** A function P (**WFS**, **A**, **ACT**) ==> true or false. This is an axact value

**Workflow Transition (WFT) :** A function WFT (**WFS**, **A**, **ACT**) ==> (**new WFS**, **new C_A**)

**Possible Actions for a WFS (P_ACT):** This is a function P_ACT (**WFS**) ==> {**A**, **ACT**s}

**Possible Actions for a WFS for an Actor (P_ACT_A):** This is a function P_ACT_A (**WFS**, **A**) ==> {**ACT**s}

## WFE NODE MODEL
---

A **WFE** can be understood as a flow between **nodes**. Each **C_A** is a node. When a WFE is at a node it is waiting for an **ACT** from that **C_A**.

Two node types exist:

**Human Node :** A node whose **C_A** is one or more human actors. The WFE waits until a matching **Actor** performs the required **ACT**.

**Autoexec Node :** A node that executes an **ACT** automatically without user intervention. The engine fires it immediately when the WFE arrives. The actor recorded in **WFAH** is the system itself. Autoexec nodes are used for:
- Fetching data from external REST APIs
- Querying internal SQL tables
- Computing derived values from existing **DynCtx** fields

Three autoexec types:

| Type | Purpose |
|---|---|
| `rest` | HTTP call to an external API; maps response fields into DynCtx |
| `sql` | Parameterised SQL query against an internal table; maps result rows into DynCtx |
| `calc` | Arithmetic / boolean expressions over existing DynCtx fields; writes computed values into DynCtx |

**Autoexec results** are referenced in `wfes_effects` via `$exec.result.field_name`, parallel to `$action.input.field_name` for human actions.

**Error handling:** Every autoexec node defines an `on_error` block with its own `wfes_effects` and `wft`, transitioning the WFE to a recoverable human node on failure.

**Chaining:** The `wft` of an autoexec node may point to another autoexec node. The engine chains them until it reaches a human node or a terminal state. Example: `rest → sql → calc → human`.

## WFE ENDING
---

**Trigger (TRIGGER) :** A side-effect execution that occurs outside the WFE context. A TRIGGER does not change **WFES** — it is not a transition. It invokes an external business service, database transaction, or computation.

- A **TRIGGER** is always represented as an **autoexec node** (`rest`, `sql`, or `calc`) — nothing else
- **Every ACT** (human or autoexec) may have a TRIGGER — it is not restricted to ending actions
- Two trigger positions exist in WFD:
  - **Action trigger** — fires every time that ACT fires, regardless of which `wft` branch is taken (audit logs, notifications)
  - **Branch trigger** — defined inside a `wft` condition branch; fires only when that branch is taken (business transactions on specific outcomes)
- Any node except `start` may end the WFE
- TRIGGERs may chain: the autoexec node's `wft` may point to another autoexec

**WFE End Response (WFE_END_RESPONSE) :** A JSON document produced when a WFE reaches its terminal state. Defined on a terminal `wft` condition branch. Values are literals or `{ "ref": "$ctx.field" }` references to **DynCtx**.

```json
"wfe_end_response": {
  "status":         "approved",
  "amount_granted": { "ref": "$ctx.credit_info.amount_requested" },
  "applicant_name": { "ref": "$ctx.applicant.name" }
}
```

The `WFE_END_RESPONSE` is stored alongside the closed WFE record and returned to the calling system.

## CONDITION EVALUATION — ZEN ENGINE
---

All conditions in WFD — transition `when`, `wft` branch `when`, listable `when`, and conditional triggers — are evaluated using the **ZEN expression language** from [gorules/zen](https://github.com/gorules/zen).

ZEN is a business-friendly expression language. It evaluates against a JSON context. The WFE engine exposes **WFES** to ZEN as:

```
$<field>          → any DynCtx top-level field (e.g. $status, $credit_info)
$wfah             → full WFAH array of { action, actor, at } objects
```

**Expression examples:**

```
$status == "pending_review"
$credit_info.amount_requested < 1000
$credit_info.amount_requested >= 1000 and $credit_grade == "A"
count(filter($wfah, # .action == "inspected")) >= 1
some($wfah, # .action == "inspected" and # .actor.role == "inspector")
```

ZEN also supports **decision tables** (spreadsheet-like input → output rules) and a **Switch node** for branching — both available in WFD for complex routing scenarios.

ZEN replaces the previous custom JSON predicate format (`{ "field": "...", "op": "...", "value": "..." }`) entirely. ZEN expressions are plain strings.

## VISIBILITY
---

**Visibility (V) :** A function V (**DynCtx**, **A**) ==> boolean. By default V is always true for all parts of **DynCtx** but some parts may have **V**. If there is **V** for a part of **DynCtx**, it should be evaluated for that specific **WFE** in runtime.

> **Visibility is strictly a DynCtx concept** — it controls which fields of the execution state an Actor can read. It does not control whether the Actor can see the WFE itself.

## LISTABLE
---

**Listable (L) :** Defines which **Actor**s can see a **WFE** in listings and queries without necessarily having the right to act on it. Actors already in any **C_A** of the WFE's current **WFES** have listability implicitly. **L** is defined in the **WFD** and may be permanent or gated on a **WFES** condition.

- Actors matched by **L** but not by **C_A** are **read-only** observers — they can see the WFE and its **DynCtx** (subject to **V**), but cannot perform any **ACT**
- **V** still applies on top of **L** — a listable actor only sees the DynCtx fields their Actor matches in **V**

## DYNCONTEXT
---

**WFE Starting Context (WFE-SDynCtx) :** This is the context each **WFE** has when it is initiated. It has some mandatory parts like **A** who has started this **WFE** but not limited to this. Other mandatory parts should be definable in **WFD**

**Immutability :** DynCtx is immutable. Either all DynCtx will be copied to a new instance or diff parts are saved and the new DynCtx is 
obtained by merging.
