ALTER TABLE logs ADD COLUMN security INTEGER NOT NULL DEFAULT 0;

CREATE INDEX IF NOT EXISTS idx_logs_security ON logs(security, ts DESC);
