use anyhow::{anyhow, bail};
use btleplug::{
    api::{Central, CharPropFlags, Characteristic, Peripheral as _, ValueNotification, WriteType},
    platform::{Adapter, Peripheral},
};
use futures::StreamExt;
use openwhoop_codec::{WhoopData, WhoopPacket, constants::WhoopGeneration};
use openwhoop_entities::packets::Model;
use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::time::{sleep, timeout};
use uuid::Uuid;

use crate::{db::DatabaseHandler, openwhoop::OpenWhoop};

#[path = "sync/gen5.rs"]
mod gen5_sync;

use self::gen5_sync::Gen5HistorySync;

#[derive(Debug, Clone, Copy)]
pub struct HistorySyncConfig {
    pub overall_timeout: Option<Duration>,
    pub idle_timeout: Duration,
}

impl Default for HistorySyncConfig {
    fn default() -> Self {
        Self {
            overall_timeout: None,
            idle_timeout: Duration::from_secs(20),
        }
    }
}

impl HistorySyncConfig {
    pub fn from_secs(overall_timeout_secs: u64, idle_timeout_secs: u64) -> Self {
        Self {
            overall_timeout: (overall_timeout_secs > 0)
                .then(|| Duration::from_secs(overall_timeout_secs)),
            idle_timeout: Duration::from_secs(idle_timeout_secs.max(1)),
        }
    }
}

pub struct WhoopDevice {
    peripheral: Peripheral,
    whoop: OpenWhoop,
    debug_packets: bool,
    adapter: Adapter,
    generation: WhoopGeneration,
    seq: u8,
}

impl WhoopDevice {
    pub fn new(
        peripheral: Peripheral,
        adapter: Adapter,
        db: DatabaseHandler,
        debug_packets: bool,
        generation: WhoopGeneration,
    ) -> Self {
        Self {
            peripheral,
            whoop: OpenWhoop::new(db, generation),
            debug_packets,
            adapter,
            generation,
            seq: 0,
        }
    }

    pub async fn connect(&mut self) -> anyhow::Result<()> {
        self.peripheral.connect().await?;
        let _ = self.adapter.stop_scan().await;
        self.peripheral.discover_services().await?;
        self.whoop.packet = None;
        self.seq = 0;
        Ok(())
    }

    pub async fn is_connected(&mut self) -> anyhow::Result<bool> {
        let is_connected = self.peripheral.is_connected().await?;
        Ok(is_connected)
    }

    fn create_char(&self, characteristic: Uuid) -> Characteristic {
        Characteristic {
            uuid: characteristic,
            service_uuid: self.generation.service(),
            properties: CharPropFlags::empty(),
            descriptors: BTreeSet::new(),
        }
    }

    async fn subscribe(&self, char: Uuid) -> anyhow::Result<()> {
        self.peripheral.subscribe(&self.create_char(char)).await?;
        Ok(())
    }

    pub async fn initialize(&mut self) -> anyhow::Result<()> {
        let generation = self.generation;
        self.subscribe(generation.data_from_strap()).await?;
        self.subscribe(generation.cmd_from_strap()).await?;
        self.subscribe(generation.events_from_strap()).await?;
        self.subscribe(generation.memfault()).await?;
        Ok(())
    }

    pub async fn send_command(&mut self, packet: WhoopPacket) -> anyhow::Result<()> {
        self.send_command_with_seq(packet).await.map(|_| ())
    }

    async fn send_command_with_seq(&mut self, packet: WhoopPacket) -> anyhow::Result<u8> {
        let seq = self.seq;
        let packet = packet.with_seq(seq);
        self.seq = self.seq.wrapping_add(1);
        let bytes = match self.generation {
            WhoopGeneration::Gen4 => packet.framed_packet()?,
            WhoopGeneration::Gen5 => packet.framed_packet_maverick()?,
            WhoopGeneration::Placeholder => {
                return Err(anyhow!(
                    "WhoopGeneration::Placeholder cannot be used for BLE command transport"
                ));
            }
        };
        self.peripheral
            .write(
                &self.create_char(self.generation.cmd_to_strap()),
                &bytes,
                WriteType::WithoutResponse,
            )
            .await?;
        Ok(seq)
    }

