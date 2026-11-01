//! Static validation of the DDL an agent wants to deploy with `apply_ddl`.
//!
//! Only `CREATE [OR REPLACE]` of one stored code object or view is accepted, and it must
//! be the object the agent declared, in the connection's own schema. Everything else —
//! DROP, TRUNCATE, DELETE, GRANT, ALTER SYSTEM, a second statement — is refused before
//! anything reaches the database. The tokenizer understands comments, quoted identifiers,
//! string literals (including Oracle `q'[..]'` and Postgres `$tag$..$tag$` bodies), so
//! keywords inside them cannot be mistaken for statements.
//!
//! Like the run_query guard this is a conservative gate, not a sandbox: a deployed
//! procedure can of course do whatever its owner may. The hard boundary is the database
//! account (use a dev/test one) and the per-connection policy plus user approval.

use crate::models::DbType;
use crate::repositories::db_repository::DdlStatement;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Kind {
    Word(String),
    Quoted(String),
    Str,
    Number,
    Symbol(char),
    /// Oracle's `/` on a line of its own, which ends a statement.
    Slash,
}

#[derive(Clone, Debug)]
struct Tok {
    kind: Kind,
    /// Char offsets of the token in the source.
    start: usize,
    end: usize,
}

fn tokenize(chars: &[char], db_type: DbType) -> Result<Vec<Tok>, String> {
    let pg = db_type == DbType::Postgres;
    let mut tokens = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        let start = i;
        let kind = if c.is_ascii_whitespace() || c == '\u{b}' {
            i += 1;
            continue;
        } else if !c.is_ascii() && !matches!(c, '\'' | '"') {
            // Postgres identifiers may hold any non-ASCII byte (incl. before `$$`), and
            // Rust's Unicode classes differ from the engines', so none is trusted here.
            return Err(format!(
                "Non-ASCII character U+{:04X} outside a string, comment or quoted identifier.",
                c as u32
            ));
        } else if c == '-' && chars.get(i + 1) == Some(&'-') {
            // Postgres also ends a line comment at a lone `\r`.
            while i < chars.len() && chars[i] != '\n' && chars[i] != '\r' {
                i += 1;
            }
            continue;
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            // Postgres nests block comments, Oracle doesn't.
            let mut depth = 1;
            i += 2;
            while i < chars.len() && depth > 0 {
                if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    i += 2;
                } else if pg && chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                    depth += 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if depth > 0 {
                return Err("Unterminated comment in the DDL.".to_string());
            }
            continue;
        } else if c == '\'' {
            i = skip_string(chars, i)?;
            Kind::Str
        } else if c == '"' {
            let (name, next) = read_quoted(chars, i)?;
            i = next;
            Kind::Quoted(name)
        } else if c.is_ascii_alphabetic() || c == '_' {
            while i < chars.len()
                && (chars[i].is_ascii_alphanumeric()
                    || matches!(chars[i], '_' | '$')
                    // Postgres reads `#` as an operator (`i#$$ x $$` is `i # string`).
                    || (!pg && chars[i] == '#'))
            {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            if chars.get(i) == Some(&'\'') {
                if pg && word.eq_ignore_ascii_case("E") {
                    return Err(
                        "E'...' string literals are not supported in deployed DDL; use '...' or $$...$$."
                            .to_string(),
                    );
                }
                if !pg && (word.eq_ignore_ascii_case("Q") || word.eq_ignore_ascii_case("NQ")) {
                    i = skip_alternative_quote(chars, i)?;
                    tokens.push(Tok {
                        kind: Kind::Str,
                        start,
                        end: i,
                    });
                    continue;
                }
            }
            if pg && word.eq_ignore_ascii_case("U") && chars.get(i) == Some(&'&') {
                return Err("U& escapes are not supported in deployed DDL.".to_string());
            }
            Kind::Word(word)
        } else if c.is_ascii_digit() {
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '.') {
                i += 1;
            }
            // Postgres 14 and older read `1e$$` as a number plus the identifier `e$$`, which
            // would hide what follows as a dollar-quoted string.
            if matches!(chars.get(i), Some('$' | '_' | '#')) {
                return Err("A number may not be directly followed by $, _ or #.".to_string());
            }
            Kind::Number
        } else if pg && c == '$' {
            match dollar_tag_end(chars, i) {
                Some(tag_end) => {
                    i = skip_dollar_quoted(chars, i, tag_end)?;
                    Kind::Str
                }
                None => {
                    i += 1;
                    Kind::Symbol('$')
                }
            }
        } else if c == '/' && !pg && alone_on_line(chars, i) {
            i += 1;
            Kind::Slash
        } else {
            i += 1;
            Kind::Symbol(c)
        };
        tokens.push(Tok {
            kind,
            start,
            end: i,
        });
    }
    Ok(tokens)
}

