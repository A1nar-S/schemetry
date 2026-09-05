use std::sync::Arc;

use anyhow::Result;

use crate::models::HistoryNamingRule;
use crate::repositories::history_naming_rule_repository::HistoryNamingRuleRepository;

pub struct HistoryNamingRuleService {
    repo: Arc<dyn HistoryNamingRuleRepository>,
}

impl HistoryNamingRuleService {
    pub fn new(repo: Arc<dyn HistoryNamingRuleRepository>) -> Self {
        Self { repo }
    }

    pub fn init_db(&self) -> Result<()> {
        self.repo.init_db()
    }

    pub fn list_rules(&self) -> Result<Vec<HistoryNamingRule>> {
        self.repo.list_rules()
    }

    /// Insert a new rule (id == 0) or update an existing one.
    pub fn save_rule(&self, rule: &HistoryNamingRule) -> Result<HistoryNamingRule> {
        if rule.id == 0 {
            self.repo.insert_rule(rule)
        } else {
            self.repo.update_rule(rule.id, rule)
        }
    }

    pub fn delete_rule(&self, id: i64) -> Result<()> {
        self.repo.delete_rule(id)
    }

    /// Rules currently enabled, ready to be passed into the Oracle repository's
    /// history-table pairing query.
    pub fn list_active_rules(&self) -> Result<Vec<HistoryNamingRule>> {
        Ok(self.repo.list_rules()?.into_iter().filter(|r| r.enabled).collect())
    }
}
