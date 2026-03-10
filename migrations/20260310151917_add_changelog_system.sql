CREATE TABLE IF NOT EXISTS bot_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    last_version TEXT NOT NULL
);

ALTER TABLE guild_settings ADD COLUMN changelog_channel_id INTEGER;
