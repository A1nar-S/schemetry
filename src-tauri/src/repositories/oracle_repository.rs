use std::collections::{BTreeMap, HashMap};
use std::sync::RwLock;

use anyhow::{anyhow, Result};
use oracle::sql_type::{OracleType, ToSql};
use oracle::{Connection, InitParams, Row};

use crate::models::{
    ColumnInfo, ConnectionRecord, HistoryFixResult, HistoryNamingRule, HistoryTableIssue,
    MatchType, SchemaObject, TableDdls, TableFilterRule,
};
use crate::repositories::filter_rule_repository::build_predicate;

/// Stores the Oracle Instant Client directory path set at startup or via Settings.
/// Using a static RwLock avoids touching the process environment (which requires `unsafe`).
static CLIENT_LIB_DIR: RwLock<String> = RwLock::new(String::new());

pub fn set_client_lib_dir_hint(dir: &str) {
    if let Ok(mut guard) = CLIENT_LIB_DIR.write() {
        *guard = dir.to_string();
    }
}

/// Base of the column/metadata fetch used for schema compare; the caller appends a
/// dynamically-built name-filter clause (see [`build_predicate`]) before `ORDER BY`.
const FETCH_SQL_BASE: &str = r#"
SELECT
    sys_context('USERENV', 'SERVER_HOST') AS SERVER_NAME,
    atc.TABLE_NAME,
    atc.COLUMN_NAME,
    atc.DATA_TYPE,
    TO_CHAR(
        CASE
            WHEN atc.DATA_TYPE = 'NUMBER'                      THEN atc.DATA_PRECISION
            WHEN atc.DATA_TYPE = 'DATE'                        THEN NULL
            WHEN atc.DATA_TYPE = 'NVARCHAR2'                   THEN atc.CHAR_LENGTH
            WHEN atc.DATA_TYPE IN ('CLOB', 'BLOB', 'LONG RAW') THEN NULL
            ELSE atc.DATA_LENGTH
        END
    ) AS DATA_LENGTH,
    atc.DATA_DEFAULT,
    acc.COMMENTS,
    ai.INDEX_NAME,
    aic.COLUMN_POSITION
FROM
    ALL_TAB_COLUMNS atc
JOIN
    ALL_COL_COMMENTS acc
        ON  atc.OWNER       = acc.OWNER
        AND atc.TABLE_NAME  = acc.TABLE_NAME
        AND atc.COLUMN_NAME = acc.COLUMN_NAME
LEFT JOIN
    ALL_IND_COLUMNS aic
        ON  atc.OWNER       = aic.INDEX_OWNER
        AND atc.TABLE_NAME  = aic.TABLE_NAME
        AND atc.COLUMN_NAME = aic.COLUMN_NAME
LEFT JOIN
    ALL_INDEXES ai
        ON  aic.INDEX_OWNER = ai.OWNER
        AND aic.INDEX_NAME  = ai.INDEX_NAME
CROSS JOIN DUAL
WHERE
    atc.OWNER = :1
    AND (atc.OWNER, atc.TABLE_NAME) NOT IN (
        SELECT vw.OWNER,  vw.VIEW_NAME   FROM ALL_VIEWS  vw
        UNION ALL
        SELECT mvw.OWNER, mvw.MVIEW_NAME FROM ALL_MVIEWS mvw
    )"#;
const FETCH_SQL_ORDER_BY: &str = "\nORDER BY\n    atc.OWNER, atc.TABLE_NAME, atc.COLUMN_NAME, ai.INDEX_NAME, aic.COLUMN_POSITION\n";

/// Base of the table-DDL fetch; the caller appends a dynamically-built name-filter
/// clause (see [`build_predicate`]) before `ORDER BY`.
const TABLE_DDL_SQL_BASE: &str = r#"
SELECT
    at.TABLE_NAME,
    DBMS_METADATA.GET_DDL('TABLE', at.TABLE_NAME, at.OWNER) AS TABLE_DDL
FROM
    ALL_TABLES at
WHERE
    at.OWNER = :1
    AND (at.OWNER, at.TABLE_NAME) NOT IN (
        SELECT vw.OWNER,  vw.VIEW_NAME   FROM ALL_VIEWS  vw
        UNION ALL
        SELECT mvw.OWNER, mvw.MVIEW_NAME FROM ALL_MVIEWS mvw
    )"#;
const TABLE_DDL_SQL_ORDER_BY: &str = "\nORDER BY\n    at.TABLE_NAME\n";

const DDL_TRANSFORM_SQL: &str = r#"
BEGIN
    DBMS_METADATA.SET_TRANSFORM_PARAM(DBMS_METADATA.SESSION_TRANSFORM, 'STORAGE', FALSE);
    DBMS_METADATA.SET_TRANSFORM_PARAM(DBMS_METADATA.SESSION_TRANSFORM, 'TABLESPACE', FALSE);
    DBMS_METADATA.SET_TRANSFORM_PARAM(DBMS_METADATA.SESSION_TRANSFORM, 'SEGMENT_ATTRIBUTES', FALSE);
    DBMS_METADATA.SET_TRANSFORM_PARAM(DBMS_METADATA.SESSION_TRANSFORM, 'SQLTERMINATOR', TRUE);
    DBMS_METADATA.SET_TRANSFORM_PARAM(DBMS_METADATA.SESSION_TRANSFORM, 'EMIT_SCHEMA', FALSE);
    DBMS_METADATA.SET_TRANSFORM_PARAM(DBMS_METADATA.SESSION_TRANSFORM, 'PRETTY', TRUE);
    DBMS_METADATA.SET_TRANSFORM_PARAM(DBMS_METADATA.SESSION_TRANSFORM, 'FORCE', FALSE);
END;
"#;

pub trait OracleRepository: Send + Sync {
    fn test_connection(&self, conn: &ConnectionRecord) -> Result<()>;
    fn fetch_single(
        &self,
        conn: &ConnectionRecord,
        filter_rules: &[TableFilterRule],
    ) -> Result<crate::models::ServerTables>;
    fn fetch_table_ddls(
        &self,
        conn: &ConnectionRecord,
        filter_rules: &[TableFilterRule],
    ) -> Result<TableDdls>;
    fn fetch_table_ddls_for_tables(
        &self,
        conn: &ConnectionRecord,
        table_names: &[String],
    ) -> Result<TableDdls>;
    fn fetch_schema_objects(
        &self,
        conn: &ConnectionRecord,
        filter_rules: &[TableFilterRule],
    ) -> Result<Vec<SchemaObject>>;
    fn fetch_object_ddl(&self, conn: &ConnectionRecord, name: &str, object_type: &str) -> Result<String>;
    /// Pair up main tables with their history-table counterpart using the given
    /// enabled naming rules (e.g. a `HIST_` prefix and/or a `_HIST` suffix rule) and
    /// report any column drift between them. Only `Prefix`/`Suffix` rules are honored;
    /// other match types are ignored. An empty rule list yields an empty result.
    fn generate_history_fix(
        &self,
        conn: &ConnectionRecord,
        naming_rules: &[HistoryNamingRule],
    ) -> Result<HistoryFixResult>;
    /// Returns `(column names, column type labels, rows)`. When `materialize_lobs` is
    /// false, binary LOB cells render as `<BLOB>` and text LOB cells as `<CLOB>` (their
    /// content can be fetched lazily via [`OracleRepository::fetch_lob_cell`]). When
    /// true, LOB content is materialized inline (capped): CLOB → text, BLOB → decoded
    /// text or hex.
    fn run_query(
        &self,
        conn: &ConnectionRecord,
        sql: &str,
        materialize_lobs: bool,
    ) -> Result<(Vec<String>, Vec<String>, Vec<Vec<Option<String>>>)>;
    /// Re-run `sql` and read the raw bytes of a single BLOB/binary cell, capped at
    /// `max_bytes`. Used for the full-bytes "Save to file" path.
    fn fetch_blob_cell(
        &self,
        conn: &ConnectionRecord,
        sql: &str,
        row_index: usize,
        col_index: usize,
        max_bytes: usize,
    ) -> Result<Vec<u8>>;
    /// Re-run `sql` and read a single LOB cell, returning text for CLOB-like columns
    /// and bytes (capped at `max_bytes`) for binary columns, based on the column type.
    fn fetch_lob_cell(
        &self,
        conn: &ConnectionRecord,
        sql: &str,
        row_index: usize,
        col_index: usize,
        max_bytes: usize,
    ) -> Result<LobCell>;
}

