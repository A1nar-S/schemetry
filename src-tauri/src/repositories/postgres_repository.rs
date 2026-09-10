use std::collections::HashMap;

use anyhow::{anyhow, Result};
use tokio_postgres::types::ToSql;
use tokio_postgres::{Client, NoTls};

use crate::models::{
    ColumnInfo, ConnectionRecord, HistoryFixResult, HistoryNamingRule, HistoryTableIssue,
    MatchType, SchemaObject, TableDdls, TableFilterRule,
};
use crate::repositories::db_repository::{DbRepository, LobCell};
use crate::repositories::filter_rule_repository::{build_predicate, ParamStyle};

/// Cap on materialized LOB-like content (characters of text / bytes decoded), mirroring
/// `oracle_repository::MATERIALIZE_TEXT_CAP`.
const MATERIALIZE_TEXT_CAP: usize = 1_000_000;

/// `tokio-postgres` is a pure async driver, but every caller in this codebase (services,
/// commands) is synchronous — they already bridge into async Tauri commands via
/// `tokio::task::spawn_blocking`, and some fan out further onto plain `std::thread`s
/// with no ambient Tokio runtime. Rather than thread a `Handle` through every call site,
/// this repository owns its own private runtime and blocks on it per call — exactly
/// what the synchronous `postgres` crate does internally.
pub struct DbPostgresRepository {
    runtime: tokio::runtime::Runtime,
}

impl DbPostgresRepository {
    pub fn new() -> Self {
        let runtime = tokio::runtime::Runtime::new().expect("failed to create Postgres runtime");
        Self { runtime }
    }

    /// The schema to introspect/target: `pg_schema` if set, else `public`.
    fn schema_of(conn: &ConnectionRecord) -> String {
        let s = conn.pg_schema.trim();
        if s.is_empty() { "public".to_string() } else { s.to_string() }
    }

    async fn connect(&self, conn: &ConnectionRecord) -> Result<Client> {
        let mut cfg = tokio_postgres::Config::new();
        cfg.host(&conn.host)
            .port(conn.port)
            .user(&conn.username)
            .password(&conn.password)
            .dbname(&conn.service_name)
            .connect_timeout(std::time::Duration::from_secs(15));

        let (client, connection) = cfg.connect(NoTls).await.map_err(|e| anyhow!(e.to_string()))?;
        tokio::spawn(async move {
            let _ = connection.await;
        });
        Ok(client)
    }

