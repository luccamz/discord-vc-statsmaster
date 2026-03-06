CREATE TABLE IF NOT EXISTS voice_stats (
    user_id INTEGER NOT NULL,
    guild_id INTEGER NOT NULL,
    total_seconds INTEGER NOT NULL DEFAULT 0,
    personal_record INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (user_id, guild_id)
);

CREATE TABLE IF NOT EXISTS guild_settings (
    guild_id INTEGER PRIMARY KEY,
    announcement_channel_id INTEGER NOT NULL,
    weeks_tracked INTEGER NOT NULL DEFAULT 0,
    reset_day INTEGER NOT NULL DEFAULT 0,
    reset_hour INTEGER NOT NULL DEFAULT 0,
    reset_minute INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS tracked_channels (
    channel_id INTEGER PRIMARY KEY,
    guild_id INTEGER NOT NULL
);
