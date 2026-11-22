use poise::serenity_prelude::{ChannelId, MessageId};
use sqlx::SqlitePool;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

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
    pub control: tokio::sync::mpsc::UnboundedSender<crate::pomodoro::Control>,
    /// Stopped from the lobby and being moved back to the work channel.
    pub returning: bool,
}

pub type Pomodoros = Arc<std::sync::Mutex<HashMap<(u64, u64), Pomodoro>>>;

/// Each user's task prompt still on screen, keyed like sessions, so it can be deleted later.
pub type Prompts = Arc<std::sync::Mutex<HashMap<(u64, u64), (ChannelId, MessageId)>>>;

pub struct Data {
    pub db: SqlitePool,
    pub active_sessions: Arc<Mutex<HashMap<(u64, u64), SessionData>>>,
    pub pomodoros: Pomodoros,
    pub prompts: Prompts,
}

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Context<'a> = poise::Context<'a, Data, Error>;
