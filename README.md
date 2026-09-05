# Schemetry

Desktop database tooling: schema comparison, DDL generation, SQL querying, and fix script generation.

## Purpose

Built for multitenant setups where each tenant runs on its own database (server) rather than a shared schema. Schemetry lets you query and compare those per-tenant databases against each other to spot drift, and generate idempotent fix scripts to bring them back in sync.

## Tech stack

- **UI:** Svelte 5 + TypeScript (Tauri frontend)
- **Build tool:** Vite, with PostCSS + Autoprefixer for the CSS pipeline
- **Desktop shell:** Tauri v2 (plus `tauri-plugin-dialog` and `tauri-plugin-shell`)
- **Styling:** Tailwind CSS v4 + custom CSS properties
- **Virtual table:** TanStack Virtual v3
- **SQL editor:** CodeMirror 6 (SQL language support, One Dark + GitHub themes for dark/light mode)
- **Oracle DB:** `oracle` crate (rust-oracle, OCI-based)
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
4. Oracle Instant Client (Windows): required by the `oracle` crate (OCI)

## Run

```powershell
npm install
npm run tauri dev
```

## Build

```powershell
npm run tauri build
```

## Tests

```powershell
# Rust unit tests
cd src-tauri
cargo test

# Integration tests (real Oracle via Docker + Flyway)
cd docker
docker compose up -d --build
docker compose ps   # wait for both oracle-* services to report "healthy"

cd ../src-tauri
$env:ORACLE_CLIENT_LIB_DIR = "<path to instantclient>"  # if not already on PATH
cargo test --test oracle_integration -- --ignored

# Teardown
cd ../docker
docker compose down -v
```

## Views

| View | Description |
|---|---|
| **Query** | Run SQL against one or more Oracle connections simultaneously. View results per server, export to XLSX. |
| **Generate DDL** | Fetch and display DDL for selected schema objects, and save it to disk as source and/or versioned migration files (timestamped or Flyway-style naming, configurable per schema). |
| **Fix Discrepancies** | Compare schema metadata across servers against a reference. Filter and select discrepancies, then generate idempotent SQL fix scripts. |
| **Fix History Tables** | Generate fix scripts specifically for history tables if they differ from main tables. |
| **Connections** | Manage named Oracle connection groups. Passwords stored securely in the OS keychain. |
| **Settings** | Configure Oracle Instant Client path. |

## License

MIT — see [LICENSE](LICENSE).
