# Temperatur-Server (Rust / Axum)

Webserver mit Webseite und REST-API zum Speichern von Temperaturdaten. Backend in Rust mit Axum, Frontend mit HTML/CSS/JS + Chart.js.

## Features

- **Webseite** auf `http://localhost:3000` – Dashboard mit Live-Chart, Statistiken, Formular, Tabelle
- **REST-API** zum Ablegen/Abfragen von Temperaturen
- **Persistenz** in `data.json` (wird beim Start geladen, bei jedem Write gespeichert)
- **Validierung** (`-100..100 °C`, finite Zahlen)
- **Filter** nach `sensor_id`, `location`, Zeitraum, Paginierung

## Schnellstart

```bash
cargo run
# oder mit log-level:
RUST_LOG=debug cargo run
# optional auf einer anderen Adresse:
BIND_ADDR=127.0.0.1:3001 cargo run
```

Server läuft auf `http://0.0.0.0:3000` → Webseite unter `http://localhost:3000`

## API

| Methode | Pfad | Beschreibung |
|---------|------|--------------|
| `GET` | `/` | Webseite |
| `GET` | `/api/health` | Status + Anzahl |
| `GET` | `/api/temperatures?limit=50&offset=0&sensor_id=x&location=y&from=2024-01-01T00:00:00Z&to=2024-12-31T23:59:59Z` | Liste (neueste zuerst) |
| `GET` | `/api/temperatures/latest` | Neueste Messung |
| `GET` | `/api/temperatures/:id` | Einzelne Messung |
| `POST` | `/api/temperatures` | Neue Messung |
| `DELETE` | `/api/temperatures/:id` | Löschen |
| `DELETE` | `/api/temperatures` | Alle löschen |
| `GET` | `/api/stats` | `count`, `avg`, `min`, `max`, `latest` |

### POST Body

```json
{
  "temperature": 21.5,
  "sensor_id": "sensor-1",      // optional
  "location": "Wohnzimmer",     // optional
  "timestamp": "2024-01-01T12:00:00Z" // optional, default: now()
}
```

### Beispiele

```bash
# Anlegen
curl -X POST http://localhost:3000/api/temperatures \
  -H "Content-Type: application/json" \
  -d '{"temperature":23.4,"sensor_id":"s1","location":"Büro"}'

# Liste
curl http://localhost:3000/api/temperatures | jq

# Gefiltert
curl "http://localhost:3000/api/temperatures?sensor_id=s1&limit=10" | jq

# Letzte
curl http://localhost:3000/api/temperatures/latest | jq

# Stats
curl http://localhost:3000/api/stats | jq

# Löschen
curl -X DELETE http://localhost:3000/api/temperatures/<id>
```

## Projektstruktur

```
.
├── Cargo.toml
├── src/main.rs        # Axum Server + API + State + Persistence
├── static/index.html  # Frontend (wird via include_str! eingebettet)
├── data.json          # Persistenz (wird auto-erzeugt)
└── target/            # Build-Artefakte
```

## Erweitern

- Datenbank statt `data.json`: `sqlx` mit SQLite/Postgres
- Auth: `tower-http` + JWT
- HTTPS: hinter Reverse-Proxy (nginx/caddy) oder `axum-server` mit TLS

## Build Release

```bash
cargo build --release
./target/release/temperatur-server
```