    pub async fn sync_history(
        &mut self,
        should_exit: Arc<AtomicBool>,
        config: HistorySyncConfig,
    ) -> anyhow::Result<()> {
        // Race the generation-specific sync loop against a connection-health
        // monitor. btleplug's notification stream can stall indefinitely after
        // a BLE disconnect without producing an error or stream-end, so the
        // monitor is the backstop that forces a fast failure in that case.
        let peripheral = self.peripheral.clone();
        let monitor = async move { connection_monitor(peripheral).await };
        tokio::pin!(monitor);

        let generation = self.generation;
        let sync = async {
            match generation {
                WhoopGeneration::Gen4 => self.sync_history_gen4(should_exit, config).await,
                WhoopGeneration::Gen5 => self.sync_history_gen5(should_exit, config).await,
                WhoopGeneration::Placeholder => Err(anyhow!(
                    "WhoopGeneration::Placeholder cannot be used for history sync"
                )),
            }
        };

        tokio::select! {
            result = sync => result,
            err = &mut monitor => Err(err),
        }
    }

    async fn sync_history_gen4(
        &mut self,
        should_exit: Arc<AtomicBool>,
        config: HistorySyncConfig,
    ) -> anyhow::Result<()> {
        let mut notifications = self.peripheral.notifications().await?;

        self.send_command(WhoopPacket::hello_harvard()).await?;
        self.send_command(WhoopPacket::set_time()?).await?;
        self.send_command(WhoopPacket::get_name()).await?;
        self.send_command(WhoopPacket::enter_high_freq_sync())
            .await?;
        self.send_command(WhoopPacket::history_start()).await?;

        let idle_timeout = config.idle_timeout;
        loop {
            if should_exit.load(Ordering::SeqCst) {
                break;
            }
            if self.whoop.history_complete {
                info!("History download complete");
                break;
            }
            tokio::select! {
                _ = sleep(idle_timeout) => {
                    bail!(
                        "No Gen4 history notifications for {}s (likely disconnect)",
                        idle_timeout.as_secs()
                    );
                }
                Some(notification) = notifications.next() => {
                    self.handle_sync_notification(notification).await?;
                }
            }
        }
        Ok(())
    }

    pub async fn sync_history_gen5(
        &mut self,
        should_exit: Arc<AtomicBool>,
        config: HistorySyncConfig,
    ) -> anyhow::Result<()> {
        let notifications = self.peripheral.notifications().await?;
        Gen5HistorySync::new(self, should_exit, notifications, config)
            .start()
            .await
    }

    async fn handle_sync_notification(
        &mut self,
        notification: ValueNotification,
    ) -> anyhow::Result<()> {
        let packet = self.notification_to_model(notification).await?;
        if let Some(reply) = self.whoop.handle_packet(packet).await? {
            self.send_command(reply).await?;
        }
        Ok(())
    }

    async fn notification_to_model(
        &self,
        notification: ValueNotification,
    ) -> anyhow::Result<Model> {
        match self.debug_packets {
            true => self.whoop.store_packet(notification).await,
            false => Ok(Model {
                id: 0,
                uuid: notification.uuid,
                bytes: notification.value,
            }),
        }
    }

/// Ring the device. Dispatches to the correct command for the generation:
    /// - Gen4: RunAlarm (cmd=68)
    /// - Maverick: RunHapticPatternMaverick / WSBLE_CMD_HAPTICS_RUN_NTF (cmd=19, revision=0x01)
    pub async fn ring_alarm(&mut self) -> anyhow::Result<()> {
        let packet = match self.generation {
            WhoopGeneration::Gen4 => WhoopPacket::run_alarm_now(),
            WhoopGeneration::Gen5 => WhoopPacket::run_haptic_pattern_gen5(),
            WhoopGeneration::Placeholder => {
                return Err(anyhow!("WhoopGeneration::Placeholder cannot ring a device"));
            }
        };
        self.send_command(packet).await
    }

