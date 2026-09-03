use super::TemperatureRepository;
use crate::{AppError, ListParams, Stats, TemperatureEntry};
use async_trait::async_trait;
use sqlx::{sqlite::SqliteConnectOptions, QueryBuilder, Sqlite, SqlitePool};
use std::{path::Path, str::FromStr, time::Duration};
use tracing::{info, warn};

const LEGACY_IMPORT_KEY: &str = "legacy_json_import_completed";

pub(crate) struct SqliteTemperatureRepository {
    pool: SqlitePool,
}

impl SqliteTemperatureRepository {
    pub(crate) async fn connect(database_url: &str) -> Result<Self, AppError> {
        let options = SqliteConnectOptions::from_str(database_url)
            .map_err(|error| AppError::Internal(format!("Ungültige DATABASE_URL: {error}")))?
            .create_if_missing(true)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePool::connect_with(options)
            .await
            .map_err(database_error)?;
        sqlx::migrate!().run(&pool).await.map_err(database_error)?;
        Ok(Self { pool })
    }
}

#[async_trait]
impl TemperatureRepository for SqliteTemperatureRepository {
    async fn count(&self) -> Result<usize, AppError> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM temperatures")
            .fetch_one(&self.pool)
            .await
            .map_err(database_error)?;
        usize::try_from(count)
            .map_err(|_| AppError::Internal("Ungültige Anzahl in der Datenbank".into()))
    }

    async fn list(&self, params: &ListParams) -> Result<Vec<TemperatureEntry>, AppError> {
        let (offset, limit) = crate::validate_list_params(params)?;
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT id, temperature, timestamp, sensor_id, location FROM temperatures WHERE 1=1",
        );
        if let Some(sensor_id) = &params.sensor_id {
            query.push(" AND sensor_id = ").push_bind(sensor_id);
        }
        if let Some(location) = &params.location {
            query.push(" AND location = ").push_bind(location);
        }
        if let Some(from) = params.from {
            query.push(" AND timestamp >= ").push_bind(from);
        }
        if let Some(to) = params.to {
            query.push(" AND timestamp <= ").push_bind(to);
        }
        query
            .push(" ORDER BY timestamp DESC, id DESC LIMIT ")
            .push_bind(limit as i64)
            .push(" OFFSET ")
            .push_bind(offset as i64);

        query
            .build_query_as::<TemperatureEntry>()
            .fetch_all(&self.pool)
            .await
            .map_err(database_error)
    }

    async fn latest(&self) -> Result<Option<TemperatureEntry>, AppError> {
        sqlx::query_as::<_, TemperatureEntry>(
            "SELECT id, temperature, timestamp, sensor_id, location \
             FROM temperatures ORDER BY timestamp DESC, id DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(database_error)
    }

    async fn get(&self, id: &str) -> Result<Option<TemperatureEntry>, AppError> {
        sqlx::query_as::<_, TemperatureEntry>(
            "SELECT id, temperature, timestamp, sensor_id, location \
             FROM temperatures WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(database_error)
    }

    async fn create(&self, entry: &TemperatureEntry) -> Result<(), AppError> {
        sqlx::query(
            "INSERT INTO temperatures (id, temperature, timestamp, sensor_id, location) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&entry.id)
        .bind(entry.temperature)
        .bind(entry.timestamp)
        .bind(&entry.sensor_id)
        .bind(&entry.location)
        .execute(&self.pool)
        .await
        .map_err(database_error)?;
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<bool, AppError> {
        let result = sqlx::query("DELETE FROM temperatures WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(database_error)?;
        Ok(result.rows_affected() > 0)
    }

    async fn delete_all(&self) -> Result<(), AppError> {
        sqlx::query("DELETE FROM temperatures")
            .execute(&self.pool)
            .await
            .map_err(database_error)?;
        Ok(())
    }

    async fn stats(&self) -> Result<Stats, AppError> {
        let (count, avg, min, max): (i64, Option<f64>, Option<f64>, Option<f64>) = sqlx::query_as(
            "SELECT COUNT(*), AVG(temperature), MIN(temperature), MAX(temperature) \
                 FROM temperatures",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(database_error)?;
        Ok(Stats {
            count: usize::try_from(count)
                .map_err(|_| AppError::Internal("Ungültige Anzahl in der Datenbank".into()))?,
            avg,
            min,
            max,
            latest: self.latest().await?,
        })
    }

    async fn import_legacy_json(&self, path: &Path) -> Result<usize, AppError> {
        let already_imported: Option<String> =
            sqlx::query_scalar("SELECT value FROM application_metadata WHERE key = ?")
                .bind(LEGACY_IMPORT_KEY)
                .fetch_optional(&self.pool)
                .await
                .map_err(database_error)?;
        if already_imported.is_some() {
            return Ok(0);
        }

        let entries = match tokio::fs::read_to_string(path).await {
            Ok(content) => {
                serde_json::from_str::<Vec<TemperatureEntry>>(&content).map_err(|error| {
                    AppError::Internal(format!(
                        "Alte Datendatei {} konnte nicht gelesen werden: {error}",
                        path.display()
                    ))
                })?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => {
                return Err(AppError::Internal(format!(
                    "Alte Datendatei {} konnte nicht geöffnet werden: {error}",
                    path.display()
                )))
            }
        };

        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        let database_is_empty: bool =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM temperatures")
                .fetch_one(&mut *transaction)
                .await
                .map_err(database_error)?
                == 0;
        let mut imported = 0;
        if database_is_empty {
            for entry in &entries {
                let result = sqlx::query(
                    "INSERT OR IGNORE INTO temperatures \
                     (id, temperature, timestamp, sensor_id, location) VALUES (?, ?, ?, ?, ?)",
                )
                .bind(&entry.id)
                .bind(entry.temperature)
                .bind(entry.timestamp)
                .bind(&entry.sensor_id)
                .bind(&entry.location)
                .execute(&mut *transaction)
                .await
                .map_err(database_error)?;
                imported += result.rows_affected() as usize;
            }
        } else if !entries.is_empty() {
            warn!("SQLite-Datenbank ist nicht leer; alter JSON-Datenbestand wird nicht importiert");
        }
        sqlx::query("INSERT INTO application_metadata (key, value) VALUES (?, ?)")
            .bind(LEGACY_IMPORT_KEY)
            .bind(path.display().to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        if imported > 0 {
            info!("{imported} Einträge aus {} importiert", path.display());
        }
        Ok(imported)
    }
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Internal(format!("Datenbankfehler: {error}"))
}
