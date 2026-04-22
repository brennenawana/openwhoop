#[macro_use]
extern crate log;

use std::{
    env, fs, io,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, anyhow};
use btleplug::{
    api::{Central, Manager as _, Peripheral as _, ScanFilter},
    platform::{Adapter, Manager, Peripheral},
};
#[cfg(target_os = "linux")]
use btleplug::api::BDAddr;
use chrono::{DateTime, Local, NaiveDateTime, NaiveTime, TimeDelta, Utc};
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};
use dotenv::dotenv;
use openwhoop::api;
use openwhoop::{
    HistorySyncConfig, OpenWhoop, WhoopDevice,
    algo::{ExerciseMetrics, SleepConsistencyAnalyzer},
    db::DatabaseHandler,
    types::activities::{ActivityType, SearchActivityPeriods},
};
use openwhoop_codec::{
    WhoopPacket,
    constants::{ALL_WHOOP_SERVICES, WhoopGeneration},
};
use openwhoop_entities::packets;
use tokio::time::sleep;

const OPENWHOOP_CONFIG_DIR: &str = ".openwhoop";

#[cfg(target_os = "linux")]
pub type DeviceId = BDAddr;

#[cfg(target_os = "macos")]
pub type DeviceId = String;

#[derive(Parser)]
pub struct OpenWhoopCli {
    #[arg(env, long)]
    pub debug_packets: bool,
    #[arg(env, long)]
    pub database_url: Option<String>,
    #[cfg(target_os = "linux")]
    #[arg(env, long)]
    pub ble_interface: Option<String>,
    #[clap(subcommand)]
    pub subcommand: OpenWhoopCommand,
}

