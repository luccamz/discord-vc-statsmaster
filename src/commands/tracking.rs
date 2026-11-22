use crate::commands::tasks::format_duration;
use crate::state::{Context, Error};
use poise::serenity_prelude as serenity;

/// Displays your total accumulated voice time in this server
#[poise::command(slash_command, guild_only)]
pub async fn stats(ctx: Context<'_>) -> Result<(), Error> {
    let user_id = ctx.author().id.get() as i64;
    let guild_id = ctx.guild_id().unwrap().get() as i64;

    let record = sqlx::query!(
        "SELECT total_seconds, personal_record, work_cycles FROM voice_stats WHERE user_id = ? AND guild_id = ?",
        user_id,
        guild_id
    )
    .fetch_optional(&ctx.data().db)
    .await?;

    let mut embed = serenity::CreateEmbed::new()
        .title("Voice Statistics")
        .color(0x3498DB)
        .thumbnail(ctx.author().face());

    match record {
        Some(row) => {
            let (hours, minutes) = (row.total_seconds / 3600, (row.total_seconds % 3600) / 60);
            let pr = if row.personal_record > 0 {
                let percent = row.total_seconds as f64 / row.personal_record as f64 * 100.0;
                format!("**{percent:.0}%** of your PR!")
            } else {
                "first week, no PR yet!".to_string()
            };
            embed = embed.field(
                "Total Time",
                format!("**{hours}** hours and **{minutes}** minutes.. {pr}"),
                false,
            );
            embed = embed.field(
                "Work Cycles",
                format!("🍅 **{}** this week", row.work_cycles),
                false,
            );
        }
        None => {
            embed = embed.description("You have no recorded voice time in this server.");
        }
    }

    let last_task = sqlx::query!(
        "SELECT description, time_spent_seconds FROM user_tasks 
         WHERE user_id = ? AND is_last_selected = 1 LIMIT 1",
        user_id
    )
    .fetch_optional(&ctx.data().db)
    .await?;
    if let Some(task) = last_task {
        embed = embed.field(
            "Last Working On",
            format!(
                "📌 {} ({})",
                task.description,
                crate::commands::tasks::format_duration(task.time_spent_seconds)
            ),
            false,
        );
    }

    let reply = poise::CreateReply::default().embed(embed);
    ctx.send(reply).await?;

    Ok(())
}

/// One leaderboard row, with medals for the top 3.
pub fn rank_line(rank: usize, user_id: i64, seconds: i64, cycles: i64) -> String {
    let place = match rank {
        1 => "🥇".to_string(),
        2 => "🥈".to_string(),
        3 => "🥉".to_string(),
        n => format!("**{n}.**"),
    };
    let mut line = format!("{place} <@{user_id}> • {}", format_duration(seconds));
    if cycles > 0 {
        line.push_str(&format!(" · 🍅 {cycles}"));
    }
    line
}

/// Displays the top users by voice time. Defaults to top 5 if no limit is provided.
#[poise::command(slash_command, guild_only)]
pub async fn leaderboard(
    ctx: Context<'_>,
    #[description = "Number of users to display (default 5)"] limit: Option<i64>,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().unwrap().get() as i64;
    let fetch_limit = limit.unwrap_or(5).clamp(1, 50);

    let records = sqlx::query!(
        "SELECT user_id, total_seconds, work_cycles FROM voice_stats WHERE guild_id = ? ORDER BY total_seconds DESC, work_cycles DESC LIMIT ?",
        guild_id,
        fetch_limit
    )
        .fetch_all(&ctx.data().db)
        .await?;

    if records.is_empty() {
        ctx.say("The leaderboard is currently empty.").await?;
        return Ok(());
    }

    let author = ctx.author().id.get() as i64;
    let author_listed = records.iter().any(|r| r.user_id == author);
    let mut lines: Vec<String> = records
        .iter()
        .enumerate()
        .map(|(i, r)| rank_line(i + 1, r.user_id, r.total_seconds, r.work_cycles))
        .collect();

    if !author_listed {
        let own = sqlx::query!(
            r#"SELECT v.total_seconds, v.work_cycles,
                      (SELECT COUNT(*) FROM voice_stats o
                       WHERE o.guild_id = v.guild_id
                         AND (o.total_seconds > v.total_seconds
                              OR (o.total_seconds = v.total_seconds
                                  AND o.work_cycles > v.work_cycles))) + 1 AS "rank!: i64"
               FROM voice_stats v WHERE v.user_id = ? AND v.guild_id = ?"#,
            author,
            guild_id
        )
        .fetch_optional(&ctx.data().db)
        .await?;
        if let Some(own) = own {
            lines.push("⋯".to_string());
            lines.push(rank_line(
                own.rank as usize,
                author,
                own.total_seconds,
                own.work_cycles,
            ));
        }
    }
    let description = lines.join("\n");

    let embed = serenity::CreateEmbed::new()
        .title(format!("Top {} Users in Voice Activity", fetch_limit))
        .description(description)
        .color(0xF1C40F);

    ctx.send(poise::CreateReply::default().embed(embed)).await?;

    Ok(())
}