/// Index just after the closing quote of the string literal starting at `chars[i]`.
fn skip_string(chars: &[char], mut i: usize) -> Result<usize, String> {
    i += 1;
    loop {
        match chars.get(i) {
            None => return Err("Unterminated string literal in the DDL.".to_string()),
            Some('\'') if chars.get(i + 1) == Some(&'\'') => i += 2,
            Some('\'') => return Ok(i + 1),
            Some(_) => i += 1,
        }
    }
}

fn read_quoted(chars: &[char], mut i: usize) -> Result<(String, usize), String> {
    i += 1;
    let mut name = String::new();
    loop {
        match chars.get(i) {
            None => return Err("Unterminated quoted identifier in the DDL.".to_string()),
            Some('"') if chars.get(i + 1) == Some(&'"') => {
                name.push('"');
                i += 2;
            }
            Some('"') => return Ok((name, i + 1)),
            Some(c) => {
                name.push(*c);
                i += 1;
            }
        }
    }
}

/// Oracle `q'<d>…<d>'`; `chars[i]` is the opening `'`. Brackets close with their mate.
fn skip_alternative_quote(chars: &[char], i: usize) -> Result<usize, String> {
    let open = *chars
        .get(i + 1)
        .filter(|c| !c.is_whitespace())
        .ok_or("Malformed q'...' literal in the DDL.")?;
    let close = match open {
        '[' => ']',
        '(' => ')',
        '{' => '}',
        '<' => '>',
        other => other,
    };
    let mut j = i + 2;
    while j + 1 < chars.len() {
        if chars[j] == close && chars[j + 1] == '\'' {
            return Ok(j + 2);
        }
        j += 1;
    }
    Err("Unterminated q'...' literal in the DDL.".to_string())
}

/// For a `$tag$` opener at `chars[i]`, the index of its closing `$`.
fn dollar_tag_end(chars: &[char], i: usize) -> Option<usize> {
    let mut j = i + 1;
    while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') {
        j += 1;
    }
    let tag_starts_with_digit = chars.get(i + 1).is_some_and(|c| c.is_ascii_digit());
    (chars.get(j) == Some(&'$') && !tag_starts_with_digit).then_some(j)
}

fn skip_dollar_quoted(chars: &[char], start: usize, tag_end: usize) -> Result<usize, String> {
    let tag = &chars[start..=tag_end];
    let mut j = tag_end + 1;
    while j + tag.len() <= chars.len() {
        if &chars[j..j + tag.len()] == tag {
            return Ok(j + tag.len());
        }
        j += 1;
    }
    Err("Unterminated $...$ quoted body in the DDL.".to_string())
}

/// `/` with nothing but blanks before and after it on its line.
fn alone_on_line(chars: &[char], i: usize) -> bool {
    let before = chars[..i]
        .iter()
        .rev()
        .take_while(|c| **c != '\n')
        .all(|c| c.is_whitespace());
    let after = chars[i + 1..]
        .iter()
        .take_while(|c| **c != '\n')
        .all(|c| c.is_whitespace());
    before && after
}