#[derive(Subcommand)]
pub enum OpenWhoopCommand {
    ///
    /// Scan for Whoop devices
    ///
    Scan,
    ///
    /// Set the default Whoop device in ~/.openwhoop/.env
    ///
    SetWhoop { whoop: DeviceId },
    ///
    /// Set the default remote database URL in ~/.openwhoop/.env
    ///
    SetRemote { remote: String },
    ///
    /// Download history data from whoop devices
    ///
    DownloadHistory {
        #[arg(long, env)]
        whoop: DeviceId,
        #[arg(
            long,
            env = "OPENWHOOP_HISTORY_TIMEOUT_SECS",
            default_value_t = 0,
            help = "Overall Gen5 history timeout in seconds; 0 disables the wall-clock cap"
        )]
        history_timeout_secs: u64,
        #[arg(
            long,
            env = "OPENWHOOP_HISTORY_IDLE_TIMEOUT_SECS",
            default_value_t = 20,
            help = "Fail the transfer if no Gen5 history packets arrive for this many seconds"
        )]
        history_idle_timeout_secs: u64,
    },
    ///
    /// Reruns the packet processing on stored packets
    /// This is used after new more of packets get handled
    ///
    ReRun,
    ///
    /// Detects sleeps and exercises
    ///
    DetectEvents,
    ///
    /// Print sleep statistics for all time and last week
    ///
    SleepStats,
    ///
    /// Print activity statistics for all time and last week
    ///
    ExerciseStats,
    ///
    /// Calculate stress for historical data
    ///
    CalculateStress,
    ///
    /// Calculate SpO2 from raw sensor data
    ///
    CalculateSpo2,
    ///
    /// Calculate skin temperature from raw sensor data
    ///
    CalculateSkinTemp,
    ///
    /// Set alarm
    ///
    SetAlarm {
        #[arg(long, env)]
        whoop: DeviceId,
        alarm_time: AlarmTime,
    },
    ///
    /// Stream realtime heart rate
    ///
    StreamHr {
        #[arg(long, env)]
        whoop: DeviceId,
    },
    ///
    /// Ring the alarm immediately
    ///
    RingAlarm {
        #[arg(long, env)]
        whoop: DeviceId,
    },
    ///
    /// Get current alarm setting from device
    ///
    GetAlarm {
        #[arg(long, env)]
        whoop: DeviceId,
    },
    ///
    /// Copy packets from one database into another
    ///
    Merge { from: String },
    Restart {
        #[arg(long, env)]
        whoop: DeviceId,
    },
    ///
    /// Erase all history data from the device
    ///
    Erase {
        #[arg(long, env)]
        whoop: DeviceId,
    },
    ///
    /// Get device firmware version info
    ///
    Version {
        #[arg(long, env)]
        whoop: DeviceId,
    },
    ///
    /// Generate Shell completions
    ///
    Completions { shell: Shell },
    ///
    /// Enable IMU data
    ///
    EnableImu {
        #[arg(long, env)]
        whoop: DeviceId,
    },
    ///
    /// Sync data between local and remote database
    ///
    Sync {
        #[arg(long, env)]
        remote: String,
    },
    ///
    /// Download firmware from WHOOP API
    ///
    DownloadFirmware {
        #[arg(long, env = "WHOOP_EMAIL")]
        email: String,
        #[arg(long, env = "WHOOP_PASSWORD")]
        password: String,
        /// Device family for firmware lookup.
        /// Supported: HARVARD (Whoop 4), PUFFIN, MAVERICK/WHOOP5 (Whoop 5.0).
        #[arg(long, default_value = "HARVARD")]
        device_name: String,
        /// MAXIM target version (HARVARD/PUFFIN).
        #[arg(long, default_value = "41.16.5.0")]
        maxim: String,
        /// NORDIC target version (HARVARD/PUFFIN).
        #[arg(long, default_value = "17.2.2.0")]
        nordic: String,
        /// RUGGLES target version (PUFFIN only).
        #[arg(long, default_value = "1.0.0.0")]
        ruggles: String,
        /// PEARL target version (PUFFIN only).
        #[arg(long, default_value = "1.0.0.0")]
        pearl: String,
        /// MAVERICK target version (WHOOP 5.0).
        #[arg(long, default_value = "50.36.1.0")]
        maverick: String,
        #[arg(long, default_value = "./firmware")]
        output_dir: String,
    },
    ///
    /// Wipe and re-run sleep staging for every cycle whose start falls
    /// in [--from, --to]. --to defaults to today. Essential for
    /// iterating on classifier thresholds.
    ///
    ReclassifySleep {
        #[arg(long)]
        from: String,
        #[arg(long)]
        to: Option<String>,
        #[arg(long, default_value = "rule-v2")]
        classifier: String,
    },
    ///
    /// Write a dev note into the dev_notes table. The dashboard
    /// (see docs/DEV_DASHBOARD_CONCEPT.md) surfaces these as an
    /// inbox for the dev.
    ///
    Note {
        /// Title (required, positional). Keep it short and specific.
        title: String,
        /// note | question | experiment | diff | status. Default: note.
        #[arg(long, default_value = "note")]
        kind: String,
        /// Markdown body. Can be multi-paragraph.
        #[arg(long)]
        body: Option<String>,
        /// Feature scope for filtering in the dashboard. Examples:
        /// "sleep_staging", "quick_wins", "classifier", "wear_tracking".
        #[arg(long)]
        feature: Option<String>,
        /// Git SHA this note relates to. If omitted, auto-resolves
        /// to current HEAD via `git rev-parse`.
        #[arg(long)]
        commit: Option<String>,
        /// Who authored the note. Default "agent".
        #[arg(long, default_value = "agent")]
        author: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    load_cli_env();

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .filter_module("sqlx::query", log::LevelFilter::Off)
        .filter_module("sea_orm_migration::migrator", log::LevelFilter::Off)
        .filter_module("bluez_async", log::LevelFilter::Off)
        .filter_module("sqlx::postgres::notice", log::LevelFilter::Off)
        .init();

    OpenWhoopCli::parse().run().await
}

fn load_cli_env() {
    if cfg!(debug_assertions) {
        if let Err(error) = dotenv() {
            println!("{}", error);
        }
        return;
    }

    let Some(env_path) = openwhoop_env_path() else {
        return;
    };
    if env_path.is_file() {
        if let Err(error) = dotenv::from_path(&env_path) {
            println!("{}", error);
        }
    }
}

fn openwhoop_config_dir_from(home: impl Into<PathBuf>) -> PathBuf {
    home.into().join(OPENWHOOP_CONFIG_DIR)
}

fn openwhoop_config_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(openwhoop_config_dir_from)
}

fn openwhoop_env_path_from(home: impl Into<PathBuf>) -> PathBuf {
    openwhoop_config_dir_from(home).join(".env")
}

fn openwhoop_env_path() -> Option<PathBuf> {
    env::var_os("HOME").map(openwhoop_env_path_from)
}

