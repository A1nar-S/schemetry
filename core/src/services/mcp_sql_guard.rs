//! Best-effort static checks on free-form SQL before the MCP `run_query` tool runs it.
//!
//! The goal is to keep an agent inside the table allowlist and inside plain reads. It is
//! deliberately conservative (anything it cannot understand is rejected) but it is *not*
//! a hard guarantee — views, synonyms, functions that read tables internally and dynamic
//! SQL cannot be seen statically. The hard boundary is a read-only transaction plus,
//! ideally, a dedicated database account that can only `SELECT` the allowed tables.

use crate::models::DbType;

const MAX_SQL_LEN: usize = 20_000;

/// Words that never belong in a read-only query (they betray data-modifying CTEs, locks
/// and `SELECT … INTO`). Quoted identifiers are exempt.
const FORBIDDEN_WORDS: &[&str] = &[
    "INSERT", "UPDATE", "DELETE", "MERGE", "DROP", "ALTER", "CREATE", "TRUNCATE", "GRANT",
    "REVOKE", "EXECUTE", "EXEC", "CALL", "COPY", "LOCK", "INTO",
    // Postgres' `TABLE name` shorthand reads a table without FROM/JOIN.
    "TABLE",
];

/// Functions/packages that read or write outside the allowlist or the database.
const FORBIDDEN_FUNCTIONS: &[&str] = &[
    "DBLINK",
    "DBLINK_EXEC",
    "DBLINK_CONNECT",
    "QUERY_TO_XML",
    "QUERY_TO_XML_AND_XMLSCHEMA",
    "QUERY_TO_XMLSCHEMA",
    "CURSOR_TO_XML",
    "TABLE_TO_XML",
    "TABLE_TO_XML_AND_XMLSCHEMA",
    "SCHEMA_TO_XML",
    "DATABASE_TO_XML",
    "DBMS_XMLGEN",
    "DBMS_XMLQUERY",
    "XMLTYPE",
    "DBURITYPE",
    "HTTPURITYPE",
    "XDBURITYPE",
    "DBMS_LOB",
    "DBMS_METADATA",
    "SYS_CONTEXT",
    "DBMS_SQL",
    "DBMS_SCHEDULER",
    "DBMS_JOB",
    "UTL_HTTP",
    "UTL_FILE",
    "UTL_TCP",
    "UTL_SMTP",
    "UTL_INADDR",
    "PG_READ_FILE",
    "PG_READ_BINARY_FILE",
    "PG_LS_DIR",
    "PG_STAT_FILE",
    "LO_IMPORT",
    "LO_EXPORT",
    "LO_GET",
    "PG_SLEEP",
    "SET_CONFIG",
    "PG_TERMINATE_BACKEND",
    "PG_CANCEL_BACKEND",
];

/// Keywords that end an alias position when scanning FROM lists.
const CLAUSE_ENDERS: &[&str] = &[
    "WHERE",
    "GROUP",
    "ORDER",
    "HAVING",
    "UNION",
    "INTERSECT",
    "EXCEPT",
    "MINUS",
    "LIMIT",
    "OFFSET",
    "FETCH",
    "CONNECT",
    "START",
    "WINDOW",
    "FOR",
    "RETURNING",
    "QUALIFY",
];

/// Functions whose parentheses legitimately contain a `FROM` that isn't a table source.
const FROM_IN_ARGS: &[&str] = &["EXTRACT", "TRIM", "SUBSTRING", "OVERLAY", "POSITION"];

#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    /// Unquoted word, upper-cased for keyword checks (original kept for names).
    Word(String),
    Quoted(String),
    Str,
    Number,
    Symbol(char),
}

/// A table the statement reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableRef {
    pub schema: Option<String>,
    pub name: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct AnalyzedSql {
    /// The statement without a trailing semicolon, ready to run.
    pub sql: String,
    pub tables: Vec<TableRef>,
}

