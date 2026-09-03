# Temperatur-Server (Rust / Axum)

Webserver mit Webseite und REST-API zum Speichern von Temperaturdaten. Backend in Rust mit Axum, Frontend mit HTML/CSS/JS + Chart.js.

## Features

- **Webseite** auf `http://localhost:3000` – Dashboard mit Live-Chart, Statistiken, Formular, Tabelle
- **REST-API** zum Ablegen/Abfragen von Temperaturen
- **Eingebaute SQLite-Datenbank** mit automatischen Migrationen
- **Automatische Übernahme** einer vorhandenen `data.json` beim ersten Start
- **Austauschbare Storage-Schicht** als Grundlage für einen späteren PostgreSQL-Adapter
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
DATABASE_URL=sqlite:///var/lib/temperatur-server/temperatures.db cargo run
```

Server läuft auf `http://0.0.0.0:3000` → Webseite unter `http://localhost:3000`


## Container und Podman

Das Image wird bei jedem veröffentlichten GitHub-Release für `linux/amd64` und
`linux/arm64` gebaut und in der GitHub Container Registry veröffentlicht. Dadurch
wird auf dem Zielsystem keine Rust-Toolchain benötigt.

```bash
podman pull ghcr.io/jonasrk15/lf7-bullshit-fahrenheit-server:latest
podman run --rm \
  --name temperatur-server \
  -p 3000:3000 \
  -v temperatur-server-data:/var/lib/temperatur-server \
  ghcr.io/jonasrk15/lf7-bullshit-fahrenheit-server:latest
```

Das Image läuft ohne Root-Rechte als UID/GID `10001`, speichert seine SQLite-Datenbank
unter `/var/lib/temperatur-server/temperatures.db` und prüft `/api/health`
automatisch. Für
reproduzierbare Deployments sollte statt `latest` ein Release-Tag wie `0.1.0`
verwendet werden.

### Podman Quadlet

Eine vorbereitete Quadlet-Datei liegt unter
[`deploy/temperatur-server.container`](deploy/temperatur-server.container). Für
ein rootful Deployment:

```bash
mkdir -p /opt/temperatur-server/data
cp deploy/temperatur-server.container /etc/containers/systemd/
systemctl daemon-reload
systemctl start temperatur-server
```

Der Mount nutzt `:Z,U`: `Z` setzt die private SELinux-Kennzeichnung und `U` passt
den Besitzer des Datenverzeichnisses an die UID des Containers an.

### Neues Image veröffentlichen

1. Einen Git-Tag erstellen, beispielsweise `v0.1.0`.
2. Aus diesem Tag auf GitHub ein Release veröffentlichen.
3. Der Workflow **Container image** veröffentlicht anschließend unter anderem
   die Tags `v0.1.0`, `0.1.0`, `0.1`, `0` und `latest` in GHCR.

Beim ersten veröffentlichten Image muss die Sichtbarkeit des GHCR-Pakets einmalig
in dessen GitHub-Paketeinstellungen auf **Public** gestellt werden, wenn Zielsysteme
es ohne `podman login ghcr.io` abrufen sollen.

### Konfiguration

| Variable | Standard | Beschreibung |
|----------|----------|--------------|
| `BIND_ADDR` | `0.0.0.0:3000` | Socket-Adresse des Servers |
| `DATABASE_URL` | `sqlite://temperatures.db` | SQLite-Verbindungs-URL; die Datenbankdatei wird bei Bedarf angelegt |
| `LEGACY_DATA_FILE` | `data.json` | Alte JSON-Datei, die beim ersten Start einmalig importiert wird |
| `SEED_DEMO` | `false` | Erzeugt beim ersten Start mit leerem Datenspeicher einen Demo-Wert |
| `CORS_ORIGIN` | nicht gesetzt | Erlaubt genau diesen zusätzlichen Browser-Origin, z. B. `https://dashboard.example` |
| `RUST_LOG` | `temperatur_server=debug,tower_http=debug` | Log-Filter |

Beim Start werden aus `migrations/` automatisch noch nicht angewendete
SQL-Migrationen ausgeführt. Ist die SQLite-Datenbank leer und existiert die alte
`data.json`, werden deren Messwerte einmalig in einer Transaktion übernommen. Die
JSON-Datei bleibt dabei als Sicherung unverändert. Ein Marker in der Datenbank
verhindert einen erneuten Import nach einem späteren Löschen der Messwerte.

### Später PostgreSQL verwenden

Die HTTP-Handler greifen nur auf das Trait `TemperatureRepository` zu. SQLite ist
in `src/storage/sqlite.rs` implementiert. Für PostgreSQL kann daher ein zweiter
Adapter (zum Beispiel `src/storage/postgres.rs`) ergänzt und anhand des Schemas von
`DATABASE_URL` ausgewählt werden, ohne REST-Routen, Validierung oder Frontend neu
zu schreiben. Da SQLite und PostgreSQL unterschiedliche SQL-Dialekte und
Migrationstabellen verwenden, benötigt der PostgreSQL-Adapter eigene Migrationen.

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
├── migrations/        # automatisch ausgeführte SQLite-Migrationen
├── src/main.rs        # Axum Server, API und Validierung
├── src/storage/       # austauschbare Datenhaltung + SQLite-Adapter
├── static/index.html  # Frontend (wird via include_str! eingebettet)
├── temperatures.db    # SQLite-Persistenz (wird auto-erzeugt)
└── target/            # Build-Artefakte
```

## Erweitern

- PostgreSQL-Adapter für die vorhandene `TemperatureRepository`-Schnittstelle
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
