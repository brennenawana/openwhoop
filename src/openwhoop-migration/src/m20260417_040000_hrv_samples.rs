use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(HrvSamples::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(HrvSamples::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(HrvSamples::WindowStart).date_time().not_null())
                    .col(ColumnDef::new(HrvSamples::WindowEnd).date_time().not_null())
                    .col(ColumnDef::new(HrvSamples::Rmssd).double().not_null())
                    .col(ColumnDef::new(HrvSamples::Sdnn).double().null())
                    .col(ColumnDef::new(HrvSamples::MeanHr).double().not_null())
                    .col(ColumnDef::new(HrvSamples::RrCount).integer().not_null())
                    .col(
                        ColumnDef::new(HrvSamples::StillnessRatio)
                            .double()
                            .not_null(),
                    )
                    .col(ColumnDef::new(HrvSamples::Context).text().not_null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_hrv_samples_start")
                    .table(HrvSamples::Table)
                    .col(HrvSamples::WindowStart)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(HrvSamples::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum HrvSamples {
    Table,
    Id,
    WindowStart,
    WindowEnd,
    Rmssd,
    Sdnn,
    MeanHr,
    RrCount,
    StillnessRatio,
    Context,
}
