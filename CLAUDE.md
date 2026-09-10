# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What This App Does

Schemetry is a **Tauri v2 desktop application** for database. It provides schema comparison across multiple Oracle and/or PostgreSQL instances, DDL generation, SQL querying with multi-server execution, and idempotent fix script generation. A connection's `db_type` (`oracle` | `postgres`) determines which repository handles it — see `repositories/dispatch_repository.rs`.

## Commands

```bash
# Frontend only (Vite dev server on port 5173)
npm run dev

# Full desktop app with hot reload (requires Rust toolchain + Oracle Instant Client)
npm run tauri dev

# Production desktop build
npm run tauri build

# TypeScript / Svelte type checking
npm run check

# Run all Rust unit tests
cd src-tauri && cargo test

# Run a single test by name (substring match)
cd src-tauri && cargo test test_name_here
```

**After any change to Rust backend code, run `cargo test` and confirm all tests pass before considering the task complete.**

## Tests

Rust unit tests cover the core business logic. Tests live in separate files alongside their source modules using `#[path = "tests/..."]`:

- `src-tauri/src/services/tests/compare.rs` — tests for `services/compare.rs` (schema comparison, `equal_ci`, `skip_index_diff`)
- `src-tauri/src/services/tests/fix.rs` — tests for `services/fix.rs` (fix-script generation, SQL builders, `generate_fix_script`)
- `src-tauri/src/repositories/tests/oracle_repository.rs` — tests for `repositories/oracle_repository.rs` (`clean_opt`, `fmt_type`, `make_idempotent`, `clean_sequence_ddl`)

`repositories/postgres_repository.rs` has no dedicated unit-test file yet — its catalog-introspection/DDL-rendering logic is exercised by the Postgres integration suite below instead.

Private functions are accessible in test files because each `mod tests` is declared as a child module of the source file via `#[cfg(test)] #[path = "tests/..."] mod tests;`.

### Integration tests (real Oracle, via Docker + Flyway)

`src-tauri/tests/oracle_integration.rs` exercises compare/fix/DDL/query logic against two **real** Oracle instances instead of in-memory fixtures. `docker/docker-compose.yml` starts two `gvenzl/oracle-free` containers (`oracle-source`, `oracle-target` — host ports 1521/1522) and migrates each with Flyway from `docker/migrations/{source,target}/V1__init.sql`. The two migrations are intentionally divergent (target is missing a table, missing a column, and has a shorter `VARCHAR2`) so schema-compare has real discrepancies to find, and the generated fix script can be executed against TARGET and re-run to prove it's idempotent.

```bash
# Start + migrate both databases (needs Docker Desktop running)
cd docker && docker compose up -d --build
docker compose ps   # wait for both oracle-* services to report "healthy"

# Run the integration tests (separate from `cargo test` — see below)
cd ../src-tauri && cargo test --test oracle_integration -- --ignored

# Tear down (drops the seeded data too)
cd ../docker && docker compose down -v
```

These tests are `#[ignore]`d so plain `cargo test` (and CI) never depends on Docker or a live database. They need the same **Oracle Instant Client** as the app itself (see Setup Requirements) — set `ORACLE_CLIENT_LIB_DIR` if it isn't already on `PATH`. Connection details default to the docker-compose file's ports/credentials and can be overridden with `SCHEMETRY_TEST_ORACLE_HOST`, `SCHEMETRY_TEST_SOURCE_PORT`, `SCHEMETRY_TEST_TARGET_PORT`, `SCHEMETRY_TEST_ORACLE_SERVICE`, `SCHEMETRY_TEST_ORACLE_USER`, `SCHEMETRY_TEST_ORACLE_PASSWORD`.

### Integration tests (real Postgres, via Docker + Flyway)

`src-tauri/tests/postgres_integration.rs` mirrors the Oracle suite above against two real `postgres:16-alpine` containers (`postgres-source`, `postgres-target` — host ports 5432/5433), migrated by Flyway from `docker/migrations/{pg-source,pg-target}/V1__init.sql` (same three intentional discrepancies, translated to Postgres types). No native client library setup is needed here — `tokio-postgres` is a pure-Rust wire-protocol client.

