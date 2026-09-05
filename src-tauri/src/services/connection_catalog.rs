use std::sync::Arc;

use anyhow::{Context, Result};

use crate::models::ConnectionRecord;
use crate::repositories::connection_repository::ConnectionRepository;

pub struct ConnectionCatalogService {
    repo: Arc<dyn ConnectionRepository>,
}

impl ConnectionCatalogService {
    pub fn new(repo: Arc<dyn ConnectionRepository>) -> Self {
        Self { repo }
    }

    pub fn init_db(&self) -> Result<()> {
        self.repo.init_db()
    }

    pub fn get_all_connections(&self) -> Result<Vec<ConnectionRecord>> {
        self.repo.get_all_connections()
    }

    pub fn insert_connection(&self, data: &ConnectionRecord) -> Result<()> {
        self.repo.insert_connection(data)
    }

    pub fn update_connection(&self, id: i64, data: &ConnectionRecord) -> Result<()> {
        self.repo.update_connection(id, data)
    }

    pub fn delete_connection(&self, id: i64) -> Result<()> {
        self.repo.delete_connection(id)
    }

    pub fn delete_all_connections(&self) -> Result<()> {
        self.repo.delete_all_connections()
    }

    /// Parse a JSON array of `ConnectionRecord` and upsert each entry.
    /// Returns the number of records processed.
    pub fn import_from_json(&self, json: &str) -> Result<usize> {
        let records: Vec<ConnectionRecord> =
            serde_json::from_str(json).context("Invalid JSON: expected an array of connection objects")?;
        let count = records.len();
        for record in &records {
            self.repo.upsert_connection(record)?;
        }
        Ok(count)
    }
}