/// Make current session not count
#[poise::command(slash_command, guild_only)]
pub async fn cancel_session(ctx: Context<'_>) -> Result<(), Error> {
    let user_id = ctx.author().id.get();
    let guild_id = ctx.guild_id().unwrap().get();
    let no_session = poise::CreateReply::default()
        .content("You have no session to cancel.")
        .ephemeral(true);

    let has_session = ctx
        .data()
        .active_sessions
        .lock()
        .await
        .get(&(user_id, guild_id))
        .is_some_and(|s| s.counted);
    if !has_session {
        ctx.send(no_session).await?;
        return Ok(());
    }

    let components = vec![serenity::CreateActionRow::Buttons(vec![
        serenity::CreateButton::new("confirm_cancel")
            .label("Confirm")
            .style(serenity::ButtonStyle::Danger),
        serenity::CreateButton::new("cancel_cancel") // hah !
            .label("Cancel")
            .style(serenity::ButtonStyle::Secondary),
    ])];

    let reply = poise::CreateReply::default()
        .content("Are you sure you want to cancel this sesh?")
        .ephemeral(true)
        .components(components);

    let handle = ctx.send(reply).await?;

    if let Some(mci) = serenity::ComponentInteractionCollector::new(ctx)
        .author_id(ctx.author().id)
        .channel_id(ctx.channel_id())
        .custom_ids(vec!["confirm_cancel".into(), "cancel_cancel".into()])
        .timeout(std::time::Duration::from_secs(30))
        .await
    {
        if mci.data.custom_id == "confirm_cancel" {
            let cancelled = match ctx
                .data()
                .active_sessions
                .lock()
                .await
                .get_mut(&(user_id, guild_id))
            {
                Some(session) if session.counted => {
                    session.counted = false;
                    true
                }
                _ => false,
            };
            let content = if cancelled {
                "Session cancelled!"
            } else {
                "You have no session to cancel."
            };

            mci.create_response(
                ctx,
                serenity::CreateInteractionResponse::UpdateMessage(
                    serenity::CreateInteractionResponseMessage::new()
                        .content(content)
                        .components(vec![]),
                ),
            )
            .await?;
        } else {
            mci.create_response(
                ctx,
                serenity::CreateInteractionResponse::UpdateMessage(
                    serenity::CreateInteractionResponseMessage::new()
                        .content("Didn't cancel.")
                        .components(vec![]),
                ),
            )
            .await?;
        }
    } else {
        let _ = handle
            .edit(
                ctx,
                poise::CreateReply::default()
                    .content("Didn't cancel (timeout).")
                    .components(vec![]),
            )
            .await;
    }

    Ok(())
}