fn word(token: &Tok) -> Option<String> {
    match &token.kind {
        Kind::Word(w) => Some(w.to_ascii_uppercase()),
        _ => None,
    }
}

fn is_word(token: Option<&Tok>, expected: &str) -> bool {
    token.and_then(word).is_some_and(|w| w == expected)
}

fn describe(token: &Tok, chars: &[char]) -> String {
    let text: String = chars[token.start..token.end.min(token.start + 30)]
        .iter()
        .collect();
    format!("'{text}'")
}

/// An identifier as the engine will store it.
fn ident(token: Option<&Tok>, db_type: DbType) -> Option<String> {
    match &token?.kind {
        Kind::Word(w) => Some(match db_type {
            DbType::Oracle => w.to_ascii_uppercase(),
            DbType::Postgres => w.to_ascii_lowercase(),
        }),
        Kind::Quoted(q) => Some(q.clone()),
        _ => None,
    }
}

#[derive(Debug)]
struct Header {
    kind: String,
    or_replace: bool,
    schema: Option<String>,
    name: String,
    /// Index of the first token after the object name.
    next: usize,
}

const ORACLE_KINDS: &[&str] = &[
    "VIEW",
    "PROCEDURE",
    "FUNCTION",
    "PACKAGE",
    "TRIGGER",
    "TYPE",
];
const POSTGRES_KINDS: &[&str] = &["VIEW", "PROCEDURE", "FUNCTION", "TRIGGER"];
/// Languages that run inside the database's own sandbox; anything else is refused.
const SAFE_LANGUAGES: &[&str] = &["SQL", "PLPGSQL"];

pub fn supported_types(db_type: DbType) -> &'static [&'static str] {
    match db_type {
        DbType::Oracle => &[
            "VIEW",
            "PROCEDURE",
            "FUNCTION",
            "PACKAGE",
            "PACKAGE BODY",
            "TRIGGER",
            "TYPE",
            "TYPE BODY",
        ],
        DbType::Postgres => POSTGRES_KINDS,
    }
}

fn parse_header(toks: &[Tok], chars: &[char], db_type: DbType) -> Result<Header, String> {
    let oracle = db_type == DbType::Oracle;
    let first = toks.first().ok_or("The DDL is empty.")?;
    if !is_word(Some(first), "CREATE") {
        return Err(format!(
            "Only CREATE [OR REPLACE] statements can be deployed (found {}).",
            describe(first, chars)
        ));
    }
    let mut i = 1;
    let mut or_replace = false;
    if is_word(toks.get(i), "OR") && is_word(toks.get(i + 1), "REPLACE") {
        or_replace = true;
        i += 2;
    }
    if oracle {
        // [NO] FORCE, EDITIONABLE/NONEDITIONABLE/EDITIONING modifiers.
        while let Some(w) = toks.get(i).and_then(word) {
            match w.as_str() {
                "EDITIONABLE" | "NONEDITIONABLE" | "EDITIONING" | "FORCE" => i += 1,
                "NO" if is_word(toks.get(i + 1), "FORCE") => i += 2,
                _ => break,
            }
        }
    }

    let Some(object_kind) = toks.get(i).and_then(word) else {
        return Err("Expected an object type after CREATE.".to_string());
    };
    let allowed = if oracle { ORACLE_KINDS } else { POSTGRES_KINDS };
    if !allowed.contains(&object_kind.as_str()) {
        return Err(format!(
            "CREATE {object_kind} cannot be deployed with apply_ddl; supported: {}.",
            supported_types(db_type).join(", ")
        ));
    }
    i += 1;
    let mut kind = object_kind.clone();
    if oracle && matches!(object_kind.as_str(), "PACKAGE" | "TYPE") && is_word(toks.get(i), "BODY")
    {
        kind.push_str(" BODY");
        i += 1;
    }

    let missing = || "Expected the object name after the object type.".to_string();
    let first_part = ident(toks.get(i), db_type).ok_or_else(missing)?;
    i += 1;
    let (schema, name) = if matches!(toks.get(i).map(|t| &t.kind), Some(Kind::Symbol('.'))) {
        let second = ident(toks.get(i + 1), db_type).ok_or_else(missing)?;
        i += 2;
        if matches!(toks.get(i).map(|t| &t.kind), Some(Kind::Symbol('.'))) {
            return Err("Object names may have at most one qualifier (schema.name).".to_string());
        }
        (Some(first_part), second)
    } else {
        (None, first_part)
    };
    Ok(Header {
        kind,
        or_replace,
        schema,
        name,
        next: i,
    })
}

