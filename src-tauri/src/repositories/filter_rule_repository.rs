use std::path::PathBuf;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::models::{FilterAction, MatchType, TableFilterRule};

/// Default rules seeded on first run, matching the naming conventions that used to be
/// hardcoded as `NOT LIKE` clauses in the Oracle fetch queries. Users can edit or delete
/// these from the Settings UI once seeded.
const DEFAULT_RULES: &[(MatchType, &str)] = &[
    (MatchType::Prefix, "V_"),
    (MatchType::Prefix, "VW_"),
    (MatchType::Prefix, "HIST_"),
    (MatchType::Prefix, "TEST_"),
    (MatchType::Prefix, "TEMP_"),
    (MatchType::Prefix, "TMP_"),
    (MatchType::Suffix, "_VW"),
    (MatchType::Suffix, "_HIST"),
    (MatchType::Suffix, "_TEST"),
    (MatchType::Suffix, "_TEMP"),
    (MatchType::Suffix, "_TMP"),
    (MatchType::Contains, "_TEST_"),
    (MatchType::Contains, "_TEMP_"),
    (MatchType::Contains, "_TMP_"),
];

pub trait FilterRuleRepository: Send + Sync {
    fn init_db(&self) -> Result<()>;
    fn list_rules(&self) -> Result<Vec<TableFilterRule>>;
    fn insert_rule(&self, rule: &TableFilterRule) -> Result<TableFilterRule>;
    fn update_rule(&self, id: i64, rule: &TableFilterRule) -> Result<TableFilterRule>;
    fn delete_rule(&self, id: i64) -> Result<()>;
}

pub struct SqliteFilterRuleRepository;

impl SqliteFilterRuleRepository {
    pub fn new() -> Self {
        Self
    }

    fn db_path() -> PathBuf {
        PathBuf::from("schemetry.db")
    }

    fn action_str(action: FilterAction) -> &'static str {
        match action {
            FilterAction::Exclude => "exclude",
            FilterAction::Include => "include",
        }
    }

    fn parse_action(s: &str) -> FilterAction {
        match s {
            "include" => FilterAction::Include,
            _ => FilterAction::Exclude,
        }
    }

    fn match_type_str(match_type: MatchType) -> &'static str {
        match match_type {
            MatchType::Prefix => "prefix",
            MatchType::Suffix => "suffix",
            MatchType::Contains => "contains",
            MatchType::Exact => "exact",
        }
    }

    fn parse_match_type(s: &str) -> MatchType {
        match s {
            "suffix" => MatchType::Suffix,
            "contains" => MatchType::Contains,
            "exact" => MatchType::Exact,
            _ => MatchType::Prefix,
        }
    }
}

impl FilterRuleRepository for SqliteFilterRuleRepository {
    fn init_db(&self) -> Result<()> {
        let conn = Connection::open(Self::db_path())?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS table_filter_rules (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                action     TEXT    NOT NULL,
                match_type TEXT    NOT NULL,
                pattern    TEXT    NOT NULL,
                enabled    INTEGER NOT NULL DEFAULT 1
            )",
            [],
        )?;

        let count: i64 = conn.query_row("SELECT COUNT(1) FROM table_filter_rules", [], |row| row.get(0))?;
        if count == 0 {
            for &(match_type, pattern) in DEFAULT_RULES {
                conn.execute(
                    "INSERT INTO table_filter_rules (action, match_type, pattern, enabled)
                     VALUES (?1, ?2, ?3, 1)",
                    params![Self::action_str(FilterAction::Exclude), Self::match_type_str(match_type), pattern],
                )?;
            }
        }

