use std::sync::Arc;

use anyhow::Result;

use crate::models::TableFilterRule;
use crate::repositories::filter_rule_repository::FilterRuleRepository;

pub struct FilterRuleService {
    repo: Arc<dyn FilterRuleRepository>,
}

impl FilterRuleService {
    pub fn new(repo: Arc<dyn FilterRuleRepository>) -> Self {
        Self { repo }
    }

    pub fn init_db(&self) -> Result<()> {
        self.repo.init_db()
    }

    pub fn list_rules(&self) -> Result<Vec<TableFilterRule>> {
        self.repo.list_rules()
    }

    /// Insert a new rule (id == 0) or update an existing one.
    pub fn save_rule(&self, rule: &TableFilterRule) -> Result<TableFilterRule> {
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
    /// filtering queries.
    pub fn list_active_rules(&self) -> Result<Vec<TableFilterRule>> {
        Ok(self.repo.list_rules()?.into_iter().filter(|r| r.enabled).collect())
    }
}
