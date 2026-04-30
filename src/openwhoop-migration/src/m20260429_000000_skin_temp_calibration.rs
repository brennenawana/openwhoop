use sea_orm_migration::prelude::*;

/// Adds per-user skin-temp calibration columns to `user_baselines`.
///
/// **Why**: the strap thermistor reports raw ADC values, not °C. The
/// previous global conversion factor of 0.04 (in `SkinTempCalculator`)
/// produced physiologically impossible readings (e.g. 38.1°C / 100.6°F
/// at the wrist while core body temp was 36.9°C). Different straps have
/// different thermistor offsets; a one-factor-fits-all conversion is
/// fundamentally wrong.
///
/// **Per-user fix**: `skin_temp_raw_median` anchors the user's median
/// stable-wear raw reading to a population-typical 32°C resting wrist
/// skin temp. `T(°C) = (raw - raw_median) × slope + 32`, where slope is
/// a thermistor characteristic assumed constant across straps.
///
/// `skin_temp_calibration_sample_count` lets the UI gate behavior: show
/// calibrated absolute temps once we have enough samples, fall back to
/// deviation-only display while calibrating.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(UserBaselines::Table)
                    .add_column(
                        ColumnDef::new(UserBaselines::SkinTempRawMedian)
                            .double()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(UserBaselines::Table)
                    .add_column(
                        ColumnDef::new(UserBaselines::SkinTempCalibrationSampleCount)
                            .integer()
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
                    .table(UserBaselines::Table)
                    .drop_column(UserBaselines::SkinTempCalibrationSampleCount)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(UserBaselines::Table)
                    .drop_column(UserBaselines::SkinTempRawMedian)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum UserBaselines {
    Table,
    SkinTempRawMedian,
    SkinTempCalibrationSampleCount,
}
