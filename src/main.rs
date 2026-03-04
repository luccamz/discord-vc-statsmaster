use poise::serenity_prelude as serenity;
use sqlx::{sqlite::SqlitePoolOptions, Row, SqlitePool};
use std::{collections::HashMap, env, sync::Arc};
use tokio::{sync::Mutex, time::Duration};
use chrono::{Datelike, Timelike};

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

    let record = sqlx::query("SELECT total_seconds FROM voice_stats WHERE user_id = ? AND guild_id = ?")
        .bind(user_id)
        .bind(guild_id)
        .fetch_optional(&ctx.data().db)
        .await?;

    match record {
        Some(row) => {
            let total_seconds: i64 = row.get("total_seconds");
            let hours = total_seconds / 3600;
            let minutes = (total_seconds % 3600) / 60;
            ctx.say(format!("You have spent {}h {}m in voice channels.", hours, minutes)).await?;
        }
        None => {
            ctx.say("You have no recorded time in this server.").await?;
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

    let mut response = format!("**Top {} Users:**\n", fetch_limit);
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

/// Toggles tracking of voice activity for a specific channel.
#[poise::command(slash_command, guild_only, required_permissions = "ADMINISTRATOR")]
async fn toggle_tracking(
    ctx: Context<'_>,
    #[description = "Voice channel to toggle tracking for"] channel: serenity::Channel,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().unwrap().get() as i64;
    let channel_id = channel.id().get() as i64;

    let existing = sqlx::query("SELECT 1 FROM tracked_channels WHERE channel_id = ?")
        .bind(channel_id)
        .fetch_optional(&ctx.data().db)
        .await?;

    if existing.is_some() {
        sqlx::query("DELETE FROM tracked_channels WHERE channel_id = ?")
            .bind(channel_id)
            .execute(&ctx.data().db)
            .await?;
        ctx.say(format!("Stopped tracking voice activity in <#{}>.", channel_id)).await?;
    } else {
        sqlx::query("INSERT INTO tracked_channels (channel_id, guild_id) VALUES (?, ?)")
            .bind(channel_id)
            .bind(guild_id)
            .execute(&ctx.data().db)
            .await?;
        ctx.say(format!("Started tracking voice activity in <#{}>.", channel_id)).await?;
    }
    Ok(())
}

#[derive(Debug, poise::ChoiceParameter)]
pub enum WeekdayChoice {
    Monday, Tuesday, Wednesday, Thursday, Friday, Saturday, Sunday
}

impl WeekdayChoice {
    fn to_chrono_num(&self) -> u32 {
        match self {
            WeekdayChoice::Monday => 0, WeekdayChoice::Tuesday => 1,
            WeekdayChoice::Wednesday => 2, WeekdayChoice::Thursday => 3,
            WeekdayChoice::Friday => 4, WeekdayChoice::Saturday => 5,
            WeekdayChoice::Sunday => 6,
        }
    }
}

async fn perform_guild_reset(
    http: &Arc<serenity::Http>, 
    db: &SqlitePool, 
    guild_id: i64, 
    channel_id: i64, 
    current_weeks: i64
) -> Result<(), Error> {
    let new_weeks = current_weeks + 1;
    
    sqlx::query("UPDATE guild_settings SET weeks_tracked = ? WHERE guild_id = ?")
        .bind(new_weeks)
        .bind(guild_id)
        .execute(db)
        .await?;

    let records = sqlx::query("SELECT user_id, total_seconds FROM voice_stats WHERE guild_id = ? ORDER BY total_seconds DESC LIMIT 5")
        .bind(guild_id)
        .fetch_all(db)
        .await?;

    let mut message = format!("**Weekly Voice Leaderboard (Week {})**\n", new_weeks);
    
    if records.is_empty() {
        message.push_str("No voice activity recorded this week.");
    } else {
        for (index, record) in records.into_iter().enumerate() {
            let user_id: i64 = record.get("user_id");
            let total_seconds: i64 = record.get("total_seconds");
            let hours = total_seconds / 3600;
            let minutes = (total_seconds % 3600) / 60;
            message.push_str(&format!("{}. <@{}>: {}h {}m\n", index + 1, user_id, hours, minutes));
        }
    }

    let channel = serenity::ChannelId::new(channel_id as u64);
    let builder = serenity::CreateMessage::new().content(message);
    let _ = channel.send_message(http, builder).await;

    sqlx::query("DELETE FROM voice_stats WHERE guild_id = ?")
        .bind(guild_id)
        .execute(db)
        .await?;

    Ok(())
}

async fn weekly_reset_task(http: Arc<serenity::Http>, db: SqlitePool) {
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    
    loop {
        interval.tick().await; // Wait for the next 60-second boundary
        
        let now = chrono::Utc::now();
        let current_day = now.weekday().num_days_from_monday() as i64;
        let current_hour = now.hour() as i64;
        let current_minute = now.minute() as i64;

        // Query only the guilds scheduled for this exact minute
        let pending_resets = sqlx::query(
            "SELECT guild_id, announcement_channel_id, weeks_tracked FROM guild_settings 
             WHERE reset_day = ? AND reset_hour = ? AND reset_minute = ?"
        )
        .bind(current_day)
        .bind(current_hour)
        .bind(current_minute)
        .fetch_all(&db)
        .await;

        if let Ok(settings) = pending_resets {
            for row in settings {
                let guild_id: i64 = row.get("guild_id");
                let channel_id: i64 = row.get("announcement_channel_id");
                let weeks_tracked: i64 = row.get("weeks_tracked");

                if let Err(e) = perform_guild_reset(&http, &db, guild_id, channel_id, weeks_tracked).await {
                    eprintln!("Failed to execute scheduled reset for guild {}: {}", guild_id, e);
                }
            }
        }
    }
}

/// Configures the weekly leaderboard reset schedule and announcement channel
#[poise::command(slash_command, guild_only, required_permissions = "ADMINISTRATOR")]
async fn config_schedule(
    ctx: Context<'_>,
    #[description = "Target channel"] channel: serenity::Channel,
    #[description = "Day of the week"] day: WeekdayChoice,
    #[description = "Local Hour (0-23)"] hour: i64,
    #[description = "Minute (0-59)"] minute: i64,
    #[description = "UTC Offset (e.g., -5 for EST, 2 for CEST)"] utc_offset: i64,
) -> Result<(), Error> {
    // Validate inputs
    if hour < 0 || hour > 23 || minute < 0 || minute > 59 {
        ctx.say("Invalid time constraint. Hour must be 0-23 and minute 0-59.").await?;
        return Ok(());
    }
    if utc_offset < -12 || utc_offset > 14 {
        ctx.say("Invalid UTC offset. Must be between -12 and 14.").await?;
        return Ok(());
    }

    let guild_id = ctx.guild_id().unwrap().get() as i64;
    let channel_id = channel.id().get() as i64;
    
    // Calculate UTC time
    let mut utc_hour = hour - utc_offset;
    let mut utc_day = day.to_chrono_num() as i64;

    // Handle day wrapping if the timezone offset pushes the hour past midnight
    if utc_hour < 0 {
        utc_hour += 24;
        // rem_euclid handles negative number wrapping correctly in Rust
        utc_day = (utc_day - 1).rem_euclid(7); 
    } else if utc_hour >= 24 {
        utc_hour -= 24;
        utc_day = (utc_day + 1).rem_euclid(7);
    }

    // Insert the calculated UTC schedule into the database
    sqlx::query(
        "INSERT INTO guild_settings (guild_id, announcement_channel_id, reset_day, reset_hour, reset_minute) 
         VALUES (?, ?, ?, ?, ?) 
         ON CONFLICT(guild_id) DO UPDATE SET 
         announcement_channel_id = excluded.announcement_channel_id,
         reset_day = excluded.reset_day,
         reset_hour = excluded.reset_hour,
         reset_minute = excluded.reset_minute"
    )
    .bind(guild_id)
    .bind(channel_id)
    .bind(utc_day)
    .bind(utc_hour)
    .bind(minute)
    .execute(&ctx.data().db)
    .await?;

    ctx.say(format!(
        "Schedule configured. Leaderboard will reset locally every {:?} at {:02}:{:02} (UTC Offset: {}).", 
        day, hour, minute, utc_offset
    )).await?;
    
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
            commands: vec![stats(), leaderboard(), reset_stats(), toggle_tracking(), config_schedule()],
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

                        let mut is_in_tracked_channel = false;
                        if let Some(channel_id_non_zero) = new.channel_id {
                            let channel_id = channel_id_non_zero.get() as i64;
                            let existing = sqlx::query("SELECT 1 FROM tracked_channels WHERE channel_id = ?")
                                .bind(channel_id)
                                .fetch_optional(&data.db)
                                .await?;
                            is_in_tracked_channel = existing.is_some();
                        }

                        if is_in_tracked_channel {
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

                let http_clone = ctx.http.clone();
                let db_clone = db.clone();
                tokio::spawn(async move {
                    weekly_reset_task(http_clone, db_clone).await;
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
