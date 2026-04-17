use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ActivitySamples::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ActivitySamples::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ActivitySamples::WindowStart)
                            .date_time()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ActivitySamples::WindowEnd)
                            .date_time()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ActivitySamples::Classification)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ActivitySamples::AccelMagnitudeMean)
                            .double()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ActivitySamples::AccelMagnitudeStd)
                            .double()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ActivitySamples::GyroMagnitudeMean)
                            .double()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ActivitySamples::DominantFrequencyHz)
                            .double()
                            .null(),
                    )
                    .col(ColumnDef::new(ActivitySamples::MeanHr).double().null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_activity_samples_start")
                    .table(ActivitySamples::Table)
                    .col(ActivitySamples::WindowStart)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ActivitySamples::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum ActivitySamples {
    Table,
    Id,
    WindowStart,
    WindowEnd,
    Classification,
    AccelMagnitudeMean,
    AccelMagnitudeStd,
    GyroMagnitudeMean,
    DominantFrequencyHz,
    MeanHr,
}
