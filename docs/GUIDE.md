# OpenWhoop — A Plain-English Guide

This is the friendly, non-technical version. If you want the technical one, read the README and the source.

## What is this thing?

OpenWhoop is a small program that talks directly to your WHOOP strap over Bluetooth, downloads the data the strap has been recording, and stores it in a file on your own computer. After that, it can do useful things with that data — figure out when you slept, when you exercised, estimate stress, blood oxygen, and skin temperature, and show it all in a dashboard.

The important thing to understand: **none of this involves WHOOP's servers**. Your data stays on your computer. The trade-off is that you only get the data your strap is currently holding — anything that's already been uploaded to the official WHOOP app and cleared off the device is gone as far as OpenWhoop is concerned.

## How the data flows

There are four moving parts. Picture them as a relay race:

1. **The strap.** It's recording your heart rate, motion, and a few other things every second, all the time. It can hold a few days of data on its own — up to about two weeks if you never sync it.

2. **Bluetooth download.** When you run `download-history`, OpenWhoop scans for your strap, connects to it, and asks for everything it's recorded since the last time you synced. The strap streams it over, and OpenWhoop dumps the raw packets into a file called `db.sqlite` in your project folder.

3. **The local database.** That `db.sqlite` file is your personal copy. It starts out as raw, unprocessed packets — basically a pile of numbers the strap sent. Other commands then chew through that pile and turn it into useful information.

4. **The dashboard / stats.** Once the data is processed, you can ask OpenWhoop to print a summary, list your sleeps, show recent workouts, etc.

The key idea: downloading and processing are separate steps. Downloading just grabs the raw stuff. Processing — sleep detection, stress calculation, etc. — happens later, on your computer, and you can re-run it any time.

## The commands you actually need

You only need to remember a handful. Run them all from the project folder.

