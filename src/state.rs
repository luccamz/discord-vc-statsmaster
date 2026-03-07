use sqlx::SqlitePool;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

pub struct SessionData {
    pub start_time: i64,
    pub active_task_id: Option<i64>,
}

pub struct Data {
    pub db: SqlitePool,
    pub active_sessions: Arc<Mutex<HashMap<(u64, u64), SessionData>>>,
}

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Context<'a> = poise::Context<'a, Data, Error>;
