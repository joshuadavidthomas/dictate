-- Create transcriptions table
CREATE TABLE IF NOT EXISTS transcriptions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    text TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    duration_ms INTEGER,
    model_id TEXT,
    audio_path TEXT,
    output_mode TEXT,
    audio_size_bytes INTEGER
);

-- Create index on created_at for faster queries
CREATE INDEX IF NOT EXISTS idx_transcriptions_created_at
ON transcriptions(created_at DESC);
