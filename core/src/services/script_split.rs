//! Splits a pasted DDL script into the statements to run one by one.
//!
//! Oracle PL/SQL units (`CREATE PACKAGE|PROCEDURE|FUNCTION|TRIGGER|TYPE`, `BEGIN`,
//! `DECLARE`) contain `;`, so they end at a line holding only `/`; everything else ends at
//! a `;` outside strings and comments. Postgres only splits at `;` (dollar quotes aside).

use crate::models::DbType;

/// One statement and the object it creates, when that can be told.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptStatement {
    pub sql: String,
    /// Upper-case type of the created object, e.g. `PACKAGE BODY`; empty for other statements.
    pub object_type: String,
    /// Unqualified, unquoted name of the created object (upper-cased for Oracle).
    pub object_name: String,
}

pub fn split_script(script: &str, db_type: DbType) -> Vec<ScriptStatement> {
    let chars: Vec<char> = script.chars().collect();
    let oracle = db_type == DbType::Oracle;
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut has_code = false;
    let mut i = 0usize;

    let flush = |buf: &mut String, has_code: &mut bool, out: &mut Vec<ScriptStatement>| {
        if *has_code {
            let sql = buf.trim().to_string();
            let (object_type, object_name) = created_object(&sql, db_type);
            out.push(ScriptStatement {
                sql,
                object_type,
                object_name,
            });
        }
        buf.clear();
        *has_code = false;
    };

    while i < chars.len() {
        let c = chars[i];
        let at_line_start = i == 0 || chars[i - 1] == '\n';

        if oracle && at_line_start && line_is_slash(&chars, i) {
            flush(&mut buf, &mut has_code, &mut out);
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }

        if c == '-' && chars.get(i + 1) == Some(&'-') {
            while i < chars.len() && chars[i] != '\n' {
                buf.push(chars[i]);
                i += 1;
            }
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            let mut depth = 1;
            buf.push_str("/*");
            i += 2;
            while i < chars.len() && depth > 0 {
                if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    buf.push_str("*/");
                    i += 2;
                } else if !oracle && chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                    depth += 1;
                    buf.push_str("/*");
                    i += 2;
                } else {
                    buf.push(chars[i]);
                    i += 1;
                }
            }
            continue;
        }

        if c == '\'' || c == '"' {
            has_code = true;
            let end = skip_quoted(&chars, i, c);
            buf.extend(&chars[i..end]);
            i = end;
            continue;
        }
        // Oracle q'[...]' alternative quoting.
        if oracle
            && (c == 'q' || c == 'Q')
            && chars.get(i + 1) == Some(&'\'')
            && chars.get(i + 2).is_some_and(|d| !d.is_whitespace())
            && !prev_is_word_char(&chars, i)
        {
            has_code = true;
            let end = skip_alternative_quote(&chars, i + 1);
            buf.extend(&chars[i..end]);
            i = end;
            continue;
        }
        // Postgres dollar quoting.
        if !oracle && c == '$' {
            if let Some(tag_end) = dollar_tag_end(&chars, i) {
                has_code = true;
                let end = skip_dollar_quoted(&chars, i, tag_end);
                buf.extend(&chars[i..end]);
                i = end;
                continue;
            }
        }

        if c == ';' && !(oracle && is_plsql_unit(&buf)) {
            flush(&mut buf, &mut has_code, &mut out);
            i += 1;
            continue;
        }

        if !c.is_whitespace() {
            has_code = true;
        }
        buf.push(c);
        i += 1;
    }
    flush(&mut buf, &mut has_code, &mut out);
    out
}

fn line_is_slash(chars: &[char], start: usize) -> bool {
    let mut i = start;
    while i < chars.len() && chars[i] != '\n' {
        i += 1;
    }
    chars[start..i].iter().collect::<String>().trim() == "/"
}

fn prev_is_word_char(chars: &[char], i: usize) -> bool {
    i > 0 && (chars[i - 1].is_alphanumeric() || matches!(chars[i - 1], '_' | '$' | '#'))
}

fn skip_quoted(chars: &[char], start: usize, quote: char) -> usize {
    let mut i = start + 1;
    while i < chars.len() {
        if chars[i] == quote {
            if chars.get(i + 1) == Some(&quote) {
                i += 2;
                continue;
            }
            return i + 1;
        }
        i += 1;
    }
    chars.len()
}