    /// Stream realtime heart rate until Ctrl-C or timeout.
    pub async fn stream_hr(&mut self, should_exit: Arc<AtomicBool>) -> anyhow::Result<()> {
        let generation = self.generation;
        self.subscribe(generation.data_from_strap()).await?;
        self.subscribe(generation.cmd_from_strap()).await?;

        let mut notifications = self.peripheral.notifications().await?;
        self.send_command(WhoopPacket::toggle_realtime_hr(true))
            .await?;

        loop {
            if should_exit.load(Ordering::SeqCst) {
                break;
            }
            let notification = notifications.next();
            let sleep_ = sleep(Duration::from_secs(30));

            tokio::select! {
                _ = sleep_ => {
                    warn!("Timed out waiting for HR data");
                    break;
                },
                Some(notification) = notification => {
                    let bytes = notification.value;
                    let packet = match generation {
                        WhoopGeneration::Gen4 => WhoopPacket::from_data(bytes),
                        WhoopGeneration::Gen5 => WhoopPacket::from_data_maverick(bytes),
                        WhoopGeneration::Placeholder => {
                            return Err(anyhow!(
                                "WhoopGeneration::Placeholder cannot parse realtime HR packets"
                            ));
                        }
                    };
                    let packet = match packet {
                        Ok(p) => p,
                        Err(_) => continue,
                    };
                    let decoded = match generation {
                        WhoopGeneration::Gen4 => WhoopData::from_packet_gen4(packet),
                        WhoopGeneration::Gen5 => WhoopData::from_packet_gen5(packet),
                        WhoopGeneration::Placeholder => {
                            return Err(anyhow!(
                                "WhoopGeneration::Placeholder cannot decode realtime HR packets"
                            ));
                        }
                    };
                    match decoded {
                        Ok(WhoopData::RealtimeHr { unix, bpm }) => {
                            let time = chrono::DateTime::from_timestamp(i64::from(unix), 0)
                                .map(|t| t.with_timezone(&chrono::Local).format("%H:%M:%S").to_string())
                                .unwrap_or_else(|| unix.to_string());
                            println!("{} HR: {} bpm", time, bpm);
                        }
                        Ok(WhoopData::Event { .. } | WhoopData::UnknownEvent { .. }) => {}
                        _ => {}
                    }
                }
            }
        }

        if let Ok(true) = self.peripheral.is_connected().await {
            self.send_command(WhoopPacket::toggle_realtime_hr(false))
                .await?;
        }
        Ok(())
    }

    pub async fn get_version(&mut self) -> anyhow::Result<()> {
        let mut notifications = self.peripheral.notifications().await?;
        self.send_command(WhoopPacket::version()).await?;

        let timeout_duration = Duration::from_secs(5);
        match timeout(timeout_duration, notifications.next()).await {
            Ok(Some(notification)) => {
                let packet = match self.generation {
                    WhoopGeneration::Gen4 => WhoopPacket::from_data(notification.value)?,
                    WhoopGeneration::Gen5 => WhoopPacket::from_data_maverick(notification.value)?,
                    WhoopGeneration::Placeholder => {
                        return Err(anyhow!(
                            "WhoopGeneration::Placeholder cannot parse version packets"
                        ));
                    }
                };
                let data = match self.generation {
                    WhoopGeneration::Gen4 => WhoopData::from_packet_gen4(packet)?,
                    WhoopGeneration::Gen5 => WhoopData::from_packet_gen5(packet)?,
                    WhoopGeneration::Placeholder => {
                        return Err(anyhow!(
                            "WhoopGeneration::Placeholder cannot decode version packets"
                        ));
                    }
                };
                if let WhoopData::VersionInfo { harvard, boylston } = data {
                    info!("version harvard {} boylston {}", harvard, boylston);
                }
                Ok(())
            }
            Ok(None) => Err(anyhow!("stream ended unexpectedly")),
            Err(_) => Err(anyhow!("timed out waiting for version notification")),
        }
    }

