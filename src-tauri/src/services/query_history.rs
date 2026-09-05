use std::sync::Arc;

use anyhow::Result;

use crate::models::QueryHistoryEntry;
use crate::repositories::query_history_repository::QueryHistoryRepository;

pub struct QueryHistoryService {
    repo: Arc<dyn QueryHistoryRepository>,
}

impl QueryHistoryService {
    pub fn new(repo: Arc<dyn QueryHistoryRepository>) -> Self {
        Self { repo }
    }

    pub fn init_db(&self) -> Result<()> {
        self.repo.init_db()
    }

    pub fn load_history(&self) -> Result<Vec<QueryHistoryEntry>> {
        self.repo.load_history()
    }

    pub fn append_query(&self, sql: &str) -> Result<()> {
        self.repo.append_query(sql)
    }

    pub fn delete_query(&self, id: i64) -> Result<()> {
        self.repo.delete_query(id)
    }

    pub fn pin_query(&self, id: i64, pinned: bool) -> Result<()> {
        self.repo.pin_query(id, pinned)
    }

    pub fn set_favorite(&self, id: i64, favorite: bool, description: &str) -> Result<()> {
        self.repo.set_favorite(id, favorite, description)
    }

    pub fn reorder_favorites(&self, ordered_ids: &[i64]) -> Result<()> {
        self.repo.reorder_favorites(ordered_ids)
    }

    pub fn clear_history(&self) -> Result<()> {
        self.repo.clear_history()
    }
}