fn skip_alternative_quote(chars: &[char], quote_at: usize) -> usize {
    let open = chars[quote_at + 1];
    let close = match open {
        '[' => ']',
        '(' => ')',
        '{' => '}',
        '<' => '>',
        other => other,
    };
    let mut i = quote_at + 2;
    while i + 1 < chars.len() {
        if chars[i] == close && chars[i + 1] == '\'' {
            return i + 2;
        }
        i += 1;
    }
    chars.len()
}

fn dollar_tag_end(chars: &[char], start: usize) -> Option<usize> {
    let mut j = start + 1;
    while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') {
        j += 1;
    }
    let digit_first = chars.get(start + 1).is_some_and(|c| c.is_ascii_digit());
    (chars.get(j) == Some(&'$') && !digit_first).then_some(j)
}

fn skip_dollar_quoted(chars: &[char], start: usize, tag_end: usize) -> usize {
    let tag: Vec<char> = chars[start..=tag_end].to_vec();
    let mut i = tag_end + 1;
    while i + tag.len() <= chars.len() {
        if chars[i..i + tag.len()] == tag[..] {
            return i + tag.len();
        }
        i += 1;
    }
    chars.len()
}

/// The first words of `sql`, comments skipped, upper-cased.
fn leading_words(sql: &str, max: usize) -> Vec<String> {
    let chars: Vec<char> = sql.chars().collect();
    let mut words = Vec::new();
    let mut i = 0;
    while i < chars.len() && words.len() < max {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '-' && chars.get(i + 1) == Some(&'-') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
        } else if c.is_alphanumeric() || matches!(c, '_' | '"' | '.' | '$' | '#') {
            let start = i;
            let mut in_quote = false;
            while i < chars.len()
                && (in_quote
                    || chars[i].is_alphanumeric()
                    || matches!(chars[i], '_' | '"' | '.' | '$' | '#'))
            {
                if chars[i] == '"' {
                    in_quote = !in_quote;
                }
                i += 1;
            }
            words.push(chars[start..i].iter().collect::<String>());
        } else {
            words.push(c.to_string());
            i += 1;
        }
    }
    words
}

fn is_plsql_unit(sql: &str) -> bool {
    let words = leading_words(sql, 8);
    let upper: Vec<String> = words.iter().map(|w| w.to_ascii_uppercase()).collect();
    match upper.first().map(String::as_str) {
        Some("BEGIN" | "DECLARE") => true,
        Some("CREATE") => upper.iter().skip(1).any(|w| {
            matches!(
                w.as_str(),
                "PACKAGE" | "PROCEDURE" | "FUNCTION" | "TRIGGER" | "TYPE"
            )
        }),
        _ => false,
    }
}

const KINDS: &[&str] = &[
    "VIEW",
    "PROCEDURE",
    "FUNCTION",
    "PACKAGE",
    "TRIGGER",
    "TYPE",
    "MATERIALIZED",
    "TABLE",
    "INDEX",
    "SEQUENCE",
    "SYNONYM",
];

/// `(type, name)` of the object a `CREATE` statement defines, or two empty strings.
fn created_object(sql: &str, db_type: DbType) -> (String, String) {
    let words = leading_words(sql, 14);
    if !words
        .first()
        .is_some_and(|w| w.eq_ignore_ascii_case("CREATE"))
    {
        return (String::new(), String::new());
    }
    let Some(kind_at) = words
        .iter()
        .position(|w| KINDS.contains(&w.to_ascii_uppercase().as_str()))
    else {
        return (String::new(), String::new());
    };
    let mut kind = words[kind_at].to_ascii_uppercase();
    let mut next = kind_at + 1;
    if kind == "MATERIALIZED" {
        kind = "MATERIALIZED VIEW".to_string();
        next += 1;
    } else if matches!(kind.as_str(), "PACKAGE" | "TYPE")
        && words
            .get(next)
            .is_some_and(|w| w.eq_ignore_ascii_case("BODY"))
    {
        kind.push_str(" BODY");
        next += 1;
    }
    let Some(raw) = words.get(next) else {
        return (kind, String::new());
    };
    let last = raw.rsplit('.').next().unwrap_or(raw);
    let quoted = last.starts_with('"');
    let name = last.trim_matches('"').to_string();
    let name = match db_type {
        DbType::Oracle if !quoted => name.to_ascii_uppercase(),
        _ => name,
    };
    (kind, name)
}

#[cfg(test)]
#[path = "tests/script_split.rs"]
mod tests;
