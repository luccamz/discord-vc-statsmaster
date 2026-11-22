use crate::state::Error;
use sqlx::SqlitePool;

/// Adds the time since `start` to the user's weekly total and to the active task, atomically.
pub async fn commit(
    db: &SqlitePool,
    user_id: i64,
    guild_id: i64,
    start: i64,
    task_id: Option<i64>,
    now: i64,
) -> Result<(), Error> {
    let duration = now - start;
    if duration <= 0 {
        return Ok(());
    }

    let mut tx = db.begin().await?;
    sqlx::query!(
        "INSERT INTO voice_stats (user_id, guild_id, total_seconds) 
         VALUES (?, ?, ?) ON CONFLICT(user_id, guild_id) 
         DO UPDATE SET total_seconds = total_seconds + ?",
        user_id,
        guild_id,
        duration,
        duration
    )
    .execute(&mut *tx)
    .await?;

    if let Some(task_id) = task_id {
        sqlx::query!(
            "UPDATE user_tasks 
             SET time_spent_seconds = time_spent_seconds + ?,
                 record_session_seconds = MAX(record_session_seconds, ?)
             WHERE task_id = ?",
            duration,
            duration,
            task_id
        )
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}