    async fn build_table_model(&self, client: &Client, schema: &str, table: &str) -> Result<TableModel> {
        let col_rows = client
            .query(
                "SELECT a.attname, format_type(a.atttypid, a.atttypmod), a.attnotnull, \
                        pg_get_expr(ad.adbin, ad.adrelid) \
                 FROM pg_attribute a \
                 JOIN pg_class c ON c.oid = a.attrelid \
                 JOIN pg_namespace n ON n.oid = c.relnamespace \
                 LEFT JOIN pg_attrdef ad ON ad.adrelid = a.attrelid AND ad.adnum = a.attnum \
                 WHERE n.nspname = $1 AND c.relname = $2 \
                   AND a.attnum > 0 AND NOT a.attisdropped \
                 ORDER BY a.attnum",
                &[&schema, &table],
            )
            .await
            .map_err(|e| anyhow!(e.to_string()))?;

        let mut columns = Vec::with_capacity(col_rows.len());
        for r in &col_rows {
            columns.push(ColumnDef {
                name: r.get(0),
                type_str: r.get(1),
                not_null: r.get(2),
                default: r.get::<_, Option<String>>(3).map(|d| d.trim().to_string()).filter(|d| !d.is_empty()),
            });
        }
        if columns.is_empty() {
            return Err(anyhow!("Table {} not found in schema {}", table, schema));
        }

        let table_comment: Option<String> = client
            .query_opt(
                "SELECT obj_description(c.oid) FROM pg_class c \
                 JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE n.nspname = $1 AND c.relname = $2",
                &[&schema, &table],
            )
            .await
            .ok()
            .flatten()
            .and_then(|row| row.get::<_, Option<String>>(0))
            .filter(|s| !s.trim().is_empty());

        let comment_rows = client
            .query(
                "SELECT a.attname, col_description(c.oid, a.attnum) \
                 FROM pg_attribute a \
                 JOIN pg_class c ON c.oid = a.attrelid \
                 JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE n.nspname = $1 AND c.relname = $2 \
                   AND a.attnum > 0 AND NOT a.attisdropped \
                   AND col_description(c.oid, a.attnum) IS NOT NULL",
                &[&schema, &table],
            )
            .await
            .unwrap_or_default();
        let mut comment_map: HashMap<String, String> = HashMap::new();
        for r in &comment_rows {
            let col: String = r.get(0);
            if let Some(text) = r.get::<_, Option<String>>(1).filter(|s| !s.trim().is_empty()) {
                comment_map.insert(col, text.trim().to_string());
            }
        }
        let column_comments: Vec<(String, String)> = columns
            .iter()
            .filter_map(|c| comment_map.get(&c.name).map(|t| (c.name.clone(), t.clone())))
            .collect();

        // Indexes not backing a constraint. `pg_get_indexdef(indexrelid, col_no, true)`
        // returns just that key column's expression, so this also handles
        // expression/function-based indexes without extra parsing.
        let index_rows = client
            .query(
                "SELECT ic.relname, ix.indisunique, ix.indexrelid, ix.indnkeyatts \
                 FROM pg_index ix \
                 JOIN pg_class ic ON ic.oid = ix.indexrelid \
                 JOIN pg_class tc ON tc.oid = ix.indrelid \
                 JOIN pg_namespace n ON n.oid = tc.relnamespace \
                 WHERE n.nspname = $1 AND tc.relname = $2 \
                   AND NOT EXISTS (SELECT 1 FROM pg_constraint co WHERE co.conindid = ix.indexrelid) \
                 ORDER BY ic.relname",
                &[&schema, &table],
            )
            .await
            .unwrap_or_default();

        let mut indexes = Vec::new();
        for r in &index_rows {
            let name: String = r.get(0);
            let unique: bool = r.get(1);
            let index_oid: u32 = r.get(2);
            let nkeys: i16 = r.get(3);
            let mut parts = Vec::with_capacity(nkeys as usize);
            for pos in 1..=nkeys {
                if let Ok(Some(row)) = client
                    .query_opt(
                        "SELECT pg_get_indexdef($1::oid, $2::int, true)",
                        &[&index_oid, &(pos as i32)],
                    )
                    .await
                {
                    if let Some(expr) = row.get::<_, Option<String>>(0) {
                        parts.push(expr);
                    }
                }
            }
            indexes.push(IndexDef { name, unique, parts });
        }

        // Constraints: pg_get_constraintdef already returns a ready-to-use definition
        // (`PRIMARY KEY (id)`, `FOREIGN KEY (x) REFERENCES y(z) ON DELETE CASCADE`,
        // `CHECK ((salary > 0))`), unlike Oracle where this has to be assembled by hand.
        let cons_rows = client
            .query(
                "SELECT co.conname, pg_get_constraintdef(co.oid) \
                 FROM pg_constraint co \
                 JOIN pg_class c ON c.oid = co.conrelid \
                 JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE n.nspname = $1 AND c.relname = $2 \
                 ORDER BY CASE co.contype WHEN 'p' THEN 1 WHEN 'u' THEN 2 WHEN 'f' THEN 3 WHEN 'c' THEN 4 ELSE 5 END, co.conname",
                &[&schema, &table],
            )
            .await
            .unwrap_or_default();
        let constraints = cons_rows
            .iter()
            .map(|r| ConstraintDef { name: r.get(0), def: r.get(1) })
            .collect();

        Ok(TableModel {
            name: table.to_string(),
            columns,
            table_comment,
            column_comments,
            indexes,
            constraints,
        })
    }
}

struct ColumnDef {
    name: String,
    type_str: String,
    not_null: bool,
    default: Option<String>,
}

struct IndexDef {
    name: String,
    unique: bool,
    parts: Vec<String>,
}

struct ConstraintDef {
    name: String,
    /// Full `pg_get_constraintdef()` output, e.g. `PRIMARY KEY (id)`.
    def: String,
}

struct TableModel {
    name: String,
    columns: Vec<ColumnDef>,
    table_comment: Option<String>,
    column_comments: Vec<(String, String)>,
    indexes: Vec<IndexDef>,
    constraints: Vec<ConstraintDef>,
}

fn escape_sql(s: &str) -> String {
    s.replace('\'', "''")
}

