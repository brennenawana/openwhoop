use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Events::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Events::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Events::Timestamp).date_time().not_null())
                    .col(ColumnDef::new(Events::EventId).integer().not_null())
                    .col(ColumnDef::new(Events::EventName).text().not_null())
                    .col(ColumnDef::new(Events::RawData).json().null())
                    .col(
                        ColumnDef::new(Events::Synced)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await?;

        // Idempotency on sync retry: a given (timestamp, event_id) pair
        // corresponds to one real event; repeat packets must not produce
        // duplicate rows.
        manager
            .create_index(
                Index::create()
                    .name("idx_events_unique_ts_id")
                    .table(Events::Table)
                    .col(Events::Timestamp)
                    .col(Events::EventId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_events_timestamp")
                    .table(Events::Table)
                    .col(Events::Timestamp)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_events_type")
                    .table(Events::Table)
                    .col(Events::EventId)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Events::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum Events {
    Table,
    Id,
    Timestamp,
    EventId,
    EventName,
    RawData,
    Synced,
}