/// A single LOB cell fetched on demand for the content viewer.
pub enum LobCell {
    Text(Option<String>),
    Binary(Vec<u8>),
}

pub struct DbOracleRepository;

impl DbOracleRepository {
    pub fn new() -> Self {
        Self
    }

    fn clean_opt(value: Option<String>) -> Option<String> {
        value.and_then(|v| {
            let cleaned = v.replace('\n', "").trim().to_string();
            if cleaned.is_empty() || cleaned.eq_ignore_ascii_case("null") {
                None
            } else {
                Some(cleaned)
            }
        })
    }

    /// Build `CREATE OR REPLACE SYNONYM name FOR owner.target[@db_link]` from ALL_SYNONYMS.
    /// DBMS_METADATA drops the target owner under EMIT_SCHEMA=FALSE, so we assemble it here.
    fn fetch_synonym_ddl(&self, db: &Connection, name: &str, schema: &str) -> Result<String> {
        let rows = db
            .query(
                "SELECT TABLE_OWNER, TABLE_NAME, DB_LINK FROM ALL_SYNONYMS \
                 WHERE OWNER = :1 AND SYNONYM_NAME = :2",
                &[&schema, &name],
            )
            .map_err(|e| anyhow!(e.to_string()))?;

        for row_result in rows {
            let row = row_result.map_err(|e| anyhow!(e.to_string()))?;
            let table_owner: Option<String> = row.get(0).ok().flatten();
            let table_name: String = row.get(1).map_err(|e| anyhow!(e.to_string()))?;
            let db_link: Option<String> = row.get(2).ok().flatten();

            let mut target = match table_owner {
                Some(owner) if !owner.trim().is_empty() => format!("{owner}.{table_name}"),
                _ => table_name,
            };
            if let Some(link) = db_link.filter(|l| !l.trim().is_empty()) {
                target.push('@');
                target.push_str(link.trim());
            }
            return Ok(format!("CREATE OR REPLACE SYNONYM {name} FOR {target};"));
        }

        Err(anyhow!("Synonym {} not found in schema {}", name, schema))
    }

    fn connect(&self, conn: &ConnectionRecord) -> Result<Connection> {
        ensure_client_initialized()?;

        let connect_string = format!("//{}:{}/{}", conn.host, conn.port, conn.service_name);
        let db = Connection::connect(&conn.username, &conn.password, &connect_string)
            .map_err(|e| anyhow!(e.to_string()))?;
        Ok(db)
    }
}

impl OracleRepository for DbOracleRepository {
    fn test_connection(&self, conn: &ConnectionRecord) -> Result<()> {
        let db = self.connect(conn)?;
        db.ping().map_err(|e| anyhow!(e.to_string()))?;
        Ok(())
    }

    fn fetch_single(
        &self,
        conn: &ConnectionRecord,
        filter_rules: &[TableFilterRule],
    ) -> Result<crate::models::ServerTables> {
        let db = self.connect(conn)?;
        let schema = conn.username.to_ascii_uppercase();

        let (filter_clause, filter_binds) = build_predicate(filter_rules, "atc.TABLE_NAME", 2);
        let sql = format!("{FETCH_SQL_BASE}{filter_clause}{FETCH_SQL_ORDER_BY}");
        let mut binds: Vec<&dyn ToSql> = vec![&schema];
        for b in &filter_binds {
            binds.push(b);
        }

        let rows = db
            .query(&sql, &binds)
            .map_err(|e| anyhow!(e.to_string()))?;

        let mut tables: crate::models::ServerTables = HashMap::new();

        for row_result in rows {
            let row = row_result.map_err(|e| anyhow!(e.to_string()))?;
            let table_name: String = row.get(1).map_err(|e| anyhow!(e.to_string()))?;
            let column_name: String = row.get(2).map_err(|e| anyhow!(e.to_string()))?;

            let info = ColumnInfo {
                column_name: Self::clean_opt(row.get(2).ok()),
                data_type: Self::clean_opt(row.get(3).ok()),
                data_length: Self::clean_opt(row.get(4).ok()),
                data_default: Self::clean_opt(row.get(5).ok()),
                comments: Self::clean_opt(row.get(6).ok()),
                index_name: Self::clean_opt(row.get(7).ok()),
                index_position: row.get::<_, Option<u32>>(8).ok().flatten(),
            };

            tables
                .entry(table_name)
                .or_default()
                .insert(column_name, info);
        }

        Ok(tables)
    }

    fn run_query(
        &self,
        conn: &ConnectionRecord,
        sql: &str,
        materialize_lobs: bool,
    ) -> Result<(Vec<String>, Vec<String>, Vec<Vec<Option<String>>>)> {
        let db = self.connect(conn)?;
        let result_set = db.query(sql, &[]).map_err(|e| anyhow!(e.to_string()))?;

        // Resolve column names, type labels, and per-column kinds before consuming the
        // result set. Binary LOBs must never be read as String — that conversion panics,
        // and with `panic = "abort"` it takes the whole app down.
        let (columns, types, kinds) = {
            let infos = result_set.column_info();
            let columns: Vec<String> = infos.iter().map(|c| c.name().to_string()).collect();
            let types: Vec<String> = infos.iter().map(|c| type_label(c.oracle_type())).collect();
            let kinds: Vec<CellKind> = infos.iter().map(|c| cell_kind(c.oracle_type())).collect();
            (columns, types, kinds)
        };
        let col_count = columns.len();

        let mut rows = Vec::new();
        for row_result in result_set {
            let row = row_result.map_err(|e| anyhow!(e.to_string()))?;
            let values = (0..col_count)
                .map(|i| read_cell(&row, i, kinds[i], materialize_lobs))
                .collect();
            rows.push(values);
        }

        Ok((columns, types, rows))
    }

    fn fetch_blob_cell(
        &self,
        conn: &ConnectionRecord,
        sql: &str,
        row_index: usize,
        col_index: usize,
        max_bytes: usize,
    ) -> Result<Vec<u8>> {
        let db = self.connect(conn)?;
        let result_set = db.query(sql, &[]).map_err(|e| anyhow!(e.to_string()))?;

        for (idx, row_result) in result_set.enumerate() {
            if idx != row_index {
                continue;
            }
            let row = row_result.map_err(|e| anyhow!(e.to_string()))?;
            let mut bytes = row
                .get::<usize, Option<Vec<u8>>>(col_index)
                .map_err(|e| anyhow!(e.to_string()))?
                .unwrap_or_default();
            if bytes.len() > max_bytes {
                bytes.truncate(max_bytes);
            }
            return Ok(bytes);
        }

        Err(anyhow!(
            "Row {row_index} is no longer in the result set (the data may have changed)."
        ))
    }

    fn fetch_lob_cell(
        &self,
        conn: &ConnectionRecord,
        sql: &str,
        row_index: usize,
        col_index: usize,
        max_bytes: usize,
    ) -> Result<LobCell> {
        let db = self.connect(conn)?;
        let result_set = db.query(sql, &[]).map_err(|e| anyhow!(e.to_string()))?;

        // Decide how to read the cell from the column's Oracle type.
        let is_binary = result_set
            .column_info()
            .get(col_index)
            .map(|c| matches!(cell_kind(c.oracle_type()), CellKind::BinaryLob | CellKind::Raw))
            .unwrap_or(false);

        for (idx, row_result) in result_set.enumerate() {
            if idx != row_index {
                continue;
            }
            let row = row_result.map_err(|e| anyhow!(e.to_string()))?;
            if is_binary {
                let mut bytes = row
                    .get::<usize, Option<Vec<u8>>>(col_index)
                    .map_err(|e| anyhow!(e.to_string()))?
                    .unwrap_or_default();
                if bytes.len() > max_bytes {
                    bytes.truncate(max_bytes);
                }
                return Ok(LobCell::Binary(bytes));
            } else {
                let text = row
                    .get::<usize, Option<String>>(col_index)
                    .map_err(|e| anyhow!(e.to_string()))?;
                return Ok(LobCell::Text(text));
            }
        }

        Err(anyhow!(
            "Row {row_index} is no longer in the result set (the data may have changed)."
        ))
    }