/// Render a readable `create table (...)` + `comment on ...` + `create index ...` +
/// `alter table ... add constraint ...` block. There's no Postgres equivalent of
/// `DBMS_METADATA.GET_DDL`, so — like the Oracle catalog-based table path — this is
/// assembled by hand from `pg_catalog`/`information_schema`.
fn render_table(m: &TableModel) -> String {
    let mut out = format!("create table {} (\n", m.name);
    for (i, c) in m.columns.iter().enumerate() {
        let mut line = format!("  {} {}", c.name, c.type_str);
        if let Some(d) = &c.default {
            line.push_str(&format!(" default {d}"));
        }
        if c.not_null {
            line.push_str(" not null");
        }
        if i + 1 < m.columns.len() {
            line.push(',');
        }
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str(");");

    if let Some(tc) = &m.table_comment {
        out.push_str(&format!("\n\ncomment on table {} is '{}';", m.name, escape_sql(tc)));
    }
    for (col, text) in &m.column_comments {
        out.push_str(&format!(
            "\ncomment on column {}.{} is '{}';",
            m.name,
            col,
            escape_sql(text)
        ));
    }

    for idx in &m.indexes {
        let kw = if idx.unique { "create unique index" } else { "create index" };
        out.push_str(&format!("\n{} {} on {} ({});", kw, idx.name, m.name, idx.parts.join(", ")));
    }

    for c in &m.constraints {
        out.push_str(&format!(
            "\nalter table {} add constraint {} {};",
            m.name, c.name, c.def
        ));
    }

    out
}

/// A short, human-readable Postgres type label for a column, mirroring
/// `oracle_repository::type_label`'s role (feeds the UI's LOB-flagging/tooltips).
fn type_label(pg_type: &str) -> String {
    pg_type.to_ascii_uppercase()
}

#[derive(Clone, Copy)]
enum CellKind {
    Text,
    /// `bytea` — Postgres's only real binary-LOB-like type.
    Binary,
}

fn cell_kind(pg_type_name: &str) -> CellKind {
    match pg_type_name {
        "bytea" => CellKind::Binary,
        _ => CellKind::Text,
    }
}

fn cap_chars(s: String, cap: usize) -> String {
    if s.chars().count() > cap {
        s.chars().take(cap).collect()
    } else {
        s
    }
}

fn bytes_to_hex(bytes: &[u8], max: usize) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len().min(max) * 2 + 16);
    for b in bytes.iter().take(max) {
        let _ = write!(s, "{b:02X}");
    }
    if bytes.len() > max {
        let _ = write!(s, "… ({} bytes)", bytes.len());
    }
    s
}

/// Decode Postgres's `simple_query` text-format `bytea` representation
/// (`\x`-prefixed hex, e.g. `\xdeadbeef`) into raw bytes.
fn decode_bytea_hex(text: &str) -> Vec<u8> {
    let hex = text.strip_prefix("\\x").unwrap_or(text);
    let mut out = Vec::with_capacity(hex.len() / 2);
    let bytes = hex.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if let Ok(b) = u8::from_str_radix(&hex[i..i + 2], 16) {
            out.push(b);
        }
        i += 2;
    }
    out
}

/// Split a generated raw table-DDL block (see [`render_table`]) into individual
/// statements. A new statement begins at a line that starts (column 0) with a known
/// DDL keyword; everything else continues the current statement — reliable here
/// because `render_table` generates this text itself.
fn split_statements(raw: &str) -> Vec<String> {
    const STARTS: &[&str] = &["create table", "create unique index", "create index", "comment on", "alter table"];
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for line in raw.lines() {
        let lower = line.to_ascii_lowercase();
        let is_start = STARTS.iter().any(|kw| lower.starts_with(kw));
        if is_start && !cur.trim().is_empty() {
            out.push(cur.trim_end().to_string());
            cur.clear();
        }
        if !cur.is_empty() {
            cur.push('\n');
        }
        cur.push_str(line);
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim_end().to_string());
    }
    out
}

/// Pull the identifier following `keyword` (e.g. the constraint name after
/// `add constraint`).
fn token_after(stmt: &str, keyword: &str) -> Option<String> {
    let lower = stmt.to_ascii_lowercase();
    let pos = lower.find(keyword)? + keyword.len();
    stmt[pos..].split_whitespace().next().map(|t| t.trim_matches('"').to_string())
}

/// Deploy wrapper for a table: `CREATE TABLE`/`CREATE INDEX` become `IF NOT EXISTS`
/// (native Postgres idempotency — no guard block needed, unlike Oracle), `COMMENT ON`
/// is always safe to reissue as-is, and `ALTER TABLE ... ADD CONSTRAINT` — the one
/// statement Postgres has no `IF NOT EXISTS` form for — gets a `pg_constraint`-guarded
/// `DO $$ ... $$` block, the same idea as Oracle's guarded blocks.
fn build_table_deploy(raw: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for stmt in split_statements(raw) {
        let lower = stmt.trim_start().to_ascii_lowercase();
        if lower.starts_with("create table") {
            out.push(stmt.replacen("create table", "create table if not exists", 1));
        } else if lower.starts_with("create unique index") {
            out.push(stmt.replacen("create unique index", "create unique index if not exists", 1));
        } else if lower.starts_with("create index") {
            out.push(stmt.replacen("create index", "create index if not exists", 1));
        } else if lower.starts_with("alter table") {
            let cons = token_after(&stmt, "add constraint").unwrap_or_default();
            out.push(format!(
                "DO $$\nBEGIN\n    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = '{cons}') THEN\n        {stmt};\n    END IF;\nEND $$;"
            ));
        } else {
            // comment on ... — always safe to reissue verbatim.
            out.push(stmt);
        }
    }
    out.join("\n\n")
}

