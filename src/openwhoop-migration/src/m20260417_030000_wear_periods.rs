use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(WearPeriods::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(WearPeriods::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(WearPeriods::Start).date_time().not_null())
                    .col(ColumnDef::new(WearPeriods::End).date_time().not_null())
                    .col(ColumnDef::new(WearPeriods::Source).text().not_null())
                    .col(
                        ColumnDef::new(WearPeriods::DurationMinutes)
                            .double()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_wear_periods_start")
                    .table(WearPeriods::Table)
                    .col(WearPeriods::Start)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(WearPeriods::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum WearPeriods {
    Table,
    Id,
    Start,
    End,
    Source,
    DurationMinutes,
}
