-- Add migration script here
CREATE TABLE IF NOT EXISTS words (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    text TEXT NOT NULL,
    count INTEGER DEFAULT 1,
    frequency INTEGER,

    reviewed INT DEFAULT 0,
    next_review_at TEXT,

    review_duration INTEGER DEFAULT 0,
    e_factor REAL DEFAULT 0,
    repitition INTEGER DEFAULT 0,

    UNIQUE(text)
);