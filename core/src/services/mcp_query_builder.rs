//! Builds the SELECT behind the MCP `query_table` tool from structured input, so the
//! table/column allowlist is enforced exactly: the agent never supplies SQL text, only
//! identifiers (validated and quoted) and values (bound on Oracle, escaped literals on
//! Postgres).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::models::DbType;

const MAX_IN_VALUES: usize = 100;
const MAX_FILTERS: usize = 20;
const MAX_VALUE_LEN: usize = 4_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FilterOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Like,
    In,
    IsNull,
    IsNotNull,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct Filter {
    pub column: String,
    pub op: FilterOp,
    /// A scalar for most operators, an array of scalars for `in`; omitted for `is_null` / `is_not_null`.
    #[serde(default)]
    pub value: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct OrderBy {
    pub column: String,
    #[serde(default)]
    pub descending: bool,
}

#[derive(Clone, Debug)]
pub struct TableQuery<'a> {
    /// Schema/owner the table lives in.
    pub schema: &'a str,
    pub table: &'a str,
    pub columns: &'a [String],
    pub filters: &'a [Filter],
    pub order_by: &'a [OrderBy],
    pub limit: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltQuery {
    pub sql: String,
    /// Oracle bind values in `:1`, `:2`, … order; empty for Postgres (literals inlined).
    pub binds: Vec<String>,
}

/// A plain SQL identifier — what real table and column names look like.
pub fn valid_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && name.len() <= 128
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '$' | '#'))
}

fn quote(name: &str, db_type: DbType) -> String {
    let name = match db_type {
        DbType::Oracle => name.to_ascii_uppercase(),
        DbType::Postgres => name.to_string(),
    };
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn scalar_text(value: &Value, db_type: DbType) -> Result<String, String> {
    let text = match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => match (db_type, b) {
            (DbType::Oracle, true) => "1".to_string(),
            (DbType::Oracle, false) => "0".to_string(),
            (DbType::Postgres, _) => b.to_string(),
        },
        _ => return Err("Filter values must be strings, numbers or booleans.".to_string()),
    };
    if text.len() > MAX_VALUE_LEN {
        return Err("A filter value is too long.".to_string());
    }
    if text.contains('\0') {
        return Err("Filter values cannot contain NUL characters.".to_string());
    }
    Ok(text)
}

fn pg_literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// Renders a value: an Oracle bind placeholder (value pushed to `binds`) or an escaped
/// Postgres literal. Postgres' `standard_conforming_strings` makes doubling quotes the only
/// escape a `'…'` literal needs; the session sets it explicitly.
fn push_value(text: String, db_type: DbType, binds: &mut Vec<String>) -> String {
    match db_type {
        DbType::Oracle => {
            binds.push(text);
            format!(":{}", binds.len())
        }
        DbType::Postgres => pg_literal(&text),
    }
}

pub fn build(query: &TableQuery, db_type: DbType) -> Result<BuiltQuery, String> {
    if !valid_identifier(query.schema) {
        return Err(format!("Unsupported schema name '{}'.", query.schema));
    }
    if !valid_identifier(query.table) {
        return Err(format!(
            "Unsupported table name '{}': only plain identifiers are accepted.",
            query.table
        ));
    }
    if query.columns.is_empty() {
        return Err("No readable columns to select.".to_string());
    }
    if query.filters.len() > MAX_FILTERS {
        return Err(format!("At most {MAX_FILTERS} filters are supported."));
    }
    for name in query
        .columns
        .iter()
        .chain(query.filters.iter().map(|f| &f.column))
        .chain(query.order_by.iter().map(|o| &o.column))
    {
        if !valid_identifier(name) {
            return Err(format!(
                "Unsupported column name '{name}': only plain identifiers are accepted."
            ));
        }
    }

    let mut binds = Vec::new();
    let mut conditions = Vec::new();
    for filter in query.filters {
        let column = quote(&filter.column, db_type);
        let condition = match filter.op {
            FilterOp::IsNull => format!("{column} IS NULL"),
            FilterOp::IsNotNull => format!("{column} IS NOT NULL"),
            FilterOp::In => {
                let Some(Value::Array(items)) = &filter.value else {
                    return Err(format!(
                        "Filter on {} needs an array for 'in'.",
                        filter.column
                    ));
                };
                if items.is_empty() || items.len() > MAX_IN_VALUES {
                    return Err(format!(
                        "'in' filters need between 1 and {MAX_IN_VALUES} values."
                    ));
                }
                let mut rendered = Vec::new();
                for item in items {
                    rendered.push(push_value(scalar_text(item, db_type)?, db_type, &mut binds));
                }
                format!("{column} IN ({})", rendered.join(", "))
            }
            op => {
                let Some(value) = &filter.value else {
                    return Err(format!("Filter on {} needs a value.", filter.column));
                };
                if value.is_array() || value.is_object() || value.is_null() {
                    return Err(format!("Filter on {} needs a scalar value.", filter.column));
                }
                let operator = match op {
                    FilterOp::Eq => "=",
                    FilterOp::Ne => "<>",
                    FilterOp::Lt => "<",
                    FilterOp::Le => "<=",
                    FilterOp::Gt => ">",
                    FilterOp::Ge => ">=",
                    FilterOp::Like => "LIKE",
                    _ => unreachable!("handled above"),
                };
                let rendered = push_value(scalar_text(value, db_type)?, db_type, &mut binds);
                format!("{column} {operator} {rendered}")
            }
        };
        conditions.push(condition);
    }

    let select_list = query
        .columns
        .iter()
        .map(|c| quote(c, db_type))
        .collect::<Vec<_>>()
        .join(", ");
    let from = format!(
        "{}.{}",
        quote(query.schema, db_type),
        quote(query.table, db_type)
    );
    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };
    let order_clause = if query.order_by.is_empty() {
        String::new()
    } else {
        let items: Vec<String> = query
            .order_by
            .iter()
            .map(|o| {
                format!(
                    "{}{}",
                    quote(&o.column, db_type),
                    if o.descending { " DESC" } else { "" }
                )
            })
            .collect();
        format!(" ORDER BY {}", items.join(", "))
    };
    // One row beyond the cap lets callers tell "exactly N rows" from "truncated".
    let fetch = query.limit.saturating_add(1);

    let sql = match db_type {
        DbType::Oracle => format!(
            "SELECT * FROM (SELECT {select_list} FROM {from}{where_clause}{order_clause}) WHERE ROWNUM <= {fetch}"
        ),
        DbType::Postgres => {
            format!("SELECT {select_list} FROM {from}{where_clause}{order_clause} LIMIT {fetch}")
        }
    };
    Ok(BuiltQuery { sql, binds })
}

#[cfg(test)]
#[path = "tests/mcp_query_builder.rs"]
mod tests;