    fn fetch_table_ddls(
        &self,
        conn: &ConnectionRecord,
        filter_rules: &[TableFilterRule],
    ) -> Result<TableDdls> {
        let db = self.connect(conn)?;
        let schema = conn.username.to_ascii_uppercase();
        let _ = db.execute(DDL_TRANSFORM_SQL, &[]);

        let (filter_clause, filter_binds) = build_predicate(filter_rules, "at.TABLE_NAME", 2);
        let sql = format!("{TABLE_DDL_SQL_BASE}{filter_clause}{TABLE_DDL_SQL_ORDER_BY}");
        let mut binds: Vec<&dyn ToSql> = vec![&schema];
        for b in &filter_binds {
            binds.push(b);
        }

        let ddl_rows = db
            .query(&sql, &binds)
            .map_err(|e| anyhow!(e.to_string()))?;

        let mut table_ddls: TableDdls = HashMap::new();
        for row_result in ddl_rows {
            let row = row_result.map_err(|e| anyhow!(e.to_string()))?;
            let table_name: String = row.get(0).map_err(|e| anyhow!(e.to_string()))?;
            let ddl: String = row.get(1).map_err(|e| anyhow!(e.to_string()))?;
            table_ddls.insert(table_name, ddl);
        }

        Ok(table_ddls)
    }

    fn fetch_table_ddls_for_tables(
        &self,
        conn: &ConnectionRecord,
        table_names: &[String],
    ) -> Result<TableDdls> {
        if table_names.is_empty() {
            return Ok(HashMap::new());
        }

        let db = self.connect(conn)?;
        let schema = conn.username.to_ascii_uppercase();
        let _ = db.execute(DDL_TRANSFORM_SQL, &[]);

        let mut table_ddls: TableDdls = HashMap::new();
        for table_name in table_names {
            let upper = table_name.trim().to_ascii_uppercase();
            let rows = db.query(
                "SELECT DBMS_METADATA.GET_DDL('TABLE', :1, :2) FROM DUAL",
                &[&upper, &schema],
            );
            let rows = match rows {
                Ok(r) => r,
                Err(_) => continue,
            };
            for row_result in rows {
                if let Ok(row) = row_result {
                    if let Ok(ddl) = row.get::<usize, String>(0) {
                        table_ddls.insert(upper.clone(), ddl);
                    }
                }
                break; // only one row expected per table
            }
        }

        Ok(table_ddls)
    }

    fn fetch_schema_objects(
        &self,
        conn: &ConnectionRecord,
        filter_rules: &[TableFilterRule],
    ) -> Result<Vec<SchemaObject>> {
        let db = self.connect(conn)?;
        let schema = conn.username.to_ascii_uppercase();

        let (filter_clause, filter_binds) = build_predicate(filter_rules, "o.OBJECT_NAME", 2);
        let sql = format!(
            "SELECT o.OBJECT_NAME, o.OBJECT_TYPE \
             FROM ALL_OBJECTS o \
             WHERE o.OWNER = :1 \
               AND o.OBJECT_TYPE IN ('TABLE','VIEW','PROCEDURE','FUNCTION','PACKAGE',\
                                     'PACKAGE BODY','TRIGGER','SEQUENCE','SYNONYM','TYPE',\
                                     'MATERIALIZED VIEW','JOB') \
               AND o.STATUS = 'VALID' \
               AND NOT (o.OBJECT_TYPE = 'TABLE' \
                        AND EXISTS (SELECT 1 FROM ALL_MVIEWS m \
                                     WHERE m.OWNER = o.OWNER AND m.MVIEW_NAME = o.OBJECT_NAME)){filter_clause} \
             ORDER BY o.OBJECT_TYPE, o.OBJECT_NAME"
        );
        let mut binds: Vec<&dyn ToSql> = vec![&schema];
        for b in &filter_binds {
            binds.push(b);
        }

        let rows = db
            .query(&sql, &binds)
            .map_err(|e| anyhow!(e.to_string()))?;

        let mut objects = Vec::new();
        for row_result in rows {
            let row = row_result.map_err(|e| anyhow!(e.to_string()))?;
            let name: String = row.get(0).map_err(|e| anyhow!(e.to_string()))?;
            let object_type: String = row.get(1).map_err(|e| anyhow!(e.to_string()))?;
            objects.push(SchemaObject { name, object_type });
        }

        Ok(objects)
    }

    fn fetch_object_ddl(
        &self,
        conn: &ConnectionRecord,
        name: &str,
        object_type: &str,
    ) -> Result<String> {
        let db = self.connect(conn)?;
        let schema = conn.username.to_ascii_uppercase();
        let upper_name = name.trim().to_ascii_uppercase();
        let upper_type = object_type.trim().to_ascii_uppercase();

        // Synonyms: build directly so the target schema is preserved. DBMS_METADATA with
        // EMIT_SCHEMA=FALSE drops the target owner, leaving an unqualified FOR clause.
        if upper_type == "SYNONYM" {
            return self.fetch_synonym_ddl(&db, &upper_name, &schema);
        }

        // Tables: build from catalog views to match PL/SQL Developer's export exactly
        // (lowercase columns, separate comment/index/constraint statements). DBMS_METADATA
        // jams indexes and constraints onto one line and uses a different style.
        if upper_type == "TABLE" {
            let model = build_table_model(&db, &schema, &upper_name)?;
            return Ok(render_table_raw(&model));
        }

        let _ = db.execute(DDL_TRANSFORM_SQL, &[]);

        // DBMS_METADATA uses underscored names for some types
        let metadata_type = match upper_type.as_str() {
            "PACKAGE BODY"      => "PACKAGE_BODY",
            "TYPE BODY"         => "TYPE_BODY",
            "MATERIALIZED VIEW" => "MATERIALIZED_VIEW",
            "JOB"               => "PROCOBJ",
            other               => other,
        };

        let rows = db
            .query(
                "SELECT DBMS_METADATA.GET_DDL(:1, :2, :3) FROM DUAL",
                &[&metadata_type, &upper_name, &schema],
            )
            .map_err(|e| anyhow!(e.to_string()))?;

        let mut ddl = String::new();
        for row_result in rows {
            let row = row_result.map_err(|e| anyhow!(e.to_string()))?;
            ddl = row.get::<usize, String>(0).map_err(|e| anyhow!(e.to_string()))?;
            break;
        }
        if ddl.is_empty() {
            return Err(anyhow!("DDL not found for {} {}", upper_type, upper_name));
        }

        Ok(clean_raw_ddl(&upper_type, &upper_name, &ddl))
    }