fn tokenize(sql: &str, db_type: DbType) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = sql.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '-' && chars.get(i + 1) == Some(&'-') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            // Postgres nests block comments, Oracle doesn't — follow the engine so
            // nothing can hide inside what we think is a comment.
            let mut depth = 1;
            i += 2;
            while i < chars.len() && depth > 0 {
                if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    i += 2;
                } else if db_type == DbType::Postgres
                    && chars[i] == '/'
                    && chars.get(i + 1) == Some(&'*')
                {
                    depth += 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if depth > 0 {
                return Err("Unterminated comment in SQL.".to_string());
            }
        } else if c == '\'' {
            i += 1;
            loop {
                match chars.get(i) {
                    None => return Err("Unterminated string literal in SQL.".to_string()),
                    Some('\'') if chars.get(i + 1) == Some(&'\'') => i += 2,
                    Some('\'') => {
                        i += 1;
                        break;
                    }
                    Some(_) => i += 1,
                }
            }
            tokens.push(Token::Str);
        } else if c == '"' {
            i += 1;
            let mut name = String::new();
            loop {
                match chars.get(i) {
                    None => return Err("Unterminated quoted identifier in SQL.".to_string()),
                    Some('"') if chars.get(i + 1) == Some(&'"') => {
                        name.push('"');
                        i += 2;
                    }
                    Some('"') => {
                        i += 1;
                        break;
                    }
                    Some(ch) => {
                        name.push(*ch);
                        i += 1;
                    }
                }
            }
            tokens.push(Token::Quoted(name));
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len()
                && (chars[i].is_alphanumeric() || matches!(chars[i], '_' | '$' | '#'))
            {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            // E'..' (Postgres escapes) and q'..' / nq'..' (Oracle alternative quoting) change
            // how strings end, which this tokenizer doesn't model.
            if chars.get(i) == Some(&'\'')
                && ["E", "Q", "NQ"]
                    .iter()
                    .any(|prefix| word.eq_ignore_ascii_case(prefix))
            {
                return Err("Escape/alternative-quoted string literals are not allowed.".into());
            }
            // Postgres U&"…" identifiers decode \XXXX escapes, which would hide a function
            // name from the blocklist.
            if word.eq_ignore_ascii_case("U")
                && chars.get(i) == Some(&'&')
                && matches!(chars.get(i + 1), Some('"') | Some('\''))
            {
                return Err("Unicode-escape (U&) literals and identifiers are not allowed.".into());
            }
            tokens.push(Token::Word(word));
        } else if c.is_ascii_digit() {
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '.') {
                i += 1;
            }
            tokens.push(Token::Number);
        } else if c == '$' {
            return Err("Dollar-quoted strings and positional parameters are not allowed.".into());
        } else if c == '@' {
            return Err("Database links (@) are not allowed.".to_string());
        } else {
            tokens.push(Token::Symbol(c));
            i += 1;
        }
    }
    Ok(tokens)
}

fn word_is(token: Option<&Token>, expected: &str) -> bool {
    matches!(token, Some(Token::Word(w)) if w.eq_ignore_ascii_case(expected))
}

fn word_in(token: Option<&Token>, set: &[&str]) -> bool {
    matches!(token, Some(Token::Word(w)) if set.iter().any(|s| w.eq_ignore_ascii_case(s)))
}

fn name_of(token: &Token, db_type: DbType) -> Option<String> {
    match token {
        // Unquoted identifiers fold to the engine's case, exactly as the database will.
        Token::Word(w) => Some(match db_type {
            DbType::Oracle => w.to_uppercase(),
            DbType::Postgres => w.to_lowercase(),
        }),
        Token::Quoted(w) => Some(w.clone()),
        _ => None,
    }
}

/// Names defined as `name AS (` at the top level of the statement — its common table
/// expressions. A `WITH` nested inside a subquery is not recognised on purpose: its names
/// are only in scope there, so treating them as global could hide a real table of the
/// same name elsewhere. (Its inner references then fail the allowlist, which is safe.)
fn cte_names(tokens: &[Token], db_type: DbType) -> Vec<String> {
    let mut names = Vec::new();
    let mut depth = 0usize;
    for (i, token) in tokens.iter().enumerate() {
        match token {
            Token::Symbol('(') => depth += 1,
            Token::Symbol(')') => depth = depth.saturating_sub(1),
            _ => {}
        }
        if depth == 0
            && word_is(tokens.get(i + 1), "AS")
            && matches!(tokens.get(i + 2), Some(Token::Symbol('(')))
            && let Some(name) = name_of(token, db_type)
        {
            names.push(name);
        }
    }
    names
}

fn is_forbidden_function(name: &str) -> bool {
    FORBIDDEN_FUNCTIONS
        .iter()
        .any(|f| name.eq_ignore_ascii_case(f))
}

