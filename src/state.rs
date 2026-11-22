use poise::serenity_prelude::{ChannelId, MessageId};
use sqlx::SqlitePool;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{Mutex, Notify};

#[derive(Clone, Copy)]
pub struct SessionData {
    pub start_time: i64,
    pub active_task_id: Option<i64>,
    pub channel_id: u64,
    /// False after `/cancel_session`: the time is dropped until the user changes channel.
    pub counted: bool,
}

pub struct Pomodoro {
    pub id: u64,
    pub allowed: [Option<ChannelId>; 2],
    pub status_msg: Option<(ChannelId, MessageId)>,
    pub cancel: Arc<Notify>,
}

pub type Pomodoros = Arc<std::sync::Mutex<HashMap<(u64, u64), Pomodoro>>>;

pub struct Data {
    pub db: SqlitePool,
    pub active_sessions: Arc<Mutex<HashMap<(u64, u64), SessionData>>>,
    pub pomodoros: Pomodoros,
}

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Context<'a> = poise::Context<'a, Data, Error>;
