use crate::commands::reply_private;
use crate::pomodoro::{self, Config};
use crate::state::{Context, Error};
use poise::serenity_prelude as serenity;
use std::time::Duration;

async fn is_tracked(ctx: Context<'_>, channel: serenity::ChannelId) -> Result<bool, Error> {
    let channel_id = channel.get() as i64;
    Ok(sqlx::query!(
        "SELECT channel_id FROM tracked_channels WHERE channel_id = ?",
        channel_id
    )
    .fetch_optional(&ctx.data().db)
    .await?
    .is_some())
}

/// Starts pomodoro mode in your current tracked voice channel.
#[poise::command(slash_command, guild_only)]
pub async fn pomodoro(
    ctx: Context<'_>,
    #[description = "Work cycle in minutes (default 25)"]
    #[min = 1]
    #[max = 240]
    work: Option<u32>,
    #[description = "Pause cycle in minutes (default 5)"]
    #[min = 1]
    #[max = 60]
    pause: Option<u32>,
    #[description = "Number of work cycles (default: until you leave)"]
    #[min = 1]
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
    if !is_tracked(ctx, work_channel).await? {
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
    if is_tracked(ctx, lobby).await? {
        return reply_private(
            ctx,
            "The pomodoro lobby is a tracked channel. Ask an admin to change it.",
        )
        .await;
    }

    let (work, pause) = (work.unwrap_or(25), pause.unwrap_or(5));
    pomodoro::start(Config {
        http: ctx.serenity_context().http.clone(),
        pomodoros: ctx.data().pomodoros.clone(),
        guild,
        user,
        work_channel,
        lobby,
        work: Duration::from_secs(work as u64 * 60),
        pause: Duration::from_secs(pause as u64 * 60),
        reps: repetitions,
        db: ctx.data().db.clone(),
    });

    let rounds = repetitions.map_or("∞".to_string(), |r| r.to_string());
    reply_private(ctx,
        format!(
            "🍅 Pomodoro started: {work} min work, {pause} min pause, {rounds} rounds. Use ⏸ on the timer to pause; leave or switch channels to stop."
        ),
    )
    .await
}
