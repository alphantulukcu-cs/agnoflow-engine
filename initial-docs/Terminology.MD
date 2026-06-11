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

A purpose-built language covering hierarchical traversal with composable steps and boolean type filters:

```
self                             → same ORGU
parent                           → parent ORGU
siblings                         → all siblings (children of parent)
siblings[F]                      → siblings matching filter F
children                         → all direct children
children[F]                      → direct children matching filter F
up[F]                            → nearest ancestor matching filter F (upward search)
```

Steps are composable with `.` — arbitrary chains are supported:

```
self.parent
self.up[bolge].children[sehir].children[ilce]
self.siblings.children[sube]
self.parent.siblings.children[special:kredi && type:sube]
```

#### Type Filter Syntax `[F]`

`F` is a boolean expression over ORGU type attributes (`orgu_type` jsonb column):

```
[sube]                        → shorthand for [type:sube]
[special:doviz]               → matches {"special":["doviz",...]}
[type:sube && special:doviz]  → AND: must match both conditions
[type:sube || type:ilce]      → OR: matches either condition
[!type:root]                  → NOT: excludes type root
[(type:sube || type:ilce) && !special:doviz]  → parentheses for grouping
```

**Operator precedence** (highest to lowest): `!` → `&&` → `||`

**Type matching semantics** for a leaf filter `[key:val]`:
- Scalar: `orgu_type->>'key' = 'val'`  (e.g. `{"type":"sube"}`)
- Array:  `orgu_type->'key' @> to_jsonb('val')` (e.g. `{"special":["doviz","kredi"]}`)
- Wildcard node `{"*":true}` always matches any filter expression

**Why selected:**
- Covers all required traversal patterns
- Readable by workflow designers without technical background
- Each step compiles to one PostgreSQL ltree CTE template
- Boolean filter expression compiles to inline SQL — no extra joins needed

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

**`orgu_type`** is stored as a jsonb column. Each ORGU can carry one or more type attributes:

```json
{"type": "sube"}
{"type": "sube", "special": ["doviz", "kredi"]}
{"*": true}
```

A wildcard node (`{"*":true}`) matches any filter expression.

Each ORGTRVLANG step compiles to a CTE-based SQL template. The multi-ORGU pipeline executor passes each step's output as the anchor set for the next step. Example for `children[F]` where `$1` = anchor uuid array, `$2` = orgt_id, `$3`/`$4` = filter key/val:

```sql
WITH members AS (
    SELECT o.orgu_id, oo.path, o.orgu_type, ...
    FROM orgu o JOIN orgt_orgu oo ON o.orgu_id = oo.orgu_id
    WHERE oo.orgt_id = $2 AND o.is_active AND oo.is_active
),
anchors AS (SELECT path FROM members WHERE orgu_id = ANY($1::uuid[]))
SELECT DISTINCT ... FROM members m
JOIN anchors a ON m.path <@ a.path AND nlevel(m.path) = nlevel(a.path) + 1
WHERE (m.orgu_type ? '*' OR <filter_sql>)
```

For a compound filter `[special:kredi && type:sube]`, `<filter_sql>` expands to:

```sql
((m.orgu_type->>'special' = 'kredi' OR m.orgu_type->'special' @> to_jsonb('kredi'::text))
 AND
 (m.orgu_type->>'type' = 'sube' OR m.orgu_type->'type' @> to_jsonb('sube'::text)))
```

**Index recommendation:** GiST on `path`, B-tree on `orgu_id` and `orgt_id`. GIN on `orgu_type` if jsonb filter queries become a bottleneck on large trees.

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

## VISIBILITY
---

**Visibility (V) :** A function V (**DynCtx**, **A**) ==> boolean. By defult V is always true for all parts of **DynCtx** but some parts may have 
**V**. If there is **V** for a part of **DynCtx**, it should be evaluated for that specific **WFE** in runtime

## DYNCONTEXT
---

**WFE Starting Context (WFE-SDynCtx) :** This is the context each **WFE** has when it is initiated. It has some mandatory parts like **A** who has started this **WFE** but not limited to this. Other mandatory parts should be definable in **WFD**

**Immutability :** DynCtx is immutable. Either all DynCtx will be copied to a new instance or diff parts are saved and the new DynCtx is 
obtained by merging.