/// Toggles tracking of voice activity for a specific channel.
#[poise::command(slash_command, guild_only, required_permissions = "ADMINISTRATOR")]
pub async fn toggle_tracking(
    ctx: Context<'_>,
    #[description = "Voice channel to toggle tracking for"] channel: serenity::Channel,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().unwrap().get() as i64;
    let channel_id = channel.id().get() as i64;

    let existing = sqlx::query!(
        "SELECT channel_id FROM tracked_channels WHERE channel_id = ?",
        channel_id
    )
    .fetch_optional(&ctx.data().db)
    .await?;

    if existing.is_none()
        && crate::commands::config::pomodoro_lobby(&ctx.data().db, guild_id).await?
            == Some(channel_id)
    {
        ctx.say("The pomodoro lobby cannot be tracked.").await?;
    } else if existing.is_some() {
        sqlx::query!(
            "DELETE FROM tracked_channels WHERE channel_id = ?",
            channel_id
        )
        .execute(&ctx.data().db)
        .await?;
        ctx.say(format!(
            "Stopped tracking voice activity in <#{}>.",
            channel_id
        ))
        .await?;
    } else {
        sqlx::query!(
            "INSERT INTO tracked_channels (channel_id, guild_id) VALUES (?, ?)",
            channel_id,
            guild_id
        )
        .execute(&ctx.data().db)
        .await?;
        ctx.say(format!(
            "Started tracking voice activity in <#{}>.",
            channel_id
        ))
        .await?;
    }
    Ok(())
}

/// Resets time of specified user or the whole server
#[poise::command(slash_command, guild_only, required_permissions = "ADMINISTRATOR")]
pub async fn reset_stats(
    ctx: Context<'_>,
    #[description = "Specific user to reset"] user: Option<serenity::User>,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().unwrap().get() as i64;

    let target_text = match &user {
        Some(u) => format!("<@{}>'s", u.id.get()),
        None => "the entire server's".to_string(),
    };

    let components = vec![serenity::CreateActionRow::Buttons(vec![
        serenity::CreateButton::new("confirm_reset")
            .label("Confirm")
            .style(serenity::ButtonStyle::Danger),
        serenity::CreateButton::new("cancel_reset")
            .label("Cancel")
            .style(serenity::ButtonStyle::Secondary),
    ])];

    let reply = poise::CreateReply::default()
        .content(format!(
            "Are you sure you want to reset {} voice statistics?",
            target_text
        ))
        .components(components);

    let handle = ctx.send(reply).await?;

    if let Some(mci) = serenity::ComponentInteractionCollector::new(ctx)
        .author_id(ctx.author().id)
        .channel_id(ctx.channel_id())
        .custom_ids(vec!["confirm_reset".into(), "cancel_reset".into()])
        .timeout(std::time::Duration::from_secs(30))
        .await
    {
        if mci.data.custom_id == "confirm_reset" {
            match user {
                Some(u) => {
                    let user_id = u.id.get() as i64;
                    sqlx::query!(
                        "UPDATE voice_stats 
                        SET total_seconds = 0, work_cycles = 0
                        WHERE user_id = ? AND guild_id = ?",
                        user_id,
                        guild_id
                    )
                    .execute(&ctx.data().db)
                    .await?;

                    mci.create_response(
                        ctx,
                        serenity::CreateInteractionResponse::UpdateMessage(
                            serenity::CreateInteractionResponseMessage::new()
                                .content(format!("Reset voice statistics for <@{}>.", user_id))
                                .components(vec![]),
                        ),
                    )
                    .await?;
                }
                None => {
                    sqlx::query!(
                        "UPDATE voice_stats 
                        SET total_seconds = 0, work_cycles = 0
                        WHERE guild_id = ?",
                        guild_id
                    )
                    .execute(&ctx.data().db)
                    .await?;

                    mci.create_response(
                        ctx,
                        serenity::CreateInteractionResponse::UpdateMessage(
                            serenity::CreateInteractionResponseMessage::new()
                                .content("Reset all voice statistics for this server.")
                                .components(vec![]),
                        ),
                    )
                    .await?;
                }
            }
        } else {
            mci.create_response(
                ctx,
                serenity::CreateInteractionResponse::UpdateMessage(
                    serenity::CreateInteractionResponseMessage::new()
                        .content("Reset cancelled.")
                        .components(vec![]),
                ),
            )
            .await?;
        }
    } else {
        let _ = handle
            .edit(
                ctx,
                poise::CreateReply::default()
                    .content("Reset cancelled (timeout).")
                    .components(vec![]),
            )
            .await;
    }

    Ok(())
}