    pub async fn get_alarm(&mut self) -> anyhow::Result<WhoopData> {
        self.subscribe(self.generation.cmd_from_strap()).await?;
        let mut notifications = self.peripheral.notifications().await?;
        self.send_command(WhoopPacket::get_alarm_time()).await?;

        let timeout_duration = Duration::from_secs(30);
        match timeout(timeout_duration, notifications.next()).await {
            Ok(Some(notification)) => {
                let packet = WhoopPacket::from_data(notification.value)?;
                let data = WhoopData::from_packet(packet, self.generation)?;
                Ok(data)
            }
            Ok(None) => Err(anyhow!("stream ended unexpectedly")),
            Err(_) => Err(anyhow!("timed out waiting for alarm notification")),
        }
    }

    pub async fn get_battery(&mut self) -> anyhow::Result<f32> {
        let cmd_uuid = self.generation.cmd_from_strap();
        self.subscribe(cmd_uuid).await?;
        let mut notifications = self.peripheral.notifications().await?;
        self.send_command(WhoopPacket::get_battery_level()).await?;

        // The notification stream can contain stale command responses from
        // initialize() (hello_harvard, get_name, etc). Skip anything that
        // isn't the battery response we just asked for.
        let fut = async {
            while let Some(notif) = notifications.next().await {
                if notif.uuid != cmd_uuid {
                    continue;
                }
                let Ok(packet) = WhoopPacket::from_data(notif.value) else {
                    continue;
                };
                let Ok(data) = WhoopData::from_packet(packet, self.generation) else {
                    continue;
                };
                if let WhoopData::BatteryLevel { percent } = data {
                    return Some(percent);
                }
            }
            None
        };

        match timeout(Duration::from_secs(10), fut).await {
            Ok(Some(percent)) => Ok(percent),
            Ok(None) => Err(anyhow!("notification stream ended before battery response")),
            Err(_) => Err(anyhow!("timed out waiting for battery response")),
        }
    }

    /// Query the device for charging + wrist-worn state via GetHelloHarvard.
    pub async fn get_hello(&mut self) -> anyhow::Result<(bool, bool)> {
        let cmd_uuid = self.generation.cmd_from_strap();
        self.subscribe(cmd_uuid).await?;
        let mut notifications = self.peripheral.notifications().await?;
        self.send_command(WhoopPacket::hello_harvard()).await?;

        let fut = async {
            while let Some(notif) = notifications.next().await {
                if notif.uuid != cmd_uuid {
                    continue;
                }
                let Ok(packet) = WhoopPacket::from_data(notif.value) else {
                    continue;
                };
                let Ok(data) = WhoopData::from_packet(packet, self.generation) else {
                    continue;
                };
                if let WhoopData::HelloHarvard { charging, is_worn } = data {
                    return Some((charging, is_worn));
                }
            }
            None
        };

        match timeout(Duration::from_secs(10), fut).await {
            Ok(Some(pair)) => Ok(pair),
            Ok(None) => Err(anyhow!("notification stream ended before hello response")),
            Err(_) => Err(anyhow!("timed out waiting for hello response")),
        }
    }
}

/// Polls `peripheral.is_connected()` on a fixed cadence and resolves with an
/// error when the peripheral is reported as disconnected — or when btleplug's
/// own is_connected call hangs past our per-poll timeout. Used as the losing
/// branch of a `tokio::select!` inside `sync_history` so BLE disconnects abort
/// the sync within a few seconds instead of hanging on a silent notification
/// stream.
async fn connection_monitor(peripheral: Peripheral) -> anyhow::Error {
    const POLL_INTERVAL: Duration = Duration::from_secs(3);
    const PER_POLL_TIMEOUT: Duration = Duration::from_secs(2);

    // Skip the first immediate tick so we don't race the initial connect.
    let mut ticker = tokio::time::interval(POLL_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await;

    loop {
        ticker.tick().await;
        match timeout(PER_POLL_TIMEOUT, peripheral.is_connected()).await {
            Ok(Ok(true)) => continue,
            Ok(Ok(false)) => return anyhow!("peripheral disconnected during history sync"),
            Ok(Err(err)) => {
                return anyhow!("is_connected check failed during history sync: {err}");
            }
            Err(_) => {
                return anyhow!(
                    "is_connected check hung for {}s — treating peripheral as gone",
                    PER_POLL_TIMEOUT.as_secs()
                );
            }
        }
    }
}
