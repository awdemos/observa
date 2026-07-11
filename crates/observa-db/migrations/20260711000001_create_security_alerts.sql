-- Immutable, append-only table for security alerts.
-- Each row is hashed together with the previous row's hash so that tampering
-- with any alert invalidates the chain.
CREATE TABLE IF NOT EXISTS security_alerts (
    id TEXT PRIMARY KEY,
    ts TEXT NOT NULL,
    source TEXT NOT NULL,
    unit TEXT,
    severity TEXT NOT NULL,
    message TEXT NOT NULL,
    raw TEXT,
    previous_hash TEXT,
    hash TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_security_alerts_ts ON security_alerts(ts DESC);
CREATE INDEX IF NOT EXISTS idx_security_alerts_severity ON security_alerts(severity);
