mod commands;
mod events;
mod state;
mod tasks;

use poise::serenity_prelude as serenity;
use sqlx::sqlite::SqlitePoolOptions;
use std::{collections::HashMap, env, sync::Arc};
use tokio::sync::Mutex;

use crate::commands::*;
use crate::state::{Data, SessionData};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    let token = env::var("DISCORD_TOKEN").expect("Expected DISCORD_TOKEN");

    let db = SqlitePoolOptions::new()
        .connect("sqlite://stats.db?mode=rwc")
        .await
        .expect("Failed to connect to database");

    sqlx::migrate!()
        .run(&db)
        .await
        .expect("Failed to run database migrations");

    let active_sessions: Arc<Mutex<HashMap<(u64, u64), SessionData>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let intents =
        serenity::GatewayIntents::non_privileged() | serenity::GatewayIntents::GUILD_VOICE_STATES;

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: vec![
                stats(),
                leaderboard(),
                reset_stats(),
                toggle_tracking(),
                config_schedule(),
                add_task(),
                todo(),
                delete_tasks(),
                config_timezone(),
            ],
            event_handler: |ctx, event, _framework, data| {
                Box::pin(async move {
                    if let Err(e) = events::handle_event(ctx, event, data).await {
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
                let db_clone_2 = db.clone();

                tokio::spawn(async move {
                    tasks::weekly_reset_task(http_clone, db_clone).await;
                });

                tokio::spawn(async move {
                    tasks::deadline_check_task(db_clone_2).await;
                });

                Ok(Data {
                    db,
                    active_sessions,
                })
            })
        })
        .build();

    let mut client = serenity::ClientBuilder::new(token, intents)
        .framework(framework)
        .await
        .unwrap();

    client.start().await.unwrap();
}