```bash
# Same docker-compose file as the Oracle suite — this also starts the Postgres containers
cd docker && docker compose up -d --build
docker compose ps   # wait for both postgres-* services to report "healthy"

cd ../src-tauri && cargo test --test postgres_integration -- --ignored
```

Connection details default to the docker-compose file's ports/credentials and can be overridden with `SCHEMETRY_TEST_PG_HOST`, `SCHEMETRY_TEST_PG_SOURCE_PORT`, `SCHEMETRY_TEST_PG_TARGET_PORT`, `SCHEMETRY_TEST_PG_DATABASE`, `SCHEMETRY_TEST_PG_USER`, `SCHEMETRY_TEST_PG_PASSWORD`.

## Architecture

**Stack:** Svelte 5 + TypeScript (frontend) · Tauri v2 (desktop shell) · Rust (backend) · SQLite (local storage) · Oracle OCI (via `oracle` crate) · PostgreSQL (via `tokio-postgres`)

### Frontend → Backend boundary

All backend calls go through `src/api.ts`, which wraps `invoke()` from `@tauri-apps/api/core`. The Rust side is in `src-tauri/src/`:

```
commands/   ← thin Tauri command handlers, delegate immediately to services
services/   ← business logic (compare, fix, query, DDL, connections, settings)
repositories/ ← data access: Oracle/Postgres queries (dispatched by `db_type`) + SQLite CRUD
```

`AppState` (in `state.rs`) is a shared `Arc<Mutex<FetchSnapshot>>` that caches schema metadata fetched from Oracle/Postgres servers, so compare and fix operations can reuse it without re-querying.

Passwords are stored in the OS keychain via the `keyring` crate; connection metadata goes to SQLite.

### Frontend routing

There is **no SvelteKit and no URL routing**. `App.svelte` holds an `activeView` string and renders one of six views: `QueryView`, `FixView`, `DdlView`, `HistoryFixView`, `ConnectionsView`, `SettingsView`.

Each view has a matching store in `src/stores/` (e.g. `fixViewState.ts`) that holds all reactive state for that view. Stores use plain Svelte `writable()`.

### UX state pattern

All async operations follow the same pattern:

```ts
setBusy(true, "Loading…");
try {
  const result = await api.someCommand(…);
  // update store
  notify("Done", "ok");
} catch (e) {
  notify(String(e), "error");
} finally {
  setBusy(false);
}
```

`notify()` and `setBusy()` are from `src/stores/notification.ts`. `StatusBar.svelte` and `BusyOverlay.svelte` consume these stores.

### Key components

- **`VirtualTable.svelte`** — TanStack Virtual v3 row virtualization. Used everywhere results are displayed. Supports sorting, column resizing, and row selection.
- **`SqlEditor.svelte`** — CodeMirror 6 with SQL language support. Theme switches via a Compartment (dark/light).
- **`Modal.svelte`** — Generic overlay; `ServerSelectorModal.svelte` is a multi-select server picker built on top.

### Theming

Dark theme by default via CSS custom properties on `:root`. Light theme overrides via `[data-theme="light"]`. All colors (`--bg-*`, `--text-*`, `--btn-*`, `--vt-*`, etc.) are CSS variables defined in `src/app.css`. The toggle store in `src/hooks/useTheme.ts` persists the choice to `localStorage`.

Custom Tailwind colors: `ember` (#b54f2e), `steel` (#255f85). Custom fonts: Chakra Petch (display), Manrope (body).

## Setup Requirements

- **Oracle Instant Client** must be installed on the machine for Oracle connections; its path is configurable from SettingsView and stored in SQLite. Not needed for Postgres-only use — `tokio-postgres` has no native client dependency.
- Rust toolchain + Tauri CLI are required to compile the backend.
- Node 18+ for the frontend.
