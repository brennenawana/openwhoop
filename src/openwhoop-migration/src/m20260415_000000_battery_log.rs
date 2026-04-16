use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(BatteryLog::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(BatteryLog::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(BatteryLog::Time)
                            .date_time()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(BatteryLog::Percent)
                            .double()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(BatteryLog::Charging)
                            .boolean()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(BatteryLog::IsWorn)
                            .boolean()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(BatteryLog::AvgBpm)
                            .small_integer()
                            .null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(BatteryLog::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum BatteryLog {
    Table,
    Id,
    Time,
    Percent,
    Charging,
    IsWorn,
    AvgBpm,
}
