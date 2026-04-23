use chrono::{Local, NaiveDateTime, TimeZone};
use openwhoop_entities::{packets, sleep_cycles};
use openwhoop_migration::{Migrator, MigratorTrait, OnConflict};
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, ConnectOptions, Database,
    DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Set,
};
use uuid::Uuid;

use openwhoop_algos::SleepCycle;
use openwhoop_codec::HistoryReading;

#[derive(Clone)]
pub struct DatabaseHandler {
    pub(crate) db: DatabaseConnection,
}

impl DatabaseHandler {
    pub fn connection(&self) -> &DatabaseConnection {
        &self.db
    }

    pub async fn new<C>(path: C) -> Self
    where
        C: Into<ConnectOptions>,
    {
        let db = Database::connect(path)
            .await
            .expect("Unable to connect to db");

        Migrator::up(&db, None)
            .await
            .expect("Error running migrations");

        Self { db }
    }

    pub async fn create_packet(
        &self,
        char: Uuid,
        data: Vec<u8>,
    ) -> anyhow::Result<openwhoop_entities::packets::Model> {
        let packet = openwhoop_entities::packets::ActiveModel {
            id: NotSet,
            uuid: Set(char),
            bytes: Set(data),
        };

        let packet = packet.insert(&self.db).await?;
        Ok(packet)
    }

    pub async fn create_reading(&self, reading: HistoryReading) -> anyhow::Result<()> {
        let time = timestamp_to_local(reading.unix)?;

        let sensor_json = reading
            .sensor_data
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?;

        let packet = openwhoop_entities::heart_rate::ActiveModel {
            id: NotSet,
            bpm: Set(i16::from(reading.bpm)),
            time: Set(time),
            rr_intervals: Set(rr_to_string(reading.rr)),
            activity: NotSet,
            stress: NotSet,
            spo2: NotSet,
            skin_temp: NotSet,
            imu_data: Set(Some(serde_json::to_value(reading.imu_data)?)),
            sensor_data: Set(sensor_json),
            synced: NotSet,
        };

        let _model = openwhoop_entities::heart_rate::Entity::insert(packet)
            .on_conflict(
                OnConflict::column(openwhoop_entities::heart_rate::Column::Time)
                    .update_column(openwhoop_entities::heart_rate::Column::Bpm)
                    .update_column(openwhoop_entities::heart_rate::Column::RrIntervals)
                    .update_column(openwhoop_entities::heart_rate::Column::SensorData)
                    .to_owned(),
            )
            .exec(&self.db)
            .await?;

        Ok(())
    }

