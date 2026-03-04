mod commands;
mod events;
mod state;
mod tasks;

use poise::serenity_prelude as serenity;
use sqlx::sqlite::SqlitePoolOptions;
use std::{collections::HashMap, env, sync::Arc};
use tokio::sync::Mutex;

use crate::state::Data;
use crate::commands::*;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    let token = env::var("DISCORD_TOKEN").expect("Expected DISCORD_TOKEN");

    let db = SqlitePoolOptions::new()
        .connect("sqlite://stats.db?mode=rwc")
        .await
        .expect("Failed to connect to database");

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS voice_stats (
            user_id INTEGER NOT NULL,
            guild_id INTEGER NOT NULL,
            total_seconds INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (user_id, guild_id)
        )"
    )
    .execute(&db)
    .await
    .expect("Failed to create table");

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS guild_settings (
            guild_id INTEGER PRIMARY KEY,
            announcement_channel_id INTEGER NOT NULL,
            weeks_tracked INTEGER NOT NULL DEFAULT 0,
            reset_day INTEGER NOT NULL DEFAULT 0,
            reset_hour INTEGER NOT NULL DEFAULT 0,
            reset_minute INTEGER NOT NULL DEFAULT 0
        )"
    )
    .execute(&db)
    .await
    .expect("Failed to create guild_settings table");

    sqlx::query(
    "CREATE TABLE IF NOT EXISTS tracked_channels (
        channel_id INTEGER PRIMARY KEY,
        guild_id INTEGER NOT NULL
    )"
    )
    .execute(&db)
    .await
    .expect("Failed to create tracked_channels table");

    let active_sessions = Arc::new(Mutex::new(HashMap::new()));
    let intents = serenity::GatewayIntents::non_privileged() | serenity::GatewayIntents::GUILD_VOICE_STATES;

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: vec![
                stats(), 
                leaderboard(), 
                reset_stats(), 
                toggle_tracking(), 
                config_schedule()
            ],
            event_handler: |ctx, event, _framework, data| {
                Box::pin(async move {
                    if let Err(e) = events::handle_event(event, data).await {
                        eprintln!("Error handling event: {:?}", e);
                    }
                    Ok(())
                })
            },
            ..Default::default()
        })
        .setup(|ctx, _ready, framework| {
            Box::pin(async move {
                poise::builtins::register_globally(ctx, &framework.options().commands).await?;

                let http_clone = ctx.http.clone();
                let db_clone = db.clone();
                tokio::spawn(async move {
                    tasks::weekly_reset_task(http_clone, db_clone).await;
                });
                Ok(Data { db, active_sessions })
            })
        })
        .build();

    let mut client = serenity::ClientBuilder::new(token, intents)
        .framework(framework)
        .await
        .unwrap();

    client.start().await.unwrap();
}