        Ok(())
    }

    fn list_rules(&self) -> Result<Vec<TableFilterRule>> {
        let conn = Connection::open(Self::db_path())?;
        let mut stmt = conn.prepare(
            "SELECT id, action, match_type, pattern, enabled FROM table_filter_rules ORDER BY id",
        )?;

        let rows = stmt
            .query_map([], |row| {
                Ok(TableFilterRule {
                    id: row.get(0)?,
                    action: Self::parse_action(&row.get::<_, String>(1)?),
                    match_type: Self::parse_match_type(&row.get::<_, String>(2)?),
                    pattern: row.get(3)?,
                    enabled: row.get::<_, i64>(4)? != 0,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(rows)
    }

    fn insert_rule(&self, rule: &TableFilterRule) -> Result<TableFilterRule> {
        let conn = Connection::open(Self::db_path())?;
        conn.execute(
            "INSERT INTO table_filter_rules (action, match_type, pattern, enabled) VALUES (?1, ?2, ?3, ?4)",
            params![
                Self::action_str(rule.action),
                Self::match_type_str(rule.match_type),
                rule.pattern,
                rule.enabled as i64,
            ],
        )
        .context("Failed to insert table filter rule")?;

        let id = conn.last_insert_rowid();
        Ok(TableFilterRule { id, ..rule.clone() })
    }

    fn update_rule(&self, id: i64, rule: &TableFilterRule) -> Result<TableFilterRule> {
        let conn = Connection::open(Self::db_path())?;

        let exists: Option<i64> = conn
            .query_row("SELECT id FROM table_filter_rules WHERE id = ?1", [id], |row| row.get(0))
            .optional()?;
        exists.ok_or_else(|| anyhow::anyhow!("Filter rule id={id} not found."))?;

        conn.execute(
            "UPDATE table_filter_rules SET action=?1, match_type=?2, pattern=?3, enabled=?4 WHERE id=?5",
            params![
                Self::action_str(rule.action),
                Self::match_type_str(rule.match_type),
                rule.pattern,
                rule.enabled as i64,
                id,
            ],
        )
        .with_context(|| format!("Failed to update filter rule id={id}"))?;

        Ok(TableFilterRule { id, ..rule.clone() })
    }

    fn delete_rule(&self, id: i64) -> Result<()> {
        let conn = Connection::open(Self::db_path())?;
        conn.execute("DELETE FROM table_filter_rules WHERE id = ?1", [id])?;
        Ok(())
    }
}

/// Build an Oracle SQL `AND` fragment (using `:N` positional binds, matching the `oracle`
/// crate's placeholder style) plus its ordered bind values for the given rules, applied to
/// `column`. Only `enabled` rules are considered. Exclude rules are ANDed together as
/// `NOT LIKE`; if any Include rules are present, they're combined into a single ORed
/// allow-list clause (a row must match at least one Include pattern to survive).
/// `start_idx` is the 1-based bind position of the first placeholder this fragment emits
/// (i.e. one past the last placeholder already used by the caller's base query).
/// Returns an empty fragment and no binds if there are no enabled rules.
pub fn build_predicate(rules: &[TableFilterRule], column: &str, start_idx: usize) -> (String, Vec<String>) {
    let enabled: Vec<&TableFilterRule> = rules.iter().filter(|r| r.enabled).collect();

    let mut clauses = Vec::new();
    let mut binds = Vec::new();
    let mut next_idx = start_idx;

    for rule in enabled.iter().filter(|r| r.action == FilterAction::Exclude) {
        clauses.push(format!("UPPER({column}) NOT LIKE :{next_idx}"));
        binds.push(like_pattern(rule.match_type, &rule.pattern));
        next_idx += 1;
    }

    let include_patterns: Vec<&TableFilterRule> = enabled
        .iter()
        .filter(|r| r.action == FilterAction::Include)
        .copied()
        .collect();
    if !include_patterns.is_empty() {
        let ors: Vec<String> = include_patterns
            .iter()
            .map(|_| {
                let clause = format!("UPPER({column}) LIKE :{next_idx}");
                next_idx += 1;
                clause
            })
            .collect();
        clauses.push(format!("({})", ors.join(" OR ")));
        for rule in include_patterns {
            binds.push(like_pattern(rule.match_type, &rule.pattern));
        }
    }

    if clauses.is_empty() {
        (String::new(), Vec::new())
    } else {
        (format!(" AND {}", clauses.join(" AND ")), binds)
    }
}

fn like_pattern(match_type: MatchType, pattern: &str) -> String {
    let upper = pattern.trim().to_ascii_uppercase();
    match match_type {
        MatchType::Prefix => format!("{upper}%"),
        MatchType::Suffix => format!("%{upper}"),
        MatchType::Contains => format!("%{upper}%"),
        MatchType::Exact => upper,
    }
}

#[cfg(test)]
#[path = "tests/filter_rule_repository.rs"]
mod tests;
