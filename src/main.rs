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
        let client = reqwest::Client::new();
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

    let db = SqlitePoolOptions::new()
        .connect(&db_url)
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
                cancel_session(),
                reset_stats(),
                toggle_tracking(),
                config_schedule(),
                add_task(),
                todo(),
                delete_tasks(),
                config_timezone(),
                config_changelogs(),
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