/// Validates `raw` as a single read-only SELECT and lists the tables it reads.
pub fn analyze(raw: &str, db_type: DbType) -> Result<AnalyzedSql, String> {
    if raw.len() > MAX_SQL_LEN {
        return Err(format!("SQL is longer than {MAX_SQL_LEN} characters."));
    }
    let tokens = tokenize(raw, db_type)?;

    // Only a single trailing semicolon is tolerated.
    let mut end = tokens.len();
    if matches!(tokens.last(), Some(Token::Symbol(';'))) {
        end -= 1;
        if !raw.trim_end().ends_with(';') {
            return Err("Put the closing semicolon last, with no comment after it.".to_string());
        }
    }
    let body = &tokens[..end];
    if body.iter().any(|t| matches!(t, Token::Symbol(';'))) {
        return Err("Only a single SQL statement is allowed.".to_string());
    }
    if !(word_is(body.first(), "SELECT") || word_is(body.first(), "WITH")) {
        return Err("Only SELECT statements are allowed.".to_string());
    }

    for token in body {
        match token {
            Token::Word(word) => {
                if FORBIDDEN_WORDS.iter().any(|w| word.eq_ignore_ascii_case(w)) {
                    return Err(format!(
                        "The keyword {} is not allowed.",
                        word.to_uppercase()
                    ));
                }
                if is_forbidden_function(word) {
                    return Err(format!("{} is not allowed.", word.to_uppercase()));
                }
            }
            // Quoted names resolve to the same functions as unquoted ones ("pg_read_file"(…),
            // "DBMS_XMLGEN".getxml(…)), so they are checked too, in any case. Quoted
            // identifiers stay exempt from FORBIDDEN_WORDS: those only matter as keywords.
            Token::Quoted(name) if is_forbidden_function(name) => {
                return Err(format!("{} is not allowed.", name.to_uppercase()));
            }
            _ => {}
        }
    }

    let ctes = cte_names(body, db_type);
    let tables = extract_tables(body, &ctes, db_type)?;

    // The text is run as given minus the trailing semicolon, so what was analysed is
    // exactly what executes.
    let sql = raw.trim().trim_end_matches(';').trim_end().to_string();
    Ok(AnalyzedSql { sql, tables })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Frame {
    /// Ordinary scope.
    Plain,
    /// Inside a FROM list: commas separate table references.
    FromList,
    /// Parentheses of EXTRACT/TRIM/…, where FROM is not a table source.
    FuncArgs,
}

fn extract_tables(
    tokens: &[Token],
    ctes: &[String],
    db_type: DbType,
) -> Result<Vec<TableRef>, String> {
    let mut stack = vec![Frame::Plain];
    let mut expecting_table = false;
    let mut tables: Vec<TableRef> = Vec::new();

    let mut i = 0usize;
    while i < tokens.len() {
        let token = &tokens[i];
        match token {
            Token::Symbol('(') => {
                // `FROM (` / `JOIN (` / `, (` must open a subquery. A parenthesised join
                // such as `FROM (secret JOIN other …)` would hide its first table from the
                // scan, so it is refused.
                if expecting_table && !word_in(tokens.get(i + 1), &["SELECT", "WITH"]) {
                    return Err(
                        "Parenthesised joins are not supported; list the tables with JOIN."
                            .to_string(),
                    );
                }
                let after_func_from = i > 0 && word_in(tokens.get(i - 1), FROM_IN_ARGS);
                stack.push(if after_func_from {
                    Frame::FuncArgs
                } else {
                    Frame::Plain
                });
                i += 1;
                expecting_table = false;
                continue;
            }
            Token::Symbol(')') => {
                if stack.len() > 1 {
                    stack.pop();
                }
                expecting_table = false;
                i += 1;
                continue;
            }
            Token::Symbol(',') => {
                if stack.last() == Some(&Frame::FromList) {
                    expecting_table = true;
                }
                i += 1;
                continue;
            }
            Token::Word(word) if !expecting_table => {
                let top = *stack.last().unwrap();
                if word.eq_ignore_ascii_case("FROM") && top != Frame::FuncArgs {
                    *stack.last_mut().unwrap() = Frame::FromList;
                    expecting_table = true;
                } else if word.eq_ignore_ascii_case("JOIN") {
                    expecting_table = true;
                } else if top == Frame::FromList
                    && CLAUSE_ENDERS.iter().any(|w| word.eq_ignore_ascii_case(w))
                {
                    *stack.last_mut().unwrap() = Frame::Plain;
                }
                i += 1;
                continue;
            }
            _ => {}
        }

        if !expecting_table {
            i += 1;
            continue;
        }

        // A table reference: [ONLY] name[.name]*
        if word_is(Some(token), "ONLY") {
            i += 1;
            continue;
        }
        if word_is(Some(token), "LATERAL") {
            return Err("LATERAL is not allowed.".to_string());
        }
        let mut parts: Vec<String> = Vec::new();
        let mut cursor = i;
        loop {
            let Some(part) = tokens.get(cursor).and_then(|t| name_of(t, db_type)) else {
                return Err("Could not understand a table reference in the FROM clause.".into());
            };
            parts.push(part);
            if matches!(tokens.get(cursor + 1), Some(Token::Symbol('.'))) {
                cursor += 2;
            } else {
                cursor += 1;
                break;
            }
        }
        if matches!(tokens.get(cursor), Some(Token::Symbol('('))) {
            return Err(format!(
                "Table functions such as {}(…) are not allowed.",
                parts.join(".")
            ));
        }
        expecting_table = false;
        i = cursor;

        match parts.len() {
            1 => {
                let name = parts.remove(0);
                let is_cte = ctes.iter().any(|c| c.eq_ignore_ascii_case(&name));
                let is_dual = db_type == DbType::Oracle && name.eq_ignore_ascii_case("DUAL");
                if !is_cte && !is_dual {
                    tables.push(TableRef { schema: None, name });
                }
            }
            2 => {
                let name = parts.pop().unwrap();
                let schema = parts.pop();
                tables.push(TableRef { schema, name });
            }
            _ => return Err("Only table or schema.table references are allowed.".to_string()),
        }
    }

    if expecting_table {
        return Err("The FROM clause is incomplete.".to_string());
    }
    Ok(tables)
}

#[cfg(test)]
#[path = "tests/mcp_sql_guard.rs"]
mod tests;
