use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(DeviceInfo::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(DeviceInfo::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(DeviceInfo::RecordedAt).date_time().not_null())
                    .col(ColumnDef::new(DeviceInfo::HarvardVersion).text().null())
                    .col(ColumnDef::new(DeviceInfo::BoylstonVersion).text().null())
                    .col(ColumnDef::new(DeviceInfo::DeviceName).text().null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_device_info_recorded")
                    .table(DeviceInfo::Table)
                    .col(DeviceInfo::RecordedAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(DeviceInfo::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum DeviceInfo {
    Table,
    Id,
    RecordedAt,
    HarvardVersion,
    BoylstonVersion,
    DeviceName,
}
