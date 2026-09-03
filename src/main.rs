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
    path::{Path as FilePath, PathBuf},
    sync::Arc,
};
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, RwLock};
use tower_http::cors::CorsLayer;
use tracing::{error, info};
use uuid::Uuid;

const DEFAULT_BIND_ADDR: &str = "0.0.0.0:3000";
const DEFAULT_DATA_FILE: &str = "data.json";
const DEFAULT_PAGE_LIMIT: usize = 100;
const MAX_PAGE_LIMIT: usize = 1_000;
const MAX_METADATA_LENGTH: usize = 200;
const MAX_REQUEST_BODY_SIZE: usize = 16 * 1024;

// --- Datenmodelle ---

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TemperatureEntry {
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

#[derive(Debug, Deserialize)]
struct ListParams {
    limit: Option<usize>,
    offset: Option<usize>,
    sensor_id: Option<String>,
    location: Option<String>,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
struct Stats {
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
    entries: RwLock<Vec<TemperatureEntry>>,
    persistence: Mutex<()>,
    data_file: PathBuf,
}

type SharedState = Arc<AppState>;

struct Config {
    bind_addr: SocketAddr,
    data_file: PathBuf,
    seed_demo: bool,
    cors_origin: Option<HeaderValue>,
}

impl Config {
    fn from_env() -> Result<Self, String> {
        let bind_addr = std::env::var("BIND_ADDR")
            .unwrap_or_else(|_| DEFAULT_BIND_ADDR.to_string())
            .parse()
            .map_err(|error| format!("Ungültige BIND_ADDR: {error}"))?;
        let data_file = std::env::var_os("DATA_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_DATA_FILE));
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
            data_file,
            seed_demo,
            cors_origin,
        })
    }
}

// --- Persistence ---

async fn load_data(state: &SharedState) -> Result<(), AppError> {
    match tokio::fs::read_to_string(&state.data_file).await {
        Ok(content) => match serde_json::from_str::<Vec<TemperatureEntry>>(&content) {
            Ok(data) => {
                let count = data.len();
                *state.entries.write().await = data;
                info!(
                    "{} Einträge aus {} geladen",
                    count,
                    state.data_file.display()
                );
                Ok(())
            }
            Err(error) => Err(AppError::Internal(format!(
                "Fehler beim Parsen von {}: {error}",
                state.data_file.display()
            ))),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            info!(
                "Keine bestehende {} gefunden, starte leer",
                state.data_file.display()
            );
            Ok(())
        }
        Err(error) => Err(AppError::Internal(format!(
            "Fehler beim Laden von {}: {error}",
            state.data_file.display()
        ))),
    }
}

fn temp_file_path(data_file: &FilePath) -> PathBuf {
    let file_name = data_file.file_name().unwrap_or_default().to_string_lossy();
    data_file.with_file_name(format!("{file_name}.tmp"))
}

async fn save_data(data_file: &FilePath, data: &[TemperatureEntry]) -> Result<(), AppError> {
    let json = serde_json::to_string_pretty(data)
        .map_err(|e| AppError::Internal(format!("Fehler beim Serialisieren: {e}")))?;
    if let Some(parent) = data_file
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent).await.map_err(|error| {
            AppError::Internal(format!(
                "Datenverzeichnis {} konnte nicht erstellt werden: {error}",
                parent.display()
            ))
        })?;
    }

    let temp_file = temp_file_path(data_file);
    let mut file = tokio::fs::File::create(&temp_file).await.map_err(|error| {
        AppError::Internal(format!(
            "Fehler beim Öffnen von {}: {error}",
            temp_file.display()
        ))
    })?;
    file.write_all(json.as_bytes()).await.map_err(|error| {
        AppError::Internal(format!(
            "Fehler beim Speichern nach {}: {error}",
            temp_file.display()
        ))
    })?;
    file.sync_all().await.map_err(|error| {
        AppError::Internal(format!(
            "Fehler beim Synchronisieren von {}: {error}",
            temp_file.display()
        ))
    })?;
    drop(file);
    tokio::fs::rename(&temp_file, data_file)
        .await
        .map_err(|error| {
            AppError::Internal(format!(
                "Fehler beim Ersetzen von {}: {error}",
                data_file.display()
            ))
        })
}

// --- API Handler ---