### One-time setup
- Put your strap's name in `.env` so you don't have to type it every time:
  ```
  WHOOP="WHOOP 4C0968309"
  DATABASE_URL=sqlite://db.sqlite?mode=rwc
  ```
  (Replace the digits with your strap's actual name, which you can find with `cargo run -r -- scan`.)

### The daily/weekly routine
1. **`download-history`** — pull whatever new data is on the strap.
2. **`detect-events`** — look through the new data and figure out when you slept and when you were active.
3. **`calculate-stress`**, **`calculate-spo2`**, **`calculate-skin-temp`** — derive the extra metrics from the raw signal.
4. **`dashboard`** — print a one-page summary of where you stand.

That's it. You can chain them in one go:

```sh
cargo run -r -- download-history && \
cargo run -r -- detect-events && \
cargo run -r -- calculate-stress && \
cargo run -r -- calculate-spo2 && \
cargo run -r -- calculate-skin-temp && \
cargo run -r -- dashboard
```

If you want, save that as a shell alias or a Makefile target so it's one keystroke.

## How often should I sync?

Think of the strap as a bucket with a slow leak at the top — it can only hold so much before the oldest data starts spilling out. As a rough guide:

- **Every day or two** is comfortable. The strap has plenty of room and the syncs are quick.
- **Every few days** is fine if you forget once in a while.
- **Once a week or longer** and you start risking gaps, especially if you're also using the official WHOOP app (which empties the bucket on its own schedule).

If you want OpenWhoop to be your primary source of truth, the safest thing is to **stop using the official app**, or at least be aware that anything the official app uploads gets cleared from the strap and won't be available to OpenWhoop afterward.

## What's in the dashboard

The `dashboard` command shows you four sections:

- **Latest sleep.** Your most recent night: when you went to bed, when you got up, total time, average heart rate and HRV, and a sleep score.
- **Last 7 days.** How many nights of sleep you have on file this week, average duration and score, a consistency rating (how regular your bedtime is), and how many workouts.
- **Today.** How much heart rate data has come in today, your min/avg/max, the most recent stress / SpO2 / skin temperature reading, and a strain score (if there's enough data — needs at least 10 minutes). At the bottom you get a tiny 24-hour heart-rate sparkline.
- **Recent activities.** The last few detected workouts with start time, end time, and duration.

Empty sections will tell you what to run to fill them in. If "latest sleep" is blank, you need to run `detect-events`. If "today" is blank, you need to run `download-history`.

## What can go wrong

- **The dashboard says you have no sleep data, but you definitely slept.** You haven't run `detect-events` since your last download. Run it.
- **Stress / SpO2 / skin temp say `--`.** Same idea — you need to run the matching `calculate-*` commands. They only process new data, so they're fast on subsequent runs.
- **Today's data is hours behind.** That's expected. The strap doesn't push data live; it only hands it over when you run `download-history`. The dashboard is a snapshot, not a live feed.
- **You see a "parse error" mentioning your strap name on macOS.** Harmless — it's the underlying Bluetooth library trying and failing to interpret your strap's name as a hardware address. Nothing is broken; the warning is just noisy.
- **You want data from before you started using OpenWhoop.** It's not on the strap anymore (the strap only holds the recent days). The only place it still exists is WHOOP's own cloud, and OpenWhoop has no way to pull from there.

## What's actually being computed (in plain English)

If you're curious what the calculate-* commands are doing:

- **Stress** uses the spacing between heartbeats (the tiny variations from beat to beat) over a two-minute window. When the spacing is very regular, your nervous system is in a "stressed" state; when it varies a lot, you're relaxed. The number is a standard physiology metric called the Baevsky stress index.
- **SpO2** (blood oxygen) uses the strap's red and infrared light sensors. It compares how much of each color is being absorbed and reflected back, which corresponds to how saturated your blood is with oxygen.
- **Skin temperature** is a direct reading from a temperature sensor inside the strap, converted from raw sensor units into degrees Celsius.
- **Strain** is a workload score from 0 to 21. It looks at how much time today you spent in each heart-rate zone and how hard each zone counts. A normal day at a desk is around 5–8; a hard workout pushes it into the teens.
- **Sleep detection** finds long stretches at night where your body wasn't moving and your heart rate dropped. **Activity detection** finds the opposite — sustained periods of elevated heart rate and motion.
- **Sleep consistency** measures how regular your bedtime and wake time are across the week. Going to bed at wildly different times costs you points.

None of these are official WHOOP numbers — they're community implementations of the same ideas. They'll be in the same ballpark as the WHOOP app but won't match it exactly.

## Where the data lives

One file: `db.sqlite` in your project folder. That's it. To inspect it manually:

```sh
sqlite3 db.sqlite
```

Then `.tables` to see what's there. The interesting tables are:

- `packets` — raw stuff straight from the strap.
- `heart_rate` — one row per second-ish, with bpm, beat-to-beat intervals, and the derived stress/SpO2/temperature.
- `sleep_cycles` — one row per night, summarizing the sleep.
- `activities` — one row per detected workout or activity period.

If you ever want to start fresh, just delete `db.sqlite` and re-download.

## Backing up

Because everything is one file, backups are trivial — copy `db.sqlite` somewhere safe. If you ever want to merge data from a backup, there's a `merge` command for that.

## The desktop app (OpenWhoop Tray)

If you'd rather not run CLI commands, there's a companion macOS menu bar app called **OpenWhoop Tray**. It wraps the same library and database but gives you:

- A tray icon that shows battery / presence / last sync at a glance
- A visual dashboard (same data as `cargo run -r -- dashboard`)
- Background auto-sync on a configurable schedule (or manual)
- Presence detection — auto-syncs when you walk back in range
- Alarm set/clear/read + a "Buzz strap" find-my-device button
- Device discovery via BLE scan instead of typing the name
- Launch-at-login support

### Installing the tray app

Pre-built `.app` bundles live in the `openwhoop-tray` repo. To install:

```sh
cp -R OpenWhoop.app /Applications/
open /Applications/OpenWhoop.app
```

First launch will prompt for Bluetooth permission. The app lives entirely in the menu bar — no dock icon. Click the tray icon to see the menu; click "Show Dashboard" to open the window.

### Data sharing between CLI and tray app

The tray app stores its database at `~/Library/Application Support/dev.brennen.openwhoop-tray/db.sqlite`. If you want to share data between the CLI and the tray app, either:

1. Point the CLI's `DATABASE_URL` at the tray app's DB path, or
2. Copy `db.sqlite` between them, or
3. Use the `merge` command to combine databases

They use the same schema and the same algorithms — the tray app just links the openwhoop library directly instead of shelling out to the binary.
