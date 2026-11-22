use poise::serenity_prelude::ChannelId;
use sqlx::SqlitePool;
use std::{collections::HashMap, sync::Arc};
use tokio::{sync::Mutex, task::JoinHandle};

pub struct SessionData {
    pub start_time: i64,
    pub active_task_id: Option<i64>,
}

pub struct Pomodoro {
    pub id: u64,
    pub allowed: [Option<ChannelId>; 2],
    pub task: JoinHandle<()>,
}

pub type Pomodoros = Arc<std::sync::Mutex<HashMap<(u64, u64), Pomodoro>>>;

pub struct Data {
    pub db: SqlitePool,
    pub active_sessions: Arc<Mutex<HashMap<(u64, u64), SessionData>>>,
    pub pomodoros: Pomodoros,
}

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Context<'a> = poise::Context<'a, Data, Error>;
