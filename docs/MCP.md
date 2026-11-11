# MCP server (schemetry-mcp)

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
| `list_schema_objects` | `allow_schema_list` | Only the policy's hidden objects are applied; the compare table-name filters are not. |
| `get_object_ddl` | `allow_ddl_read` | DDL only, never data. Hidden objects are refused. Package types are explicit: `PACKAGE` returns the **spec only**, `PACKAGE BODY` the body only, and on Oracle `PACKAGE SPEC AND BODY` both as one script (spec, a `/` line, body, a `/` line - the `.pck` convention). The response's `includes` (`["spec"]`, `["body"]` or `["spec","body"]`) says which parts are present. The combined type is refused on PostgreSQL, and hidden if either half is hidden by policy. |
| `list_invalid_objects`, `get_object_errors` | `allow_invalid_objects` (off by default; also needs `allow_schema_list`, and `allow_ddl_read` for `get_object_errors`) | Oracle: objects with `STATUS = 'INVALID'` and their compile errors/warnings (line, position, message) so an agent can fix code and verify it compiles. Bounded; hidden objects are omitted/refused. PostgreSQL returns a "not applicable" result. |
| `save_migration_script` | `allow_save_scripts` (plus `allow_ddl_read` for `source: {"server": true}`) | Uses the app's folder/naming rules; confined to the schema folder; `dry_run` previews. Names are restricted to letters, digits and `_ - $ #`. The schema is derived from the connection. Give exactly one of `ddl` (inline, <= 1 MB) or `source`, so large DDL need not pass through the agent: `{"server": true}` saves the DDL fetched from the connection (same as `get_object_ddl`, idempotency-wrapped like inline `ddl`); `{"file": "relative/path.sql"}` copies a file from the schema's folder **byte-for-byte** (no re-encoding or wrapping; <= 8 MB, text only) into the code/migration files. File paths must be relative; absolute paths, drive letters, `..` and symlinks resolving outside the schema folder are refused. The response never echoes file content: each file is described by `kind`, `path`, `overwrites_existing`, `size` (bytes), `sha256`, `lines`, `encoding` (the encoding it is/will be written in), `line_ending` (`CRLF`/`LF`/`mixed`/`none`) and, when it already exists, a `diff` against the on-disk file (`added`/`removed` line counts, `identical`, plus the existing file's size, sha256 and line ending; line endings are ignored when counting). A dry run may pass `include_content: true` to also get the decoded content of files up to 64 KB (larger ones report `content_omitted`). The audit entry records the source kind, byte count and sha256, never the content. `PACKAGE SPEC AND BODY` (Oracle only) saves spec and body into one file and requires `ddl` to contain both `CREATE PACKAGE` and `CREATE PACKAGE BODY`; it uses the **PACKAGE** extension/storage settings (`.pck` by default), while `PACKAGE` and `PACKAGE BODY` save separately (`.pck` / `.pkb`). **Overwrite protection:** an existing target file with uncommitted git changes (modified, staged, untracked or ignored) is never overwritten - the call is denied and audited - unless `force: true` is passed; if git cannot answer inside a repository the overwrite is refused too. Files that are clean in git are overwritten (history keeps them), and outside a git repo the save proceeds with `git: "unknown"`. A dry run adds `git_status` and `would_be_blocked` per file, and with `include_content` a `diff_preview` of what would be lost. `force` and the refusal reason are recorded in the audit entry (never file content). `description` is optional (the object name is used in the file name when the naming template has no `{object}`). Encoding, line endings and the file-name template are set per schema folder; a save is refused if the text can't be represented in the folder's encoding. |
| `compare_schemas`, `generate_fix_script` | `allow_compare_fix` on **every** involved connection | Fix scripts are returned, never executed. |
| `compare_object` | `allow_compare_object` **and** `allow_ddl_read` (off by default) | Compares a local DDL file with one object's server DDL **inside Schemetry** — the file is not returned whole, only the differing lines appear in the diff, and only DDL source extensions (`.sql`, `.pck`, `.pks`, `.pkb`, `.prc`, `.fnc`, `.trg`, `.vw`, `.tps`, `.tpb`, `.pls`, `.plb`, `.ddl`) can be named. `file_path` is relative to the schema's folder (configured schema root + schema, or the schema's folder override); absolute paths, `..` and symlinks leaving the folder are refused, and files over 1 MB are rejected. The file is decoded with the folder's configured encoding. Both sides are normalised (line endings, trailing whitespace, blank edges, a final `;`; for Oracle also a trailing `/` line and the `CREATE` header: keyword case/spacing, `OR REPLACE`, `EDITIONABLE`, own-schema prefix, quoted upper-case names — see `ddl_compare::normalize_ddl`). Returns `{"identical": true}` or a unified diff (`-` server, `+` file; capped at 400 lines inline, at 100,000 lines when it goes to a file) with added/removed/hunk counts. Hidden objects are refused, and so is any file whose text mentions a hidden object. The audit entry holds the path, result and diff line counts — never file or DDL content. |
| `query_table` | `data_access = allowlist` | Structured columns / filters / ordering — no SQL text. Table and column rules are enforced exactly. |
| `run_query` | `data_access = allowlist` **and** `allow_raw_sql` | One read-only `SELECT`, see the caveat below. |
| `grep_result`, `read_result` | the permission of the tool that wrote the file, on every connection it came from | Search (`grep -n -C` style, literal and case-insensitive by default, `regex: true` for a regular expression) or read numbered line ranges of a result file by its `file.id`. See *Large results* below. |
| `apply_ddl` | `allow_apply_ddl` (**off** by default, also for connections that existed before it) | Deploys one `CREATE [OR REPLACE]` to a dev/test database. See *Deploying DDL* below. |

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

### Large results

`get_object_ddl`, `generate_fix_script`, `compare_object`, `compare_schemas`, `query_table` and `run_query` take an optional `output`: `auto` (default), `inline` or `file`. In `auto` mode a result over **16 KB** is written to a temp file instead of being returned inline, so a large package body or result set does not fill the agent's context. The response then carries:

```json
"output": "file",
"file": { "id": "r_…", "path": "…/schemetry-mcp/<pid>-<nonce>/r_…-get_object_ddl-ORDER_API.sql",
          "format": "sql", "bytes": 412345, "lines": 9812, "sha256": "…", "expires_in_secs": 7200 },
"preview": "<first 40 lines>",
"outline": [{ "line": 2, "kind": "PROCEDURE", "name": "STEP_0" }, …],
"hint": "…"
```

- **SQL** (`get_object_ddl`, `generate_fix_script`) gets a `preview` and an `outline`: where each `CREATE`/`ALTER TABLE` statement, PL/SQL procedure/function and fix-script table section starts (up to 500 items). The fix script is written **whole**; inline it is still cut at 500 KB.
- **Diffs** (`compare_object`) are written as `.diff` with the counts inline.
- **Rows** (`query_table`, `run_query`) are written as JSON Lines: `{"columns":[…]}` then one JSON array per row (values escaped, so each row is one greppable line), with the first 20 rows inline. The policy's `max_rows` still applies; only the 1 MB byte cap rises to the 50 MB file cap.
- **Discrepancies** (`compare_schemas`) are written as JSON Lines with the first 20 inline and a `summary` (counts per difference and for the top 50 tables).

`inline` keeps the old behaviour and caps; `get_object_ddl` refuses DDL over 1 MB inline and asks for `output: "file"`. The agent can read the file with its own tools (e.g. Claude Code's Grep/Read on `file.path`) or through `grep_result` / `read_result`, which work for clients without file access. These two recheck the policy of the tool that produced the file (and the global kill switch for data), so revoking a permission stops further reads through MCP at once. Each call is audited with the producing call's target plus `result_id` and `source_tool`; the producing call's audit entry records the file's id, size, line count and sha256, never its content.

Files live in a per-process folder under the OS temp directory (`0700`/`0600` on Unix; the per-user temp folder on Windows), expire after **2 hours**, are capped at 200 files / 256 MB per process (oldest removed first), and are deleted when `schemetry-mcp` exits normally. Folders older than 24 hours left by a killed or crashed process are removed the next time the server starts.

**LOB cells as files.** By default `query_table` and `run_query` show LOB cells as `<BLOB>` / `<CLOB>` placeholders (Postgres `bytea` as cut hex) and cut other cells at 4,000 characters. With `lob_files: true`, each such cell — Oracle `BLOB`, `CLOB`, `NCLOB`, `LONG`, `LONG RAW`, `BFILE`, Postgres `bytea`, and any text cell over 4,000 characters — is read whole and written to its own result file, named `<id>-<tool>[-<table>]-<column>-row<n>.<ext>` with the type guessed from its content (`pdf`, `png`, `jpg`, `gif`, `webp`, `docx`, `xlsx`, `pptx`, `zip`, `gz`, `svg`, `html`, `xml`, `json`, `txt`, else `bin`). The cell then reads `<BLOB file r_…>` (or `CLOB`, `BYTEA`, `TEXT`), and the response lists every file in `lob_files`: `row_index`, `column`, `source_type`, `id`, `path`, `mime`, `binary`, `bytes`, `sha256`, plus a 200-character `preview` for text. Per call at most 50 cells, 32 MB per cell and 64 MB in total are read; cells over a limit keep their placeholder and are listed in `lob_files_skipped` with the reason (an Oracle LOB over the limit is read only one byte past it, never in full). Column rules apply as usual — a LOB column must be readable to be selected. Text LOB files can be searched with `grep_result` / `read_result`; binary ones (images, PDF, Office, …) are refused there and are meant to be opened from `file.path` with the agent's own tools. The audit entry lists each file's row, column, type, size and sha256, never content.

**Caveat:** for `query_table` / `run_query`, table data (including LOB files) sits in plain text in the user's temp folder until the file expires or the process exits. Once the agent has the path, its own file reads are outside Schemetry's audit log — the same as data returned inline, which the agent can keep too. Use `output: "inline"` and leave `lob_files` off for data that should not touch disk.

### Deploying DDL (`apply_ddl`)

`apply_ddl(connection_id, object_name, object_type, ddl | file)` lets an agent deploy a fixed object to a **dev/test** database and then check the result (Oracle: the object's `ALL_ERRORS` are returned with the response). It is deliberately narrow:

- **What is accepted.** Exactly one `CREATE [OR REPLACE]` of the declared `object_type` and `object_name`, in the connection's own schema (an unqualified name or `own_schema.name`). Oracle: `VIEW`, `PROCEDURE`, `FUNCTION`, `PACKAGE`, `PACKAGE BODY`, `TRIGGER`, `TYPE`, `TYPE BODY`; a trailing `/` is fine, and a package spec plus body (two statements, both `CREATE OR REPLACE`, same name, declared as `PACKAGE`) is allowed. Postgres: `VIEW`, `FUNCTION`, `PROCEDURE`, `TRIGGER`. Anything else — `DROP`, `TRUNCATE`, `DELETE`, `GRANT`, `ALTER …`, a second statement, tables/indexes/users/links, another schema, `LANGUAGE c`/untrusted Postgres languages, database-level or other-schema triggers — is refused before the database is touched. Validation is token-based (comments, quoted identifiers, `'…'`, Oracle `q'[…]'` and Postgres `$tag$…$tag$` are understood), so keywords inside literals don't count. Postgres `BEGIN ATOMIC` bodies and `E'…'` strings are not supported; SQL*Plus commands (`SET`, `SHOW ERRORS`, …) must be left out.
- **Source.** `ddl` text, or `file`: a path relative to the connection's schema folder (symlinks and `..` can't leave it). Exactly one of the two. Limit 1 MB.
- **Execution.** On the connection's own database account (not the dedicated read account), with a timeout; Postgres runs in a transaction with `search_path` set to the schema. Oracle reports compile errors from `ALL_ERRORS` for the object.
- **Approval (`apply_ddl_requires_approval`, default on).** MCP over stdio has no channel to the human, so approval happens in the app: the first call only files a *pending request* (statement type, object, size, sha256 — and the SQL, so you can read what you approve) and returns `status: "pending"` with an approval id. You review it under **MCP Audit → Pending deploys** and click Approve (or Reject); the agent then repeats the identical call. An approval is bound to (connection, object type, object, sha256 of the normalised DDL), is **single use**, and lapses after 15 minutes (a request left unanswered lapses after an hour). Editing the script, changing the object, or switching the policy flag off invalidates it. Only the GUI can approve; the MCP process has no approve path.
- **Audit.** One entry per call (including denials and the pending step): object, sha256, byte size, approval id, status, compile-error count — never the SQL itself. The requested SQL lives only in the `mcp_apply_approvals` table until it is consumed or rejected, and expired rows are purged after a week.

Trade-offs: the validator keeps an agent to "deploy this one object", but a deployed procedure can of course do whatever its owner may — the real boundary is a dev/test account with only the privileges you accept, plus your review of the SQL. Approvals live in the same SQLite file as the policies, so anyone who can write that file could forge one; that is the same trust level as editing the policy itself. With approval switched off the deploy runs on the first call — don't, except on a throwaway database.

### Audit log

Every tool call — allowed, denied or failed — appends one entry (time, tool, connection, arguments, outcome and reason, rows/bytes returned, duration, MCP client name) to `mcp_audit_log`. Entries are hash-chained (`hash = sha256(prev_hash ‖ entry)`), SQLite triggers reject `UPDATE`/`DELETE`, and nothing in Schemetry can edit or purge the log. Schemetry verifies the chain on startup and shows a warning banner if it is broken or truncated; the **MCP Audit** view filters by connection / tool / outcome / table or object / date and exports CSV.

Each entry also records *what the call touched* under `target` in its arguments JSON (so it is covered by the hash and old entries verify unchanged): the table(s) for `query_table` and `run_query` (taken from the SQL guard's parsed table list), plus selected columns, filter count, row limit and truncation flag for `query_table`; object type and name for DDL/migration tools; tables or connections for compare/fix tools. It appears as the **Target** column, in the CSV export, and as a top-level `target` text in the external sink payload.

On a machine you own no local file is truly immutable: the guarantee is *no cleanup path in the app* plus *tampering is detectable*. For a copy outside your control, forward each entry to an external sink in **Settings** (syslog over UDP or `/dev/log`, Windows Event Log, or an HTTP endpoint). If the audit log can't be written, the call's result is withheld.