fn format_dotenv_value(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':'))
    {
        value.to_string()
    } else {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

fn upsert_dotenv_value(contents: &str, key: &str, value: &str) -> String {
    let assignment = format!("{key}={}", format_dotenv_value(value));
    let mut replaced = false;
    let mut lines = Vec::new();

    for line in contents.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with(&format!("{key}=")) || trimmed.starts_with(&format!("export {key}="))
        {
            if !replaced {
                lines.push(assignment.clone());
                replaced = true;
            }
            continue;
        }
        lines.push(line.to_string());
    }

    if !replaced {
        lines.push(assignment);
    }

    let mut updated = lines.join("\n");
    updated.push('\n');
    updated
}

fn write_openwhoop_env_value(env_path: &Path, key: &str, value: &str) -> anyhow::Result<()> {
    if let Some(parent) = env_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let contents = match fs::read_to_string(env_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", env_path.display()));
        }
    };

    fs::write(env_path, upsert_dotenv_value(&contents, key, value))
        .with_context(|| format!("failed to write {}", env_path.display()))
}

fn set_default_whoop(whoop: &DeviceId) -> anyhow::Result<PathBuf> {
    let Some(env_path) = openwhoop_env_path() else {
        anyhow::bail!("HOME is unavailable");
    };

    write_openwhoop_env_value(&env_path, "WHOOP", &whoop.to_string())?;
    Ok(env_path)
}

fn set_default_remote(remote: &str) -> anyhow::Result<PathBuf> {
    let Some(env_path) = openwhoop_env_path() else {
        anyhow::bail!("HOME is unavailable");
    };

    write_openwhoop_env_value(&env_path, "REMOTE", remote)?;
    Ok(env_path)
}

fn default_sqlite_database_url(config_dir: &Path) -> String {
    format!(
        "sqlite://{}?mode=rwc",
        config_dir.join("db.sqlite").display()
    )
}

fn default_database_url() -> anyhow::Result<Option<String>> {
    let Some(config_dir) = openwhoop_config_dir() else {
        return Ok(None);
    };

    fs::create_dir_all(&config_dir)
        .with_context(|| format!("failed to create {}", config_dir.display()))?;

    Ok(Some(default_sqlite_database_url(&config_dir)))
}

fn resolve_database_url(database_url: Option<String>) -> anyhow::Result<String> {
    if let Some(database_url) = database_url {
        return Ok(database_url);
    }

    if cfg!(debug_assertions) {
        anyhow::bail!("DATABASE_URL is not set");
    }

    default_database_url()?
        .ok_or_else(|| anyhow!("DATABASE_URL is not set and HOME is unavailable"))
}

async fn download_firmware(
    email: &str,
    password: &str,
    device_name: &str,
    maxim: &str,
    nordic: &str,
    ruggles: &str,
    pearl: &str,
    maverick: &str,
    output_dir: &str,
) -> anyhow::Result<()> {
    info!("authenticating...");
    let client = api::WhoopApiClient::sign_in(email, password).await?;

    let normalized_device = device_name.trim().to_ascii_uppercase();
    let (api_device_name, chip_names): (&str, Vec<&str>) = match normalized_device.as_str() {
        "HARVARD" => ("HARVARD", vec!["MAXIM", "NORDIC"]),
        "PUFFIN" => ("PUFFIN", vec!["MAXIM", "NORDIC", "RUGGLES", "PEARL"]),
        "MAVERICK" | "MAVERIC" | "WHOOP5" | "WHOOP_5" | "WHOOP 5" | "WHOOP5.0" | "WHOOP_5.0"
        | "WHOOP 5.0" | "WHOOP-5" | "WHOOP-5.0" => ("MAVERICK", vec!["AMBIQ"]),
        other => anyhow::bail!("unknown device family: {other}"),
    };

    let target_versions: std::collections::HashMap<&str, &str> = [
        ("MAXIM", maxim),
        ("NORDIC", nordic),
        ("RUGGLES", ruggles),
        ("PEARL", pearl),
        ("AMBIQ", maverick),
    ]
    .into_iter()
    .collect();

    let current: Vec<api::ChipFirmware> = chip_names
        .iter()
        .map(|c| api::ChipFirmware {
            chip_name: c.to_string(),
            version: "1.0.0.0".into(),
        })
        .collect();

    let upgrade: Vec<api::ChipFirmware> = chip_names
        .iter()
        .map(|c| api::ChipFirmware {
            chip_name: c.to_string(),
            version: target_versions.get(c).unwrap_or(&"1.0.0.0").to_string(),
        })
        .collect();

    info!("device: {api_device_name}");
    for uv in &upgrade {
        info!("  target {}: {}", uv.chip_name, uv.version);
    }

    info!("downloading firmware...");
    let fw_b64 = client
        .download_firmware(api_device_name, current, upgrade)
        .await?;

    api::decode_and_extract(&fw_b64, std::path::Path::new(output_dir))?;
    Ok(())
}

