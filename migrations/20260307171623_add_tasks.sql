CREATE TABLE IF NOT EXISTS user_tasks (
    task_id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    description TEXT NOT NULL,
    time_spent_seconds INTEGER NOT NULL DEFAULT 0,
    record_session_seconds INTEGER NOT NULL DEFAULT 0,
    completed_at INTEGER
);

CREATE INDEX idx_user_tasks_user_id ON user_tasks(user_id);
