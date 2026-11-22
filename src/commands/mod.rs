pub mod config;
pub mod pomodoro;
pub mod tasks;
pub mod tracking;

pub use config::*;
pub use pomodoro::*;
pub use tasks::*;
pub use tracking::*;

use crate::state::{Context, Error};

pub async fn reply_private(ctx: Context<'_>, content: impl Into<String>) -> Result<(), Error> {
    ctx.send(
        poise::CreateReply::default()
            .content(content)
            .ephemeral(true),
    )
    .await?;
    Ok(())
}