/// Wrap raw DDL in an idempotent deploy form, leaning on Postgres's native
/// `IF NOT EXISTS`/`CREATE OR REPLACE` support wherever it exists (which covers far
/// more cases than Oracle, where almost everything needs a hand-rolled guard).
pub(crate) fn build_deploy_script(object_type: &str, name: &str, raw_cleaned: &str) -> String {
    let raw = raw_cleaned.trim();
    match object_type {
        "TABLE" => build_table_deploy(raw),
        "SEQUENCE" => {
            let upper = raw.to_ascii_uppercase();
            match upper.find("CREATE SEQUENCE") {
                Some(pos) => format!("{}CREATE SEQUENCE IF NOT EXISTS{}", &raw[..pos], &raw[pos + "CREATE SEQUENCE".len()..]),
                None => raw.to_string(),
            }
        }
        // No `CREATE OR REPLACE MATERIALIZED VIEW` in Postgres — redeploy via drop+create.
        "MATERIALIZED VIEW" => format!("DROP MATERIALIZED VIEW IF EXISTS {name};\n{raw}"),
        // `CREATE OR REPLACE TRIGGER` needs Postgres 14+ (matches the docker test image).
        "TRIGGER" => raw.replacen("CREATE TRIGGER", "CREATE OR REPLACE TRIGGER", 1),
        // VIEW/FUNCTION/PROCEDURE already render as `CREATE OR REPLACE ...` — idempotent as-is.
        _ => raw.to_string(),
    }
}

/// Probe a user SQL statement's result-column names/types by preparing
/// `SELECT * FROM (<sql>) AS q LIMIT 0` (fetches zero rows). Returns `None` (rather
/// than an error) if the statement can't be wrapped this way — e.g. multi-statement
/// scripts or non-SELECT statements — so callers can fall back to plain-text decoding.
async fn probe_columns(client: &Client, sql: &str) -> Option<Vec<(String, String)>> {
    let trimmed = sql.trim().trim_end_matches(';');
    let probe = format!("SELECT * FROM ({trimmed}) AS __schemetry_probe LIMIT 0");
    let stmt = client.prepare(&probe).await.ok()?;
    Some(
        stmt.columns()
            .iter()
            .map(|c| (c.name().to_string(), c.type_().name().to_string()))
            .collect(),
    )
}

impl DbRepository for DbPostgresRepository {
    fn test_connection(&self, conn: &ConnectionRecord) -> Result<()> {
        self.runtime.block_on(async {
            let client = self.connect(conn).await?;
            client
                .simple_query("SELECT 1")
                .await
                .map_err(|e| anyhow!(e.to_string()))?;
            Ok(())
        })
    }

    fn fetch_single(
        &self,
        conn: &ConnectionRecord,
        filter_rules: &[TableFilterRule],
    ) -> Result<crate::models::ServerTables> {
        self.runtime.block_on(async {
            let client = self.connect(conn).await?;
            let schema = Self::schema_of(conn);

            let (filter_clause, filter_binds) = build_predicate(filter_rules, "c.relname", 2, ParamStyle::Dollar);
            let sql = format!(
                "SELECT c.relname, a.attname, format_type(a.atttypid, a.atttypmod), \
                        pg_get_expr(ad.adbin, ad.adrelid), col_description(c.oid, a.attnum), \
                        (SELECT ic.relname FROM pg_index ix JOIN pg_class ic ON ic.oid = ix.indexrelid \
                          WHERE ix.indrelid = c.oid AND a.attnum = ANY(ix.indkey) \
                          ORDER BY ic.relname LIMIT 1) \
                 FROM pg_attribute a \
                 JOIN pg_class c ON c.oid = a.attrelid \
                 JOIN pg_namespace n ON n.oid = c.relnamespace \
                 LEFT JOIN pg_attrdef ad ON ad.adrelid = a.attrelid AND ad.adnum = a.attnum \
                 WHERE n.nspname = $1 AND c.relkind = 'r' \
                   AND a.attnum > 0 AND NOT a.attisdropped{filter_clause} \
                 ORDER BY c.relname, a.attname"
            );
            let mut params: Vec<&(dyn ToSql + Sync)> = vec![&schema];
            for b in &filter_binds {
                params.push(b);
            }

            let rows = client.query(&sql, &params).await.map_err(|e| anyhow!(e.to_string()))?;

            let mut tables: crate::models::ServerTables = HashMap::new();
            for row in &rows {
                let table_name: String = row.get(0);
                let column_name: String = row.get(1);
                let info = ColumnInfo {
                    column_name: Some(column_name.clone()),
                    data_type: row.get::<_, Option<String>>(2),
                    data_length: None,
                    data_default: row.get::<_, Option<String>>(3).map(|d| d.trim().to_string()).filter(|d| !d.is_empty()),
                    comments: row.get::<_, Option<String>>(4).filter(|s| !s.trim().is_empty()),
                    index_name: row.get::<_, Option<String>>(5),
                    index_position: None,
                };
                tables.entry(table_name).or_default().insert(column_name, info);
            }
            Ok(tables)
        })
    }

