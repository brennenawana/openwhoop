use anyhow::Result;
use chrono::{Local, NaiveDateTime, TimeDelta, Timelike};
use openwhoop::{
    algo::{SleepConsistencyAnalyzer, SleepCycle, StrainCalculator},
    db::{DatabaseHandler, SearchHistory},
    types::activities::{ActivityPeriod, ActivityType, SearchActivityPeriods},
};
use openwhoop_codec::ParsedHistoryReading;
use openwhoop_entities::heart_rate;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";

const SPARK_BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

pub async fn dashboard_command(db: &DatabaseHandler) -> Result<()> {
    let now = Local::now().naive_local();
    let today = now.date();
    let day_start = today.and_hms_opt(0, 0, 0).unwrap();
    let week_start = (today - chrono::Days::new(7)).and_hms_opt(0, 0, 0).unwrap();

    let sleep_cycles = db.get_sleep_cycles(None).await?;
    let recent_activities = db
        .search_activities(SearchActivityPeriods {
            from: Some(week_start),
            to: None,
            activity: None,
        })
        .await?;

    let today_rows = heart_rate::Entity::find()
        .filter(heart_rate::Column::Time.gte(day_start))
        .order_by_asc(heart_rate::Column::Time)
        .all(db.connection())
        .await?;

    let today_parsed = db
        .search_history(SearchHistory {
            from: Some(day_start),
            to: None,
            limit: None,
        })
        .await?;

    print_header(now, today_rows.len());
    print_latest_sleep(sleep_cycles.last());
    print_week(&sleep_cycles, &recent_activities, week_start);
    print_today(&today_rows, &today_parsed, sleep_cycles.last());
    print_activities(&recent_activities);
    Ok(())
}

fn print_header(now: NaiveDateTime, hr_count: usize) {
    println!("{BOLD}{CYAN}╭─ WHOOP DASHBOARD ──────────────────────╮{RESET}");
    println!(
        "{CYAN}│{RESET} {}  {DIM}• {} HR samples today{RESET}",
        now.format("%a %b %d  %H:%M"),
        hr_count
    );
    println!("{BOLD}{CYAN}╰────────────────────────────────────────╯{RESET}\n");
}

fn print_latest_sleep(latest: Option<&SleepCycle>) {
    println!("{BOLD}LATEST SLEEP{RESET}");
    let Some(s) = latest else {
        println!("  {DIM}No sleep cycles detected. Run `detect-events`.{RESET}\n");
        return;
    };
    let dur = s.end - s.start;
    println!("  Night:      {}", s.id.format("%a %b %d"));
    println!(
        "  Duration:   {}    {DIM}Score:{RESET} {:.0}",
        format_duration(dur),
        s.score
    );
    println!(
        "  In bed:     {} → {}",
        s.start.format("%H:%M"),
        s.end.format("%H:%M")
    );
    println!(
        "  Heart rate: {} / {} / {} bpm  {DIM}(min/avg/max){RESET}",
        s.min_bpm, s.avg_bpm, s.max_bpm
    );
    println!(
        "  HRV:        {} / {} / {} ms\n",
        s.min_hrv, s.avg_hrv, s.max_hrv
    );
}

fn print_week(
    sleep: &[SleepCycle],
    activities: &[ActivityPeriod],
    week_start: NaiveDateTime,
) {
    println!("{BOLD}LAST 7 DAYS{RESET}");
    let week_sleep: Vec<SleepCycle> = sleep
        .iter()
        .filter(|s| s.start >= week_start)
        .copied()
        .collect();

    if week_sleep.is_empty() {
        println!("  {DIM}No sleep nights in the last 7 days.{RESET}");
    } else {
        let total_secs: i64 = week_sleep.iter().map(|s| (s.end - s.start).num_seconds()).sum();
        let avg_dur = TimeDelta::seconds(total_secs / week_sleep.len() as i64);
        let avg_score: f64 =
            week_sleep.iter().map(|s| s.score).sum::<f64>() / week_sleep.len() as f64;
        println!(
            "  Sleep nights:    {}     Avg duration: {}",
            week_sleep.len(),
            format_duration(avg_dur)
        );
        println!("  Avg sleep score: {:.0}", avg_score);
        if let Ok(metrics) =
            SleepConsistencyAnalyzer::new(week_sleep).calculate_consistency_metrics()
        {
            println!("  Consistency:     {:.0}/100", metrics.score.total_score);
        }
    }

    let workouts: Vec<&ActivityPeriod> = activities
        .iter()
        .filter(|a| matches!(a.activity, ActivityType::Activity))
        .collect();
    if workouts.is_empty() {
        println!("  {DIM}No activities detected this week.{RESET}\n");
    } else {
        let total_min: i64 = workouts.iter().map(|a| (a.to - a.from).num_minutes()).sum();
        println!(
            "  Activities:      {}     Total time:   {}h {:02}m\n",
            workouts.len(),
            total_min / 60,
            total_min % 60
        );
    }
}

