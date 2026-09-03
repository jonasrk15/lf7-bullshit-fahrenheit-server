use axum::{
    extract::DefaultBodyLimit,
    extract::{Path, Query, State},
    http::{header::CONTENT_TYPE, HeaderValue, Method, StatusCode},
    response::{Html, IntoResponse, Json},
    routing::{get, post},
    Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
};
use tower_http::cors::CorsLayer;
use tracing::{error, info};
use uuid::Uuid;

mod storage;
use storage::{SqliteTemperatureRepository, TemperatureRepository};

const DEFAULT_BIND_ADDR: &str = "0.0.0.0:3000";
const DEFAULT_DATABASE_URL: &str = "sqlite://temperatures.db";
const DEFAULT_LEGACY_DATA_FILE: &str = "data.json";
const DEFAULT_PAGE_LIMIT: usize = 100;
const MAX_PAGE_LIMIT: usize = 1_000;
const MAX_METADATA_LENGTH: usize = 200;
const MAX_REQUEST_BODY_SIZE: usize = 16 * 1024;

// --- Datenmodelle ---

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub(crate) struct TemperatureEntry {
    id: String,
    temperature: f64,
    timestamp: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sensor_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateTemperature {
    temperature: f64,
    sensor_id: Option<String>,
    location: Option<String>,
    timestamp: Option<DateTime<Utc>>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ListParams {
    limit: Option<usize>,
    offset: Option<usize>,
    sensor_id: Option<String>,
    location: Option<String>,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Stats {
    count: usize,
    avg: Option<f64>,
    min: Option<f64>,
    max: Option<f64>,
    latest: Option<TemperatureEntry>,
}

#[derive(Debug, Serialize)]
struct Health {
    status: String,
    count: usize,
    version: String,
}

// --- Shared State ---

struct AppState {
    repository: Arc<dyn TemperatureRepository>,
}

type SharedState = Arc<AppState>;

struct Config {
    bind_addr: SocketAddr,
    database_url: String,
    legacy_data_file: PathBuf,
    seed_demo: bool,
    cors_origin: Option<HeaderValue>,
}

impl Config {
    fn from_env() -> Result<Self, String> {
        let bind_addr = std::env::var("BIND_ADDR")
            .unwrap_or_else(|_| DEFAULT_BIND_ADDR.to_string())
            .parse()
            .map_err(|error| format!("Ungültige BIND_ADDR: {error}"))?;
        let database_url = std::env::var("DATABASE_URL")
            .unwrap_or_else(|_| DEFAULT_DATABASE_URL.to_string());
        if !database_url.starts_with("sqlite:") {
            return Err(
                "Diese Version unterstützt nur SQLite-DATABASE_URLs; PostgreSQL kann über einen zusätzlichen Repository-Adapter ergänzt werden"
                    .to_string(),
            );
        }
        let legacy_data_file = std::env::var_os("LEGACY_DATA_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_LEGACY_DATA_FILE));
        let seed_demo = std::env::var("SEED_DEMO")
            .map(|value| {
                value
                    .parse::<bool>()
                    .map_err(|_| "SEED_DEMO muss 'true' oder 'false' sein".to_string())
            })
            .unwrap_or(Ok(false))?;
        let cors_origin = match std::env::var("CORS_ORIGIN") {
            Ok(value) => Some(
                value
                    .parse::<HeaderValue>()
                    .map_err(|error| format!("Ungültige CORS_ORIGIN: {error}"))?,
            ),
            Err(std::env::VarError::NotPresent) => None,
            Err(error) => return Err(format!("Ungültige CORS_ORIGIN: {error}")),
        };

        Ok(Self {
            bind_addr,
            database_url,
            legacy_data_file,
            seed_demo,
            cors_origin,
        })
    }
}

// --- API Handler ---

async fn health_handler(State(state): State<SharedState>) -> Result<Json<Health>, AppError> {
    let count = state.repository.count().await?;
    Ok(Json(Health {
        status: "ok".to_string(),
        count,
        version: env!("CARGO_PKG_VERSION").to_string(),
    }))
}

async fn list_temperatures(
    State(state): State<SharedState>,
    Query(params): Query<ListParams>,
) -> Result<Json<Vec<TemperatureEntry>>, AppError> {
    Ok(Json(state.repository.list(&params).await?))
}

pub(crate) fn validate_list_params(params: &ListParams) -> Result<(usize, usize), AppError> {
    let limit = params.limit.unwrap_or(DEFAULT_PAGE_LIMIT);
    if limit > MAX_PAGE_LIMIT {
        return Err(AppError::BadRequest(format!(
            "limit darf höchstens {MAX_PAGE_LIMIT} sein"
        )));
    }
    if matches!((&params.from, &params.to), (Some(from), Some(to)) if from > to) {
        return Err(AppError::BadRequest(
            "from darf nicht nach to liegen".to_string(),
        ));
    }
    Ok((params.offset.unwrap_or(0), limit))
}

fn normalize_metadata(value: Option<String>, field: &str) -> Result<Option<String>, AppError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.chars().count() > MAX_METADATA_LENGTH {
        return Err(AppError::BadRequest(format!(
            "{field} darf höchstens {MAX_METADATA_LENGTH} Zeichen lang sein"
        )));
    }
    Ok(Some(value.to_string()))
}

async fn get_latest(State(state): State<SharedState>) -> Result<Json<TemperatureEntry>, AppError> {
    match state.repository.latest().await? {
        Some(entry) => Ok(Json(entry)),
        None => Err(AppError::NotFound("Keine Daten vorhanden".into())),
    }
}

async fn get_by_id(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<TemperatureEntry>, AppError> {
    state
        .repository
        .get(&id)
        .await?
        .map(Json)
        .ok_or_else(|| AppError::NotFound(format!("ID {id} nicht gefunden")))
}

async fn create_temperature(
    State(state): State<SharedState>,
    Json(payload): Json<CreateTemperature>,
) -> Result<(StatusCode, Json<TemperatureEntry>), AppError> {
    // Validierung
    if !payload.temperature.is_finite() {
        return Err(AppError::BadRequest(
            "Temperatur muss eine endliche Zahl sein".into(),
        ));
    }
    if payload.temperature < -100.0 || payload.temperature > 100.0 {
        return Err(AppError::BadRequest(
            "Temperatur muss zwischen -100 und 100 °C liegen".into(),
        ));
    }

    let entry = TemperatureEntry {
        id: Uuid::new_v4().to_string(),
        temperature: payload.temperature,
        timestamp: payload.timestamp.unwrap_or_else(Utc::now),
        sensor_id: normalize_metadata(payload.sensor_id, "sensor_id")?,
        location: normalize_metadata(payload.location, "location")?,
    };

    state.repository.create(&entry).await?;

    Ok((StatusCode::CREATED, Json(entry)))
}

async fn delete_by_id(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    if !state.repository.delete(&id).await? {
        return Err(AppError::NotFound(format!("ID {id} nicht gefunden")));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_all(State(state): State<SharedState>) -> Result<StatusCode, AppError> {
    state.repository.delete_all().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_stats(State(state): State<SharedState>) -> Result<Json<Stats>, AppError> {
    Ok(Json(state.repository.stats().await?))
}

// --- Error Handling ---

#[derive(Debug)]
enum AppError {
    BadRequest(String),
    NotFound(String),
    Internal(String),
}

impl Display for AppError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadRequest(message) | Self::NotFound(message) | Self::Internal(message) => {
                formatter.write_str(message)
            }
        }
    }
}

impl Error for AppError {}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let (status, msg) = match self {
            AppError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            AppError::NotFound(m) => (StatusCode::NOT_FOUND, m),
            AppError::Internal(m) => {
                error!("{m}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Interner Serverfehler".into(),
                )
            }
        };
        let body = Json(serde_json::json!({ "error": msg }));
        (status, body).into_response()
    }
}

// --- Frontend ---

async fn index_handler() -> Html<&'static str> {
    Html(include_str!("../static/index.html"))
}

// --- Main ---

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "temperatur_server=debug,tower_http=debug".into()),
        )
        .init();

    let config = Config::from_env().map_err(std::io::Error::other)?;
    let repository = Arc::new(
        SqliteTemperatureRepository::connect(&config.database_url).await?,
    );
    repository
        .import_legacy_json(&config.legacy_data_file)
        .await?;
    let state: SharedState = Arc::new(AppState {
        repository: repository.clone(),
    });

    if config.seed_demo && repository.count().await? == 0 {
        repository
            .create(&TemperatureEntry {
                id: Uuid::new_v4().to_string(),
                temperature: 21.5,
                timestamp: Utc::now(),
                sensor_id: Some("demo-sensor-1".into()),
                location: Some("Wohnzimmer".into()),
            })
            .await?;
    }

    let mut app = Router::new()
        .route("/", get(index_handler))
        .route("/api/health", get(health_handler))
        .route(
            "/api/temperatures",
            get(list_temperatures)
                .post(create_temperature)
                .delete(delete_all),
        )
        .route("/api/temperatures/latest", get(get_latest))
        .route("/api/temperatures/:id", get(get_by_id).delete(delete_by_id))
        .route("/api/temperatures/clear", post(delete_all))
        .route("/api/temperatures/:id/delete", post(delete_by_id))
        .route("/api/stats", get(get_stats))
        .with_state(state)
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_SIZE))
        .layer(tower_http::trace::TraceLayer::new_for_http());

    if let Some(origin) = config.cors_origin {
        app = app.layer(
            CorsLayer::new()
                .allow_origin(origin)
                .allow_methods([Method::GET, Method::POST, Method::DELETE])
                .allow_headers([CONTENT_TYPE]),
        );
    }

    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    let port = config.bind_addr.port();
    info!("🚀 Server läuft auf http://{}", config.bind_addr);
    info!("   Webseite: http://localhost:{port}");
    info!("   API:      http://localhost:{port}/api/temperatures");
    println!("Server läuft auf http://{}", config.bind_addr);

    axum::serve(listener, app).await?;
    Ok(())
}