async fn health_handler(State(state): State<SharedState>) -> Json<Health> {
    let count = state.entries.read().await.len();
    Json(Health {
        status: "ok".to_string(),
        count,
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

async fn list_temperatures(
    State(state): State<SharedState>,
    Query(params): Query<ListParams>,
) -> Result<Json<Vec<TemperatureEntry>>, AppError> {
    let (offset, limit) = validate_list_params(&params)?;
    let data = state.entries.read().await;
    let mut filtered: Vec<TemperatureEntry> = data
        .iter()
        .filter(|e| {
            if let Some(ref sid) = params.sensor_id {
                if e.sensor_id.as_deref() != Some(sid) {
                    return false;
                }
            }
            if let Some(ref loc) = params.location {
                if e.location.as_deref() != Some(loc) {
                    return false;
                }
            }
            if let Some(ref from) = params.from {
                if e.timestamp < *from {
                    return false;
                }
            }
            if let Some(ref to) = params.to {
                if e.timestamp > *to {
                    return false;
                }
            }
            true
        })
        .cloned()
        .collect();

    // Neueste zuerst sortieren (timestamp absteigend)
    filtered.sort_by_key(|entry| std::cmp::Reverse(entry.timestamp));

    let result: Vec<TemperatureEntry> = filtered.into_iter().skip(offset).take(limit).collect();
    Ok(Json(result))
}

fn validate_list_params(params: &ListParams) -> Result<(usize, usize), AppError> {
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
    let data = state.entries.read().await;
    let latest = data.iter().max_by_key(|e| e.timestamp).cloned();
    match latest {
        Some(entry) => Ok(Json(entry)),
        None => Err(AppError::NotFound("Keine Daten vorhanden".into())),
    }
}

async fn get_by_id(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<TemperatureEntry>, AppError> {
    let data = state.entries.read().await;
    data.iter()
        .find(|e| e.id == id)
        .cloned()
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

    let _persistence = state.persistence.lock().await;
    let mut data = state.entries.write().await;
    data.push(entry.clone());
    let snapshot = data.clone();
    drop(data);

    if let Err(error) = save_data(&state.data_file, &snapshot).await {
        state
            .entries
            .write()
            .await
            .retain(|item| item.id != entry.id);
        return Err(error);
    }

    Ok((StatusCode::CREATED, Json(entry)))
}

async fn delete_by_id(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    let _persistence = state.persistence.lock().await;
    let mut data = state.entries.write().await;
    let index = data
        .iter()
        .position(|entry| entry.id == id)
        .ok_or_else(|| AppError::NotFound(format!("ID {id} nicht gefunden")))?;
    let removed = data.remove(index);
    let snapshot = data.clone();
    drop(data);

    if let Err(error) = save_data(&state.data_file, &snapshot).await {
        state.entries.write().await.insert(index, removed);
        return Err(error);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_all(State(state): State<SharedState>) -> Result<StatusCode, AppError> {
    let _persistence = state.persistence.lock().await;
    let mut data = state.entries.write().await;
    if data.is_empty() {
        return Ok(StatusCode::NO_CONTENT);
    }

    let previous = std::mem::take(&mut *data);
    drop(data);

    if let Err(error) = save_data(&state.data_file, &[]).await {
        *state.entries.write().await = previous;
        return Err(error);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn get_stats(State(state): State<SharedState>) -> Json<Stats> {
    let data = state.entries.read().await;
    if data.is_empty() {
        return Json(Stats {
            count: 0,
            avg: None,
            min: None,
            max: None,
            latest: None,
        });
    }
    let count = data.len();
    let sum: f64 = data.iter().map(|e| e.temperature).sum();
    let avg = sum / count as f64;
    let min = data
        .iter()
        .map(|e| e.temperature)
        .fold(f64::INFINITY, f64::min);
    let max = data
        .iter()
        .map(|e| e.temperature)
        .fold(f64::NEG_INFINITY, f64::max);
    let latest = data.iter().max_by_key(|e| e.timestamp).cloned();

    Json(Stats {
        count,
        avg: Some(avg),
        min: Some(min),
        max: Some(max),
        latest,
    })
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
    let state: SharedState = Arc::new(AppState {
        entries: RwLock::new(Vec::new()),
        persistence: Mutex::new(()),
        data_file: config.data_file.clone(),
    });
    load_data(&state).await?;

    if config.seed_demo && state.entries.read().await.is_empty() {
        let mut data = state.entries.write().await;
        data.push(TemperatureEntry {
            id: Uuid::new_v4().to_string(),
            temperature: 21.5,
            timestamp: Utc::now(),
            sensor_id: Some("demo-sensor-1".into()),
            location: Some("Wohnzimmer".into()),
        });
        let snapshot = data.clone();
        drop(data);
        save_data(&state.data_file, &snapshot).await?;
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
    info!("🚀 Server läuft auf http://{}", config.bind_addr);
    info!("   Webseite: http://localhost:3000");
    info!("   API:      http://localhost:3000/api/temperatures");
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
    async fn persistence_round_trip_uses_configured_path() {
        let directory = std::env::temp_dir().join(Uuid::new_v4().to_string());
        let data_file = directory.join("nested/temperatures.json");
        let entry = TemperatureEntry {
            id: Uuid::new_v4().to_string(),
            temperature: 21.5,
            timestamp: Utc::now(),
            sensor_id: Some("sensor-1".to_string()),
            location: Some("Labor".to_string()),
        };

        save_data(&data_file, std::slice::from_ref(&entry))
            .await
            .unwrap();
        let state = Arc::new(AppState {
            entries: RwLock::new(Vec::new()),
            persistence: Mutex::new(()),
            data_file,
        });
        load_data(&state).await.unwrap();

        let loaded = state.entries.read().await;
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, entry.id);
        drop(loaded);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
