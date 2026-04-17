# Research Prompt — Deep-Sleep HR Gate Design

Paste the block below into a research-capable model (e.g. ChatGPT o3 with browse, Gemini Deep Research, Claude with web search). It's self-contained — the model doesn't need any code access.

---

## Prompt

I'm building a rule-based sleep-stage classifier for a consumer wrist wearable (WHOOP 4.0 strap) using locally-recorded data only (no cloud, no model training, no PSG ground truth). I've hit a specific design problem and want you to synthesize the published literature to help me choose the best fix — or suggest a better one I haven't considered.

Please search the web for relevant papers and return a grounded recommendation with citations. I care more about correctness than speed.

### System context

- **Sensor suite per 30-second epoch:** wrist PPG-derived heart rate (1 Hz), beat-to-beat RR intervals, 3-axis accelerometer (both gravity vector + higher-rate IMU for activity counts), 6-axis IMU including gyro, skin temperature, on-wrist capacitive sensor, PPG-derived respiratory rate.
- **Design constraint:** no PSG ground truth available; no labeled training data. Phase 1 must ship a rule-based classifier. Phase 2 plans to train a LightGBM model on MESA Sleep Study data offline and ship it as ONNX, but that's out of scope for this specific question.
- **Target accuracy ceiling:** the ~75–80% epoch agreement / κ ≈ 0.55–0.65 that Wulterkens 2021, Fonseca 2023 (Philips), and Altini & Kinnunen (Oura) report for consumer wrist-based staging. Not trying to beat those; trying to reach them.

### Current classifier — what's working

The classifier evaluates hierarchical rules per 30-second epoch (first match wins): Wake → Deep → REM → Light → Wake fallback. Features per epoch come from the last 5 minutes centered on the epoch (HRV frequency-domain), 2 minutes centered (respiratory), or the epoch itself (time-domain HRV, motion, stillness).

Rules use a mix of:
- **Absolute thresholds** anchored on a per-user baseline (e.g. `hr_mean < resting_hr + 8`, `rmssd > user_sleep_rmssd_median`)
- **Within-night percentiles** that self-normalize (e.g. `hf_power > 50th percentile of this night's valid epochs`, `lf_hf_ratio > 50th percentile`)
- **Temporal cues** (relative position in sleep window, is_first_half)
- **Motion** (stillness ratio from gravity-vector delta, activity count from accelerometer magnitude)

The Deep rule specifically says:
```
motion_stillness_ratio > 0.95
AND hr_mean < baseline.resting_hr + 8 BPM
AND hf_power > 50th percentile of this night's HF
AND rmssd > baseline.sleep_rmssd_median
AND relative_night_position < 0.6
```

### The specific problem

`baseline.resting_hr` is currently computed as the **mean of per-night minimum HR values across the rolling 14-night window**. For a single night or for a new user, it's just the nightly minimum HR.

For an athletic user I observed (resting awake HR ~58–62, nightly minimum HR observed 48 BPM):
- The Deep rule requires `hr_mean < 48 + 8 = 56` BPM.
- But the nightly minimum of 48 BPM **is itself a Deep-sleep epoch** — that's when HR is lowest.
- So the rule is nearly tautological: "an epoch qualifies for Deep if its HR is within 8 BPM of a value that's already a Deep-sleep HR."
- **Result:** on a 5h34m night, only 17 of 668 epochs cleared the Deep gate (2.7% Deep instead of the 10–30% target). Most real Deep epochs ran at HR 52–58 and missed by a few BPM.

When I force-swap the baseline to use the population-default `resting_hr = 62` instead of the personalized 48, the same night produces **134 Deep epochs (21.3%)** — right in the expected range. This confirms the issue is the semantic mismatch between "nightly minimum HR" and what the classifier rule calls "resting HR".

### The three options I'm weighing

**Option 1 — Widen the offset.** Keep the design; change the constant from `+8` to `+12` or `+15`. One-line fix. But it doesn't address the underlying mismatch — it just buys enough slack that some non-minimum-HR epochs clear the gate.