async fn scan_command(
    adapter: &Adapter,
    device_id: Option<DeviceId>,
) -> anyhow::Result<(Peripheral, WhoopGeneration)> {
    adapter
        .start_scan(ScanFilter {
            services: ALL_WHOOP_SERVICES.to_vec(),
        })
        .await?;

    loop {
        let peripherals = adapter.peripherals().await?;

        for peripheral in peripherals {
            let Some(properties) = peripheral.properties().await? else {
                continue;
            };

            let Some(generation) = ALL_WHOOP_SERVICES.iter().find_map(|svc| {
                if properties.services.contains(svc) {
                    WhoopGeneration::from_service(*svc)
                } else {
                    None
                }
            }) else {
                continue;
            };

            let Some(device_id) = device_id.as_ref() else {
                println!("Address: {}", properties.address);
                println!("Name: {:?}", properties.local_name);
                println!("RSSI: {:?}", properties.rssi);
                println!("Generation: {:?}", generation);
                println!();
                continue;
            };

            #[cfg(target_os = "linux")]
            if properties.address == *device_id {
                return Ok((peripheral, generation));
            }

            #[cfg(target_os = "macos")]
            {
                let Some(name) = properties.local_name else {
                    continue;
                };
                if sanitize_name(&name).starts_with(device_id) {
                    return Ok((peripheral, generation));
                }
            }
        }

        sleep(Duration::from_secs(1)).await;
    }
}

#[derive(Clone, Copy, Debug)]
pub enum AlarmTime {
    DateTime(NaiveDateTime),
    Time(NaiveTime),
    Minute,
    Minute5,
    Minute10,
    Minute15,
    Minute30,
    Hour,
}

impl FromStr for AlarmTime {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Ok(t) = s.parse() {
            return Ok(Self::DateTime(t));
        }

        if let Ok(t) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
            return Ok(Self::DateTime(t));
        }

        if let Ok(t) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
            return Ok(Self::DateTime(t));
        }

        if let Ok(t) = s.parse() {
            return Ok(Self::Time(t));
        }

        match s {
            "minute" | "1min" | "min" => Ok(Self::Minute),
            "5minute" | "5min" => Ok(Self::Minute5),
            "10minute" | "10min" => Ok(Self::Minute10),
            "15minute" | "15min" => Ok(Self::Minute15),
            "30minute" | "30min" => Ok(Self::Minute30),
            "hour" | "h" => Ok(Self::Hour),
            _ => Err(anyhow!("Invalid alarm time")),
        }
    }
}

impl AlarmTime {
    pub fn unix(self) -> DateTime<Utc> {
        let mut now = Utc::now();
        let timezone_df = Local::now().offset().to_owned();

        match self {
            AlarmTime::DateTime(dt) => dt.and_utc() - timezone_df,
            AlarmTime::Time(t) => {
                let current_time = now.time();
                if current_time > t {
                    now += TimeDelta::days(1);
                }

                now.with_time(t).unwrap() - timezone_df
            }
            _ => {
                let offset = self.offset();
                now + offset
            }
        }
    }

    fn offset(self) -> TimeDelta {
        match self {
            AlarmTime::DateTime(_) => todo!(),
            AlarmTime::Time(_) => todo!(),
            AlarmTime::Minute => TimeDelta::minutes(1),
            AlarmTime::Minute5 => TimeDelta::minutes(5),
            AlarmTime::Minute10 => TimeDelta::minutes(10),
            AlarmTime::Minute15 => TimeDelta::minutes(15),
            AlarmTime::Minute30 => TimeDelta::minutes(30),
            AlarmTime::Hour => TimeDelta::hours(1),
        }
    }
}

#[cfg(target_os = "macos")]
pub fn sanitize_name(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .to_string()
}

fn parse_date_arg(s: &str) -> anyhow::Result<NaiveDateTime> {
    // Accept YYYY-MM-DD (whole-day) or a full NaiveDateTime string.
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(d.and_hms_opt(0, 0, 0).unwrap());
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Ok(dt);
    }
    anyhow::bail!("invalid date '{s}'; use YYYY-MM-DD or 'YYYY-MM-DD HH:MM:SS'")
}