// - dieses kommentar ist das einzige element, welches durch menschlichen einfluss entstanden ist

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_is_trimmed_and_empty_values_are_omitted() {
        assert_eq!(
            normalize_metadata(Some("  sensor-1  ".to_string()), "sensor_id").unwrap(),
            Some("sensor-1".to_string())
        );
        assert_eq!(
            normalize_metadata(Some("  ".to_string()), "location").unwrap(),
            None
        );
    }

    #[test]
    fn metadata_length_is_bounded() {
        let error =
            normalize_metadata(Some("x".repeat(MAX_METADATA_LENGTH + 1)), "sensor_id").unwrap_err();
        assert!(matches!(error, AppError::BadRequest(_)));
    }

    #[test]
    fn list_parameters_are_validated() {
        let params = ListParams {
            limit: Some(MAX_PAGE_LIMIT + 1),
            offset: None,
            sensor_id: None,
            location: None,
            from: None,
            to: None,
        };
        assert!(matches!(
            validate_list_params(&params),
            Err(AppError::BadRequest(_))
        ));

        let params = ListParams {
            limit: None,
            offset: Some(10),
            sensor_id: None,
            location: None,
            from: Some("2026-01-02T00:00:00Z".parse().unwrap()),
            to: Some("2026-01-01T00:00:00Z".parse().unwrap()),
        };
        assert!(matches!(
            validate_list_params(&params),
            Err(AppError::BadRequest(_))
        ));
    }

    #[tokio::test]
    async fn sqlite_round_trip_and_legacy_import_work() {
        let directory = tempfile::tempdir().unwrap();
        let database_file = directory.path().join("temperatures.db");
        let database_url = format!("sqlite://{}", database_file.display());
        let legacy_file = directory.path().join("data.json");
        let entry = TemperatureEntry {
            id: Uuid::new_v4().to_string(),
            temperature: 21.5,
            timestamp: Utc::now(),
            sensor_id: Some("sensor-1".to_string()),
            location: Some("Labor".to_string()),
        };

        tokio::fs::write(&legacy_file, serde_json::to_vec(&vec![entry.clone()]).unwrap())
            .await
            .unwrap();
        let repository = SqliteTemperatureRepository::connect(&database_url)
            .await
            .unwrap();
        assert_eq!(repository.import_legacy_json(&legacy_file).await.unwrap(), 1);
        assert_eq!(repository.import_legacy_json(&legacy_file).await.unwrap(), 0);
        let loaded = repository.list(&ListParams::default()).await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, entry.id);

        let second = TemperatureEntry {
            id: Uuid::new_v4().to_string(),
            temperature: 23.5,
            timestamp: entry.timestamp + chrono::Duration::minutes(1),
            sensor_id: Some("sensor-2".to_string()),
            location: Some("Büro".to_string()),
        };
        repository.create(&second).await.unwrap();
        let filtered = repository
            .list(&ListParams {
                sensor_id: Some("sensor-2".to_string()),
                ..ListParams::default()
            })
            .await
            .unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, second.id);

        let stats = repository.stats().await.unwrap();
        assert_eq!(stats.count, 2);
        assert_eq!(stats.avg, Some(22.5));
        assert_eq!(stats.latest.unwrap().id, second.id);
        assert!(repository.delete(&entry.id).await.unwrap());
        assert!(!repository.delete("missing").await.unwrap());
        repository.delete_all().await.unwrap();
        assert_eq!(repository.count().await.unwrap(), 0);
    }
}
