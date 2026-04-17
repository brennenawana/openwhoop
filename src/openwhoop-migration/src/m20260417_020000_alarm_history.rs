use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(AlarmHistory::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AlarmHistory::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(AlarmHistory::Action).text().not_null())
                    .col(ColumnDef::new(AlarmHistory::ActionAt).date_time().not_null())
                    .col(ColumnDef::new(AlarmHistory::ScheduledFor).date_time().null())
                    .col(ColumnDef::new(AlarmHistory::Enabled).boolean().null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_alarm_history_at")
                    .table(AlarmHistory::Table)
                    .col(AlarmHistory::ActionAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(AlarmHistory::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum AlarmHistory {
    Table,
    Id,
    Action,
    ActionAt,
    ScheduledFor,
    Enabled,
}