    pub async fn create_readings(&self, readings: Vec<HistoryReading>) -> anyhow::Result<()> {
        if readings.is_empty() {
            return Ok(());
        }
        let payloads = readings
            .into_iter()
            .map(|r| {
                let time = timestamp_to_local(r.unix)?;
                let sensor_json = r
                    .sensor_data
                    .as_ref()
                    .map(serde_json::to_value)
                    .transpose()?;
                Ok(openwhoop_entities::heart_rate::ActiveModel {
                    id: NotSet,
                    bpm: Set(i16::from(r.bpm)),
                    time: Set(time),
                    rr_intervals: Set(rr_to_string(r.rr)),
                    activity: NotSet,
                    stress: NotSet,
                    spo2: NotSet,
                    skin_temp: NotSet,
                    imu_data: Set(Some(serde_json::to_value(r.imu_data)?)),
                    sensor_data: Set(sensor_json),
                    synced: NotSet,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        // SQLite limits to 999 SQL variables per statement.
        // heart_rate has 11 columns, so max 90 rows per batch.
        for chunk in payloads.chunks(90) {
            openwhoop_entities::heart_rate::Entity::insert_many(chunk.to_vec())
                .on_conflict(
                    OnConflict::column(openwhoop_entities::heart_rate::Column::Time)
                        .update_column(openwhoop_entities::heart_rate::Column::Bpm)
                        .update_column(openwhoop_entities::heart_rate::Column::RrIntervals)
                        .update_column(openwhoop_entities::heart_rate::Column::SensorData)
                        .to_owned(),
                )
                .exec(&self.db)
                .await?;
        }

        Ok(())
    }

    pub async fn get_packets(&self, id: i32) -> anyhow::Result<Vec<packets::Model>> {
        let stream = packets::Entity::find()
            .filter(packets::Column::Id.gt(id))
            .order_by_asc(packets::Column::Id)
            .limit(10_000)
            .all(&self.db)
            .await?;

        Ok(stream)
    }

    pub async fn get_latest_sleep(
        &self,
    ) -> anyhow::Result<Option<openwhoop_entities::sleep_cycles::Model>> {
        let sleep = sleep_cycles::Entity::find()
            .order_by_desc(sleep_cycles::Column::End)
            .one(&self.db)
            .await?;

        Ok(sleep)
    }

    pub async fn create_sleep(&self, sleep: SleepCycle) -> anyhow::Result<()> {
        // Look up any existing row first so we can detect whether this
        // call extends a previously-staged cycle. If bounds change, the
        // old epochs / stage minutes cover only part of the new window,
        // so we must invalidate staging — otherwise `stage_unclassified`
        // skips the row (it filters on `classifier_version IS NULL`) and
        // the History view keeps showing the truncated stage totals.
        let existing = sleep_cycles::Entity::find()
            .filter(sleep_cycles::Column::SleepId.eq(sleep.id))
            .one(&self.db)
            .await?;

        if let Some(row) = existing {
            let bounds_changed = row.start != sleep.start || row.end != sleep.end;
            let upd = sleep_cycles::ActiveModel {
                id: Set(row.id),
                start: Set(sleep.start),
                end: Set(sleep.end),
                min_bpm: Set(sleep.min_bpm.into()),
                max_bpm: Set(sleep.max_bpm.into()),
                avg_bpm: Set(sleep.avg_bpm.into()),
                min_hrv: Set(sleep.min_hrv.into()),
                max_hrv: Set(sleep.max_hrv.into()),
                avg_hrv: Set(sleep.avg_hrv.into()),
                ..Default::default()
            };
            upd.update(&self.db).await?;

            if bounds_changed {
                self.delete_sleep_epochs_for_cycle(row.id).await?;
                self.reset_cycle_staging_fields(row.id).await?;
            }
            return Ok(());
        }

        let model = sleep_cycles::ActiveModel {
            id: Set(Uuid::new_v4()),
            sleep_id: Set(sleep.id),
            start: Set(sleep.start),
            end: Set(sleep.end),
            min_bpm: Set(sleep.min_bpm.into()),
            max_bpm: Set(sleep.max_bpm.into()),
            avg_bpm: Set(sleep.avg_bpm.into()),
            min_hrv: Set(sleep.min_hrv.into()),
            max_hrv: Set(sleep.max_hrv.into()),
            avg_hrv: Set(sleep.avg_hrv.into()),
            score: Set(sleep.score.into()),
            synced: NotSet,
            ..Default::default()
        };
        sleep_cycles::Entity::insert(model).exec(&self.db).await?;

        Ok(())
    }
}

fn timestamp_to_local(unix: u64) -> anyhow::Result<NaiveDateTime> {
    let millis = i64::try_from(unix)?;
    let dt = Local
        .timestamp_millis_opt(millis)
        .single()
        .ok_or_else(|| anyhow::anyhow!("ambiguous or invalid unix timestamp: {}", millis))?;

    Ok(dt.naive_local())
}

fn rr_to_string(rr: Vec<u16>) -> String {
    rr.iter().map(u16::to_string).collect::<Vec<_>>().join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_and_get_packets() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        let uuid = Uuid::new_v4();
        let data = vec![0xAA, 0xBB, 0xCC];

        let packet = db.create_packet(uuid, data.clone()).await.unwrap();
        assert_eq!(packet.uuid, uuid);
        assert_eq!(packet.bytes, data);

        let packets = db.get_packets(0).await.unwrap();
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].uuid, uuid);
    }

    #[tokio::test]
    async fn create_reading_and_search_history() {
        let db = DatabaseHandler::new("sqlite::memory:").await;

        let reading = HistoryReading {
            unix: 1735689600000, // 2025-01-01 00:00:00 UTC in millis
            bpm: 72,
            rr: vec![833, 850],
            imu_data: vec![],
            sensor_data: None,
        };

        db.create_reading(reading).await.unwrap();

        let history = db
            .search_history(crate::SearchHistory::default())
            .await
            .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].bpm, 72);
        assert_eq!(history[0].rr, vec![833, 850]);
    }

    #[tokio::test]
    async fn create_readings_batch() {
        let db = DatabaseHandler::new("sqlite::memory:").await;

        let readings: Vec<HistoryReading> = (0..5)
            .map(|i| HistoryReading {
                unix: 1735689600000 + i * 1000,
                bpm: 70 + u8::try_from(i).unwrap(),
                rr: vec![850],
                imu_data: vec![],
                sensor_data: None,
            })
            .collect();

        db.create_readings(readings).await.unwrap();

        let history = db
            .search_history(crate::SearchHistory::default())
            .await
            .unwrap();
        assert_eq!(history.len(), 5);
    }

    #[tokio::test]
    async fn create_and_get_sleep() {
        let db = DatabaseHandler::new("sqlite::memory:").await;

        let start = chrono::NaiveDate::from_ymd_opt(2025, 1, 1)
            .unwrap()
            .and_hms_opt(22, 0, 0)
            .unwrap();
        let end = chrono::NaiveDate::from_ymd_opt(2025, 1, 2)
            .unwrap()
            .and_hms_opt(6, 0, 0)
            .unwrap();

        let sleep = SleepCycle {
            id: end.date(),
            start,
            end,
            min_bpm: 50,
            max_bpm: 70,
            avg_bpm: 60,
            min_hrv: 30,
            max_hrv: 80,
            avg_hrv: 55,
            score: 100.0,
        };

        db.create_sleep(sleep).await.unwrap();

        let latest = db.get_latest_sleep().await.unwrap();
        assert!(latest.is_some());
        let latest = latest.unwrap();
        assert_eq!(latest.min_bpm, 50);
        assert_eq!(latest.avg_bpm, 60);
    }

    #[tokio::test]
    async fn extending_sleep_bounds_invalidates_staging() {
        use openwhoop_entities::sleep_epochs;

        let db = DatabaseHandler::new("sqlite::memory:").await;

        let start = chrono::NaiveDate::from_ymd_opt(2026, 4, 22)
            .unwrap()
            .and_hms_opt(22, 0, 0)
            .unwrap();
        let initial_end = start + chrono::TimeDelta::hours(2);
        let extended_end = start + chrono::TimeDelta::hours(8);

        let sleep = SleepCycle {
            id: initial_end.date(),
            start,
            end: initial_end,
            min_bpm: 50,
            max_bpm: 70,
            avg_bpm: 60,
            min_hrv: 30,
            max_hrv: 80,
            avg_hrv: 55,
            score: 85.0,
        };
        db.create_sleep(sleep).await.unwrap();

        // Simulate a staging run: populate classifier_version, stage
        // totals, and a couple of epoch rows.
        let cycle_id = sleep_cycles::Entity::find()
            .one(&db.db)
            .await
            .unwrap()
            .unwrap()
            .id;
        sleep_cycles::ActiveModel {
            id: sea_orm::ActiveValue::Unchanged(cycle_id),
            classifier_version: Set(Some("rule-v2".to_string())),
            awake_minutes: Set(Some(5.0)),
            light_minutes: Set(Some(80.0)),
            deep_minutes: Set(Some(15.0)),
            rem_minutes: Set(Some(20.0)),
            performance_score: Set(Some(72.0)),
            ..Default::default()
        }
        .update(&db.db)
        .await
        .unwrap();
        sleep_epochs::ActiveModel {
            id: NotSet,
            sleep_cycle_id: Set(cycle_id),
            epoch_start: Set(start),
            epoch_end: Set(start + chrono::TimeDelta::seconds(30)),
            stage: Set("Light".to_string()),
            classifier_version: Set("rule-v2".to_string()),
            ..Default::default()
        }
        .insert(&db.db)
        .await
        .unwrap();

        // Re-insert with the same sleep_id but an extended end. This
        // mirrors what happens when a later sync pulls in the rest of
        // the night.
        let extended = SleepCycle {
            id: initial_end.date(),
            start,
            end: extended_end,
            min_bpm: 48,
            max_bpm: 75,
            avg_bpm: 61,
            min_hrv: 30,
            max_hrv: 80,
            avg_hrv: 55,
            score: 85.0,
        };
        db.create_sleep(extended).await.unwrap();

        let reloaded = sleep_cycles::Entity::find_by_id(cycle_id)
            .one(&db.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reloaded.end, extended_end);
        assert_eq!(reloaded.classifier_version, None, "staging must be invalidated when bounds change");
        assert_eq!(reloaded.awake_minutes, None);
        assert_eq!(reloaded.light_minutes, None);
        assert_eq!(reloaded.deep_minutes, None);
        assert_eq!(reloaded.rem_minutes, None);
        assert_eq!(reloaded.performance_score, None);

        let remaining_epochs = sleep_epochs::Entity::find()
            .filter(sleep_epochs::Column::SleepCycleId.eq(cycle_id))
            .all(&db.db)
            .await
            .unwrap();
        assert!(remaining_epochs.is_empty(), "old epochs must be wiped");
    }

    #[tokio::test]
    async fn idempotent_sleep_insert_preserves_staging() {
        let db = DatabaseHandler::new("sqlite::memory:").await;

        let start = chrono::NaiveDate::from_ymd_opt(2026, 4, 22)
            .unwrap()
            .and_hms_opt(22, 0, 0)
            .unwrap();
        let end = start + chrono::TimeDelta::hours(8);

        let sleep = SleepCycle {
            id: end.date(),
            start,
            end,
            min_bpm: 50,
            max_bpm: 70,
            avg_bpm: 60,
            min_hrv: 30,
            max_hrv: 80,
            avg_hrv: 55,
            score: 85.0,
        };
        db.create_sleep(sleep.clone()).await.unwrap();

        let cycle_id = sleep_cycles::Entity::find()
            .one(&db.db)
            .await
            .unwrap()
            .unwrap()
            .id;
        sleep_cycles::ActiveModel {
            id: sea_orm::ActiveValue::Unchanged(cycle_id),
            classifier_version: Set(Some("rule-v2".to_string())),
            light_minutes: Set(Some(300.0)),
            ..Default::default()
        }
        .update(&db.db)
        .await
        .unwrap();

        // Re-insert the same cycle (unchanged bounds). Staging fields
        // should be preserved — otherwise every sync would redo work.
        db.create_sleep(sleep).await.unwrap();

        let reloaded = sleep_cycles::Entity::find_by_id(cycle_id)
            .one(&db.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reloaded.classifier_version.as_deref(), Some("rule-v2"));
        assert_eq!(reloaded.light_minutes, Some(300.0));
    }

    #[tokio::test]
    async fn upsert_reading_on_conflict() {
        let db = DatabaseHandler::new("sqlite::memory:").await;

        let reading = HistoryReading {
            unix: 1735689600000,
            bpm: 72,
            rr: vec![833],
            imu_data: vec![],
            sensor_data: None,
        };
        db.create_reading(reading).await.unwrap();

        // Insert again with different bpm - should upsert
        let reading2 = HistoryReading {
            unix: 1735689600000,
            bpm: 80,
            rr: vec![750],
            imu_data: vec![],
            sensor_data: None,
        };
        db.create_reading(reading2).await.unwrap();

        let history = db
            .search_history(crate::SearchHistory::default())
            .await
            .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].bpm, 80);
    }
}