    fn fetch_table_ddls(
        &self,
        conn: &ConnectionRecord,
        filter_rules: &[TableFilterRule],
    ) -> Result<TableDdls> {
        self.runtime.block_on(async {
            let client = self.connect(conn).await?;
            let schema = Self::schema_of(conn);

            let (filter_clause, filter_binds) = build_predicate(filter_rules, "c.relname", 2, ParamStyle::Dollar);
            let sql = format!(
                "SELECT c.relname FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE n.nspname = $1 AND c.relkind = 'r'{filter_clause} ORDER BY c.relname"
            );
            let mut params: Vec<&(dyn ToSql + Sync)> = vec![&schema];
            for b in &filter_binds {
                params.push(b);
            }
            let rows = client.query(&sql, &params).await.map_err(|e| anyhow!(e.to_string()))?;

            let mut table_ddls: TableDdls = HashMap::new();
            for row in &rows {
                let table_name: String = row.get(0);
                if let Ok(model) = self.build_table_model(&client, &schema, &table_name).await {
                    table_ddls.insert(table_name, render_table(&model));
                }
            }
            Ok(table_ddls)
        })
    }

    fn fetch_table_ddls_for_tables(
        &self,
        conn: &ConnectionRecord,
        table_names: &[String],
    ) -> Result<TableDdls> {
        if table_names.is_empty() {
            return Ok(HashMap::new());
        }
        self.runtime.block_on(async {
            let client = self.connect(conn).await?;
            let schema = Self::schema_of(conn);
            let mut table_ddls: TableDdls = HashMap::new();
            for table_name in table_names {
                if let Ok(model) = self.build_table_model(&client, &schema, table_name.trim()).await {
                    table_ddls.insert(table_name.clone(), render_table(&model));
                }
            }
            Ok(table_ddls)
        })
    }

    fn fetch_schema_objects(
        &self,
        conn: &ConnectionRecord,
        filter_rules: &[TableFilterRule],
    ) -> Result<Vec<SchemaObject>> {
        self.runtime.block_on(async {
            let client = self.connect(conn).await?;
            let schema = Self::schema_of(conn);

            let (filter_clause, filter_binds) = build_predicate(filter_rules, "c.relname", 2, ParamStyle::Dollar);
            let tables_sql = format!(
                "SELECT c.relname, CASE c.relkind WHEN 'r' THEN 'TABLE' WHEN 'v' THEN 'VIEW' \
                        WHEN 'm' THEN 'MATERIALIZED VIEW' WHEN 'S' THEN 'SEQUENCE' END \
                 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE n.nspname = $1 AND c.relkind IN ('r','v','m','S'){filter_clause}"
            );
            let mut params: Vec<&(dyn ToSql + Sync)> = vec![&schema];
            for b in &filter_binds {
                params.push(b);
            }
            let mut objects = Vec::new();
            for row in client.query(&tables_sql, &params).await.map_err(|e| anyhow!(e.to_string()))? {
                objects.push(SchemaObject { name: row.get(0), object_type: row.get(1) });
            }

            for row in client
                .query(
                    "SELECT p.proname, CASE WHEN p.prokind = 'p' THEN 'PROCEDURE' ELSE 'FUNCTION' END \
                     FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace WHERE n.nspname = $1",
                    &[&schema],
                )
                .await
                .unwrap_or_default()
            {
                objects.push(SchemaObject { name: row.get(0), object_type: row.get(1) });
            }

            for row in client
                .query(
                    "SELECT t.tgname, 'TRIGGER' FROM pg_trigger t \
                     JOIN pg_class c ON c.oid = t.tgrelid JOIN pg_namespace n ON n.oid = c.relnamespace \
                     WHERE n.nspname = $1 AND NOT t.tgisinternal",
                    &[&schema],
                )
                .await
                .unwrap_or_default()
            {
                objects.push(SchemaObject { name: row.get(0), object_type: row.get(1) });
            }

            objects.sort_by(|a, b| (&a.object_type, &a.name).cmp(&(&b.object_type, &b.name)));
            Ok(objects)
        })
    }

