use sea_orm_migration::prelude::*;

use crate::m20250127_195808_sleep_cycles::SleepCycles;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // sleep_epochs: one row per 30-second epoch inside a sleep cycle.
        manager
            .create_table(
                Table::create()
                    .table(SleepEpochs::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(SleepEpochs::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(SleepEpochs::SleepCycleId).uuid().not_null())
                    .col(
                        ColumnDef::new(SleepEpochs::EpochStart)
                            .date_time()
                            .not_null(),
                    )
                    .col(ColumnDef::new(SleepEpochs::EpochEnd).date_time().not_null())
                    .col(ColumnDef::new(SleepEpochs::Stage).text().not_null())
                    .col(ColumnDef::new(SleepEpochs::Confidence).double().null())
                    .col(ColumnDef::new(SleepEpochs::HrMean).double().null())
                    .col(ColumnDef::new(SleepEpochs::HrStd).double().null())
                    .col(ColumnDef::new(SleepEpochs::HrMin).double().null())
                    .col(ColumnDef::new(SleepEpochs::HrMax).double().null())
                    .col(ColumnDef::new(SleepEpochs::Rmssd).double().null())
                    .col(ColumnDef::new(SleepEpochs::Sdnn).double().null())
                    .col(ColumnDef::new(SleepEpochs::Pnn50).double().null())
                    .col(ColumnDef::new(SleepEpochs::LfPower).double().null())
                    .col(ColumnDef::new(SleepEpochs::HfPower).double().null())
                    .col(ColumnDef::new(SleepEpochs::LfHfRatio).double().null())
                    .col(
                        ColumnDef::new(SleepEpochs::MotionActivityCount)
                            .double()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(SleepEpochs::MotionStillnessRatio)
                            .double()
                            .null(),
                    )
                    .col(ColumnDef::new(SleepEpochs::RespRate).double().null())
                    .col(ColumnDef::new(SleepEpochs::FeatureBlob).json().null())
                    .col(
                        ColumnDef::new(SleepEpochs::ClassifierVersion)
                            .text()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_sleep_epochs_sleep_cycles")
                            .from(SleepEpochs::Table, SleepEpochs::SleepCycleId)
                            .to(SleepCycles::Table, SleepCycles::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_sleep_epochs_cycle")
                    .table(SleepEpochs::Table)
                    .col(SleepEpochs::SleepCycleId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_sleep_epochs_start")
                    .table(SleepEpochs::Table)
                    .col(SleepEpochs::EpochStart)
                    .to_owned(),
            )
            .await?;

        // New staging columns on sleep_cycles. All nullable: existing rows stay valid
        // until reclassified. Keep existing `score` column as-is; it will be populated
        // with performance_score at write time for backwards compat.
        let double_cols = [
            SleepCyclesNewCols::AwakeMinutes,
            SleepCyclesNewCols::LightMinutes,
            SleepCyclesNewCols::DeepMinutes,
            SleepCyclesNewCols::RemMinutes,
            SleepCyclesNewCols::SleepLatencyMinutes,
            SleepCyclesNewCols::WasoMinutes,
            SleepCyclesNewCols::SleepEfficiency,
            SleepCyclesNewCols::AvgRespiratoryRate,
            SleepCyclesNewCols::MinRespiratoryRate,
            SleepCyclesNewCols::MaxRespiratoryRate,
            SleepCyclesNewCols::SkinTempDeviationC,
            SleepCyclesNewCols::SleepNeedHours,
            SleepCyclesNewCols::SleepDebtHours,
            SleepCyclesNewCols::PerformanceScore,
        ];
        for col in double_cols {
            manager
                .alter_table(
                    Table::alter()
                        .table(SleepCycles::Table)
                        .add_column(ColumnDef::new(col).double().null())
                        .to_owned(),
                )
                .await?;
        }

        let int_cols = [
            SleepCyclesNewCols::WakeEventCount,
            SleepCyclesNewCols::CycleCount,
        ];
        for col in int_cols {
            manager
                .alter_table(
                    Table::alter()
                        .table(SleepCycles::Table)
                        .add_column(ColumnDef::new(col).integer().null())
                        .to_owned(),
                )
                .await?;
        }

        manager
            .alter_table(
                Table::alter()
                    .table(SleepCycles::Table)
                    .add_column(
                        ColumnDef::new(SleepCyclesNewCols::ClassifierVersion)
                            .text()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        // user_baselines: rolling per-user thresholds. One row per recompute; the
        // latest row by computed_at is the active baseline.
        manager
            .create_table(
                Table::create()
                    .table(UserBaselines::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(UserBaselines::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(UserBaselines::ComputedAt)
                            .date_time()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(UserBaselines::WindowNights)
                            .integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(UserBaselines::RestingHr).double().null())
                    .col(
                        ColumnDef::new(UserBaselines::SleepRmssdMedian)
                            .double()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(UserBaselines::SleepRmssdP25)
                            .double()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(UserBaselines::SleepRmssdP75)
                            .double()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(UserBaselines::HfPowerMedian)
                            .double()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(UserBaselines::LfHfRatioMedian)
                            .double()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(UserBaselines::SleepDurationMeanHours)
                            .double()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(UserBaselines::RespiratoryRateMean)
                            .double()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(UserBaselines::RespiratoryRateStd)
                            .double()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(UserBaselines::SkinTempMeanC)
                            .double()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(UserBaselines::SkinTempStdC)
                            .double()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_user_baselines_computed_at")
                    .table(UserBaselines::Table)
                    .col(UserBaselines::ComputedAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(UserBaselines::Table).to_owned())
            .await?;

        manager
            .drop_table(Table::drop().table(SleepEpochs::Table).to_owned())
            .await?;

        let new_cols: [SleepCyclesNewCols; 17] = [
            SleepCyclesNewCols::AwakeMinutes,
            SleepCyclesNewCols::LightMinutes,
            SleepCyclesNewCols::DeepMinutes,
            SleepCyclesNewCols::RemMinutes,
            SleepCyclesNewCols::SleepLatencyMinutes,
            SleepCyclesNewCols::WasoMinutes,
            SleepCyclesNewCols::SleepEfficiency,
            SleepCyclesNewCols::WakeEventCount,
            SleepCyclesNewCols::CycleCount,
            SleepCyclesNewCols::AvgRespiratoryRate,
            SleepCyclesNewCols::MinRespiratoryRate,
            SleepCyclesNewCols::MaxRespiratoryRate,
            SleepCyclesNewCols::SkinTempDeviationC,
            SleepCyclesNewCols::SleepNeedHours,
            SleepCyclesNewCols::SleepDebtHours,
            SleepCyclesNewCols::PerformanceScore,
            SleepCyclesNewCols::ClassifierVersion,
        ];

        for col in new_cols {
            manager
                .alter_table(
                    Table::alter()
                        .table(SleepCycles::Table)
                        .drop_column(col)
                        .to_owned(),
                )
                .await?;
        }

        Ok(())
    }
}

#[derive(DeriveIden)]
pub enum SleepEpochs {
    Table,
    Id,
    SleepCycleId,
    EpochStart,
    EpochEnd,
    Stage,
    Confidence,
    HrMean,
    HrStd,
    HrMin,
    HrMax,
    Rmssd,
    Sdnn,
    Pnn50,
    LfPower,
    HfPower,
    LfHfRatio,
    MotionActivityCount,
    MotionStillnessRatio,
    RespRate,
    FeatureBlob,
    ClassifierVersion,
}

#[derive(DeriveIden)]
pub enum UserBaselines {
    Table,
    Id,
    ComputedAt,
    WindowNights,
    RestingHr,
    SleepRmssdMedian,
    SleepRmssdP25,
    SleepRmssdP75,
    HfPowerMedian,
    LfHfRatioMedian,
    SleepDurationMeanHours,
    RespiratoryRateMean,
    RespiratoryRateStd,
    SkinTempMeanC,
    SkinTempStdC,
}

#[derive(DeriveIden, Clone, Copy)]
enum SleepCyclesNewCols {
    AwakeMinutes,
    LightMinutes,
    DeepMinutes,
    RemMinutes,
    SleepLatencyMinutes,
    WasoMinutes,
    SleepEfficiency,
    WakeEventCount,
    CycleCount,
    AvgRespiratoryRate,
    MinRespiratoryRate,
    MaxRespiratoryRate,
    SkinTempDeviationC,
    SleepNeedHours,
    SleepDebtHours,
    PerformanceScore,
    ClassifierVersion,
}
