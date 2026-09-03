use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Json},
    routing::get,
    Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tower_http::cors::CorsLayer;
use tracing::info;
use uuid::Uuid;

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
}

type SharedState = Arc<AppState>;

const DATA_FILE: &str = "data.json";
const DATA_TEMP_FILE: &str = "data.json.tmp";

// --- Persistence ---

async fn load_data(state: &SharedState) -> bool {
    match tokio::fs::read_to_string(DATA_FILE).await {
        Ok(content) => match serde_json::from_str::<Vec<TemperatureEntry>>(&content) {
            Ok(data) => {
                let count = data.len();
                *state.entries.write().await = data;
                info!("{} Einträge aus {DATA_FILE} geladen", count);
                true
            }
            Err(e) => {
                eprintln!("Fehler beim Parsen von {DATA_FILE}: {e}");
                false
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            info!("Keine bestehende {DATA_FILE} gefunden, starte leer");
            true
        }
        Err(e) => {
            eprintln!("Fehler beim Laden von {DATA_FILE}: {e}");
            false
        }
    }
}

async fn save_data(data: &[TemperatureEntry]) -> Result<(), AppError> {
    let json = serde_json::to_string_pretty(data)
        .map_err(|e| AppError::Internal(format!("Fehler beim Serialisieren: {e}")))?;
    tokio::fs::write(DATA_TEMP_FILE, json)
        .await
        .map_err(|e| {
            AppError::Internal(format!("Fehler beim Speichern nach {DATA_TEMP_FILE}: {e}"))
        })?;
    tokio::fs::rename(DATA_TEMP_FILE, DATA_FILE)
        .await
        .map_err(|e| AppError::Internal(format!("Fehler beim Ersetzen von {DATA_FILE}: {e}")))
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
) -> Json<Vec<TemperatureEntry>> {
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
            if let Some(from) = params.from {
                if e.timestamp < from {
                    return false;
                }
            }
            if let Some(to) = params.to {
                if e.timestamp > to {
                    return false;
                }
            }
            true
        })
        .cloned()
        .collect();

    // Neueste zuerst sortieren (timestamp absteigend)
    filtered.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

    let offset = params.offset.unwrap_or(0);
    let limit = params.limit.unwrap_or(100);

    let result: Vec<TemperatureEntry> = filtered.into_iter().skip(offset).take(limit).collect();
    Json(result)
}

async fn get_latest(
    State(state): State<SharedState>,
) -> Result<Json<TemperatureEntry>, AppError> {
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
        sensor_id: payload.sensor_id.filter(|s| !s.trim().is_empty()),
        location: payload.location.filter(|s| !s.trim().is_empty()),
    };

    let _persistence = state.persistence.lock().await;
    let mut data = state.entries.write().await;
    data.push(entry.clone());
    let snapshot = data.clone();
    drop(data);

    if let Err(error) = save_data(&snapshot).await {
        state.entries.write().await.retain(|item| item.id != entry.id);
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

    if let Err(error) = save_data(&snapshot).await {
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

    if let Err(error) = save_data(&[]).await {
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
    let min = data.iter().map(|e| e.temperature).fold(f64::INFINITY, f64::min);
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

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let (status, msg) = match self {
            AppError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            AppError::NotFound(m) => (StatusCode::NOT_FOUND, m),
            AppError::Internal(m) => {
                eprintln!("{m}");
                (StatusCode::INTERNAL_SERVER_ERROR, "Interner Serverfehler".into())
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
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "temperatur_server=debug,tower_http=debug".into()),
        )
        .init();

    let state: SharedState = Arc::new(AppState {
        entries: RwLock::new(Vec::new()),
        persistence: Mutex::new(()),
    });
    let can_seed_data = load_data(&state).await;

    // Beispiel-Daten wenn leer (optional, zum Testen)
    if can_seed_data && state.entries.read().await.is_empty() {
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
        if let Err(error) = save_data(&snapshot).await {
            eprintln!("Initiale Beispieldaten konnten nicht gespeichert werden: {error:?}");
        }
    }

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/api/health", get(health_handler))
        .route("/api/temperatures", get(list_temperatures).post(create_temperature).delete(delete_all))
        .route("/api/temperatures/latest", get(get_latest))
        .route("/api/temperatures/:id", get(get_by_id).delete(delete_by_id))
        .route("/api/stats", get(get_stats))
        .with_state(state)
        .layer(CorsLayer::permissive())
        .layer(tower_http::trace::TraceLayer::new_for_http());

    let addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".into());
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    info!("🚀 Server läuft auf http://{}", addr);
    info!("   Webseite: http://localhost:3000");
    info!("   API:      http://localhost:3000/api/temperatures");
    println!("Server läuft auf http://{addr}");

    axum::serve(listener, app).await.unwrap();
}

// - dieses kommentar ist das einzige element, welches durch menschlichen einfluss entstanden ist
