mod commands;
mod events;
mod pomodoro;
mod prompt;
mod sessions;
mod state;
mod tasks;

use poise::serenity_prelude as serenity;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use std::{collections::HashMap, env, str::FromStr, sync::Arc};
use tokio::sync::Mutex;

use crate::commands::*;
use crate::state::{Data, SessionData};

async fn broadcast_changelogs(
    ctx: &serenity::Context,
    db: &sqlx::SqlitePool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let current_version = env!("CARGO_PKG_VERSION");

    let state = sqlx::query!("SELECT last_version FROM bot_state WHERE id = 1")
        .fetch_optional(db)
        .await?;

    let should_broadcast = match state {
        Some(row) => row.last_version != current_version,
        None => {
            sqlx::query!(
                "INSERT INTO bot_state (id, last_version) VALUES (1, ?)",
                current_version
            )
            .execute(db)
            .await?;
            false
        }
    };

    if should_broadcast {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()?;
        let url = format!(
            "https://api.github.com/repos/luccamz/discord-vc-statsmaster/releases/tags/v{}",
            current_version
        );

        let res = client
            .get(&url)
            .header("User-Agent", "discord-vc-statsmaster")
            .send()
            .await;

        let briefing = match res {
            Ok(response) if response.status().is_success() => {
                let json: serde_json::Value = response.json().await.unwrap_or_default();
                let body = json["body"].as_str().unwrap_or("");

                let mut snippet = body.lines().take(3).collect::<Vec<&str>>().join("\n");

                if snippet.is_empty() {
                    "General stability updates and background improvements.".to_string()
                } else {
                    if snippet.len() > 300 {
                        snippet.truncate(300);
                        snippet.push_str("...");
                    }
                    snippet
                }
            }
            _ => "General stability updates and background improvements.".to_string(),
        };

        let changelog_text = format!(
            "**v{} is now live!**\n\n{}\n\n**Full release notes:** https://github.com/luccamz/discord-vc-statsmaster/releases/tag/v{}",
            current_version, briefing, current_version
        );

        let settings = sqlx::query!(
            "SELECT changelog_channel_id FROM guild_settings WHERE changelog_channel_id IS NOT NULL"
        )
        .fetch_all(db)
        .await?;

        for setting in settings {
            if let Some(channel_id) = setting.changelog_channel_id {
                let channel = serenity::ChannelId::new(channel_id as u64);
                let _ = channel
                    .send_message(
                        &ctx.http,
                        serenity::CreateMessage::new().content(&changelog_text),
                    )
                    .await;
            }
        }

        sqlx::query!(
            "UPDATE bot_state SET last_version = ? WHERE id = 1",
            current_version
        )
        .execute(db)
        .await?;
    }

    Ok(())
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    let token = env::var("DISCORD_TOKEN").expect("Expected DISCORD_TOKEN");

    let db_url = env::var("DATABASE_URL").unwrap_or("sqlite://stats.db?mode=rwc".to_string());

    let db_options = SqliteConnectOptions::from_str(&db_url)
        .expect("Invalid DATABASE_URL")
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(std::time::Duration::from_secs(5));

    let db = SqlitePoolOptions::new()
        .connect_with(db_options)
        .await
        .expect("Failed to connect to database");

    sqlx::migrate!()
        .run(&db)
        .await
        .expect("Failed to run database migrations");

    let active_sessions: Arc<Mutex<HashMap<(u64, u64), SessionData>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let pomodoros: state::Pomodoros = Default::default();
    let prompts: state::Prompts = Default::default();
    let shutdown_sessions = active_sessions.clone();
    let shutdown_pomodoros = pomodoros.clone();
    let shutdown_prompts = prompts.clone();
    let shutdown_db = db.clone();
    let intents =
        serenity::GatewayIntents::non_privileged() | serenity::GatewayIntents::GUILD_VOICE_STATES;

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: vec![
                stats(),
                leaderboard(),
                cancel_session(),
                reset_stats(),
                toggle_tracking(),
                config_schedule(),
                add_task(),
                edit_deadline(),
                todo(),
                delete_tasks(),
                config_timezone(),
                config_changelogs(),
                config_pomodoro_lobby(),
                pomodoro(),
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

                if let Err(e) = broadcast_changelogs(ctx, &db).await {
                    eprintln!("Failed to broadcast changelogs: {}", e);
                }

                let http_clone = ctx.http.clone();
                let db_clone = db.clone();
                let db_clone_2 = db.clone();
                let sessions_clone = active_sessions.clone();

                tokio::spawn(async move {
                    tasks::weekly_reset_task(http_clone, db_clone, sessions_clone).await;
                });

                tokio::spawn(async move {
                    tasks::deadline_check_task(db_clone_2).await;
                });

                Ok(Data {
                    db,
                    active_sessions,
                    pomodoros,
                    prompts,
                })
            })
        })
        .build();

    let mut client = serenity::ClientBuilder::new(token, intents)
        .framework(framework)
        .await
        .unwrap();

    tokio::spawn(sessions::flush_on_shutdown(
        shutdown_sessions,
        shutdown_db.clone(),
        client.shard_manager.clone(),
        client.http.clone(),
        shutdown_pomodoros,
        shutdown_prompts,
    ));

    client.start().await.unwrap();
    shutdown_db.close().await;
}
