CREATE TABLE temperatures (
    id          TEXT PRIMARY KEY NOT NULL,
    temperature REAL NOT NULL CHECK (temperature BETWEEN -100.0 AND 100.0),
    timestamp   TEXT NOT NULL,
    sensor_id   TEXT,
    location    TEXT
);

CREATE INDEX idx_temperatures_timestamp
    ON temperatures (timestamp DESC);

CREATE INDEX idx_temperatures_sensor_timestamp
    ON temperatures (sensor_id, timestamp DESC);

CREATE INDEX idx_temperatures_location_timestamp
    ON temperatures (location, timestamp DESC);

CREATE TABLE application_metadata (
    key   TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);
