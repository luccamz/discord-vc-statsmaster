use crate::commands::reply_private;
use crate::pomodoro::{self, Config};
use crate::state::{Context, Data, Error};
use poise::serenity_prelude as serenity;
use std::time::Duration;

/// A user's pomodoro defaults; `reps` is `None` for an endless run.
pub struct PomodoroDefaults {
    pub work: u32,
    pub pause: u32,
    pub reps: Option<u32>,
    pub auto: bool,
}

pub fn endless_if_zero(reps: u32) -> Option<u32> {
    (reps > 0).then_some(reps)
}

pub async fn pomodoro_defaults(
    db: &sqlx::SqlitePool,
    user_id: i64,
) -> Result<PomodoroDefaults, Error> {
    let row = sqlx::query!(
        "SELECT pomodoro_work, pomodoro_pause, pomodoro_reps, pomodoro_auto 
         FROM user_settings WHERE user_id = ?",
        user_id
    )
    .fetch_optional(db)
    .await?;
    Ok(match row {
        Some(r) => PomodoroDefaults {
            work: r.pomodoro_work as u32,
            pause: r.pomodoro_pause as u32,
            reps: endless_if_zero(r.pomodoro_reps as u32),
            auto: r.pomodoro_auto,
        },
        None => PomodoroDefaults {
            work: 25,
            pause: 5,
            reps: None,
            auto: false,
        },
    })
}

async fn is_tracked(db: &sqlx::SqlitePool, channel: serenity::ChannelId) -> Result<bool, Error> {
    let channel_id = channel.get() as i64;
    Ok(sqlx::query!(
        "SELECT channel_id FROM tracked_channels WHERE channel_id = ?",
        channel_id
    )
    .fetch_optional(db)
    .await?
    .is_some())
}

fn launch(
    ctx: &serenity::Context,
    data: &Data,
    guild: serenity::GuildId,
    user: serenity::UserId,
    work_channel: serenity::ChannelId,
    lobby: serenity::ChannelId,
    params: &PomodoroDefaults,
) {
    pomodoro::start(Config {
        http: ctx.http.clone(),
        pomodoros: data.pomodoros.clone(),
        guild,
        user,
        work_channel,
        lobby,
        work: Duration::from_secs(params.work as u64 * 60),
        pause: Duration::from_secs(params.pause as u64 * 60),
        reps: params.reps,
        db: data.db.clone(),
        shard: ctx.shard.clone(),
        prompts: data.prompts.clone(),
    });
}

/// Starts a pomodoro with the user's defaults when they chose it as their default mode.
/// Does nothing without a usable lobby.
pub async fn auto_start(
    ctx: &serenity::Context,
    data: &Data,
    guild: serenity::GuildId,
    user: serenity::UserId,
    work_channel: serenity::ChannelId,
) -> Result<(), Error> {
    let defaults = pomodoro_defaults(&data.db, user.get() as i64).await?;
    if !defaults.auto {
        return Ok(());
    }
    let Some(lobby) = crate::commands::config::pomodoro_lobby(&data.db, guild.get() as i64)
        .await?
        .map(|id| serenity::ChannelId::new(id as u64))
    else {
        return Ok(());
    };
    if !is_tracked(&data.db, lobby).await? {
        launch(ctx, data, guild, user, work_channel, lobby, &defaults);
    }
    Ok(())
}

/// Starts pomodoro mode in your current tracked voice channel.
#[poise::command(slash_command, guild_only)]
pub async fn pomodoro(
    ctx: Context<'_>,
    #[description = "Work cycle in minutes (default: yours from /config_pomodoro)"]
    #[min = 1]
    #[max = 240]
    work: Option<u32>,
    #[description = "Pause cycle in minutes (default: yours from /config_pomodoro)"]
    #[min = 1]
    #[max = 60]
    pause: Option<u32>,
    #[description = "Number of work cycles, 0 = until you leave (default: yours)"]
    #[min = 0]
    #[max = 100]
    repetitions: Option<u32>,
) -> Result<(), Error> {
    let guild = ctx.guild_id().unwrap();
    let user = ctx.author().id;
    let current = ctx
        .guild()
        .and_then(|g| g.voice_states.get(&user).and_then(|v| v.channel_id));

    let Some(work_channel) = current else {
        return reply_private(ctx, "Join a tracked voice channel first.").await;
    };
    if !is_tracked(&ctx.data().db, work_channel).await? {
        return reply_private(ctx, "Pomodoro mode only works in tracked voice channels.").await;
    }

    let lobby = crate::commands::config::pomodoro_lobby(&ctx.data().db, guild.get() as i64)
        .await?
        .map(|id| serenity::ChannelId::new(id as u64));

    let Some(lobby) = lobby else {
        return reply_private(
            ctx,
            "No pomodoro lobby is configured. Ask an admin to run `/config_pomodoro_lobby`.",
        )
        .await;
    };
    if is_tracked(&ctx.data().db, lobby).await? {
        return reply_private(
            ctx,
            "The pomodoro lobby is a tracked channel. Ask an admin to change it.",
        )
        .await;
    }

    let defaults = pomodoro_defaults(&ctx.data().db, user.get() as i64).await?;
    let work = work.unwrap_or(defaults.work);
    let pause = pause.unwrap_or(defaults.pause);
    let repetitions = repetitions.map_or(defaults.reps, endless_if_zero);
    let params = PomodoroDefaults {
        work,
        pause,
        reps: repetitions,
        auto: defaults.auto,
    };
    launch(
        ctx.serenity_context(),
        ctx.data(),
        guild,
        user,
        work_channel,
        lobby,
        &params,
    );

    let rounds = repetitions.map_or("∞".to_string(), |r| r.to_string());
    reply_private(ctx,
        format!(
            "🍅 Pomodoro started: {work} min work, {pause} min pause, {rounds} rounds. Use ⏸ to pause and ⏹ to go back to normal mode."
        ),
    )
    .await?;

    // The timer carries the task dropdown from now on, so the join prompt goes away.
    let key = (user.get(), guild.get());
    let prompt = ctx
        .data()
        .prompts
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&key);
    if let Some((channel, msg)) = prompt {
        let _ = channel.delete_message(ctx, msg).await;
    }
    Ok(())
}
