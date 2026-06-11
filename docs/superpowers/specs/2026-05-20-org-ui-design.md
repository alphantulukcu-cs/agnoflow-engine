# Org UI — Design Spec
**Date:** 2026-05-20  
**Status:** Approved  
**Goal:** Single-page developer tool for exploring the ORGT hierarchy and testing ORGTRVLANG traversal queries against the running org-api.

---

## Scope

- Vite + React + TypeScript SPA at `workflow-engine/org-ui/`
- Read-only: visualizes org tree, runs traversal queries, shows results
- Connects to org-api at `http://localhost:3000`
- No auth, no mutations

Out of scope: WFD/WFE workflow layer, user-role management, write operations.

---

## Layout — 50/50 Split

```
┌─────────────────────────────┬─────────────────────────────┐
│  [Tenant ▾]  [Tree ▾]       │                             │
│─────────────────────────────│  self = <uuid>              │
│                             │  parent                     │
│      ReactFlow org tree     │                             │
│                             │  [Run ▶]                    │
│   (click node →             │─────────────────────────────│
│    auto-fills self= line)   │  Result nodes (name, type,  │
│                             │  uuid per row)              │
│                             │  (highlighted in tree too)  │
└─────────────────────────────┴─────────────────────────────┘
```

---

## Dependencies

| Package | Purpose |
|---|---|
| `@xyflow/react` | ReactFlow tree canvas |
| `dagre` | Automatic top-down tree layout |
| `@types/dagre` | TypeScript types for dagre |

org-api needs `tower-http` CORS middleware added so the UI (localhost:5173) can reach it.

---

## Components

### `App.tsx`
Root layout. Renders `<LeftPanel>` and `<RightPanel>` side by side. Holds global state: selected tenant, selected tree, loaded ORGUs, query string, result IDs (Set<string>).

### `LeftPanel.tsx`
- Tenant dropdown → `GET /orgtnt`
- Tree dropdown → `GET /orgtnt/:id/orgt`
- ReactFlow canvas → `GET /orgt/:id/orgu` on tree select
- Nodes are dagre-positioned (top-down)
- Node click → writes `self = <uuid>` into the shared query state
- Result nodes are highlighted (distinct color/border)

### `RightPanel.tsx`
- `<textarea>` for the query (shared state from App)
- Run button
- Results list: each result row shows `name`, `orgu_t`, `orgu_id`

### `api.ts`
Typed fetch wrappers for all org-api endpoints. No external HTTP library.

### `layout.ts`
Dagre layout helper: takes `Orgu[]`, returns `Node[]` + `Edge[]` for ReactFlow.

---

## Data Flow

1. **Mount** → `GET /orgtnt` → tenant dropdown populated
2. **Tenant selected** → `GET /orgtnt/:id/orgt` → tree dropdown populated
3. **Tree selected** → `GET /orgt/:id/orgu` → ORGUs loaded → dagre layout → ReactFlow renders
4. **Node clicked** → `self = <uuid>` written to query textarea (replaces first line if it starts with `self =`)
5. **Run clicked** → parse query:
   - Line 1 must match `self = <uuid>` → extract UUID
   - Line 2 (trimmed) → traversal expression
   - → `GET /orgu/<uuid>/traverse?expr=<expr>`
   - → result `Orgu[]` stored as highlighted ID set
   - → results list rendered in right panel
   - → matching nodes in ReactFlow re-render with highlight style
6. **Error** (parse error, API error) → shown inline below Run button

---

## Query Format

```
self = 3fa85f64-5717-4562-b3fc-2c963f66afa6
children[branch]
```

- Line 1: `self = <uuid>` (required, sets anchor node)
- Line 2: any valid ORGTRVLANG expression (`self`, `parent`, `siblings`, `siblings[T]`, `children`, `children[T]`, `up[T]`, `up[T].children`, `up[T].children[T]`, `children[T].children[T]`)
- Extra lines ignored

---

## CORS Change to org-api

Add `tower-http = { version = "0.5", features = ["cors"] }` to `org-api/Cargo.toml` and wrap the router with `CorsLayer::permissive()` in `main.rs`. Permissive is fine — this is a developer tool with no auth.

---

## Project Structure

```
org-ui/
├── index.html
├── vite.config.ts
├── tsconfig.json
├── package.json
└── src/
    ├── main.tsx
    ├── App.tsx
    ├── LeftPanel.tsx
    ├── RightPanel.tsx
    ├── api.ts
    ├── layout.ts
    └── types.ts          ← Orgtnt, Orgt, Orgu interfaces
```

---

## Error States

| Scenario | Handling |
|---|---|
| No tenant/tree selected | Run button disabled |
| `self =` line missing or invalid UUID | Inline error below Run |
| Unknown traversal expr | API returns 400 → shown inline |
| Network error | Inline error below Run |
| Empty result set | "No results" message in results panel |
