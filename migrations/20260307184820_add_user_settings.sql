CREATE TABLE IF NOT EXISTS user_settings (
    user_id INTEGER PRIMARY KEY,
    timezone_offset INTEGER NOT NULL DEFAULT 0
);
