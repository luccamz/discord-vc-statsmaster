use poise::serenity_prelude as serenity;
use sqlx::{sqlite::SqlitePoolOptions, Row, SqlitePool};
use std::{collections::HashMap, env, sync::Arc};
use tokio::sync::Mutex;

pub struct Data {
    db: SqlitePool,
    active_sessions: Arc<Mutex<HashMap<(u64, u64), i64>>>,
}

type Error = Box<dyn std::error::Error + Send + Sync>;
type Context<'a> = poise::Context<'a, Data, Error>;

/// Displays your total accumulated voice time in this server
#[poise::command(slash_command, guild_only)]
async fn stats(ctx: Context<'_>) -> Result<(), Error> {
    let user_id = ctx.author().id.get() as i64;
    let guild_id = ctx.guild_id().unwrap().get() as i64;

    // Use runtime query and bind variables
    let record = sqlx::query("SELECT total_seconds FROM voice_stats WHERE user_id = ? AND guild_id = ?")
        .bind(user_id)
        .bind(guild_id)
        .fetch_optional(&ctx.data().db)
        .await?;

    match record {
        Some(row) => {
            // Extract the data manually using the Row trait
            let total_seconds: i64 = row.get("total_seconds");
            let hours = total_seconds / 3600;
            let minutes = (total_seconds % 3600) / 60;
            ctx.say(format!("You have spent {}h {}m in voice channels.", hours, minutes)).await?;
        }
        None => {
            ctx.say("You have no recorded voice time in this server.").await?;
        }
    }
    Ok(())
}

/// Displays the top users by voice time. Defaults to top 5 if no limit is provided.
#[poise::command(slash_command, guild_only)]
async fn leaderboard(
    ctx: Context<'_>,
    #[description = "Number of users to display (default 5)"] limit: Option<i64>,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().unwrap().get() as i64;
    let fetch_limit = limit.unwrap_or(5).clamp(1, 50);

    let records = sqlx::query("SELECT user_id, total_seconds FROM voice_stats WHERE guild_id = ? ORDER BY total_seconds DESC LIMIT ?")
        .bind(guild_id)
        .bind(fetch_limit)
        .fetch_all(&ctx.data().db)
        .await?;

    if records.is_empty() {
        ctx.say("The leaderboard is currently empty.").await?;
        return Ok(());
    }

    let mut response = format!("**Top {} Users in Voice Activity:**\n", fetch_limit);
    for (index, row) in records.into_iter().enumerate() {
        let user_id: i64 = row.get("user_id");
        let total_seconds: i64 = row.get("total_seconds");
        
        let hours = total_seconds / 3600;
        let minutes = (total_seconds % 3600) / 60;
        response.push_str(&format!(
            "{}. <@{}>: {}h {}m\n",
            index + 1,
            user_id,
            hours,
            minutes
        ));
    }

    ctx.say(response).await?;
    Ok(())
}

/// Resets voice time. Provide a user to reset only their stats, or omit to reset the entire server.
#[poise::command(slash_command, guild_only, required_permissions = "ADMINISTRATOR")]
async fn reset_stats(
    ctx: Context<'_>,
    #[description = "Specific user to reset"] user: Option<serenity::User>,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().unwrap().get() as i64;

    match user {
        Some(u) => {
            let user_id = u.id.get() as i64;
            sqlx::query("DELETE FROM voice_stats WHERE user_id = ? AND guild_id = ?")
                .bind(user_id)
                .bind(guild_id)
                .execute(&ctx.data().db)
                .await?;
            ctx.say(format!("Reset voice statistics for <@{}>.", user_id)).await?;
        }
        None => {
            sqlx::query("DELETE FROM voice_stats WHERE guild_id = ?")
                .bind(guild_id)
                .execute(&ctx.data().db)
                .await?;
            ctx.say("Reset all voice statistics for this server.").await?;
        }
    }
    Ok(())
}

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

    let active_sessions = Arc::new(Mutex::new(HashMap::new()));
    let intents = serenity::GatewayIntents::non_privileged() | serenity::GatewayIntents::GUILD_VOICE_STATES;

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: vec![stats(), leaderboard(), reset_stats()],
            event_handler: |ctx, event, _framework, data| {
                Box::pin(async move {
                    if let serenity::FullEvent::VoiceStateUpdate { old: _, new } = event {
                        let user_id = new.user_id.get();
                        let guild_id = match new.guild_id {
                            Some(id) => id.get(),
                            None => return Ok(()),
                        };

                        let mut sessions = data.active_sessions.lock().await;
                        let now = chrono::Utc::now().timestamp();

                        let is_in_channel = new.channel_id.is_some();

                        if is_in_channel {
                            sessions.entry((user_id, guild_id)).or_insert(now);
                        } else {
                            if let Some(start_time) = sessions.remove(&(user_id, guild_id)) {
                                let duration = now - start_time;
                                if duration > 0 {
                                    sqlx::query(
                                        "INSERT INTO voice_stats (user_id, guild_id, total_seconds) 
                                         VALUES (?, ?, ?) 
                                         ON CONFLICT(user_id, guild_id) 
                                         DO UPDATE SET total_seconds = total_seconds + ?"
                                    )
                                    .bind(user_id as i64)
                                    .bind(guild_id as i64)
                                    .bind(duration)
                                    .bind(duration)
                                    .execute(&data.db)
                                    .await?;
                                }
                            }
                        }
                    }
                    Ok(())
                })
            },
            ..Default::default()
        })
        .setup(|ctx, _ready, framework| {
            Box::pin(async move {
                poise::builtins::register_globally(ctx, &framework.options().commands).await?;
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
