CREATE TABLE IF NOT EXISTS observa_meta (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);

INSERT OR IGNORE INTO observa_meta (key, value) VALUES ('schema_version', '2');
