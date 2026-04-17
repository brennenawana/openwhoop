use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(DevNotes::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(DevNotes::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(DevNotes::CreatedAt).date_time().not_null())
                    .col(ColumnDef::new(DevNotes::Author).text().not_null())
                    .col(ColumnDef::new(DevNotes::Kind).text().not_null())
                    .col(ColumnDef::new(DevNotes::Title).text().not_null())
                    .col(ColumnDef::new(DevNotes::BodyMd).text().null())
                    .col(ColumnDef::new(DevNotes::RelatedCommit).text().null())
                    .col(ColumnDef::new(DevNotes::RelatedFeature).text().null())
                    .col(
                        ColumnDef::new(DevNotes::RelatedRangeStart)
                            .date_time()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(DevNotes::RelatedRangeEnd)
                            .date_time()
                            .null(),
                    )
                    .col(ColumnDef::new(DevNotes::ResolvedAt).date_time().null())
                    .col(ColumnDef::new(DevNotes::ResolvedBy).text().null())
                    .col(ColumnDef::new(DevNotes::PayloadJson).json().null())
                    .to_owned(),
            )
            .await?;

        // Sort-by-created for "latest activity" feeds.
        manager
            .create_index(
                Index::create()
                    .name("idx_dev_notes_created")
                    .table(DevNotes::Table)
                    .col(DevNotes::CreatedAt)
                    .to_owned(),
            )
            .await?;

        // Inbox filter: unresolved notes show up in the dashboard.
        manager
            .create_index(
                Index::create()
                    .name("idx_dev_notes_resolved")
                    .table(DevNotes::Table)
                    .col(DevNotes::ResolvedAt)
                    .to_owned(),
            )
            .await?;

        // Filter by feature — each panel in the dashboard can scope to its
        // own feature's unresolved notes.
        manager
            .create_index(
                Index::create()
                    .name("idx_dev_notes_feature")
                    .table(DevNotes::Table)
                    .col(DevNotes::RelatedFeature)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(DevNotes::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum DevNotes {
    Table,
    Id,
    CreatedAt,
    Author,
    Kind,
    Title,
    BodyMd,
    RelatedCommit,
    RelatedFeature,
    RelatedRangeStart,
    RelatedRangeEnd,
    ResolvedAt,
    ResolvedBy,
    PayloadJson,
}
