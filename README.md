# Schemetry

[![CI](https://github.com/A1nar-S/schemetry/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/A1nar-S/schemetry/actions/workflows/ci.yml)
[![Security](https://github.com/A1nar-S/schemetry/actions/workflows/security.yml/badge.svg?branch=main)](https://github.com/A1nar-S/schemetry/actions/workflows/security.yml)
[![codecov](https://codecov.io/gh/A1nar-S/schemetry/graph/badge.svg)](https://codecov.io/gh/A1nar-S/schemetry)
[![Latest release](https://img.shields.io/github/v/release/A1nar-S/schemetry?sort=semver)](https://github.com/A1nar-S/schemetry/releases/latest)
[![License: MIT](https://img.shields.io/github/license/A1nar-S/schemetry)](LICENSE)
![Platform: Windows](https://img.shields.io/badge/platform-Windows-0078D6)
[![Built with Tauri](https://img.shields.io/badge/built%20with-Tauri%20v2-24C8DB?logo=tauri&logoColor=white)](https://tauri.app)

Desktop database tooling: schema comparison, DDL generation, SQL querying, and fix script generation. Currently supports Oracle and PostgreSQL.

## Purpose

Built for multitenant setups where each tenant runs on its own database (server) rather than a shared schema. Schemetry lets you query and compare those per-tenant databases against each other to spot drift, and generate idempotent fix scripts to bring them back in sync. Each connection is either Oracle or PostgreSQL; schema comparison and fix generation only run across connections of the same engine.

## Tech stack

- **UI:** Svelte 5 + TypeScript (Tauri frontend)
- **Build tool:** Vite, with PostCSS + Autoprefixer for the CSS pipeline
- **Desktop shell:** Tauri v2 (plus `tauri-plugin-dialog` and `tauri-plugin-shell`)
- **Styling:** Tailwind CSS v4 + custom CSS properties
- **Virtual table:** TanStack Virtual v3
- **SQL editor:** CodeMirror 6 (SQL language support, One Dark + GitHub themes for dark/light mode)
- **Oracle DB:** `oracle` crate (rust-oracle, OCI-based)
- **PostgreSQL DB:** `tokio-postgres` (pure-Rust wire-protocol client, no native client library needed)
- **Local storage:** SQLite via `rusqlite` (connections, query history, settings)
- **Credential storage:** OS keychain via `keyring` crate
- **DataFrame / CSV:** Polars
- **Async runtime:** Tokio
- **XLSX export:** `rust_xlsxwriter`
- **IPC/serialization:** `serde` / `serde_json`
- **Utilities (Rust):** `chrono` (dates), `anyhow` (errors), `regex`, `base64`, `encoding_rs`, `windows-sys` (Win32 APIs)

## Prerequisites

1. Install Rust toolchain (`cargo`, `rustc`) via [rustup](https://rustup.rs/)
2. Install Node.js (LTS) and npm
3. Install Tauri CLI: `cargo install tauri-cli`
4. Oracle Instant Client (Windows): required by the `oracle` crate (OCI) for Oracle connections. Not needed for PostgreSQL-only use.

## Run

```powershell
npm install
npm run tauri dev
```

## Repository layout

Cargo workspace at the repo root:

| Crate | Path | Purpose |
|---|---|---|
| `schemetry-core` | [core/](core) | Models, repositories, services, SQLite bootstrap, MCP policy/audit/gateway. No GUI dependencies. |
| `schemetry` | [src-tauri/](src-tauri) | The Tauri desktop app (commands + shell). |
| `schemetry-mcp` | [mcp/](mcp) | The headless stdio MCP server (protocol wrapper over the core gateway). |

## Build

```powershell
npm run tauri build          # desktop app
cargo build --release -p schemetry-mcp   # headless MCP server only
```

## Tests

```powershell
# Rust unit tests (all crates)
cargo test --workspace

# Integration tests (real Oracle via Docker + Flyway)
cd docker
docker compose up -d --build
docker compose ps   # wait for both oracle-* services to report "healthy"

cd ..
$env:ORACLE_CLIENT_LIB_DIR = "<path to instantclient>"  # if not already on PATH
cargo test -p schemetry-core --test oracle_integration -- --ignored

# Integration tests (real PostgreSQL via Docker + Flyway)
# Same docker-compose file also starts the postgres-* containers
cd docker
docker compose ps   # wait for both postgres-* services to report "healthy"

cd ..
cargo test -p schemetry-core --test postgres_integration -- --ignored
cargo test -p schemetry-core --test mcp_integration -- --ignored   # MCP gateway end to end

# Teardown
cd docker
docker compose down -v
```

## MCP server (`schemetry-mcp`)

A stdio [MCP](https://modelcontextprotocol.io) server that lets AI agents (Claude Code, VS Code, …) list schema objects, read DDL, save migration scripts, compare schemas, generate fix scripts and (if you allow it) read table data — using the same connections, keychain passwords and folder rules as the GUI. It works without the GUI running, on Windows and Linux. **Agents get nothing by default:** every connection is opted in explicitly, per capability, in the connection's *MCP* policy.

### Install and configure

- The Windows installer puts `schemetry-mcp.exe` next to `Schemetry.exe`; releases also attach a standalone Windows `.exe` and a Linux x64 binary.
- Build it yourself (no WebView/GTK libraries needed, so it works on a bare Ubuntu server):

  ```bash
  cargo build --release -p schemetry-mcp
  # -> target/release/schemetry-mcp   (schemetry-mcp.exe on Windows)
  ```

- Database location: `--db <path>`, else `SCHEMETRY_DB`, else the OS app-data dir (the file the GUI uses). Connection passwords come from the OS keychain: Windows Credential Manager, or the Secret Service (gnome-keyring / KeePassXC) over D-Bus on Linux — a headless Linux box needs a running, unlocked Secret Service for passwords to resolve.
- Client configuration:

  ```json
  { "mcpServers": { "schemetry": { "command": "/path/to/schemetry-mcp", "args": ["--db", "/path/to/schemetry.db"] } } }
  ```

  On Windows use e.g. `"command": "C:\\path\\to\\schemetry-mcp.exe"`. stdout is reserved for the protocol; logs go to stderr.

### Tools

| Tool | Needs (in the connection's policy) | Notes |
|---|---|---|
| `list_connections` | connection has MCP enabled | id, name, engine, schema and the effective policy, including why each tool is or is not available (see below). Never passwords or hosts. |
| `list_schema_objects` | `allow_schema_list` | Active compare filter rules and the policy's hidden objects are applied. |
| `get_object_ddl` | `allow_ddl_read` | DDL only, never data. Hidden objects are refused. Package types are explicit: `PACKAGE` returns the **spec only**, `PACKAGE BODY` the body only, and on Oracle `PACKAGE SPEC AND BODY` both as one script (spec, a `/` line, body, a `/` line — the `.pck` convention). The response's `includes` (`["spec"]`, `["body"]` or `["spec","body"]`) says which parts are present. The combined type is refused on PostgreSQL, and hidden if either half is hidden by policy. |
| `list_invalid_objects`, `get_object_errors` | `allow_invalid_objects` (off by default; also needs `allow_schema_list`, and `allow_ddl_read` for `get_object_errors`) | Oracle: objects with `STATUS = 'INVALID'` and their compile errors/warnings (line, position, message) so an agent can fix code and verify it compiles. Bounded; hidden objects are omitted/refused. PostgreSQL returns a "not applicable" result. |
| `save_migration_script` | `allow_save_scripts` | Uses the app's folder/naming rules; confined to the schema folder; `dry_run` previews. Names are restricted to letters, digits and `_ - $ #`. The schema is derived from the connection. `PACKAGE SPEC AND BODY` (Oracle only) saves spec and body into one file and requires `ddl` to contain both `CREATE PACKAGE` and `CREATE PACKAGE BODY`; it uses the **PACKAGE** extension/storage settings (`.pck` by default), while `PACKAGE` and `PACKAGE BODY` save separately (`.pck` / `.pkb`). |
| `compare_schemas`, `generate_fix_script` | `allow_compare_fix` on **every** involved connection | Fix scripts are returned, never executed. |
| `compare_object` | `allow_compare_object` **and** `allow_ddl_read` (off by default) | Compares a local DDL file with one object's server DDL **inside Schemetry** — the file is not returned whole, only the differing lines appear in the diff, and only DDL source extensions (`.sql`, `.pck`, `.pks`, `.pkb`, `.prc`, `.fnc`, `.trg`, `.vw`, `.tps`, `.tpb`, `.pls`, `.plb`, `.ddl`) can be named. `file_path` is relative to the schema's folder (configured schema root + schema, or the schema's folder override); absolute paths, `..` and symlinks leaving the folder are refused, and files over 1 MB are rejected. The file is decoded with the folder's configured encoding. Both sides are normalised (line endings, trailing whitespace, blank edges, a final `;`; for Oracle also a trailing `/` line and the `CREATE` header: keyword case/spacing, `OR REPLACE`, `EDITIONABLE`, own-schema prefix, quoted upper-case names — see `ddl_compare::normalize_ddl`). Returns `{"identical": true}` or a unified diff (`-` server, `+` file, capped at 400 lines) with added/removed/hunk counts. Hidden objects are refused, and so is any file whose text mentions a hidden object. The audit entry holds the path, result and diff line counts — never file or DDL content. |
| `query_table` | `data_access = allowlist` | Structured columns / filters / ordering — no SQL text. Table and column rules are enforced exactly. |
| `run_query` | `data_access = allowlist` **and** `allow_raw_sql` | One read-only `SELECT`, see the caveat below. |

Policy options (edit them in **Connections → MCP**): per-capability toggles, a table/column allow/deny list with `*`/`?` wildcards (deny wins; Oracle matches case-insensitively, Postgres allow-rules are case-sensitive), hidden objects, `max_rows` (default 200), `query_timeout_secs` (default 30), and an optional dedicated database account. **Test as MCP** shows what an agent would see. A global kill switch in **Settings** disables all data tools regardless of policy.

**Why is a tool unavailable?** Each connection's `policy` in `list_connections` keeps the flat fields (`list_schema_objects`, `get_object_ddl`, `save_migration_script`, `compare_and_fix`, `data_access`, `run_query`, `table_rules`, limits) and adds `tools`: one entry per MCP tool, generated from the tool list so new tools appear automatically.

```json
"tools": {
  "get_object_ddl":        { "allowed": true },
  "save_migration_script": { "allowed": false, "reason_code": "policy_disabled",
                            "reason": "disabled in this connection's MCP policy (allow_save_scripts=off)" },
  "query_table":           { "allowed": false, "reason_code": "data_access_none",
                            "reason": "data_access is none in this connection's MCP policy (needs allowlist)" }
}
```

`reason_code` is one of `policy_disabled`, `data_access_none`, `kill_switch`, `table_allowlist_empty`. The policy and the response also carry `data_tools_disabled_globally` (the global kill switch state) and a one-line `hint` telling the user where to enable a tool (Connections > MCP policy). Reasons name settings only, never table rules, accounts or hosts.

**Table data and raw SQL.** `query_table` builds the SQL itself, so its allowlist is exact. `run_query` is best-effort: statements are checked against the allowlist (every referenced table must exist in the connection's own schema, so catalog views are out of reach), and run in a read-only transaction with a row cap and timeout, but views, synonyms, functions that read tables internally and dynamic SQL can't be seen statically — this is *not* a hard guarantee. The hard boundary is a **dedicated database account** (set it per connection; its password lives in the OS keychain and is used only for table-data reads) that the DBA grants `SELECT` on the allowed tables only.

**Function allowlist in `run_query`.** Any call-like token in the SQL — `name(…)`, package/schema-qualified calls such as `PKG.FUNC(…)`, Oracle object-type constructors (`DBURITYPE('…')`) and member calls (`….getclob()`) — must be on a curated list of safe scalar, aggregate and window functions for the connection's engine (`COUNT`, `SUM`, `COALESCE`, `NVL`, `UPPER`, `SUBSTR`, `ROUND`, `TO_CHAR`, `EXTRACT`, `DATE_TRUNC`, `ROW_NUMBER`, `LAG`, …; see `core/src/services/mcp_sql_guard.rs`). Everything else is refused, including file, network, XML-query, dynamic-SQL, sequence-advancing and `set_config`/`pg_sleep`-style functions; a short blocklist stays on top as a second barrier. Quoted names are case-sensitive as in the engine (`"count"(…)` is the built-in on Postgres, `"COUNT"(…)` on Oracle). Only the `pg_catalog.` qualifier is accepted on Postgres; qualified calls are refused on Oracle, as are `DBMS_*`/`UTL_*` packages even without parentheses. Parameterless Oracle functions written without parentheses can't be told from columns, so the dedicated account remains the real boundary.

To see which function was refused, look at the `run_query` reason: the error returned to the agent is `Denied: Function <name> is not on the run_query function allowlist for <engine>; …` and the same text is stored as the reason of the `denied` entry in the **MCP Audit** view (the audit arguments hold the full SQL). If a legitimate function is refused, add it to the engine's list in `mcp_sql_guard.rs` after checking it cannot read files, the network, other tables or change state.

### Audit log

Every tool call — allowed, denied or failed — appends one entry (time, tool, connection, arguments, outcome and reason, rows/bytes returned, duration, MCP client name) to `mcp_audit_log`. Entries are hash-chained (`hash = sha256(prev_hash ‖ entry)`), SQLite triggers reject `UPDATE`/`DELETE`, and nothing in Schemetry can edit or purge the log. Schemetry verifies the chain on startup and shows a warning banner if it is broken or truncated; the **MCP Audit** view filters by connection / tool / outcome / table or object / date and exports CSV.

Each entry also records *what the call touched* under `target` in its arguments JSON (so it is covered by the hash and old entries verify unchanged): the table(s) for `query_table` and `run_query` (taken from the SQL guard's parsed table list), plus selected columns, filter count, row limit and truncation flag for `query_table`; object type and name for DDL/migration tools; tables or connections for compare/fix tools. It appears as the **Target** column, in the CSV export, and as a top-level `target` text in the external sink payload.

On a machine you own no local file is truly immutable: the guarantee is *no cleanup path in the app* plus *tampering is detectable*. For a copy outside your control, forward each entry to an external sink in **Settings** (syslog over UDP or `/dev/log`, Windows Event Log, or an HTTP endpoint). If the audit log can't be written, the call's result is withheld.

## Views

| View | Description |
|---|---|
| **Query** | Run SQL against one or more Oracle and/or PostgreSQL connections simultaneously. View results per server, export to XLSX. |
| **Generate DDL** | Fetch and display DDL for selected schema objects, and save it to disk as source and/or versioned migration files (timestamped or Flyway-style naming, configurable per schema). |
| **Fix Discrepancies** | Compare schema metadata across servers of the same database engine against a reference. Filter and select discrepancies, then generate idempotent SQL fix scripts. |
| **Fix History Tables** | Generate fix scripts specifically for history tables if they differ from main tables. |
| **Connections** | Manage named Oracle and PostgreSQL connections, grouped and organized per tenant. Passwords stored securely in the OS keychain. Edit each connection's MCP policy. |
| **MCP Audit** | Read-only, tamper-evident log of everything AI agents did through the MCP server. |
| **Settings** | Configure Oracle Instant Client path (Oracle connections only), the MCP kill switch and the external audit sink. |

## License

MIT — see [LICENSE](LICENSE).
