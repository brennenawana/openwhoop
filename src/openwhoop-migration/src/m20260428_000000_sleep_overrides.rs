use sea_orm_migration::prelude::*;

/// Adds nullable `original_start` / `original_end` columns to `sleep_cycles`
/// to support user-driven bounds correction (e.g. "I stayed in bed until
/// 6:30, not 5:11").
///
/// **Semantics**: `start` / `end` always hold the *effective* bounds —
/// what every downstream consumer reads. When the user edits the times,
/// `start` / `end` are rewritten in place and the **detector's previous
/// values** are stored in `original_start` / `original_end`. This keeps
/// the rest of the codebase oblivious to the override mechanism while
/// preserving the round-trip data needed for "reset to detected." NULL
/// means the cycle has never been overridden — the detector's bounds are
/// authoritative.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(SleepCycles::Table)
                    .add_column(
                        ColumnDef::new(SleepCycles::OriginalStart)
                            .date_time()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(SleepCycles::Table)
                    .add_column(
                        ColumnDef::new(SleepCycles::OriginalEnd)
                            .date_time()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(SleepCycles::Table)
                    .drop_column(SleepCycles::OriginalEnd)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(SleepCycles::Table)
                    .drop_column(SleepCycles::OriginalStart)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum SleepCycles {
    Table,
    OriginalStart,
    OriginalEnd,
}