fn print_today(
    rows: &[heart_rate::Model],
    parsed: &[ParsedHistoryReading],
    latest_sleep: Option<&SleepCycle>,
) {
    println!("{BOLD}TODAY{RESET}");
    if rows.is_empty() {
        println!("  {DIM}No HR data yet today. Run `download-history`.{RESET}\n");
        return;
    }

    let bpms: Vec<u8> = rows.iter().map(|r| r.bpm.max(0).min(255) as u8).collect();
    let min_bpm = *bpms.iter().min().unwrap();
    let max_bpm = *bpms.iter().max().unwrap();
    let avg_bpm = bpms.iter().map(|&b| u32::from(b)).sum::<u32>() / bpms.len() as u32;
    let current_bpm = *bpms.last().unwrap();
    let last_time = rows.last().unwrap().time;

    let latest_stress = rows.iter().rev().find_map(|r| r.stress);
    let latest_spo2 = rows.iter().rev().find_map(|r| r.spo2);
    let latest_temp = rows.iter().rev().find_map(|r| r.skin_temp);

    println!(
        "  Samples:    {}    {DIM}Last seen:{RESET} {}",
        rows.len(),
        last_time.format("%H:%M:%S")
    );
    println!(
        "  {GREEN}Current HR:{RESET} {} bpm",
        current_bpm
    );
    println!(
        "  HR range:   min {} / avg {} / max {} bpm",
        min_bpm, avg_bpm, max_bpm
    );
    println!(
        "  Stress:     {}    SpO2: {}    Skin temp: {}",
        opt_f(latest_stress, 1, ""),
        opt_pct(latest_spo2),
        opt_f(latest_temp, 1, "°C"),
    );

    if let Some(sleep) = latest_sleep {
        let resting = sleep.min_bpm.max(40);
        let max_hr = max_bpm.max(180);
        match StrainCalculator::new(max_hr, resting).calculate(parsed) {
            Some(s) => println!("  {GREEN}Strain:{RESET}     {:.1}/21", s.0),
            None => println!(
                "  {DIM}Strain:     need 10+ min of HR data ({}/600 samples){RESET}",
                parsed.len()
            ),
        }
    }

    let spark = hourly_sparkline(rows);
    if !spark.trim().is_empty() {
        println!("\n  {DIM}Hourly HR (24h):{RESET}");
        println!("  {}", spark);
        println!("  {DIM}0     6     12    18    {RESET}\n");
    } else {
        println!();
    }
}

fn print_activities(activities: &[ActivityPeriod]) {
    println!("{BOLD}RECENT ACTIVITIES{RESET}");
    let workouts: Vec<&ActivityPeriod> = activities
        .iter()
        .filter(|a| matches!(a.activity, ActivityType::Activity))
        .collect();
    if workouts.is_empty() {
        println!("  {DIM}(none in the last 7 days){RESET}\n");
        return;
    }
    for a in workouts.iter().rev().take(5) {
        let dur = a.to - a.from;
        println!(
            "  {YELLOW}{}{RESET}  {} → {}  ({})",
            a.from.format("%a %b %d"),
            a.from.format("%H:%M"),
            a.to.format("%H:%M"),
            format_duration(dur)
        );
    }
    println!();
}

fn hourly_sparkline(rows: &[heart_rate::Model]) -> String {
    let mut sums = [0u32; 24];
    let mut counts = [0u32; 24];
    for r in rows {
        let h = r.time.hour() as usize;
        sums[h] += u32::from(r.bpm.max(0) as u16);
        counts[h] += 1;
    }
    let avgs: Vec<Option<u32>> = (0..24)
        .map(|h| (counts[h] > 0).then(|| sums[h] / counts[h]))
        .collect();
    let valid: Vec<u32> = avgs.iter().filter_map(|x| *x).collect();
    if valid.is_empty() {
        return String::new();
    }
    let lo = *valid.iter().min().unwrap();
    let hi = *valid.iter().max().unwrap();
    let span = (hi - lo).max(1);
    avgs.into_iter()
        .map(|v| match v {
            None => ' ',
            Some(b) => SPARK_BARS[((b - lo) * 7 / span).min(7) as usize],
        })
        .collect()
}

fn format_duration(d: TimeDelta) -> String {
    let mins = d.num_minutes().max(0);
    format!("{}h {:02}m", mins / 60, mins % 60)
}

fn opt_f(v: Option<f64>, decimals: usize, suffix: &str) -> String {
    match v {
        Some(x) => format!("{:.*}{}", decimals, x, suffix),
        None => "--".into(),
    }
}

fn opt_pct(v: Option<f64>) -> String {
    match v {
        Some(x) => format!("{:.0}%", x),
        None => "--".into(),
    }
}