    fn generate_history_fix(
        &self,
        conn: &ConnectionRecord,
        naming_rules: &[HistoryNamingRule],
    ) -> Result<HistoryFixResult> {
        let db = self.connect(conn)?;
        let schema = conn.username.to_ascii_uppercase();

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

        // Owned pattern/wildcard strings, computed up front so `join_binds` and
        // `not_like_binds` below can hold stable references into them.
        let patterns: Vec<String> = rules.iter().map(|r| r.pattern.trim().to_ascii_uppercase()).collect();
        let wildcards: Vec<String> = rules
            .iter()
            .zip(&patterns)
            .map(|(r, p)| match r.match_type {
                MatchType::Prefix => format!("{p}%"),
                _ => format!("%{p}"),
            })
            .collect();

        // Find main/history table pairs: one LEFT JOIN per enabled naming rule.
        //
        // NOTE: the `oracle` crate binds parameters by their physical left-to-right
        // position in the SQL text, not by the numeral written after the colon (that's
        // just a label). The join clauses below are spliced into the SQL *before* the
        // `WHERE t.OWNER = :N` clause, so their placeholders occur first in the final
        // text — `join_binds` and `not_like_binds` are kept separate from `schema` and
        // reassembled in that same physical order below.
        let mut joins = String::new();
        let mut name_cols: Vec<String> = Vec::new();
        let mut not_like_clauses: Vec<String> = Vec::new();
        let mut join_binds: Vec<&dyn ToSql> = Vec::new();
        let mut not_like_binds: Vec<&dyn ToSql> = Vec::new();
        let mut next_idx = 2;

        for (i, rule) in rules.iter().enumerate() {
            let alias = format!("h{i}");
            match rule.match_type {
                MatchType::Prefix => joins.push_str(&format!(
                    " LEFT JOIN ALL_TABLES {alias} ON {alias}.OWNER = t.OWNER \
                      AND LOWER({alias}.TABLE_NAME) = LOWER(:{next_idx} || t.TABLE_NAME)"
                )),
                _ => joins.push_str(&format!(
                    " LEFT JOIN ALL_TABLES {alias} ON {alias}.OWNER = t.OWNER \
                      AND LOWER({alias}.TABLE_NAME) = LOWER(t.TABLE_NAME || :{next_idx})"
                )),
            }
            join_binds.push(&patterns[i]);
            next_idx += 1;
            not_like_clauses.push(format!("UPPER(t.TABLE_NAME) NOT LIKE :{next_idx}"));
            not_like_binds.push(&wildcards[i]);
            next_idx += 1;
            name_cols.push(format!("{alias}.TABLE_NAME"));
        }

        let mut binds: Vec<&dyn ToSql> = Vec::new();
        binds.extend(join_binds);
        binds.push(&schema);
        binds.extend(not_like_binds);

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
            "SELECT t.TABLE_NAME, {history_name_expr} \
             FROM ALL_TABLES t{joins} \
             WHERE t.OWNER = :1 \
               AND {not_like} \
               AND ({has_pair_clause}) \
             ORDER BY t.TABLE_NAME",
            not_like = not_like_clauses.join(" AND "),
        );

        // Prefix patterns are also used to recognize a history column that carries the
        // naming rule's prefix ahead of the main table's column name (e.g. `HIST_ID`).
        let prefix_patterns: Vec<&str> = rules
            .iter()
            .zip(&patterns)
            .filter(|(r, _)| r.match_type == MatchType::Prefix)
            .map(|(_, p)| p.as_str())
            .collect();

        let pair_rows = db
            .query(&pair_sql, &binds)
            .map_err(|e| anyhow!(e.to_string()))?;

        let mut pairs: Vec<(String, String)> = Vec::new();
        for row_result in pair_rows {
            let row = row_result.map_err(|e| anyhow!(e.to_string()))?;
            let main_table: String = row.get(0).map_err(|e| anyhow!(e.to_string()))?;
            let history_table: String = row.get(1).map_err(|e| anyhow!(e.to_string()))?;
            pairs.push((main_table, history_table));
        }

        let col_sql = "SELECT col.COLUMN_NAME, col.DATA_TYPE, \
                              TO_CHAR(DECODE(col.DATA_TYPE, \
                                'NUMBER', col.DATA_PRECISION, \
                                'DATE', NULL, \
                                'NVARCHAR2', col.CHAR_LENGTH, \
                                col.DATA_LENGTH)) AS DATA_LENGTH \
                       FROM ALL_TAB_COLUMNS col \
                       WHERE col.TABLE_NAME = :1 AND col.OWNER = :2 \
                         AND col.DATA_TYPE NOT IN ('CLOB','BLOB','NCLOB','NBLOB') \
                         AND col.COLUMN_ID != 1 \
                       ORDER BY col.COLUMN_ID";

        let mut issues: Vec<HistoryTableIssue> = Vec::new();
        let mut fix_sql = String::new();