    fn fetch_object_ddl(&self, conn: &ConnectionRecord, name: &str, object_type: &str) -> Result<String> {
        self.runtime.block_on(async {
            let client = self.connect(conn).await?;
            let schema = Self::schema_of(conn);
            let upper_type = object_type.trim().to_ascii_uppercase();

            match upper_type.as_str() {
                "TABLE" => {
                    let model = self.build_table_model(&client, &schema, name).await?;
                    Ok(render_table(&model))
                }
                "VIEW" | "MATERIALIZED VIEW" => {
                    let relkind = if upper_type == "VIEW" { "v" } else { "m" };
                    let row = client
                        .query_opt(
                            "SELECT pg_get_viewdef(c.oid, true) FROM pg_class c \
                             JOIN pg_namespace n ON n.oid = c.relnamespace \
                             WHERE n.nspname = $1 AND c.relname = $2 AND c.relkind = $3",
                            &[&schema, &name, &relkind],
                        )
                        .await
                        .map_err(|e| anyhow!(e.to_string()))?
                        .ok_or_else(|| anyhow!("{} {} not found", upper_type, name))?;
                    let def: String = row.get(0);
                    let keyword = if upper_type == "VIEW" { "CREATE OR REPLACE VIEW" } else { "CREATE MATERIALIZED VIEW" };
                    Ok(format!("{keyword} {name} AS\n{}", def.trim()))
                }
                "SEQUENCE" => {
                    let row = client
                        .query_opt(
                            "SELECT increment_by, min_value, max_value, cache_size, cycle \
                             FROM pg_sequences WHERE schemaname = $1 AND sequencename = $2",
                            &[&schema, &name],
                        )
                        .await
                        .map_err(|e| anyhow!(e.to_string()))?
                        .ok_or_else(|| anyhow!("Sequence {} not found", name))?;
                    let increment: i64 = row.get(0);
                    let min_value: Option<i64> = row.get(1);
                    let max_value: Option<i64> = row.get(2);
                    let cache: i64 = row.get(3);
                    let cycle: bool = row.get(4);
                    let mut ddl = format!("CREATE SEQUENCE {name} INCREMENT BY {increment}");
                    if let Some(v) = min_value {
                        ddl.push_str(&format!(" MINVALUE {v}"));
                    }
                    if let Some(v) = max_value {
                        ddl.push_str(&format!(" MAXVALUE {v}"));
                    }
                    // Always reset to 1 on (re)deploy, matching the Oracle path's
                    // normalization in `clean_sequence_ddl`.
                    ddl.push_str(&format!(" START WITH 1 CACHE {cache}"));
                    ddl.push_str(if cycle { " CYCLE" } else { " NO CYCLE" });
                    ddl.push(';');
                    Ok(ddl)
                }
                "FUNCTION" | "PROCEDURE" => {
                    let row = client
                        .query_opt(
                            "SELECT pg_get_functiondef(p.oid) FROM pg_proc p \
                             JOIN pg_namespace n ON n.oid = p.pronamespace \
                             WHERE n.nspname = $1 AND p.proname = $2 LIMIT 1",
                            &[&schema, &name],
                        )
                        .await
                        .map_err(|e| anyhow!(e.to_string()))?
                        .ok_or_else(|| anyhow!("{} {} not found", upper_type, name))?;
                    Ok(row.get::<_, String>(0))
                }
                "TRIGGER" => {
                    let row = client
                        .query_opt(
                            "SELECT pg_get_triggerdef(t.oid, true) FROM pg_trigger t \
                             JOIN pg_class c ON c.oid = t.tgrelid JOIN pg_namespace n ON n.oid = c.relnamespace \
                             WHERE n.nspname = $1 AND t.tgname = $2 AND NOT t.tgisinternal",
                            &[&schema, &name],
                        )
                        .await
                        .map_err(|e| anyhow!(e.to_string()))?
                        .ok_or_else(|| anyhow!("Trigger {} not found", name))?;
                    Ok(format!("{};", row.get::<_, String>(0).trim_end_matches(';')))
                }
                other => Err(anyhow!("Object type '{}' is not supported for PostgreSQL connections", other)),
            }
        })
    }