/// Triggers name their table after `ON`; it must not reach into another schema, and
/// Oracle's database-level triggers are out of scope.
fn check_trigger_target(
    toks: &[Tok],
    from: usize,
    schema: &str,
    db_type: DbType,
) -> Result<(), String> {
    let Some(on) = (from..toks.len()).find(|&i| is_word(toks.get(i), "ON")) else {
        return Ok(());
    };
    if is_word(toks.get(on + 1), "DATABASE")
        || is_word(toks.get(on + 1), "PLUGGABLE")
        || is_word(toks.get(on + 1), "NESTED")
    {
        return Err("Database-level and nested-table triggers cannot be deployed.".to_string());
    }
    if matches!(toks.get(on + 2).map(|t| &t.kind), Some(Kind::Symbol('.')))
        && let Some(target_schema) = ident(toks.get(on + 1), db_type)
        && !same_schema(&target_schema, schema, db_type)
    {
        return Err(format!(
            "The trigger must be on a table in schema {schema}, not {target_schema}."
        ));
    }
    Ok(())
}

fn same_schema(found: &str, expected: &str, db_type: DbType) -> bool {
    match db_type {
        DbType::Oracle => found == expected.to_ascii_uppercase(),
        DbType::Postgres => found == expected,
    }
}

fn check_unsafe_language(toks: &[Tok]) -> Result<(), String> {
    for (i, token) in toks.iter().enumerate() {
        if is_word(Some(token), "LANGUAGE") {
            match toks.get(i + 1).map(|t| &t.kind) {
                Some(Kind::Word(w))
                    if !SAFE_LANGUAGES.contains(&w.to_ascii_uppercase().as_str()) =>
                {
                    return Err(format!("LANGUAGE {w} functions cannot be deployed."));
                }
                Some(Kind::Str | Kind::Quoted(_)) => {
                    return Err("Quote-free LANGUAGE names are required in deployed DDL.".into());
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// Oracle code units end with `;` as part of the unit; views and type specs must not.
fn keeps_terminator(kind: &str, db_type: DbType) -> bool {
    db_type == DbType::Oracle
        && matches!(
            kind,
            "PROCEDURE" | "FUNCTION" | "PACKAGE" | "PACKAGE BODY" | "TRIGGER" | "TYPE BODY"
        )
}

/// Normalises line endings and trims, so the digest the user approves is stable.
pub fn normalize(ddl: &str) -> String {
    ddl.trim_start_matches('\u{feff}')
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim()
        .to_string()
}

/// Validates `ddl` (already [`normalize`]d) as the deployment of `object_type`
/// `object_name` into `schema`, returning the statements to execute. Errors are
/// messages for the agent.
pub fn validate(
    ddl: &str,
    object_type: &str,
    object_name: &str,
    schema: &str,
    db_type: DbType,
) -> Result<Vec<DdlStatement>, String> {
    if let Some(c) = ddl.chars().find(|c| {
        (c.is_control() && !matches!(c, '\t' | '\n' | '\r'))
            || matches!(*c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}')
    }) {
        // Invisible characters would let the approval screen show something else.
        return Err(format!("Control or invisible character U+{:04X} in the DDL.", c as u32));
    }
    let chars: Vec<char> = ddl.chars().collect();
    let tokens = tokenize(&chars, db_type)?;

    let mut groups: Vec<&[Tok]> = tokens
        .split(|t| t.kind == Kind::Slash)
        .filter(|g| !g.is_empty())
        .collect();
    if groups.is_empty() {
        return Err("The DDL is empty.".to_string());
    }
    if groups.len() > 2 {
        return Err(
            "At most a package specification and its body can be deployed together.".into(),
        );
    }

    let expected_name = match db_type {
        DbType::Oracle => object_name.to_ascii_uppercase(),
        DbType::Postgres => object_name.to_string(),
    };
    let mut headers = Vec::new();
    let mut statements = Vec::new();
    for group in groups.drain(..) {
        let header = parse_header(group, &chars, db_type)?;
        if let Some(found) = &header.schema
            && !same_schema(found, schema, db_type)
        {
            return Err(format!(
                "The object must be in the connection's own schema ({schema}), not {found}."
            ));
        }
        if header.name != expected_name {
            return Err(format!(
                "The DDL creates {}, but object_name is {object_name}.",
                header.name
            ));
        }
        if header.kind == "TRIGGER" {
            check_trigger_target(group, header.next, schema, db_type)?;
        }
        if db_type == DbType::Postgres {
            check_unsafe_language(group)?;
        } else if group.iter().any(|t| {
            is_word(Some(t), "EXTERNAL")
                || is_word(Some(t), "LANGUAGE")
                || is_word(Some(t), "WRAPPED")
        }) {
            return Err(
                "External routines and wrapped source (LANGUAGE / EXTERNAL / WRAPPED) cannot be deployed."
                    .to_string(),
            );
        }

        // Past the header, a `;` is only acceptable as the very last token — unless the
        // unit is Oracle PL/SQL, where it ends every inner statement.
        let keep = keeps_terminator(&header.kind, db_type);
        let mut last = group.len() - 1;
        if !keep {
            if group[last].kind == Kind::Symbol(';') {
                last -= 1;
            }
            if group[..=last].iter().any(|t| t.kind == Kind::Symbol(';')) {
                return Err(
                    "Only one statement can be deployed at a time (found a ';' before the end)."
                        .to_string(),
                );
            }
        }
        statements.push(DdlStatement {
            sql: chars[group[0].start..group[last].end].iter().collect(),
            object_type: header.kind.clone(),
        });
        headers.push(header);
    }

    let declared = object_type.trim().to_ascii_uppercase();
    match headers.as_slice() {
        [only] => {
            if only.kind != declared {
                return Err(format!(
                    "The DDL creates a {}, but object_type is {declared}.",
                    only.kind
                ));
            }
        }
        [spec, body] => {
            let pair = spec.kind == "PACKAGE" && body.kind == "PACKAGE BODY";
            if !pair || !spec.or_replace || !body.or_replace {
                return Err(
                    "Two statements are only allowed as CREATE OR REPLACE PACKAGE followed by \
                            CREATE OR REPLACE PACKAGE BODY."
                        .to_string(),
                );
            }
            if declared != "PACKAGE" {
                return Err(
                    "A package specification with its body must be declared with object_type PACKAGE."
                        .to_string(),
                );
            }
        }
        _ => return Err("The DDL must contain exactly one statement.".to_string()),
    }
    Ok(statements)
}

/// `CREATE PACKAGE + PACKAGE BODY`-style plan summary for the approval screen.
pub fn summarize(statements: &[DdlStatement]) -> String {
    let kinds: Vec<&str> = statements.iter().map(|s| s.object_type.as_str()).collect();
    format!("CREATE {}", kinds.join(" + "))
}

#[cfg(test)]
#[path = "tests/mcp_ddl_guard.rs"]
mod tests;
