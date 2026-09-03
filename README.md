# Temperatur-Server (Rust / Axum)

Webserver mit Webseite und REST-API zum Speichern von Temperaturdaten. Backend in Rust mit Axum, Frontend mit HTML/CSS/JS + Chart.js.

## Features

- **Webseite** auf `http://localhost:3000` – Dashboard mit Live-Chart, Statistiken, Formular, Tabelle
- **REST-API** zum Ablegen/Abfragen von Temperaturen
- **Atomare Persistenz** in `data.json` (Pfad konfigurierbar)
- **Validierung** (`-100..100 °C`, begrenzte Metadaten und Request-Größe)
- **Filter** nach `sensor_id`, `location`, Zeitraum, begrenzte Paginierung
- **Sichere Voreinstellung** ohne Cross-Origin-Zugriff oder Demo-Daten
- **Automatisierte Checks** für Formatierung, Clippy und Tests

## Schnellstart

```bash
cargo run
# oder mit log-level:
RUST_LOG=debug cargo run
# optional auf einer anderen Adresse:
BIND_ADDR=127.0.0.1:3001 cargo run
# optional mit anderem Speicherort:
DATA_FILE=/var/lib/temperatur-server/data.json cargo run
```

Server läuft auf `http://0.0.0.0:3000` → Webseite unter `http://localhost:3000`

### Konfiguration

| Variable | Standard | Beschreibung |
|----------|----------|--------------|
| `BIND_ADDR` | `0.0.0.0:3000` | Socket-Adresse des Servers |
| `DATA_FILE` | `data.json` | Pfad zur JSON-Datei; fehlende Verzeichnisse werden angelegt |
| `SEED_DEMO` | `false` | Erzeugt beim ersten Start mit leerem Datenspeicher einen Demo-Wert |
| `CORS_ORIGIN` | nicht gesetzt | Erlaubt genau diesen zusätzlichen Browser-Origin, z. B. `https://dashboard.example` |
| `RUST_LOG` | `temperatur_server=debug,tower_http=debug` | Log-Filter |

Eine vorhandene, aber ungültige Datendatei beendet den Start mit einer Fehlermeldung. Dadurch wird eine beschädigte Datei nicht unbemerkt mit neuen Daten überschrieben.

## API

| Methode | Pfad | Beschreibung |
|---------|------|--------------|
| `GET` | `/` | Webseite |
| `GET` | `/api/health` | Status + Anzahl |
| `GET` | `/api/temperatures?limit=50&offset=0&sensor_id=x&location=y&from=2024-01-01T00:00:00Z&to=2024-12-31T23:59:59Z` | Liste (neueste zuerst, maximal 1000 pro Request) |
| `GET` | `/api/temperatures/latest` | Neueste Messung |
| `GET` | `/api/temperatures/:id` | Einzelne Messung |
| `POST` | `/api/temperatures` | Neue Messung |
| `DELETE` | `/api/temperatures/:id` | Löschen |
| `POST` | `/api/temperatures/:id/delete` | Löschen (Browser-Kompatibilitätsroute) |
| `DELETE` | `/api/temperatures` | Alle löschen |
| `POST` | `/api/temperatures/clear` | Alle löschen |
| `GET` | `/api/stats` | `count`, `avg`, `min`, `max`, `latest` |

### POST Body

```json
{
  "temperature": 21.5,
  "sensor_id": "sensor-1",
  "location": "Wohnzimmer",
  "timestamp": "2024-01-01T12:00:00Z"
}
```

`sensor_id`, `location` und `timestamp` sind optional. Ohne `timestamp` verwendet der Server die aktuelle UTC-Zeit. `sensor_id` und `location` sind auf jeweils 200 Zeichen begrenzt.

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
curl -X POST http://localhost:3000/api/temperatures/<id>/delete
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

## Qualität prüfen

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```