    fn generate_history_fix(
        &self,
        conn: &ConnectionRecord,
        naming_rules: &[HistoryNamingRule],
    ) -> Result<HistoryFixResult> {
        self.runtime.block_on(async {
            let client = self.connect(conn).await?;
            let schema = Self::schema_of(conn);

            let rules: Vec<&HistoryNamingRule> = naming_rules
                .iter()
                .filter(|r| {
                    r.enabled
                        && !r.pattern.trim().is_empty()
                        && matches!(r.match_type, MatchType::Prefix | MatchType::Suffix)
                })
                .collect();
            if rules.is_empty() {
                return Ok(HistoryFixResult::default());
            }

            let patterns: Vec<String> = rules.iter().map(|r| r.pattern.trim().to_ascii_lowercase()).collect();
            let wildcards: Vec<String> = rules
                .iter()
                .zip(&patterns)
                .map(|(r, p)| match r.match_type {
                    MatchType::Prefix => format!("{p}%"),
                    _ => format!("%{p}"),
                })
                .collect();

            // Unlike Oracle (where the `oracle` crate binds by physical left-to-right
            // position and the `:N` label is cosmetic), Postgres binds strictly by the
            // `$N` label itself — so `binds` must be built in exact `$1, $2, $3, ...`
            // order, not in the order each placeholder happens to be written into the
            // SQL text.
            let mut joins = String::new();
            let mut name_cols: Vec<String> = Vec::new();
            let mut not_like_clauses: Vec<String> = Vec::new();
            let mut binds: Vec<&(dyn ToSql + Sync)> = vec![&schema];
            let mut next_idx = 2;

            for (i, rule) in rules.iter().enumerate() {
                let alias = format!("h{i}");
                match rule.match_type {
                    MatchType::Prefix => joins.push_str(&format!(
                        " LEFT JOIN information_schema.tables {alias} ON {alias}.table_schema = t.table_schema \
                          AND lower({alias}.table_name) = lower(${next_idx} || t.table_name)"
                    )),
                    _ => joins.push_str(&format!(
                        " LEFT JOIN information_schema.tables {alias} ON {alias}.table_schema = t.table_schema \
                          AND lower({alias}.table_name) = lower(t.table_name || ${next_idx})"
                    )),
                }
                binds.push(&patterns[i]);
                next_idx += 1;
                not_like_clauses.push(format!("lower(t.table_name) NOT LIKE ${next_idx}"));
                binds.push(&wildcards[i]);
                next_idx += 1;
                name_cols.push(format!("{alias}.table_name"));
            }

            let history_name_expr = if name_cols.len() == 1 {
                name_cols[0].clone()
            } else {
                format!("COALESCE({})", name_cols.join(", "))
            };
            let has_pair_clause = name_cols
                .iter()
                .map(|c| format!("{c} IS NOT NULL"))
                .collect::<Vec<_>>()
                .join(" OR ");

            let pair_sql = format!(
                "SELECT t.table_name, {history_name_expr} FROM information_schema.tables t{joins} \
                 WHERE t.table_schema = $1 AND t.table_type = 'BASE TABLE' \
                   AND {not_like} AND ({has_pair_clause}) \
                 ORDER BY t.table_name",
                not_like = not_like_clauses.join(" AND "),
            );

            let prefix_patterns: Vec<&str> = rules
                .iter()
                .zip(&patterns)
                .filter(|(r, _)| r.match_type == MatchType::Prefix)
                .map(|(_, p)| p.as_str())
                .collect();

            let pair_rows = client.query(&pair_sql, &binds).await.map_err(|e| anyhow!(e.to_string()))?;
            let mut pairs: Vec<(String, String)> = Vec::new();
            for row in &pair_rows {
                let main_table: String = row.get(0);
                let history_table: String = row.get(1);
                pairs.push((main_table, history_table));
            }

            let col_sql = "SELECT a.attname, format_type(a.atttypid, a.atttypmod) \
                           FROM pg_attribute a JOIN pg_class c ON c.oid = a.attrelid \
                           JOIN pg_namespace n ON n.oid = c.relnamespace \
                           WHERE n.nspname = $1 AND c.relname = $2 \
                             AND a.attnum > 1 AND NOT a.attisdropped \
                             AND format_type(a.atttypid, a.atttypmod) NOT IN ('bytea') \
                           ORDER BY a.attnum";

            let mut issues: Vec<HistoryTableIssue> = Vec::new();
            let mut fix_sql = String::new();

            for (main_table, history_table) in &pairs {
                let main_cols = client
                    .query(col_sql, &[&schema, main_table])
                    .await
                    .map_err(|e| anyhow!(e.to_string()))?
                    .iter()
                    .map(|r| (r.get::<_, String>(0), r.get::<_, String>(1)))
                    .collect::<Vec<_>>();
                let history_cols = client
                    .query(col_sql, &[&schema, history_table])
                    .await
                    .map_err(|e| anyhow!(e.to_string()))?
                    .iter()
                    .map(|r| (r.get::<_, String>(0), r.get::<_, String>(1)))
                    .collect::<Vec<_>>();

                let mut table_fixes = String::new();

                for (col_name, data_type) in &main_cols {
                    let found = history_cols.iter().find(|(hcol, _)| {
                        hcol.eq_ignore_ascii_case(col_name)
                            || prefix_patterns.iter().any(|p| hcol.eq_ignore_ascii_case(&format!("{p}{col_name}")))
                    });

                    match found {
                        None => {
                            issues.push(HistoryTableIssue {
                                history_table: history_table.clone(),
                                column_name: col_name.clone(),
                                issue_type: "MISSING".to_string(),
                                main_type: data_type.clone(),
                                history_type: String::new(),
                            });
                            table_fixes.push_str(&format!(
                                "ALTER TABLE {history_table} ADD COLUMN IF NOT EXISTS {col_name} {data_type};\n"
                            ));
                        }
                        Some((history_col, history_type)) => {
                            if data_type != history_type {
                                issues.push(HistoryTableIssue {
                                    history_table: history_table.clone(),
                                    column_name: col_name.clone(),
                                    issue_type: "TYPE_MISMATCH".to_string(),
                                    main_type: data_type.clone(),
                                    history_type: history_type.clone(),
                                });
                                table_fixes.push_str(&format!(
                                    "ALTER TABLE {history_table} ALTER COLUMN {history_col} TYPE {data_type};\n"
                                ));
                            }
                        }
                    }
                }

                if !table_fixes.is_empty() {
                    fix_sql.push_str(&format!("\n-- Alter table {history_table}\n{table_fixes}"));
                }
            }

            Ok(HistoryFixResult { issues, fix_sql })
        })
    }

