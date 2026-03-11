use crate::state::{Context, Error};
use poise::serenity_prelude as serenity;

/// Displays your total accumulated voice time in this server
#[poise::command(slash_command, guild_only)]
pub async fn stats(ctx: Context<'_>) -> Result<(), Error> {
    let user_id = ctx.author().id.get() as i64;
    let guild_id = ctx.guild_id().unwrap().get() as i64;

    let record = sqlx::query!(
        "SELECT total_seconds, personal_record FROM voice_stats WHERE user_id = ? AND guild_id = ?",
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
            let total_seconds: i64 = row.total_seconds;
            let hours = total_seconds / 3600;
            let minutes = (total_seconds % 3600) / 60;
            let percent = if row.personal_record > 0 {
                Some((total_seconds as f64 / row.personal_record as f64) * 100.0)
            } else {
                None
            };

            embed = embed.field(
                "Total Time",
                format!(
                    "**{}** hours and **{}** minutes.. **{:.0}%** of your PR!",
                    hours,
                    minutes,
                    percent.unwrap_or(9999999.0)
                ),
                false,
            );
        }
        None => {
            embed = embed.description("You have no recorded voice time in this server.");
        }
    }

    let reply = poise::CreateReply::default().embed(embed);
    ctx.send(reply).await?;

    Ok(())
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
        "SELECT user_id, total_seconds FROM voice_stats WHERE guild_id = ? ORDER BY total_seconds DESC LIMIT ?",
        guild_id,
        fetch_limit
    )
        .fetch_all(&ctx.data().db)
        .await?;

    if records.is_empty() {
        ctx.say("The leaderboard is currently empty.").await?;
        return Ok(());
    }

    let mut description = String::new();

    for (index, row) in records.into_iter().enumerate() {
        let user_id: i64 = row.user_id;
        let total_seconds: i64 = row.total_seconds;

        let hours = total_seconds / 3600;
        let minutes = (total_seconds % 3600) / 60;

        description.push_str(&format!(
            "**{}.** <@{}> • {}h {}m\n",
            index + 1,
            user_id,
            hours,
            minutes
        ));
    }

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
        .timeout(std::time::Duration::from_secs(30))
        .await
    {
        if mci.data.custom_id == "confirm_cancel" {
            let mut sessions = ctx.data().active_sessions.lock().await;

            sessions.remove(&(user_id, guild_id));

            mci.create_response(
                ctx,
                serenity::CreateInteractionResponse::UpdateMessage(
                    serenity::CreateInteractionResponseMessage::new()
                        .content("Session cancelled!")
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

    if existing.is_some() {
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
        .timeout(std::time::Duration::from_secs(30))
        .await
    {
        if mci.data.custom_id == "confirm_reset" {
            match user {
                Some(u) => {
                    let user_id = u.id.get() as i64;
                    sqlx::query!(
                        "UPDATE voice_stats 
                        SET total_seconds = 0
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
                        SET total_seconds = 0
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
