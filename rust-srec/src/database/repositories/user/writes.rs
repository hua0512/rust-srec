use super::super::row_write::{Mutation, RowWrite, WriteMode};
use crate::database::models::UserDbModel;

pub(crate) async fn write_user(
    connection: &mut sqlx::SqliteConnection,
    model: &UserDbModel,
    mode: WriteMode,
    updated_at: i64,
) -> Result<(), sqlx::Error> {
    let mutation = match mode {
        WriteMode::Insert => Mutation::Insert,
        WriteMode::Update => Mutation::Update,
        WriteMode::Import => Mutation::Upsert,
    };
    let mut row = RowWrite::new("users", mutation, &model.id)?;
    row.field("username", &model.username, true)?;
    row.field("password_hash", &model.password_hash, true)?;
    row.field("email", &model.email, true)?;
    row.field("roles", &model.roles, true)?;
    row.field("is_active", model.is_active, true)?;
    row.field("must_change_password", model.must_change_password, true)?;
    row.field("last_login_at", model.last_login_at, true)?;
    row.field("created_at", model.created_at, false)?;
    row.field("updated_at", updated_at, true)?;
    row.execute(connection).await
}

pub(crate) async fn import_user(
    connection: &mut sqlx::SqliteConnection,
    model: &UserDbModel,
) -> Result<(), sqlx::Error> {
    write_user(connection, model, WriteMode::Import, model.updated_at).await
}

pub(crate) enum EmailSlots<'a> {
    All,
    User(&'a str),
}

pub(crate) async fn release_email_slots(
    connection: &mut sqlx::SqliteConnection,
    slots: EmailSlots<'_>,
) -> Result<(), sqlx::Error> {
    match slots {
        EmailSlots::All => {
            sqlx::query("UPDATE users SET email = NULL WHERE email IS NOT NULL")
                .execute(connection)
                .await?;
        }
        EmailSlots::User(id) => {
            sqlx::query("UPDATE users SET email = NULL WHERE id = ? AND email IS NOT NULL")
                .bind(id)
                .execute(connection)
                .await?;
        }
    }
    Ok(())
}

pub(crate) async fn delete_user(
    connection: &mut sqlx::SqliteConnection,
    id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(id)
        .execute(connection)
        .await?;
    Ok(())
}
