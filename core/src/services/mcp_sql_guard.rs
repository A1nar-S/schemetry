//! Best-effort static checks on free-form SQL before the MCP `run_query` tool runs it.
//!
//! The goal is to keep an agent inside the table allowlist and inside plain reads. It is
//! deliberately conservative (anything it cannot understand is rejected) but it is *not*
//! a hard guarantee — views, synonyms, functions that read tables internally and dynamic
//! SQL cannot be seen statically. The hard boundary is a read-only transaction plus,
//! ideally, a dedicated database account that can only `SELECT` the allowed tables.
//!
//! Function calls are handled with an **allowlist**: every call-like token (`name(`,
//! `pkg.name(`, constructors, member calls) must be a known-safe scalar, aggregate or window
//! function for the connection's engine. A small blocklist stays on top as a second barrier.

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

/// Defence in depth, checked *before* the function allowlist: functions/packages that read
/// or write outside the table allowlist or the database. The allowlist
/// ([`COMMON_FUNCTIONS`] & co.) is the primary control; this list only guarantees a clear
/// "blocked" message and a second barrier should an allowlist entry ever be added by mistake.
const FORBIDDEN_FUNCTIONS: &[&str] = &[
    "NEXTVAL",
    "CURRVAL",
    "SETVAL",
    "LASTVAL",
    "XMLQUERY",
    "XMLTABLE",
    "EXTRACTVALUE",
    "EXISTSNODE",
    "XMLSEQUENCE",
    "UPDATEXML",
    "PG_ADVISORY_LOCK",
    "PG_ADVISORY_XACT_LOCK",
    "PG_RELOAD_CONF",
    "LO_CREATE",
    "LO_FROM_BYTEA",
    "LO_PUT",
    "LO_UNLINK",
    "HTP",
    "HTF",
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

/// Oracle package families that never belong in a read-only query (DBMS_*, UTL_*, …). Checked
/// on every name, called or not, because Oracle lets parameterless package functions be used
/// without parentheses (`UTL_INADDR.GET_HOST_NAME`).
const ORACLE_FORBIDDEN_PREFIXES: &[&str] = &["DBMS_", "UTL_", "OWA_", "CTX_", "SDO_", "APEX_"];

/// Functions allowed on both engines (SQL standard or identical in Oracle and Postgres).
/// Names are upper-case; unquoted calls are matched case-insensitively.
#[rustfmt::skip]
const COMMON_FUNCTIONS: &[&str] = &[
    // aggregates
    "COUNT", "SUM", "AVG", "MIN", "MAX", "STDDEV", "STDDEV_POP", "STDDEV_SAMP", "VARIANCE",
    "VAR_POP", "VAR_SAMP", "CORR", "COVAR_POP", "COVAR_SAMP", "PERCENTILE_CONT",
    "PERCENTILE_DISC", "ROLLUP", "CUBE", "GROUPING",
    // window / ranking
    "ROW_NUMBER", "RANK", "DENSE_RANK", "PERCENT_RANK", "CUME_DIST", "NTILE", "LAG", "LEAD",
    "FIRST_VALUE", "LAST_VALUE", "NTH_VALUE",
    // conditional / conversion
    "COALESCE", "NULLIF", "GREATEST", "LEAST", "CAST", "TO_CHAR", "TO_DATE", "TO_NUMBER",
    "TO_TIMESTAMP",
    // strings
    "UPPER", "LOWER", "INITCAP", "TRIM", "LTRIM", "RTRIM", "LPAD", "RPAD", "SUBSTR", "LENGTH",
    "REPLACE", "TRANSLATE", "CONCAT", "POSITION", "OVERLAY", "ASCII", "CHR", "REGEXP_REPLACE",
    // numbers
    "ABS", "CEIL", "FLOOR", "ROUND", "TRUNC", "MOD", "POWER", "SQRT", "EXP", "LN", "LOG", "SIGN",
    "SIN", "COS", "TAN", "ASIN", "ACOS", "ATAN", "ATAN2", "SINH", "COSH", "TANH", "WIDTH_BUCKET",
    // dates
    "EXTRACT", "CURRENT_DATE", "CURRENT_TIMESTAMP", "LOCALTIMESTAMP",
    // types that take a precision/length: `CAST(x AS NUMBER(10, 2))`, `INTERVAL DAY(3) TO SECOND`
    "NUMERIC", "DECIMAL", "DEC", "CHAR", "VARCHAR", "FLOAT", "TIMESTAMP", "TIME", "DATE",
    "INTERVAL", "YEAR", "MONTH", "DAY", "HOUR", "MINUTE", "SECOND",
];

#[rustfmt::skip]
const ORACLE_FUNCTIONS: &[&str] = &[
    "MEDIAN", "LISTAGG", "APPROX_COUNT_DISTINCT", "RATIO_TO_REPORT", "GROUPING_ID", "NVL", "NVL2",
    "DECODE", "NANVL", "SYSDATE", "SYSTIMESTAMP", "INSTR", "SOUNDEX", "NLS_UPPER", "NLS_LOWER",
    "NLS_INITCAP", "LENGTHB", "REGEXP_SUBSTR", "REGEXP_INSTR", "REGEXP_COUNT", "REGEXP_LIKE",
    "REMAINDER", "BITAND", "ADD_MONTHS", "MONTHS_BETWEEN", "LAST_DAY", "NEXT_DAY", "FROM_TZ",
    "SYS_EXTRACT_UTC", "NUMTODSINTERVAL", "NUMTOYMINTERVAL", "TO_DSINTERVAL", "TO_YMINTERVAL",
    "TO_TIMESTAMP_TZ", "TO_NCHAR", "TO_CLOB", "TO_BINARY_FLOAT", "TO_BINARY_DOUBLE", "RAWTOHEX",
    "HEXTORAW", "JSON_VALUE", "JSON_QUERY", "JSON_EXISTS", "JSON_OBJECT", "JSON_ARRAY",
    "JSON_ARRAYAGG", "JSON_OBJECTAGG", "VARCHAR2", "NVARCHAR2", "NCHAR", "NUMBER", "RAW",
    "UROWID",
];

#[rustfmt::skip]
const POSTGRES_FUNCTIONS: &[&str] = &[
    "STRING_AGG", "ARRAY_AGG", "BOOL_AND", "BOOL_OR", "EVERY", "BIT_AND", "BIT_OR", "MODE",
    "ANY_VALUE", "SUBSTRING", "LEFT", "RIGHT", "BTRIM", "SPLIT_PART", "STRPOS", "CONCAT_WS",
    "FORMAT", "REVERSE", "MD5", "TO_HEX", "STARTS_WITH", "CHAR_LENGTH", "CHARACTER_LENGTH",
    "OCTET_LENGTH", "BIT_LENGTH", "REGEXP_MATCH", "CEILING", "DIV", "CBRT", "DEGREES", "RADIANS",
    "PI", "SCALE", "NOW", "DATE_TRUNC", "DATE_PART", "DATE_BIN", "AGE", "MAKE_DATE",
    "MAKE_TIMESTAMP", "MAKE_INTERVAL", "ISFINITE", "CURRENT_TIME", "LOCALTIME", "ARRAY_LENGTH",
    "ARRAY_TO_STRING", "ARRAY_POSITION", "STRING_TO_ARRAY", "ARRAY_APPEND", "CARDINALITY",
    "NUM_NULLS", "NUM_NONNULLS", "TO_JSON", "TO_JSONB", "ROW_TO_JSON", "JSON_BUILD_OBJECT",
    "JSONB_BUILD_OBJECT", "JSON_BUILD_ARRAY", "JSONB_BUILD_ARRAY", "JSON_AGG", "JSONB_AGG",
    "JSON_OBJECT_AGG", "JSONB_OBJECT_AGG", "JSON_TYPEOF", "JSONB_TYPEOF", "JSON_ARRAY_LENGTH",
    "JSONB_ARRAY_LENGTH", "JSONB_PRETTY", "JSON_EXTRACT_PATH", "JSON_EXTRACT_PATH_TEXT",
    "JSONB_EXTRACT_PATH", "JSONB_EXTRACT_PATH_TEXT", "JSONB_STRIP_NULLS", "VARYING", "BIT",
];

/// Reserved or purely syntactic words that legitimately sit directly before a `(` without
/// being a function call (`IN (…)`, `OVER (…)`, `EXISTS (…)`). None of them can name a
/// function unquoted in either engine, so they are never mistaken for one.
#[rustfmt::skip]
const NON_FUNCTION_KEYWORDS: &[&str] = &[
    "IN", "AND", "OR", "NOT", "EXISTS", "ON", "WHERE", "SELECT", "FROM", "AS", "WHEN", "THEN",
    "ELSE", "CASE", "VALUES", "UNION", "INTERSECT", "EXCEPT", "MINUS", "ALL", "ANY",
    "SOME", "DISTINCT", "USING", "BETWEEN", "IS", "JOIN", "WITH", "HAVING", "GROUP",
    "ARRAY", "ROW",
];

/// Whether the word at `i` is a keyword rather than a call. Words that are not reserved in
/// both engines (`FILTER`, `OVER`, `KEEP`, `BY`, …) only count in the position where the
/// syntax uses them, so a user function of that name is still checked.
fn is_keyword_before_paren(tokens: &[Token], i: usize, word: &str, db_type: DbType) -> bool {
    let prev = i.checked_sub(1).and_then(|p| tokens.get(p));
    match word.to_ascii_uppercase().as_str() {
        // Reserved in Postgres only; an Oracle function may carry these names.
        "LIMIT" | "OFFSET" | "ARRAY" => db_type == DbType::Postgres,
        "OVER" | "FILTER" | "KEEP" => matches!(prev, Some(Token::Symbol(')'))),
        "SETS" => word_is(prev, "GROUPING"),
        "BY" => word_in(prev, &["ORDER", "GROUP", "PARTITION", "SIBLINGS"]),
        "PRIOR" => word_is(prev, "BY"),
        "MATERIALIZED" => word_in(prev, &["AS", "NOT"]),
        upper => NON_FUNCTION_KEYWORDS.contains(&upper),
    }
}

/// Keywords that end a FROM list. Only words the engine reserves count: any other word in
/// that position is a table alias (`FROM a start, secret`), and treating it as a clause
/// would drop the comma-separated table after it.
fn ends_from_list(word: &str, db_type: DbType) -> bool {
    const BOTH: &[&str] = &[
        "WHERE",
        "GROUP",
        "ORDER",
        "HAVING",
        "UNION",
        "INTERSECT",
        "FOR",
    ];
    const POSTGRES: &[&str] = &["EXCEPT", "LIMIT", "OFFSET", "FETCH", "WINDOW", "RETURNING"];
    const ORACLE: &[&str] = &["MINUS", "CONNECT", "START"];
    let engine = match db_type {
        DbType::Postgres => POSTGRES,
        DbType::Oracle => ORACLE,
    };
    BOTH.iter()
        .chain(engine)
        .any(|w| word.eq_ignore_ascii_case(w))
}

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
    // Engines disagree on whether a lone CR ends a `--` comment, so the guard can't know
    // what the database will run.
    if chars
        .iter()
        .enumerate()
        .any(|(n, &c)| c == '\r' && chars.get(n + 1) != Some(&'\n'))
    {
        return Err("A carriage return must be followed by a line feed.".to_string());
    }
    let mut tokens = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_whitespace() || c == '\u{b}' {
            i += 1;
        } else if !c.is_ascii() && c != '"' && c != '\'' {
            // The engines' notion of an identifier character differs from Rust's Unicode
            // classes, so a non-ASCII character could hide a token boundary.
            return Err(format!(
                "Non-ASCII character U+{:04X} outside a string or quoted identifier.",
                c as u32
            ));
        } else if c == '-' && chars.get(i + 1) == Some(&'-') {
            // Postgres also ends a line comment at a lone `\r`; ending early only exposes more tokens.
            while i < chars.len() && chars[i] != '\n' && chars[i] != '\r' {
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
        } else if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len()
                && (chars[i].is_ascii_alphanumeric() || matches!(chars[i], '_' | '$' | '#'))
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
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            if matches!(chars.get(i), Some('e' | 'E')) {
                let mut j = i + 1;
                if matches!(chars.get(j), Some('+' | '-')) {
                    j += 1;
                }
                if chars.get(j).is_some_and(|ch| ch.is_ascii_digit()) {
                    i = j;
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            // Oracle's f/d suffixes (1.5f); anything else glued to a number could swallow
            // a keyword (`2FROM secret`) so it is refused.
            if db_type == DbType::Oracle
                && matches!(chars.get(i), Some('f' | 'F' | 'd' | 'D'))
                && !chars
                    .get(i + 1)
                    .is_some_and(|ch| ch.is_alphanumeric() || *ch == '_')
            {
                i += 1;
            }
            if chars
                .get(i)
                .is_some_and(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '$' | '#'))
            {
                return Err("A number may not be directly followed by letters.".to_string());
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

/// Blocklist (defence in depth). Quoted names are compared case-insensitively on purpose:
/// this layer errs on the side of rejecting.
fn is_forbidden_function(name: &str, db_type: DbType) -> bool {
    FORBIDDEN_FUNCTIONS
        .iter()
        .any(|f| name.eq_ignore_ascii_case(f))
        || (db_type == DbType::Oracle
            && ORACLE_FORBIDDEN_PREFIXES.iter().any(|p| {
                name.len() >= p.len()
                    && name.is_char_boundary(p.len())
                    && name[..p.len()].eq_ignore_ascii_case(p)
            }))
}

fn on_allowlist(name: &str, db_type: DbType) -> bool {
    let engine_specific = match db_type {
        DbType::Oracle => ORACLE_FUNCTIONS,
        DbType::Postgres => POSTGRES_FUNCTIONS,
    };
    [COMMON_FUNCTIONS, engine_specific]
        .iter()
        .any(|list| list.iter().any(|f| name.eq_ignore_ascii_case(f)))
}

fn is_name(token: Option<&Token>) -> bool {
    matches!(token, Some(Token::Word(_) | Token::Quoted(_)))
}

/// How a call is written in the error message: unquoted names as typed, quoted ones quoted.
fn display_name(token: &Token) -> String {
    match token {
        Token::Word(w) => w.clone(),
        Token::Quoted(q) => format!("\"{q}\""),
        _ => String::new(),
    }
}

/// Whether the engine would resolve this name to the allowlisted built-in of that name.
/// Unquoted names fold to the engine's case, so any case matches; a quoted name is
/// case-sensitive, so it only matches when it is already spelled in the engine's folded
/// case. Non-ASCII names never match (engines fold them differently than Rust does).
fn name_on_allowlist(token: &Token, db_type: DbType) -> bool {
    match token {
        Token::Word(w) => w.is_ascii() && on_allowlist(w, db_type),
        Token::Quoted(q) => {
            let folded = match db_type {
                DbType::Oracle => q.to_ascii_uppercase(),
                DbType::Postgres => q.to_ascii_lowercase(),
            };
            q.is_ascii() && *q == folded && on_allowlist(q, db_type)
        }
        _ => false,
    }
}

fn not_on_allowlist(display: &str, db_type: DbType) -> String {
    let engine = match db_type {
        DbType::Oracle => "Oracle",
        DbType::Postgres => "Postgres",
    };
    format!(
        "Function {display} is not on the run_query function allowlist for {engine}; \
         only listed scalar, aggregate and window functions may be called."
    )
}

/// Indices of the names in `WITH name(col, …) AS (` CTE headers, which look like calls.
/// Only recognised before the main SELECT at the top level, so a call can't pose as one.
fn cte_column_list_headers(tokens: &[Token]) -> Vec<usize> {
    let mut headers = Vec::new();
    let mut depth = 0usize;
    for (i, token) in tokens.iter().enumerate() {
        match token {
            Token::Symbol('(') => depth += 1,
            Token::Symbol(')') => depth = depth.saturating_sub(1),
            _ => {}
        }
        if depth == 0 && word_is(Some(token), "SELECT") {
            break;
        }
        let after_with_or_comma = i > 0
            && (word_in(tokens.get(i - 1), &["WITH", "RECURSIVE"])
                || matches!(tokens.get(i - 1), Some(Token::Symbol(','))));
        if depth == 0
            && is_name(Some(token))
            && after_with_or_comma
            && matches!(tokens.get(i + 1), Some(Token::Symbol('(')))
        {
            let mut j = i + 2;
            while is_name(tokens.get(j)) && matches!(tokens.get(j + 1), Some(Token::Symbol(','))) {
                j += 2;
            }
            if is_name(tokens.get(j))
                && matches!(tokens.get(j + 1), Some(Token::Symbol(')')))
                && word_is(tokens.get(j + 2), "AS")
            {
                headers.push(i);
            }
        }
    }
    headers
}

/// The function allowlist: every name directly followed by `(` — plain, schema/package
/// qualified or a member call — must be a known-safe built-in. This also catches Oracle
/// object-type constructors (`DBURITYPE('…')`) and type methods (`….getclob()`), since
/// none of them is on the list.
fn check_function_calls(
    tokens: &[Token],
    cte_headers: &[usize],
    db_type: DbType,
) -> Result<(), String> {
    for (i, token) in tokens.iter().enumerate() {
        if !is_name(Some(token))
            || !matches!(tokens.get(i + 1), Some(Token::Symbol('(')))
            || cte_headers.contains(&i)
        {
            continue;
        }

        // Walk back over `a.b.` qualifiers.
        let mut start = i;
        while start >= 2
            && matches!(tokens[start - 1], Token::Symbol('.'))
            && is_name(tokens.get(start - 2))
        {
            start -= 2;
        }
        let chain: Vec<String> = tokens[start..=i]
            .iter()
            .filter(|t| is_name(Some(t)))
            .map(display_name)
            .collect();
        let display = chain.join(".");

        if start >= 1 && matches!(tokens[start - 1], Token::Symbol('.')) {
            return Err(format!(
                "Member call .{}(…) on an expression: {}",
                chain.last().cloned().unwrap_or_default(),
                not_on_allowlist(&display, db_type)
            ));
        }

        if start == i {
            if let Token::Word(w) = token
                && is_keyword_before_paren(tokens, i, w, db_type)
            {
                continue;
            }
        } else {
            // Only the Postgres catalog schema is a known home for built-ins.
            let pg_catalog_call = db_type == DbType::Postgres
                && start + 2 == i
                && match &tokens[start] {
                    Token::Word(w) => w.eq_ignore_ascii_case("pg_catalog"),
                    Token::Quoted(q) => q == "pg_catalog",
                    _ => false,
                };
            if !pg_catalog_call {
                return Err(not_on_allowlist(&display, db_type));
            }
        }

        if !name_on_allowlist(token, db_type) {
            return Err(not_on_allowlist(&display, db_type));
        }
    }
    Ok(())
}

/// Names defined as `name AS (` at the top level of the statement — its common table
/// expressions. A `WITH` nested inside a subquery is not recognised on purpose: its names
/// are only in scope there, so treating them as global could hide a real table of the
/// same name elsewhere. (Its inner references then fail the allowlist, which is safe.)
///
/// Only the leading WITH clause counts (a `WINDOW secret AS (…)` is not a CTE). A CTE name
/// is in scope for later CTEs and the main query only: a body that mentions its own name
/// or a later CTE's would read the real table of that name (or, in Oracle, a recursive
/// reference), so it is refused rather than letting the name hide the table.
fn cte_names(
    tokens: &[Token],
    cte_headers: &[usize],
    db_type: DbType,
) -> Result<Vec<String>, String> {
    let close_of = |open: usize| -> Option<usize> {
        if !matches!(tokens.get(open), Some(Token::Symbol('('))) {
            return None;
        }
        let mut depth = 0usize;
        for (k, t) in tokens.iter().enumerate().skip(open) {
            match t {
                Token::Symbol('(') => depth += 1,
                Token::Symbol(')') => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(k);
                    }
                }
                _ => {}
            }
        }
        None
    };

    let mut defs: Vec<(String, usize, usize, bool)> = Vec::new();
    let mut depth = 0usize;
    for (i, token) in tokens.iter().enumerate() {
        match token {
            Token::Symbol('(') => depth += 1,
            Token::Symbol(')') => depth = depth.saturating_sub(1),
            _ => {}
        }
        if depth == 0 && word_is(Some(token), "SELECT") {
            break;
        }
        let is_header = cte_headers.contains(&i);
        if depth != 0
            || !(is_header
                || (word_is(tokens.get(i + 1), "AS")
                    && matches!(tokens.get(i + 2), Some(Token::Symbol('(')))))
        {
            continue;
        }
        let Some(name) = name_of(token, db_type) else {
            continue;
        };
        let after_columns = if is_header {
            close_of(i + 1).map(|c| c + 1)
        } else {
            Some(i + 1)
        };
        let open = after_columns.map(|a| a + 1);
        let Some((open, close)) = open.and_then(|o| close_of(o).map(|c| (o, c))) else {
            return Err("Could not understand a WITH clause.".to_string());
        };
        defs.push((name, open + 1, close, is_header));
    }

    // `WITH RECURSIVE` puts every CTE in scope everywhere (Postgres).
    if db_type == DbType::Postgres && word_is(tokens.get(1), "RECURSIVE") {
        return Ok(defs.into_iter().map(|(name, ..)| name).collect());
    }

    for (k, (name, from, to, is_header)) in defs.iter().enumerate() {
        let body = &tokens[*from..*to];
        // Oracle recursion needs a column list and a UNION ALL, so only then is a
        // self-reference the CTE itself rather than the table of that name.
        let mut depth = 0usize;
        let has_top_level_union_all = body.iter().enumerate().any(|(n, t)| {
            match t {
                Token::Symbol('(') => depth += 1,
                Token::Symbol(')') => depth = depth.saturating_sub(1),
                _ => {}
            }
            depth == 0 && word_is(Some(t), "UNION") && word_is(body.get(n + 1), "ALL")
        });
        let recursive_oracle = db_type == DbType::Oracle && *is_header && has_top_level_union_all;
        let first = if recursive_oracle { k + 1 } else { k };
        for token in body {
            let Some(used) = name_of(token, db_type) else {
                continue;
            };
            let same_as_self = recursive_oracle && used.eq_ignore_ascii_case(name);
            if !same_as_self
                && defs[first..]
                    .iter()
                    .any(|(later, ..)| later.eq_ignore_ascii_case(&used))
            {
                return Err(format!(
                    "The WITH query {name} refers to {used}, which is not allowed \
                     (self-referencing and forward references are not supported)."
                ));
            }
        }
    }
    Ok(defs.into_iter().map(|(name, ..)| name).collect())
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
                if is_forbidden_function(word, db_type) {
                    return Err(format!(
                        "{} is blocked and not on the run_query function allowlist.",
                        word.to_uppercase()
                    ));
                }
            }
            // Quoted names resolve to the same functions as unquoted ones ("pg_read_file"(…),
            // "DBMS_XMLGEN".getxml(…)), so they are checked too, in any case. Quoted
            // identifiers stay exempt from FORBIDDEN_WORDS: those only matter as keywords.
            Token::Quoted(name) if is_forbidden_function(name, db_type) => {
                return Err(format!(
                    "\"{name}\" is blocked and not on the run_query function allowlist."
                ));
            }
            _ => {}
        }
    }

    let cte_headers = cte_column_list_headers(body);
    check_function_calls(body, &cte_headers, db_type)?;

    let ctes = cte_names(body, &cte_headers, db_type)?;
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
    let mut label_end: Option<usize> = None;

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
            // Postgres allows any keyword as a label after AS or a dot (`1 AS from`, `c.group`),
            // where it is a name, not a clause.
            Token::Word(_)
                if i > 0
                    && label_end != Some(i - 1)
                    && (matches!(tokens.get(i - 1), Some(Token::Symbol('.')))
                        || word_is(tokens.get(i - 1), "AS")) =>
            {
                // The label itself must not make the next keyword look like a label too
                // (`AS as FROM secret`, `t.as JOIN secret`).
                label_end = Some(i);
                i += 1;
                continue;
            }
            Token::Word(word) if !expecting_table => {
                let top = *stack.last().unwrap();
                if word.eq_ignore_ascii_case("FROM") && top != Frame::FuncArgs {
                    *stack.last_mut().unwrap() = Frame::FromList;
                    expecting_table = true;
                } else if word.eq_ignore_ascii_case("JOIN")
                    || (db_type == DbType::Oracle && word.eq_ignore_ascii_case("APPLY"))
                {
                    expecting_table = true;
                } else if top == Frame::FromList && ends_from_list(word, db_type) {
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
                // Names are already folded to the engine's case, so a quoted "Secret" is
                // not the CTE `secret`.
                let is_cte = ctes.iter().any(|c| *c == name);
                let is_dual = db_type == DbType::Oracle && name == "DUAL";
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
