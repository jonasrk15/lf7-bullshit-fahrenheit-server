mod sqlite;

use crate::{AppError, ListParams, Stats, TemperatureEntry};
use async_trait::async_trait;
use std::path::Path;

pub(crate) use sqlite::SqliteTemperatureRepository;

#[async_trait]
pub(crate) trait TemperatureRepository: Send + Sync {
    async fn count(&self) -> Result<usize, AppError>;
    async fn list(&self, params: &ListParams) -> Result<Vec<TemperatureEntry>, AppError>;
    async fn latest(&self) -> Result<Option<TemperatureEntry>, AppError>;
    async fn get(&self, id: &str) -> Result<Option<TemperatureEntry>, AppError>;
    async fn create(&self, entry: &TemperatureEntry) -> Result<(), AppError>;
    async fn delete(&self, id: &str) -> Result<bool, AppError>;
    async fn delete_all(&self) -> Result<(), AppError>;
    async fn stats(&self) -> Result<Stats, AppError>;
    async fn import_legacy_json(&self, path: &Path) -> Result<usize, AppError>;
}
