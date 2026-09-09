//! Repository-private row binding mechanism. Identifiers come only from static
//! repository declarations; callers retain connection, transaction and clock ownership.

use sqlx::{Arguments, Encode, Sqlite, SqliteConnection, Type, sqlite::SqliteArguments};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum WriteMode {
    Insert,
    Update,
    Import,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Mutation {
    Insert,
    Update,
    Upsert,
}

pub(super) struct RowWrite {
    table: &'static str,
    mutation: Mutation,
    columns: Vec<(&'static str, bool)>,
    arguments: SqliteArguments,
}

impl RowWrite {
    pub(super) fn new(
        table: &'static str,
        mutation: Mutation,
        id: &str,
    ) -> Result<Self, sqlx::Error> {
        let mut arguments = SqliteArguments::default();
        arguments.add(id).map_err(sqlx::Error::Encode)?;
        Ok(Self {
            table,
            mutation,
            columns: vec![("id", false)],
            arguments,
        })
    }

    pub(super) fn field<'a, T: Encode<'a, Sqlite> + Type<Sqlite>>(
        &mut self,
        name: &'static str,
        value: T,
        mutable: bool,
    ) -> Result<(), sqlx::Error> {
        if self.mutation == Mutation::Update && !mutable {
            return Ok(());
        }
        self.arguments.add(value).map_err(sqlx::Error::Encode)?;
        self.columns.push((name, mutable));
        Ok(())
    }

    fn statement(self) -> (String, SqliteArguments) {
        let sql = match self.mutation {
            Mutation::Update => {
                let assignments = self
                    .columns
                    .iter()
                    .enumerate()
                    .filter(|(_, (_, mutable))| *mutable)
                    .map(|(index, (name, _))| format!("{name} = ?{}", index + 1))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("UPDATE {} SET {assignments} WHERE id = ?1", self.table)
            }
            Mutation::Insert | Mutation::Upsert => {
                let columns = self
                    .columns
                    .iter()
                    .map(|(name, _)| *name)
                    .collect::<Vec<_>>()
                    .join(", ");
                let values = (1..=self.columns.len())
                    .map(|index| format!("?{index}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let mut sql = format!("INSERT INTO {} ({columns}) VALUES ({values})", self.table);
                if self.mutation == Mutation::Upsert {
                    let assignments = self
                        .columns
                        .iter()
                        .filter(|(_, mutable)| *mutable)
                        .map(|(name, _)| format!("{name} = excluded.{name}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    sql.push_str(&format!(" ON CONFLICT(id) DO UPDATE SET {assignments}"));
                }
                sql
            }
        };
        (sql, self.arguments)
    }

    pub(super) async fn execute(
        self,
        connection: &mut SqliteConnection,
    ) -> Result<(), sqlx::Error> {
        let (sql, arguments) = self.statement();
        sqlx::query_with::<Sqlite, _>(sqlx::AssertSqlSafe(sql), arguments)
            .execute(connection)
            .await?;
        Ok(())
    }

    pub(super) async fn fetch_optional<T>(
        self,
        connection: &mut SqliteConnection,
    ) -> Result<Option<T>, sqlx::Error>
    where
        T: for<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> + Send + Unpin,
    {
        let (mut sql, arguments) = self.statement();
        sql.push_str(" RETURNING *");
        sqlx::query_as_with::<Sqlite, T, _>(sqlx::AssertSqlSafe(sql), arguments)
            .fetch_optional(connection)
            .await
    }
}