        for (main_table, history_table) in &pairs {
            let main_cols = fetch_columns(&db, col_sql, main_table, &schema)?;
            let history_cols = fetch_columns(&db, col_sql, history_table, &schema)?;

            let mut table_fixes = String::new();

            for (col_name, data_type, data_len) in &main_cols {
                let main_fmt = fmt_type(data_type, data_len.as_deref());

                // Match: same name, or the history column carries one of the configured prefixes.
                let found = history_cols.iter().find(|(hcol, _, _)| {
                    hcol.eq_ignore_ascii_case(col_name)
                        || prefix_patterns.iter().any(|p| hcol.eq_ignore_ascii_case(&format!("{p}{col_name}")))
                });

                match found {
                    None => {
                        issues.push(HistoryTableIssue {
                            history_table: history_table.clone(),
                            column_name: col_name.clone(),
                            issue_type:  "MISSING".to_string(),
                            main_type:   main_fmt.clone(),
                            history_type: String::new(),
                        });
                        table_fixes.push_str(&format!(
                            "ALTER TABLE {history_table} ADD {col_name} {main_fmt};\n"
                        ));
                    }
                    Some((history_col, history_dt, history_len)) => {
                        let history_fmt = fmt_type(history_dt, history_len.as_deref());
                        if main_fmt != history_fmt {
                            issues.push(HistoryTableIssue {
                                history_table: history_table.clone(),
                                column_name: col_name.clone(),
                                issue_type:  "TYPE_MISMATCH".to_string(),
                                main_type:   main_fmt.clone(),
                                history_type: history_fmt.clone(),
                            });
                            table_fixes.push_str(&format!(
                                "ALTER TABLE {history_table} MODIFY {history_col} {main_fmt};\n"
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
    }
}

fn fmt_type(data_type: &str, data_length: Option<&str>) -> String {
    match data_type {
        "NUMBER" | "VARCHAR2" | "NVARCHAR2" => {
            if let Some(len) = data_length {
                let trimmed = len.trim();
                if !trimmed.is_empty() && trimmed != "0" {
                    return format!("{}({})", data_type, trimmed);
                }
            }
            data_type.to_string()
        }
        _ => data_type.to_string(),
    }
}

fn fetch_columns(
    db: &Connection,
    sql: &str,
    table_name: &str,
    schema: &str,
) -> Result<Vec<(String, String, Option<String>)>> {
    let rows = db
        .query(sql, &[&table_name, &schema])
        .map_err(|e| anyhow!(e.to_string()))?;
    let mut cols = Vec::new();
    for row_result in rows {
        let row = row_result.map_err(|e| anyhow!(e.to_string()))?;
        let col_name:   String         = row.get(0).map_err(|e| anyhow!(e.to_string()))?;
        let data_type:  String         = row.get(1).map_err(|e| anyhow!(e.to_string()))?;
        let data_len:   Option<String> = row.get(2).ok().flatten();
        cols.push((col_name, data_type, data_len));
    }
    Ok(cols)
}

fn clean_sequence_ddl(ddl: &str, name: &str) -> String {
    // Remove schema-qualified quoted name like "SCHEMA"."NAME" -> NAME
    // Pattern: one or two quoted identifiers separated by a dot before the sequence name
    let re_schema = regex::Regex::new(r#"(?i)(CREATE\s+SEQUENCE\s+)"[^"]*"\."[^"]*""#).ok();
    let ddl = if let Some(re) = re_schema {
        re.replace(ddl, format!("${{1}}{name}")).into_owned()
    } else {
        ddl.to_string()
    };

    // Strip default-value noise keywords (case-insensitive, with surrounding spaces)
    let noise: &[&str] = &["NOORDER", "NOCYCLE", "NOKEEP", "NOSCALE", "GLOBAL"];
    let mut result = ddl;
    for kw in noise {
        // Match the keyword surrounded by whitespace
        let pat = format!(r"(?i)\s+{kw}\b");
        if let Ok(re) = regex::Regex::new(&pat) {
            result = re.replace_all(&result, "").into_owned();
        }
    }

    // Normalize START WITH to 1: GET_DDL emits the sequence's current value, but a deploy
    // script should always seed from 1.
    if let Ok(re) = regex::Regex::new(r"(?i)START\s+WITH\s+\d+") {
        result = re.replace_all(&result, "START WITH 1").into_owned();
    }

    // Collapse multiple spaces (but not newlines) to one
    if let Ok(re) = regex::Regex::new(r"  +") {
        result = re.replace_all(&result, " ").into_owned();
    }

    result.trim().trim_end_matches(';').trim().to_string()
}

// ── Catalog-based table DDL (PL/SQL Developer style) ──────────────────────────

struct ColumnDef {
    name: String,            // as stored (upper)
    type_str: String,        // formatted, e.g. NUMBER(10), VARCHAR2(20)
    default: Option<String>, // verbatim DATA_DEFAULT, trimmed
    not_null: bool,
}

struct IndexDef {
    name: String,
    unique: bool,
    parts: Vec<String>, // column names or function-based expressions
}

enum ConstraintKind {
    Primary,
    Unique,
    ForeignKey,
    Check,
}

struct ConstraintDef {
    name: String,
    kind: ConstraintKind,
    cols: Vec<String>,
    ref_table: Option<String>,   // FK referenced table
    ref_cols: Vec<String>,       // FK referenced columns
    delete_rule: Option<String>, // CASCADE / SET NULL (NO ACTION omitted)
    novalidate: bool,
    check_cond: Option<String>,
}

struct TableModel {
    name: String,
    columns: Vec<ColumnDef>,
    table_comment: Option<String>,
    column_comments: Vec<(String, String)>, // (col, text) in column order
    indexes: Vec<IndexDef>,
    constraints: Vec<ConstraintDef>,
}

/// Format an Oracle column type the way PL/SQL Developer prints it: `NUMBER(10)` (zero scale
/// dropped), `NUMBER(10,2)`, bare `NUMBER`, `VARCHAR2(20)` / `VARCHAR2(20 CHAR)`, `RAW(n)`,
/// otherwise the stored `DATA_TYPE` (covers DATE, TIMESTAMP(n), CLOB, BLOB, FLOAT, …).
fn format_column_type(
    data_type: &str,
    data_length: Option<i64>,
    data_precision: Option<i64>,
    data_scale: Option<i64>,
    char_length: Option<i64>,
    char_used: Option<&str>,
) -> String {
    let dt = data_type.to_uppercase();
    match dt.as_str() {
        "NUMBER" => match (data_precision, data_scale) {
            (Some(p), Some(s)) if s > 0 => format!("NUMBER({p},{s})"),
            (Some(p), _) => format!("NUMBER({p})"),
            (None, Some(s)) if s > 0 => format!("NUMBER(*,{s})"),
            _ => "NUMBER".to_string(),
        },
        "VARCHAR2" | "VARCHAR" | "CHAR" | "NVARCHAR2" | "NCHAR" => {
            if char_used == Some("C") {
                match char_length {
                    Some(l) => format!("{dt}({l} CHAR)"),
                    None => dt,
                }
            } else {
                match data_length {
                    Some(l) => format!("{dt}({l})"),
                    None => dt,
                }
            }
        }
        "RAW" => match data_length {
            Some(l) => format!("RAW({l})"),
            None => "RAW".to_string(),
        },
        _ => data_type.to_string(),
    }
}

fn escape_sql(s: &str) -> String {
    s.replace('\'', "''")
}

/// True for an auto NOT-NULL check constraint (`"COL" IS NOT NULL`), which PL/SQL Developer
/// renders inline as `not null` rather than as a separate constraint.
fn is_not_null_check(cond: &str) -> bool {
    regex::Regex::new(r#"(?i)^"?[A-Za-z0-9_$#]+"?\s+IS\s+NOT\s+NULL$"#)
        .map(|re| re.is_match(cond.trim()))
        .unwrap_or(false)
}

/// Fetch a single string value from a two-bind query, or `None`.
fn query_one(db: &Connection, sql: &str, p1: &str, p2: &str) -> Option<String> {
    let rows = db.query(sql, &[&p1, &p2]).ok()?;
    for r in rows.flatten() {
        let v: Option<String> = r.get(0).ok().flatten();
        if let Some(s) = v {
            return Some(s);
        }
    }
    None
}

/// Ordered column names for a constraint (PK/UK/FK), by `POSITION`.
fn cons_columns(db: &Connection, owner: &str, cons: &str) -> Vec<String> {
    let mut map: BTreeMap<i64, String> = BTreeMap::new();
    if let Ok(rows) = db.query(
        "SELECT POSITION, COLUMN_NAME FROM ALL_CONS_COLUMNS WHERE OWNER=:1 AND CONSTRAINT_NAME=:2",
        &[&owner, &cons],
    ) {
        for r in rows.flatten() {
            let pos: Option<i64> = r.get(0).ok().flatten();
            let col: Option<String> = r.get(1).ok().flatten();
            if let Some(c) = col {
                map.insert(pos.unwrap_or(0), c);
            }
        }
    }
    map.into_values().collect()
}

/// Ordered index key parts; for function-based indexes the SYS_NC… columns are replaced by
/// their expressions (e.g. `UPPER(INOS)`).
fn index_parts(db: &Connection, schema: &str, index: &str, function_based: bool) -> Vec<String> {
    let mut map: BTreeMap<i64, String> = BTreeMap::new();
    if let Ok(rows) = db.query(
        "SELECT COLUMN_POSITION, COLUMN_NAME FROM ALL_IND_COLUMNS \
         WHERE INDEX_OWNER=:1 AND INDEX_NAME=:2",
        &[&schema, &index],
    ) {
        for r in rows.flatten() {
            let pos: Option<i64> = r.get(0).ok().flatten();
            let col: Option<String> = r.get(1).ok().flatten();
            if let (Some(p), Some(c)) = (pos, col) {
                map.insert(p, c);
            }
        }
    }
    if function_based {
        if let Ok(rows) = db.query(
            "SELECT COLUMN_POSITION, COLUMN_EXPRESSION FROM ALL_IND_EXPRESSIONS \
             WHERE INDEX_OWNER=:1 AND INDEX_NAME=:2",
            &[&schema, &index],
        ) {
            for r in rows.flatten() {
                let pos: Option<i64> = r.get(0).ok().flatten();
                let expr: Option<String> = r.get(1).ok().flatten();
                if let (Some(p), Some(e)) = (pos, expr) {
                    map.insert(p, e.trim().to_string());
                }
            }
        }
    }
    map.into_values().collect()
}

/// Query the catalog views and assemble a [`TableModel`].
fn build_table_model(db: &Connection, schema: &str, table: &str) -> Result<TableModel> {
    // Columns
    let col_rows = db
        .query(
            "SELECT COLUMN_NAME, DATA_TYPE, DATA_LENGTH, DATA_PRECISION, DATA_SCALE, \
                    CHAR_LENGTH, CHAR_USED, NULLABLE, DATA_DEFAULT \
             FROM ALL_TAB_COLUMNS WHERE OWNER = :1 AND TABLE_NAME = :2 ORDER BY COLUMN_ID",
            &[&schema, &table],
        )
        .map_err(|e| anyhow!(e.to_string()))?;
    let mut columns = Vec::new();
    for r in col_rows {
        let r = r.map_err(|e| anyhow!(e.to_string()))?;
        let name: String = r.get(0).map_err(|e| anyhow!(e.to_string()))?;
        let data_type: String = r.get(1).map_err(|e| anyhow!(e.to_string()))?;
        let data_length: Option<i64> = r.get(2).ok().flatten();
        let data_precision: Option<i64> = r.get(3).ok().flatten();
        let data_scale: Option<i64> = r.get(4).ok().flatten();
        let char_length: Option<i64> = r.get(5).ok().flatten();
        let char_used: Option<String> = r.get(6).ok().flatten();
        let nullable: Option<String> = r.get(7).ok().flatten();
        let data_default: Option<String> = r.get(8).ok().flatten();
        let default = data_default
            .map(|d| d.trim().to_string())
            .filter(|d| !d.is_empty());
        columns.push(ColumnDef {
            type_str: format_column_type(
                &data_type,
                data_length,
                data_precision,
                data_scale,
                char_length,
                char_used.as_deref(),
            ),
            name,
            default,
            not_null: nullable.as_deref() == Some("N"),
        });
    }
    if columns.is_empty() {
        return Err(anyhow!("Table {} not found in schema {}", table, schema));
    }

    // Table comment
    let table_comment = query_one(
        db,
        "SELECT COMMENTS FROM ALL_TAB_COMMENTS WHERE OWNER=:1 AND TABLE_NAME=:2",
        schema,
        table,
    )
    .filter(|s| !s.trim().is_empty());

    // Column comments → keyed by column, emitted later in column order
    let mut comment_map: HashMap<String, String> = HashMap::new();
    if let Ok(rows) = db.query(
        "SELECT COLUMN_NAME, COMMENTS FROM ALL_COL_COMMENTS \
         WHERE OWNER=:1 AND TABLE_NAME=:2 AND COMMENTS IS NOT NULL",
        &[&schema, &table],
    ) {
        for r in rows.flatten() {
            let c: Option<String> = r.get(0).ok().flatten();
            let t: Option<String> = r.get(1).ok().flatten();
            if let (Some(c), Some(t)) = (c, t) {
                if !t.trim().is_empty() {
                    comment_map.insert(c, t.trim().to_string());
                }
            }
        }
    }
    let column_comments: Vec<(String, String)> = columns
        .iter()
        .filter_map(|c| comment_map.get(&c.name).map(|t| (c.name.clone(), t.clone())))
        .collect();

    // Indexes (excluding those backing PK/UK constraints)
    let mut indexes = Vec::new();
    if let Ok(rows) = db.query(
        "SELECT i.INDEX_NAME, i.UNIQUENESS, i.INDEX_TYPE FROM ALL_INDEXES i \
         WHERE i.TABLE_OWNER=:1 AND i.TABLE_NAME=:2 \
           AND NOT EXISTS (SELECT 1 FROM ALL_CONSTRAINTS c \
                            WHERE c.OWNER=i.TABLE_OWNER AND c.TABLE_NAME=i.TABLE_NAME \
                              AND c.INDEX_NAME=i.INDEX_NAME) \
         ORDER BY i.INDEX_NAME",
        &[&schema, &table],
    ) {
        for r in rows.flatten() {
            let iname: String = match r.get(0) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let uniqueness: Option<String> = r.get(1).ok().flatten();
            let itype: Option<String> = r.get(2).ok().flatten();
            let is_fb = itype
                .as_deref()
                .map(|t| t.starts_with("FUNCTION-BASED"))
                .unwrap_or(false);
            let parts = index_parts(db, schema, &iname, is_fb);
            indexes.push(IndexDef {
                unique: uniqueness.as_deref() == Some("UNIQUE"),
                name: iname,
                parts,
            });
        }
    }

    // Constraints: PK, UK, FK, CHECK (skipping auto NOT-NULL checks)
    let mut constraints = Vec::new();
    if let Ok(rows) = db.query(
        "SELECT CONSTRAINT_NAME, CONSTRAINT_TYPE, SEARCH_CONDITION, R_OWNER, \
                R_CONSTRAINT_NAME, DELETE_RULE, VALIDATED \
         FROM ALL_CONSTRAINTS WHERE OWNER=:1 AND TABLE_NAME=:2 \
           AND CONSTRAINT_TYPE IN ('P','U','R','C') \
         ORDER BY DECODE(CONSTRAINT_TYPE,'P',1,'U',2,'R',3,'C',4), CONSTRAINT_NAME",
        &[&schema, &table],
    ) {
        for r in rows.flatten() {
            let cname: String = match r.get(0) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let ctype: String = match r.get(1) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let search: Option<String> = r.get(2).ok().flatten();
            let r_owner: Option<String> = r.get(3).ok().flatten();
            let r_cons: Option<String> = r.get(4).ok().flatten();
            let delete_rule: Option<String> = r.get(5).ok().flatten();
            let validated: Option<String> = r.get(6).ok().flatten();
            let novalidate = validated.as_deref() == Some("NOT VALIDATED");

            match ctype.as_str() {
                "P" | "U" => constraints.push(ConstraintDef {
                    kind: if ctype == "P" {
                        ConstraintKind::Primary
                    } else {
                        ConstraintKind::Unique
                    },
                    cols: cons_columns(db, schema, &cname),
                    name: cname,
                    ref_table: None,
                    ref_cols: Vec::new(),
                    delete_rule: None,
                    novalidate,
                    check_cond: None,
                }),
                "R" => {
                    let cols = cons_columns(db, schema, &cname);
                    let (ref_table, ref_cols) = match (r_owner.as_deref(), r_cons.as_deref()) {
                        (Some(o), Some(rc)) => (
                            query_one(
                                db,
                                "SELECT TABLE_NAME FROM ALL_CONSTRAINTS \
                                 WHERE OWNER=:1 AND CONSTRAINT_NAME=:2",
                                o,
                                rc,
                            ),
                            cons_columns(db, o, rc),
                        ),
                        _ => (None, Vec::new()),
                    };
                    let dr = delete_rule.filter(|d| d.to_uppercase() != "NO ACTION");
                    constraints.push(ConstraintDef {
                        name: cname,
                        kind: ConstraintKind::ForeignKey,
                        cols,
                        ref_table,
                        ref_cols,
                        delete_rule: dr,
                        novalidate,
                        check_cond: None,
                    });
                }
                "C" => {
                    let cond = search.unwrap_or_default().trim().to_string();
                    if cond.is_empty() || is_not_null_check(&cond) {
                        continue;
                    }
                    constraints.push(ConstraintDef {
                        name: cname,
                        kind: ConstraintKind::Check,
                        cols: Vec::new(),
                        ref_table: None,
                        ref_cols: Vec::new(),
                        delete_rule: None,
                        novalidate,
                        check_cond: Some(cond),
                    });
                }
                _ => {}
            }
        }
    }

    Ok(TableModel {
        name: table.to_string(),
        columns,
        table_comment,
        column_comments,
        indexes,
        constraints,
    })
}

/// Render one constraint as a PL/SQL Developer `alter table … add constraint …;` statement.
fn render_constraint(table: &str, c: &ConstraintDef) -> String {
    let head = format!("\nalter table {}\n  add constraint {} ", table, c.name);
    match c.kind {
        ConstraintKind::Primary => format!("{head}primary key ({});", c.cols.join(", ")),
        ConstraintKind::Unique => format!("{head}unique ({});", c.cols.join(", ")),
        ConstraintKind::Check => {
            format!("{head}check ({});", c.check_cond.clone().unwrap_or_default())
        }
        ConstraintKind::ForeignKey => {
            let mut s = format!(
                "{head}foreign key ({})\n  references {} ({})",
                c.cols.join(", "),
                c.ref_table.clone().unwrap_or_default(),
                c.ref_cols.join(", "),
            );
            if let Some(rule) = &c.delete_rule {
                match rule.to_uppercase().as_str() {
                    "CASCADE" => s.push_str("\n  on delete cascade"),
                    "SET NULL" => s.push_str("\n  on delete set null"),
                    _ => {}
                }
            }
            if c.novalidate {
                s.push_str("\n  novalidate");
            }
            s.push(';');
            s
        }
    }
}

/// Render the full raw `.tab` export (PL/SQL Developer style) from a [`TableModel`].
fn render_table_raw(m: &TableModel) -> String {
    let mut out = format!("create table {}\n(\n", m.name);

    let max_name = m
        .columns
        .iter()
        .map(|c| c.name.to_lowercase().chars().count())
        .max()
        .unwrap_or(0);
    for (i, c) in m.columns.iter().enumerate() {
        let col_lc = c.name.to_lowercase();
        let pad = max_name - col_lc.chars().count() + 1;
        let mut line = format!("  {}{}{}", col_lc, " ".repeat(pad), c.type_str);
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
    out.push_str(")\n;");

    if let Some(tc) = &m.table_comment {
        out.push_str(&format!(
            "\ncomment on table {}\n  is '{}';",
            m.name,
            escape_sql(tc)
        ));
    }
    for (col, text) in &m.column_comments {
        out.push_str(&format!(
            "\ncomment on column {}.{}\n  is '{}';",
            m.name,
            col.to_lowercase(),
            escape_sql(text)
        ));
    }

    for idx in &m.indexes {
        let kw = if idx.unique {
            "create unique index"
        } else {
            "create index"
        };
        out.push_str(&format!(
            "\n{} {} on {} ({});",
            kw,
            idx.name,
            m.name,
            idx.parts.join(", ")
        ));
    }

    for c in &m.constraints {
        out.push_str(&render_constraint(&m.name, c));
    }

    out
}

/// Split a CREATE statement from any trailing `COMMENT ON …` block appended by
/// `GET_DEPENDENT_DDL`. Handles both `\nCOMMENT ON` and `\n\nCOMMENT ON`.
fn split_create_and_comments(ddl: &str) -> (&str, Option<&str>) {
    let upper = ddl.to_ascii_uppercase();
    match upper.find("\nCOMMENT ON") {
        Some(idx) => (ddl[..idx].trim(), Some(ddl[idx..].trim())),
        None => (ddl, None),
    }
}

/// Split a `COMMENT ON …` block into individual statements (one per `COMMENT ON`),
/// folding continuation lines into the preceding statement.
fn split_comments(comment_part: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in comment_part.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.to_ascii_uppercase().starts_with("COMMENT ON") {
            out.push(trimmed.to_string());
        } else if let Some(last) = out.last_mut() {
            last.push(' ');
            last.push_str(trimmed);
        }
    }
    out
}

fn indent_lines(s: &str, spaces: usize) -> String {
    let pad = " ".repeat(spaces);
    s.lines()
        .map(|l| if l.trim().is_empty() { String::new() } else { format!("{pad}{l}") })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Produce the raw DDL shown in the preview and written to the code file. Code objects are
/// normalized to `CREATE OR REPLACE`; tables are reformatted; sequences are cleaned.
/// Materialized views and jobs keep their `DBMS_METADATA` form.
fn clean_raw_ddl(object_type: &str, name: &str, raw_ddl: &str) -> String {
    let ddl = raw_ddl.trim();
    match object_type {
        "VIEW" | "PROCEDURE" | "FUNCTION" | "PACKAGE" | "PACKAGE BODY" | "TYPE" | "TYPE BODY"
        | "SYNONYM" | "TRIGGER" => {
            // Strip EDITIONABLE/NONEDITIONABLE and schema-qualified names added by DBMS_METADATA
            let ddl = regex::Regex::new(r"(?i)\s+(?:NON)?EDITIONABLE\s+")
                .expect("valid regex")
                .replace_all(ddl, " ")
                .into_owned();
            // Strip double quotes from the object name in the CREATE header only.
            // Targets: CREATE OR REPLACE [type] "NAME" — does not touch body content.
            let ddl = regex::Regex::new(
                r#"(?i)(CREATE\s+OR\s+REPLACE\s+(?:PACKAGE\s+BODY|PACKAGE|PROCEDURE|FUNCTION|TRIGGER|VIEW|TYPE\s+BODY|TYPE|SYNONYM)\s+)"([^"]+)""#
            )
            .expect("valid regex")
            .replace_all(&ddl, "$1$2")
            .into_owned();
            let ddl = ddl.trim();
            if ddl.to_ascii_uppercase().starts_with("CREATE OR REPLACE") {
                ddl.to_string()
            } else {
                ddl.replacen("CREATE ", "CREATE OR REPLACE ", 1)
            }
        }
        "SEQUENCE" => clean_sequence_ddl(ddl, name),
        // MATERIALIZED VIEW, JOB, etc.: pass the DBMS_METADATA output through.
        // (TABLE is handled earlier via the catalog model and never reaches here.)
        _ => ddl.to_string(),
    }
}

/// Strip a SQL*Plus `/` terminator and a trailing `;`, but preserve the `;` that closes a
/// PL/SQL block (`END;`) so anonymous blocks (e.g. scheduler jobs) stay valid.
fn strip_terminator(s: &str) -> String {
    let s = s.trim();
    let s = s.strip_suffix('/').map(|x| x.trim_end()).unwrap_or(s);
    if s.to_ascii_uppercase().trim_end().ends_with("END;") {
        s.to_string()
    } else {
        s.trim_end_matches(';').trim_end().to_string()
    }
}

/// Render one statement as an indented `EXECUTE IMMEDIATE q'[…]';` for a deploy block.
/// Multi-line statements (e.g. CREATE TABLE) are placed on their own lines inside the quote.
fn execute_immediate(stmt: &str) -> String {
    let stmt = strip_terminator(stmt);
    if stmt.contains('\n') {
        let indented = indent_lines(&stmt, 12);
        format!("        EXECUTE IMMEDIATE q'[\n{indented}\n        ]';")
    } else {
        format!("        EXECUTE IMMEDIATE q'[{stmt}]';")
    }
}

/// Build one idempotent guarded block: run `statements` only when the object is absent,
/// determined by `SELECT COUNT(*) FROM {view} WHERE {col} = '{name}'`.
fn guarded_block(view: &str, col: &str, name: &str, statements: &[String]) -> String {
    let body = statements
        .iter()
        .filter(|s| !s.trim().is_empty())
        .map(|s| execute_immediate(s))
        .collect::<Vec<_>>()
        .join("\n\n");
    format!(
        "DECLARE\n    v_count NUMBER;\nBEGIN\n    SELECT COUNT(*) INTO v_count FROM {view}\n    WHERE {col} = '{name}';\n\n    IF v_count = 0 THEN\n{body}\n    END IF;\nEND;\n/"
    )
}

/// Deploy wrapper for a single-statement structural object (sequence / MV / job), with any
/// trailing comment statements emitted inside the same guarded block.
fn single_guarded(view: &str, col: &str, name: &str, raw_cleaned: &str) -> String {
    let ddl = raw_cleaned.trim();
    let (create_part, comment_part) = split_create_and_comments(ddl);
    let mut stmts = vec![create_part.to_string()];
    if let Some(comments) = comment_part {
        stmts.extend(split_comments(comments));
    }
    guarded_block(view, col, name, &stmts)
}

/// Split the generated raw table DDL into individual statements. A new statement begins at a
/// line that starts (column 0) with a known DDL keyword; everything else (indented lines,
/// `)`, `;`) continues the current statement. Reliable because we generate the raw ourselves.
fn split_ddl_statements(raw: &str) -> Vec<String> {
    const STARTS: &[&str] = &[
        "create table",
        "create unique index",
        "create index",
        "comment on",
        "alter table",
    ];
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

/// Pull the identifier following one of `keywords` (e.g. the index name after
/// `create index`, the constraint name after `add constraint`).
fn token_after(stmt: &str, keyword: &str) -> Option<String> {
    let lower = stmt.to_ascii_lowercase();
    let pos = lower.find(keyword)? + keyword.len();
    stmt[pos..]
        .split_whitespace()
        .next()
        .map(|t| t.trim_matches('"').to_string())
}

/// Deploy wrapper for a table: the CREATE TABLE (+ comments) in one block guarded by
/// USER_TABLES, then each index guarded by USER_INDEXES and each constraint by
/// USER_CONSTRAINTS — every statement individually idempotent.
fn build_table_deploy(name: &str, raw: &str) -> String {
    let mut table_group: Vec<String> = Vec::new();
    let mut blocks: Vec<String> = Vec::new();

    for stmt in split_ddl_statements(raw) {
        let lower = stmt.trim_start().to_ascii_lowercase();
        if lower.starts_with("create unique index") || lower.starts_with("create index") {
            let idx = token_after(&stmt, "index").unwrap_or_default();
            blocks.push(guarded_block("USER_INDEXES", "INDEX_NAME", &idx, &[stmt]));
        } else if lower.starts_with("alter table") {
            let cons = token_after(&stmt, "add constraint").unwrap_or_default();
            blocks.push(guarded_block("USER_CONSTRAINTS", "CONSTRAINT_NAME", &cons, &[stmt]));
        } else {
            // create table + comment on … → grouped into the table block
            table_group.push(stmt);
        }
    }

    let mut out: Vec<String> = Vec::new();
    if !table_group.is_empty() {
        out.push(guarded_block("USER_TABLES", "TABLE_NAME", name, &table_group));
    }
    out.extend(blocks);
    out.join("\n\n")
}

/// Wrap raw DDL in the idempotent deploy form. Tables fan out into one guarded block per
/// statement (table/indexes/constraints); sequences/MVs/jobs get a single guarded block;
/// code objects are already idempotent via `CREATE OR REPLACE` and pass through unchanged.
pub(crate) fn build_deploy_script(object_type: &str, name: &str, raw_cleaned: &str) -> String {
    match object_type {
        "TABLE" => build_table_deploy(name, raw_cleaned),
        "SEQUENCE" => single_guarded("USER_SEQUENCES", "SEQUENCE_NAME", name, raw_cleaned),
        "MATERIALIZED VIEW" => single_guarded("USER_MVIEWS", "MVIEW_NAME", name, raw_cleaned),
        "JOB" => single_guarded("USER_SCHEDULER_JOBS", "JOB_NAME", name, raw_cleaned),
        // Code objects rely on CREATE OR REPLACE for idempotency.
        _ => raw_cleaned.trim().to_string(),
    }
}

fn ensure_client_initialized() -> Result<()> {
    if InitParams::is_initialized() {
        return Ok(());
    }

    let dir = CLIENT_LIB_DIR
        .read()
        .ok()
        .map(|g| g.clone())
        .unwrap_or_default();
    if !dir.trim().is_empty() {
        let _ = configure_client_lib_dir(&dir)?;
    }

    Ok(())
}

pub fn configure_client_lib_dir(dir: &str) -> Result<bool> {
    let trimmed = dir.trim();
    if trimmed.is_empty() {
        return Ok(false);
    }

    let mut params = InitParams::new();
    params
        .oracle_client_lib_dir(trimmed)
        .map_err(|e| anyhow!(e.to_string()))?;
    params.init().map_err(|e| anyhow!(e.to_string()))
}

/// How a result-set column should be materialized into a display string.
#[derive(Clone, Copy)]
enum CellKind {
    /// Plain value readable as text (VARCHAR2, NUMBER, DATE, …).
    Text,
    /// Text LOB — shown as a `<CLOB>` placeholder; full text fetched on demand.
    TextLob,
    /// Large binary LOB — shown as a `<BLOB>` placeholder, never read as text.
    BinaryLob,
    /// Small RAW value — rendered inline as hex.
    Raw,
}

fn cell_kind(oracle_type: &OracleType) -> CellKind {
    match oracle_type {
        OracleType::BLOB | OracleType::BFILE | OracleType::LongRaw => CellKind::BinaryLob,
        OracleType::CLOB | OracleType::NCLOB | OracleType::Long => CellKind::TextLob,
        OracleType::Raw(_) => CellKind::Raw,
        _ => CellKind::Text,
    }
}

/// Cap on materialized LOB content (characters of text / bytes decoded), to bound
/// memory; longer values are truncated. Excel further clips cells at 32,767 chars.
const MATERIALIZE_TEXT_CAP: usize = 1_000_000;

fn read_cell(row: &Row, i: usize, kind: CellKind, materialize_lobs: bool) -> Option<String> {
    match kind {
        CellKind::BinaryLob => {
            if !materialize_lobs {
                return Some("<BLOB>".to_string());
            }
            match row.get::<usize, Option<Vec<u8>>>(i) {
                Ok(Some(bytes)) => Some(bytes_to_display(&bytes)),
                Ok(None) => None,
                Err(_) => Some("<BLOB>".to_string()),
            }
        }
        CellKind::TextLob => {
            if !materialize_lobs {
                return Some("<CLOB>".to_string());
            }
            match row.get::<usize, Option<String>>(i) {
                Ok(Some(s)) => Some(cap_chars(s, MATERIALIZE_TEXT_CAP)),
                Ok(None) => None,
                Err(_) => Some("<CLOB>".to_string()),
            }
        }
        CellKind::Raw => match row.get::<usize, Option<Vec<u8>>>(i) {
            Ok(Some(bytes)) => Some(bytes_to_hex(&bytes, 512)),
            Ok(None) => None,
            Err(_) => Some("<RAW>".to_string()),
        },
        CellKind::Text => match row.get::<usize, Option<String>>(i) {
            Ok(v) => v,
            Err(_) => Some("<?>".to_string()),
        },
    }
}

/// Truncate a string to at most `cap` characters (on a char boundary).
fn cap_chars(s: String, cap: usize) -> String {
    if s.chars().count() > cap {
        s.chars().take(cap).collect()
    } else {
        s
    }
}

/// Render BLOB bytes for inline display: decode as UTF-8 text when the content looks
/// textual (valid UTF-8, no NUL bytes), otherwise fall back to a hex string.
fn bytes_to_display(bytes: &[u8]) -> String {
    let sample = &bytes[..bytes.len().min(8192)];
    let looks_text = std::str::from_utf8(sample)
        .map(|s| !s.contains('\0'))
        .unwrap_or(false);
    if looks_text {
        match String::from_utf8(bytes.to_vec()) {
            Ok(s) => cap_chars(s, MATERIALIZE_TEXT_CAP),
            Err(_) => bytes_to_hex(bytes, 8192),
        }
    } else {
        bytes_to_hex(bytes, 8192)
    }
}

/// A short, human-readable Oracle type label for a column (sent to the UI so it can
/// flag openable LOB cells and show types in tooltips).
fn type_label(oracle_type: &OracleType) -> String {
    match oracle_type {
        OracleType::Varchar2(_) => "VARCHAR2".to_string(),
        OracleType::NVarchar2(_) => "NVARCHAR2".to_string(),
        OracleType::Char(_) => "CHAR".to_string(),
        OracleType::NChar(_) => "NCHAR".to_string(),
        OracleType::Number(_, _) => "NUMBER".to_string(),
        OracleType::Float(_) => "FLOAT".to_string(),
        OracleType::BinaryFloat => "BINARY_FLOAT".to_string(),
        OracleType::BinaryDouble => "BINARY_DOUBLE".to_string(),
        OracleType::Int64 => "INTEGER".to_string(),
        OracleType::UInt64 => "INTEGER".to_string(),
        OracleType::Date => "DATE".to_string(),
        OracleType::Timestamp(_) => "TIMESTAMP".to_string(),
        OracleType::TimestampTZ(_) => "TIMESTAMP WITH TIME ZONE".to_string(),
        OracleType::TimestampLTZ(_) => "TIMESTAMP WITH LOCAL TIME ZONE".to_string(),
        OracleType::IntervalDS(_, _) => "INTERVAL DAY TO SECOND".to_string(),
        OracleType::IntervalYM(_) => "INTERVAL YEAR TO MONTH".to_string(),
        OracleType::CLOB => "CLOB".to_string(),
        OracleType::NCLOB => "NCLOB".to_string(),
        OracleType::BLOB => "BLOB".to_string(),
        OracleType::BFILE => "BFILE".to_string(),
        OracleType::Long => "LONG".to_string(),
        OracleType::LongRaw => "LONG RAW".to_string(),
        OracleType::Raw(_) => "RAW".to_string(),
        OracleType::Rowid => "ROWID".to_string(),
        OracleType::Boolean => "BOOLEAN".to_string(),
        other => format!("{other:?}"),
    }
}

/// Uppercase hex of `bytes`, capped at `max` bytes with a count suffix when truncated.
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

#[cfg(test)]
#[path = "tests/oracle_repository.rs"]
mod tests;
