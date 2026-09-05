use std::path::PathBuf;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::models::{HistoryNamingRule, MatchType};

/// Default seed rules, matching the naming convention that used to be hardcoded as a
/// single prefix + suffix pair. Users can edit or delete these from the History Tables
/// view once seeded.
const DEFAULT_RULES: &[(MatchType, &str)] = &[
    (MatchType::Prefix, "HIST_"),
    (MatchType::Suffix, "_HIST"),
];

pub trait HistoryNamingRuleRepository: Send + Sync {
    fn init_db(&self) -> Result<()>;
    fn list_rules(&self) -> Result<Vec<HistoryNamingRule>>;
    fn insert_rule(&self, rule: &HistoryNamingRule) -> Result<HistoryNamingRule>;
    fn update_rule(&self, id: i64, rule: &HistoryNamingRule) -> Result<HistoryNamingRule>;
    fn delete_rule(&self, id: i64) -> Result<()>;
}

pub struct SqliteHistoryNamingRuleRepository;

impl SqliteHistoryNamingRuleRepository {
    pub fn new() -> Self {
        Self
    }

    fn db_path() -> PathBuf {
        PathBuf::from("schemetry.db")
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
            _ => MatchType::Prefix,
        }
    }
}

impl HistoryNamingRuleRepository for SqliteHistoryNamingRuleRepository {
    fn init_db(&self) -> Result<()> {
        let conn = Connection::open(Self::db_path())?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS history_naming_rules (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                match_type TEXT    NOT NULL,
                pattern    TEXT    NOT NULL,
                enabled    INTEGER NOT NULL DEFAULT 1
            )",
            [],
        )?;

        let count: i64 = conn.query_row("SELECT COUNT(1) FROM history_naming_rules", [], |row| row.get(0))?;
        if count == 0 {
            for &(match_type, pattern) in DEFAULT_RULES {
                conn.execute(
                    "INSERT INTO history_naming_rules (match_type, pattern, enabled)
                     VALUES (?1, ?2, 1)",
                    params![Self::match_type_str(match_type), pattern],
                )?;
            }
        }

        Ok(())
    }

    fn list_rules(&self) -> Result<Vec<HistoryNamingRule>> {
        let conn = Connection::open(Self::db_path())?;
        let mut stmt = conn.prepare(
            "SELECT id, match_type, pattern, enabled FROM history_naming_rules ORDER BY id",
        )?;

        let rows = stmt
            .query_map([], |row| {
                Ok(HistoryNamingRule {
                    id: row.get(0)?,
                    match_type: Self::parse_match_type(&row.get::<_, String>(1)?),
                    pattern: row.get(2)?,
                    enabled: row.get::<_, i64>(3)? != 0,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(rows)
    }

    fn insert_rule(&self, rule: &HistoryNamingRule) -> Result<HistoryNamingRule> {
        let conn = Connection::open(Self::db_path())?;
        conn.execute(
            "INSERT INTO history_naming_rules (match_type, pattern, enabled) VALUES (?1, ?2, ?3)",
            params![
                Self::match_type_str(rule.match_type),
                rule.pattern,
                rule.enabled as i64,
            ],
        )
        .context("Failed to insert history naming rule")?;

        let id = conn.last_insert_rowid();
        Ok(HistoryNamingRule { id, ..rule.clone() })
    }

    fn update_rule(&self, id: i64, rule: &HistoryNamingRule) -> Result<HistoryNamingRule> {
        let conn = Connection::open(Self::db_path())?;

        let exists: Option<i64> = conn
            .query_row("SELECT id FROM history_naming_rules WHERE id = ?1", [id], |row| row.get(0))
            .optional()?;
        exists.ok_or_else(|| anyhow::anyhow!("History naming rule id={id} not found."))?;

        conn.execute(
            "UPDATE history_naming_rules SET match_type=?1, pattern=?2, enabled=?3 WHERE id=?4",
            params![
                Self::match_type_str(rule.match_type),
                rule.pattern,
                rule.enabled as i64,
                id,
            ],
        )
        .with_context(|| format!("Failed to update history naming rule id={id}"))?;

        Ok(HistoryNamingRule { id, ..rule.clone() })
    }

    fn delete_rule(&self, id: i64) -> Result<()> {
        let conn = Connection::open(Self::db_path())?;
        conn.execute("DELETE FROM history_naming_rules WHERE id = ?1", [id])?;
        Ok(())
    }
}