**Option 2 — Redefine "resting_hr".** Change the baseline computation from "nightly minimum HR" to something that better approximates awake-resting HR, e.g. the 25th percentile of HR across sleep epochs, or the mean HR across non-Wake epochs. The Deep rule's `+8` offset stays intact but the reference point is now a realistic awake-resting value (~58–62 for this user). Also affects strain computation and sleep-need calculation, which currently use the same `resting_hr` field.

**Option 3 — Replace the absolute gate with a within-night percentile.** Drop the `hr_mean < resting_hr + N` rule entirely. Replace it with `hr_mean < 25th percentile of this night's epoch HRs`. Self-calibrating per night, immune to baseline-drift, works across fitness levels. Possible risks: degenerate on nights with zero true Deep (always labels quietest 25% as Deep regardless of whether it really is); loses the absolute-physiology semantics.

### What I want from you

1. **A grounded recommendation among Options 1 / 2 / 3**, or a distinct Option 4 you think is better. Ideally with reasoning from published work.

2. **Evidence on specific questions:**
   - In published rule-based or ML wrist-based Deep-sleep classifiers, how is the HR reference point typically defined? (Awake-measured resting HR? Nightly minimum? Some percentile? Per-night relative?)
   - Is there normative data on the relationship between nightly minimum HR and awake resting HR in adults? (My guess is nightly min = awake resting − 5 to 12 BPM for a non-athlete, wider for athletic users.)
   - How discriminating is HR alone for Deep vs Light sleep in wrist-based staging? (I suspect it's a weak individual feature and that HF HRV + motion do most of the work.)
   - Is there any published approach that uses "percentile of this night's HR distribution" as a Deep gate?

3. **Consumer-wearable validation study comparisons.** If specific papers report the feature set and thresholds their classifier uses for Deep, summarize: Wulterkens 2021, Fonseca 2023, Altini & Kinnunen (Oura 2021), Beattie 2017 (Fitbit Surge), Kotzen 2022 (SleepPPG-Net). I'm particularly interested in whether they use an absolute HR threshold or a relative/percentile approach.

4. **MESA-style preprocessing conventions.** Since Phase 2 will train on MESA, what HR-normalization conventions does the MESA preprocessing pipeline (e.g. `bzhai/multimodal_sleep_stage_benchmark`) apply to features before handing them to the classifier? If MESA normalizes per-subject, that's a strong argument for Option 2 or 3 in my Phase 1 so features stay compatible.

5. **Athletic-population considerations.** The specific user I'm tuning against has a sub-50 nightly minimum HR (endurance-athlete range). Are there papers addressing consumer-staging accuracy in athletic populations with unusually low resting HR?

### Output format I'd like

```
### Recommended approach
[Option 1, 2, 3, or a new Option 4, with one-paragraph justification]

### Key evidence
- [Bullet points, each with an inline citation or DOI/URL]

### Per-question answers
- Q1: [answer + citation]
- Q2: [answer + citation]
- ...

### If I chose differently
[One paragraph on what to watch for if I picked a non-recommended option]

### Uncertainties / what the literature can't tell me
[Things where published work is silent or conflicting]
```

Please prioritize papers from 2018 onward for consumer wearables and the classic HRV/PSG references (Task Force 1996, AASM manuals, etc.) for physiology baselines. If you find a strong paper arguing for a specific approach, cite it and quote the relevant passage.

---

## Notes

- This prompt assumes the model has web access. If you're using a non-browse model, strip the "search the web" language and paste in the key papers yourself.
- I'd recommend Gemini Deep Research or ChatGPT o3 Deep Research for this — the job is literature synthesis more than reasoning-over-math.
- Expected runtime: 5–15 minutes of the model gathering + reading sources. Don't accept the first-pass answer without at least one follow-up asking for specific paper quotes.
- After you get a recommendation back, drop it in a new note (`docs/DEEP_HR_RESEARCH_FINDINGS.md` maybe) so we don't lose it. If the answer is clear, I'll implement. If it raises new questions, we'll iterate.