    fn run_query(
        &self,
        conn: &ConnectionRecord,
        sql: &str,
        materialize_lobs: bool,
    ) -> Result<(Vec<String>, Vec<String>, Vec<Vec<Option<String>>>)> {
        self.runtime.block_on(async {
            let client = self.connect(conn).await?;
            let probed = probe_columns(&client, sql).await;

            let messages = client
                .simple_query(sql)
                .await
                .map_err(|e| anyhow!(e.to_string()))?;

            let mut columns: Vec<String> = Vec::new();
            let mut types: Vec<String> = Vec::new();
            let mut kinds: Vec<CellKind> = Vec::new();
            let mut rows: Vec<Vec<Option<String>>> = Vec::new();

            for message in &messages {
                if let tokio_postgres::SimpleQueryMessage::Row(row) = message {
                    if columns.is_empty() {
                        for (i, col) in row.columns().iter().enumerate() {
                            let (type_name, kind) = match probed.as_ref().and_then(|p| p.get(i)) {
                                Some((_, t)) => (t.clone(), cell_kind(t)),
                                None => ("text".to_string(), CellKind::Text),
                            };
                            columns.push(col.name().to_string());
                            types.push(type_label(&type_name));
                            kinds.push(kind);
                        }
                    }
                    let mut values = Vec::with_capacity(row.len());
                    for i in 0..row.len() {
                        let kind = kinds.get(i).copied().unwrap_or(CellKind::Text);
                        values.push(match (row.get(i), kind) {
                            (None, _) => None,
                            (Some(text), CellKind::Binary) if !materialize_lobs => {
                                let _ = text;
                                Some("<BYTEA>".to_string())
                            }
                            (Some(text), CellKind::Binary) => {
                                Some(bytes_to_hex(&decode_bytea_hex(text), 8192))
                            }
                            (Some(text), CellKind::Text) => Some(cap_chars(text.to_string(), MATERIALIZE_TEXT_CAP)),
                        });
                    }
                    rows.push(values);
                }
            }

            Ok((columns, types, rows))
        })
    }

    fn fetch_blob_cell(
        &self,
        conn: &ConnectionRecord,
        sql: &str,
        row_index: usize,
        col_index: usize,
        max_bytes: usize,
    ) -> Result<Vec<u8>> {
        self.runtime.block_on(async {
            let client = self.connect(conn).await?;
            let messages = client.simple_query(sql).await.map_err(|e| anyhow!(e.to_string()))?;
            let mut idx = 0;
            for message in &messages {
                if let tokio_postgres::SimpleQueryMessage::Row(row) = message {
                    if idx == row_index {
                        let mut bytes = row.get(col_index).map(decode_bytea_hex).unwrap_or_default();
                        if bytes.len() > max_bytes {
                            bytes.truncate(max_bytes);
                        }
                        return Ok(bytes);
                    }
                    idx += 1;
                }
            }
            Err(anyhow!("Row {row_index} is no longer in the result set (the data may have changed)."))
        })
    }

    fn fetch_lob_cell(
        &self,
        conn: &ConnectionRecord,
        sql: &str,
        row_index: usize,
        col_index: usize,
        max_bytes: usize,
    ) -> Result<LobCell> {
        self.runtime.block_on(async {
            let client = self.connect(conn).await?;
            let probed = probe_columns(&client, sql).await;
            let is_binary = probed
                .as_ref()
                .and_then(|p| p.get(col_index))
                .map(|(_, t)| matches!(cell_kind(t), CellKind::Binary))
                .unwrap_or(false);

            let messages = client.simple_query(sql).await.map_err(|e| anyhow!(e.to_string()))?;
            let mut idx = 0;
            for message in &messages {
                if let tokio_postgres::SimpleQueryMessage::Row(row) = message {
                    if idx == row_index {
                        return Ok(if is_binary {
                            let mut bytes = row.get(col_index).map(decode_bytea_hex).unwrap_or_default();
                            if bytes.len() > max_bytes {
                                bytes.truncate(max_bytes);
                            }
                            LobCell::Binary(bytes)
                        } else {
                            LobCell::Text(row.get(col_index).map(str::to_string))
                        });
                    }
                    idx += 1;
                }
            }
            Err(anyhow!("Row {row_index} is no longer in the result set (the data may have changed)."))
        })
    }
}