/// Resolve current git HEAD SHA (short form). Silently returns None
/// if git isn't available or we're not in a repo — note-writing
/// should never block on a missing commit ref.
fn git_head_sha() -> Option<String> {
    use std::process::Command;
    let out = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

impl OpenWhoopCli {
    async fn run(self) -> anyhow::Result<()> {
        if let OpenWhoopCommand::SetWhoop { whoop } = &self.subcommand {
            let env_path = set_default_whoop(whoop)?;
            println!("Set WHOOP={} in {}", whoop, env_path.display());
            return Ok(());
        }

        if let OpenWhoopCommand::SetRemote { remote } = &self.subcommand {
            let env_path = set_default_remote(remote)?;
            println!("Set REMOTE={} in {}", remote, env_path.display());
            return Ok(());
        }

        if let OpenWhoopCommand::DownloadFirmware {
            email,
            password,
            device_name,
            maxim,
            nordic,
            ruggles,
            pearl,
            maverick,
            output_dir,
        } = &self.subcommand
        {
            return download_firmware(
                email,
                password,
                device_name,
                maxim,
                nordic,
                ruggles,
                pearl,
                maverick,
                output_dir,
            )
            .await;
        }

        if let OpenWhoopCommand::Note {
            title,
            kind,
            body,
            feature,
            commit,
            author,
        } = &self.subcommand
        {
            let kind_enum = openwhoop::db::DevNoteKind::parse(kind.as_str())
                .ok_or_else(|| anyhow!(
                    "unknown --kind '{kind}'; expected one of note|question|experiment|diff|status"
                ))?;
            let resolved_commit = match commit.clone() {
                Some(s) => Some(s),
                None => git_head_sha(),
            };
            let input = openwhoop::db::DevNoteInput {
                author: Some(author.clone()),
                kind: Some(kind_enum),
                title: title.clone(),
                body_md: body.clone(),
                related_commit: resolved_commit,
                related_feature: feature.clone(),
                ..Default::default()
            };
            let database_url = resolve_database_url(self.database_url.clone())?;
            let db = DatabaseHandler::new(database_url).await;
            let id = db
                .create_dev_note(chrono::Local::now().naive_local(), input)
                .await?;
            println!("note #{id} created (kind={kind}, author={author})");
            return Ok(());
        }

        if let OpenWhoopCommand::ReclassifySleep {
            from,
            to,
            classifier,
        } = &self.subcommand
        {
            // Accept any rule-v* tag — the actual classifier always
            // runs the version compiled into the binary. The CLI flag
            // is mostly informational; we just refuse non-rule values
            // (which would imply a future ML classifier we haven't
            // built yet).
            if !classifier.starts_with("rule-v") {
                anyhow::bail!(
                    "unknown classifier '{classifier}'; only rule-v* tags are supported in phase 1"
                );
            }
            let from_dt = parse_date_arg(from)?;
            let to_dt = match to {
                Some(s) => parse_date_arg(s)?,
                None => chrono::Local::now().date_naive().and_hms_opt(23, 59, 59).unwrap(),
            };
            let database_url = resolve_database_url(self.database_url.clone())?;
            let db_handler = DatabaseHandler::new(database_url).await;
            let result =
                openwhoop::sleep_staging::reclassify_range(&db_handler, from_dt, to_dt).await?;
            println!(
                "reclassify-sleep: considered={} succeeded={} failed={} classifier={}",
                result.cycles_considered, result.cycles_succeeded, result.cycles_failed, classifier
            );
            return Ok(());
        }

        let database_url = resolve_database_url(self.database_url.clone())?;
        let adapter = self.create_ble_adapter().await?;
        let db_handler = DatabaseHandler::new(database_url).await;

        match self.subcommand {
            OpenWhoopCommand::Scan => {
                scan_command(&adapter, None).await?;
            }
            OpenWhoopCommand::SetWhoop { .. } => unreachable!(),
            OpenWhoopCommand::SetRemote { .. } => unreachable!(),
            OpenWhoopCommand::DownloadHistory {
                whoop,
                history_timeout_secs,
                history_idle_timeout_secs,
            } => {
                let (peripheral, generation) = scan_command(&adapter, Some(whoop)).await?;
                let mut whoop = WhoopDevice::new(
                    peripheral,
                    adapter,
                    db_handler,
                    self.debug_packets,
                    generation,
                );

                let should_exit = Arc::new(AtomicBool::new(false));

                let se = should_exit.clone();
                ctrlc::set_handler(move || {
                    println!("Received CTRL+C!");
                    se.store(true, Ordering::SeqCst);
                })?;

                whoop.connect().await?;
                whoop.initialize().await?;
                whoop
                    .sync_history(
                        should_exit,
                        HistorySyncConfig::from_secs(
                            history_timeout_secs,
                            history_idle_timeout_secs,
                        ),
                    )
                    .await?;

                info!("Exiting...");

                if matches!(generation, WhoopGeneration::Gen4) {
                    loop {
                        if let Ok(true) = whoop.is_connected().await {
                            whoop
                                .send_command(WhoopPacket::exit_high_freq_sync())
                                .await?;
                            break;
                        } else {
                            whoop.connect().await?;
                            sleep(Duration::from_secs(1)).await;
                        }
                    }
                }
            }
            OpenWhoopCommand::ReRun => {
                let mut whoop = OpenWhoop::new(db_handler.clone(), WhoopGeneration::Gen4);
                let mut id = 0;
                loop {
                    let packets = db_handler.get_packets(id).await?;
                    if packets.is_empty() {
                        break;
                    }

                    for packet in packets {
                        id = packet.id;
                        whoop.handle_packet(packet).await?;
                    }

                    println!("{}", id);
                }
            }
            OpenWhoopCommand::DetectEvents => {
                // Feature 6: wrap the full pipeline in a sync_log attempt.
                // Logger failures must NOT block the pipeline — we swallow
                // write errors and keep going.
                let audit_db_url = resolve_database_url(self.database_url.clone())?;
                let audit_db = DatabaseHandler::new(audit_db_url).await;
                let started_at = chrono::Local::now().naive_local();
                let log_id = audit_db
                    .begin_sync_attempt(started_at, Some("manual".to_string()))
                    .await
                    .ok();

                let whoop = OpenWhoop::new(db_handler, WhoopGeneration::Placeholder);
                let pipeline_result: anyhow::Result<()> = async {
                    whoop.detect_sleeps().await?;
                    whoop.detect_events().await?;
                    // Feature 4: wear-period tracking BEFORE sleep staging
                    // (not strictly required, but lets staging optionally
                    // consult wear data in the future).
                    whoop.update_wear_periods().await?;
                    whoop.stage_sleep().await?;
                    // Feature 5 + 7: daytime HRV + activity classification,
                    // both scoped to wear_periods and excluding sleep.
                    whoop.compute_daytime_hrv().await?;
                    whoop.classify_activities().await?;
                    Ok(())
                }
                .await;

                if let Some(id) = log_id {
                    let ended_at = chrono::Local::now().naive_local();
                    match &pipeline_result {
                        Ok(_) => {
                            let _ = audit_db
                                .finish_sync_attempt(
                                    id,
                                    ended_at,
                                    openwhoop::db::SyncOutcome::Success,
                                    openwhoop::db::SyncCounts::default(),
                                )
                                .await;
                        }
                        Err(e) => {
                            let _ = audit_db
                                .fail_sync_attempt(id, ended_at, format!("{e:#}"))
                                .await;
                        }
                    }
                }
                pipeline_result?;
            }
            OpenWhoopCommand::SleepStats => {
                let whoop = OpenWhoop::new(db_handler, WhoopGeneration::Placeholder);
                let sleep_records = whoop.database.get_sleep_cycles(None).await?;

                if sleep_records.is_empty() {
                    println!("No sleep records found, exiting now");
                    return Ok(());
                }

                let mut last_week = sleep_records
                    .iter()
                    .rev()
                    .take(7)
                    .copied()
                    .collect::<Vec<_>>();

                last_week.reverse();
                let analyzer = SleepConsistencyAnalyzer::new(sleep_records);
                let metrics = analyzer.calculate_consistency_metrics()?;
                println!("All time: \n{}", metrics);
                let analyzer = SleepConsistencyAnalyzer::new(last_week);
                let metrics = analyzer.calculate_consistency_metrics()?;
                println!("\nWeek: \n{}", metrics);
            }
            OpenWhoopCommand::ExerciseStats => {
                let whoop = OpenWhoop::new(db_handler, WhoopGeneration::Placeholder);
                let exercises = whoop
                    .database
                    .search_activities(
                        SearchActivityPeriods::default().with_activity(ActivityType::Activity),
                    )
                    .await?;

                if exercises.is_empty() {
                    println!("No activities found, exiting now");
                    return Ok(());
                };

                let last_week = exercises
                    .iter()
                    .rev()
                    .take(7)
                    .copied()
                    .rev()
                    .collect::<Vec<_>>();

                let metrics = ExerciseMetrics::new(exercises)?;
                let last_week = ExerciseMetrics::new(last_week)?;

                println!("All time: \n{}", metrics);
                println!("Last week: \n{}", last_week);
            }
            OpenWhoopCommand::CalculateStress => {
                let whoop = OpenWhoop::new(db_handler, WhoopGeneration::Placeholder);
                whoop.calculate_stress().await?;
            }
            OpenWhoopCommand::CalculateSpo2 => {
                let whoop = OpenWhoop::new(db_handler, WhoopGeneration::Placeholder);
                whoop.calculate_spo2().await?;
            }
            OpenWhoopCommand::CalculateSkinTemp => {
                let whoop = OpenWhoop::new(db_handler, WhoopGeneration::Placeholder);
                whoop.calculate_skin_temp().await?;
            }
            OpenWhoopCommand::SetAlarm { whoop, alarm_time } => {
                let (peripheral, generation) = scan_command(&adapter, Some(whoop)).await?;
                // Feature 3: separate handle to the same DB for the
                // alarm-history write, since db_handler is moved into
                // WhoopDevice below.
                let audit_db_url = resolve_database_url(self.database_url.clone())?;
                let audit_db = DatabaseHandler::new(audit_db_url).await;
                let mut whoop = WhoopDevice::new(
                    peripheral,
                    adapter,
                    db_handler,
                    self.debug_packets,
                    generation,
                );
                whoop.connect().await?;

                let time = alarm_time.unix();
                let now = Utc::now();

                if time < now {
                    error!(
                        "Time {} is in past, current time: {}",
                        time.format("%Y-%m-%d %H:%M:%S"),
                        now.format("%Y-%m-%d %H:%M:%S")
                    );
                    return Ok(());
                }

                let packet = WhoopPacket::alarm_time(u32::try_from(time.timestamp())?, generation);
                whoop.send_command(packet).await?;
                let time = time.with_timezone(&Local);

                println!("Alarm time set for: {}", time.format("%Y-%m-%d %H:%M:%S"));
                let _ = audit_db
                    .create_alarm_entry(
                        openwhoop::db::AlarmAction::Set,
                        chrono::Local::now().naive_local(),
                        Some(time.naive_local()),
                        Some(true),
                    )
                    .await;
            }
            OpenWhoopCommand::StreamHr { whoop } => {
                let (peripheral, generation) = scan_command(&adapter, Some(whoop)).await?;
                let mut whoop = WhoopDevice::new(
                    peripheral,
                    adapter,
                    db_handler,
                    self.debug_packets,
                    generation,
                );
                let should_exit = Arc::new(AtomicBool::new(false));
                let se = should_exit.clone();
                ctrlc::set_handler(move || {
                    se.store(true, Ordering::SeqCst);
                })?;
                whoop.connect().await?;
                whoop.stream_hr(should_exit).await?;
            }
            OpenWhoopCommand::RingAlarm { whoop } => {
                let (peripheral, generation) = scan_command(&adapter, Some(whoop)).await?;
                let mut whoop = WhoopDevice::new(
                    peripheral,
                    adapter,
                    db_handler,
                    self.debug_packets,
                    generation,
                );
                whoop.connect().await?;
                whoop.ring_alarm().await?;
                println!("Alarm triggered.");
            }
            OpenWhoopCommand::GetAlarm { whoop } => {
                let (peripheral, generation) = scan_command(&adapter, Some(whoop)).await?;
                let audit_db_url = resolve_database_url(self.database_url.clone())?;
                let audit_db = DatabaseHandler::new(audit_db_url).await;
                let mut whoop =
                    WhoopDevice::new(peripheral, adapter, db_handler, false, generation);
                whoop.connect().await?;
                let data = whoop.get_alarm().await?;
                if let openwhoop_codec::WhoopData::AlarmInfo { enabled, unix } = data {
                    let scheduled = chrono::DateTime::from_timestamp(i64::from(unix), 0)
                        .map(|d| d.naive_utc());
                    let _ = audit_db
                        .create_alarm_entry(
                            openwhoop::db::AlarmAction::Queried,
                            chrono::Local::now().naive_local(),
                            scheduled,
                            Some(enabled),
                        )
                        .await;
                    if enabled {
                        let alarm_time = DateTime::from_timestamp(i64::from(unix), 0)
                            .ok_or_else(|| anyhow!("Invalid alarm timestamp"))?
                            .with_timezone(&Local);
                        println!(
                            "Alarm is set for: {}",
                            alarm_time.format("%Y-%m-%d %H:%M:%S")
                        );
                    } else {
                        println!("No alarm is currently set");
                    }
                } else {
                    error!("Unexpected response from device: {:?}", data);
                }
            }
            OpenWhoopCommand::Merge { from } => {
                let from_db = DatabaseHandler::new(from).await;

                let mut id = 0;
                loop {
                    let packets = from_db.get_packets(id).await?;
                    if packets.is_empty() {
                        break;
                    }

                    for packets::Model {
                        uuid,
                        bytes,
                        id: c_id,
                    } in packets
                    {
                        id = c_id;
                        db_handler.create_packet(uuid, bytes).await?;
                    }

                    println!("{}", id);
                }
            }
            OpenWhoopCommand::Restart { whoop } => {
                let (peripheral, generation) = scan_command(&adapter, Some(whoop)).await?;
                let mut whoop = WhoopDevice::new(
                    peripheral,
                    adapter,
                    db_handler,
                    self.debug_packets,
                    generation,
                );
                whoop.connect().await?;
                whoop.send_command(WhoopPacket::restart()).await?;
            }
            OpenWhoopCommand::Erase { whoop } => {
                let (peripheral, generation) = scan_command(&adapter, Some(whoop)).await?;
                let mut whoop = WhoopDevice::new(
                    peripheral,
                    adapter,
                    db_handler,
                    self.debug_packets,
                    generation,
                );
                whoop.connect().await?;
                whoop.send_command(WhoopPacket::erase()).await?;
                info!("Erase command sent - device will trim all stored history data");
            }
            OpenWhoopCommand::Version { whoop } => {
                let (peripheral, generation) = scan_command(&adapter, Some(whoop)).await?;
                let mut whoop =
                    WhoopDevice::new(peripheral, adapter, db_handler, false, generation);

                whoop.connect().await?;
                whoop.get_version().await?;
            }
            OpenWhoopCommand::EnableImu { whoop } => {
                let (peripheral, generation) = scan_command(&adapter, Some(whoop)).await?;
                let mut whoop =
                    WhoopDevice::new(peripheral, adapter, db_handler, false, generation);
                whoop.connect().await?;
                whoop
                    .send_command(WhoopPacket::toggle_r7_data_collection())
                    .await?;
            }
            OpenWhoopCommand::Sync { remote } => {
                let remote_db = DatabaseHandler::new(remote).await;
                let sync = openwhoop::db::sync::DatabaseSync::new(
                    db_handler.connection(),
                    remote_db.connection(),
                );
                sync.run().await?;
            }
            OpenWhoopCommand::Completions { shell } => {
                let mut command = OpenWhoopCli::command();
                let bin_name = command.get_name().to_string();
                generate(shell, &mut command, bin_name, &mut io::stdout());
            }
            OpenWhoopCommand::DownloadFirmware { .. } => {
                unreachable!("handled before BLE/DB init")
            }
            OpenWhoopCommand::ReclassifySleep { .. } => {
                unreachable!("handled before BLE init")
            }
            OpenWhoopCommand::Note { .. } => {
                unreachable!("handled before BLE init")
            }
        }

        Ok(())
    }

    async fn create_ble_adapter(&self) -> anyhow::Result<Adapter> {
        let manager = Manager::new().await?;

        #[cfg(target_os = "linux")]
        match self.ble_interface.as_ref() {
            Some(interface) => Self::adapter_from_name(&manager, interface).await,
            None => Self::default_adapter(&manager).await,
        }

        #[cfg(target_os = "macos")]
        Self::default_adapter(&manager).await
    }

    #[cfg(target_os = "linux")]
    async fn adapter_from_name(manager: &Manager, interface: &str) -> anyhow::Result<Adapter> {
        let adapters = manager.adapters().await?;
        let mut c_adapter = Err(anyhow!("Adapter: `{}` not found", interface));
        for adapter in adapters {
            let name = adapter.adapter_info().await?;
            if name.starts_with(interface) {
                c_adapter = Ok(adapter);
                break;
            }
        }

        c_adapter
    }

    async fn default_adapter(manager: &Manager) -> anyhow::Result<Adapter> {
        let adapters = manager.adapters().await?;
        adapters
            .into_iter()
            .next()
            .ok_or(anyhow!("No BLE adapters found"))
    }
}
