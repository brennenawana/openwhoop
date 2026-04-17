use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(SyncLog::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(SyncLog::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(SyncLog::AttemptStartedAt)
                            .date_time()
                            .not_null(),
                    )
                    .col(ColumnDef::new(SyncLog::AttemptEndedAt).date_time().null())
                    .col(ColumnDef::new(SyncLog::Outcome).text().not_null())
                    .col(ColumnDef::new(SyncLog::ErrorMessage).text().null())
                    .col(
                        ColumnDef::new(SyncLog::HeartRateRowsAdded)
                            .integer()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(SyncLog::PacketsDownloaded)
                            .integer()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(SyncLog::SleepCyclesCreated)
                            .integer()
                            .default(0),
                    )
                    .col(ColumnDef::new(SyncLog::Trigger).text().null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_sync_log_started")
                    .table(SyncLog::Table)
                    .col(SyncLog::AttemptStartedAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(SyncLog::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum SyncLog {
    Table,
    Id,
    AttemptStartedAt,
    AttemptEndedAt,
    Outcome,
    ErrorMessage,
    HeartRateRowsAdded,
    PacketsDownloaded,
    SleepCyclesCreated,
    Trigger,
}
