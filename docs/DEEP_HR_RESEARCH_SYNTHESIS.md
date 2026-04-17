# Deep-HR Gate — Research Synthesis & Recommended Path

**Sources:** `DEEP_HR_RESEARCH_FINDINGS_CHAT_GPT.md`, `DEEP_HR_RESEARCH_FINDINGS_CLAUDE.md`, `DEEP_HR_RESEARCH_FINDINGS_GEMINI.md`. Three independent research passes, generated against the same prompt.

## TL;DR

All three models independently land on the same answer: **don't use Options 1 or 2 alone; do Option 3 (within-night percentile) — and decouple it from the user-facing `resting_hr` baseline.** The strongest framing is what ChatGPT and Claude both call **Option 4: a two-part fix**.

- **Classifier change:** drop `hr_mean < baseline.resting_hr + 8`, replace with `hr_mean < ~25th percentile of this night's valid epoch HRs`. Keep every other Deep gate (stillness, HF, RMSSD, rel-night) intact.
- **Strain/recovery change (separate):** the `baseline.resting_hr` field has a second job feeding strain and sleep-need calculations. Leave that field's definition alone *for now* — it's already producing usable strain numbers, and changing it has knock-on effects we don't need to take on tonight.

This single change is expected to lift Deep% from 2.7% (last test) into the 10–30% PRD range without touching anything else.

## Where the three sources agree (unanimous)

1. **The current rule is structurally broken on a WHOOP strap.** Anchoring "Deep" to a value that's itself derived from Deep sleep is tautological. WHOOP's own product docs (per Claude's citation) confirm WHOOP's user-facing "resting HR" is weighted toward the final SWS episode — meaning *every* WHOOP-derived RHR will collapse the headroom of any `+N bpm` rule. The athletic user case (Brennen, nightly min 48) is the pathological version of a problem that exists for all WHOOP users.

2. **Option 1 (widen the offset) is wrong.** It papers over the pathology. Will likely restore Deep% on Brennen's specific night, but:
   - inflates Deep in average users where `resting_hr` is already Deep-adjacent;
   - introduces per-subject tuning creep;
   - doesn't address the underlying "comparing Deep to Deep" issue.

3. **Option 2 alone (redefine `resting_hr`) is wrong.** It tangles strain/recovery, where absolute-physiology semantics matter. Also: the obvious replacements for nightly-min (25th percentile of sleep HR, mean of non-Wake epochs) are *still Deep-adjacent* — only partially break the tautology.

4. **Option 3 (within-night percentile) is closest to published practice.** All three independently cite the same convergent literature:
   - Walch et al. 2019 (Apple Watch): per-night 90th-percentile dispersion scaling.
   - Altini & Kinnunen 2021 (Oura): per-night 5th–95th percentile robust scaling, gives them ~16% accuracy lift from HRV over motion-only.
   - Roberts et al. 2020: night-level z-scoring consistently improves performance.
   - Perez-Pozuelo et al. 2022 (HypnosPy): rule-based ECDF-quantile gate on HR — published existence proof of a quantile rule (for sleep/wake; same family).
   - Xiao et al. 2013: per-subject percentile-rank normalization of HRV features.
   - Sridhar et al. 2020: within-night z-scores HR before deep-learning staging.
   - **None** of the published wrist staging classifiers (Wulterkens 2021, Fonseca 2020, Beattie 2017, Kotzen 2022, Altini 2021) use an absolute BPM threshold for Deep. They all either normalize per-night/per-subject or let an ML model absorb the variance.

5. **HR alone is a weak Deep discriminator.** Mean HR differs only 2–5 BPM between N2 and N3 with heavy overlap (Tobaldini 2013, Shaffer & Ginsberg 2017). The real Deep-discriminating features are HF power and RMSSD (parasympathetic dominance), plus motion stillness. **Implication:** the HR gate in the rule is doing a small fraction of the actual Deep detection work; the HF and RMSSD gates are the real workhorses. The current `+8` rule is *vetoing* epochs that the stronger features correctly identified.

6. **MESA forward-compatibility favors per-night normalization.** Per Claude's research, MESA's bzhai pipeline uses a single global `StandardScaler` across the pooled training set. Per-night percentile features (what we'd ship) and pooled z-scores (what MESA does) both collapse between-subject BPM differences — they're not identical, but conceptually compatible. We should *document* the on-device features as per-night-z-scored or per-night-percentile-ranked so Phase 2 LightGBM training fits to features in the same family.

## Where the three sources differ (minor)

- **Exact percentile cutoff:** ChatGPT proposes 25th, Gemini proposes 30th, Claude doesn't commit. None of the three found an empirically-calibrated number in the literature. **Action:** start at 25th; tune on real data.

- **Hysteresis / smoothing:** ChatGPT recommends explicit "2 of last 3 epochs satisfy the rule" hysteresis. Claude and Gemini don't emphasize it. **Note:** our existing classifier already runs a 3-epoch median filter as post-processing, which achieves a similar effect. Keep the median filter; skip the additional in-rule hysteresis for now.

- **Decoupled `resting_hr` urgency:** Claude makes a stronger case that we *should* eventually redefine `resting_hr` (for strain/recovery), even if not now. ChatGPT and Gemini are more comfortable leaving it. **Action:** defer this part — Option 2's work becomes a future ticket.

- **Z-score vs percentile:** Gemini calls them "mathematically equivalent for non-parametric normalization" but notes percentiles are more robust against outlier spikes (e.g., a 120-BPM spike when getting up to use the bathroom). ChatGPT and Claude both lean percentile for rule-based systems. **Action:** use percentile.

## What we won't do (and why)

- **Decoupling `resting_hr` for strain.** Defer. The strain calculation just shipped; it's producing reasonable numbers. Changing the meaning of `resting_hr` invalidates downstream values without a clear immediate win. Add to `docs/SLEEP_STAGING.md` §14 as a follow-up.
- **Add explicit in-rule hysteresis.** Defer. The post-processing 3-epoch median filter already provides similar smoothing.
- **Replace ALL the within-night thresholds with percentiles.** The HF/LF-HF/HR-std percentiles are already in place and working. Only the Deep HR gate needs to change.
- **Implement the full "hybrid relative gate" architecture (ChatGPT's Option 4 in full).** That's a bigger refactor. The minimal intervention — replace one gate — captures most of the literature's prescription.

## Recommended path forward — concrete steps

### Step 1: Add HR percentile to within-night thresholds

In `src/openwhoop-algos/src/sleep_staging/classifier.rs`:

```rust
struct NightThresholds {
    hf_p50: Option<f64>,
    lfhf_p50: Option<f64>,
    hr_std_p50: Option<f64>,
    hr_p25: Option<f64>,  // NEW
}
```

Compute it the same way as the existing percentiles, using `hr_mean` from valid (non-Unknown) epochs.

### Step 2: Update Deep rule

Replace:
```rust
&& hr_mean < resting_hr + DEEP_HR_OFFSET_BPM
```
with:
```rust
&& hr_mean < hr_p25_threshold
```

Where `hr_p25_threshold = th.hr_p25.unwrap_or(resting_hr + DEEP_HR_OFFSET_BPM)` — falls back to old behavior on a degenerate night with no valid HR data, but otherwise uses the relative gate.

### Step 3: Bump classifier version

`CLASSIFIER_VERSION = "rule-v2"` (was `"rule-v1"`). This makes reclassification observably-distinct in the DB and lets us A/B compare.

### Step 4: Update constants

- Remove `DEEP_HR_OFFSET_BPM` from active use (keep it as a fallback constant, with a doc comment explaining it's only used when no within-night HR distribution is available).
- Add new constant `DEEP_HR_PERCENTILE: f64 = 0.25` with the literature citation.

### Step 5: Reclassify and verify

Against Brennen's tray DB copy:
- Expected: Deep ~15–25%, REM ~10–15%, Light ~50–65%, Wake/Unknown rest.
- Verify: Deep front-loaded (≥60% in first half), REM back-loaded (≥55% in second half).
- Performance score should rise back into 65–75 range from current 63.1.

### Step 6: Document the calibration history

Add an entry to `docs/SLEEP_STAGING.md` § "Calibration history" explaining the rule-v1 → rule-v2 transition, the literature basis, and the deferred `resting_hr` work.

### Step 7 (deferred): Phase 2 forward-compatibility note

Add a TODO in code or DECISIONS noting that when Phase 2 ML training begins, the features fed to LightGBM must be normalized to match what the rule-based classifier sees in production — per-night percentile rank or per-night z-score, not raw BPM. Otherwise the model will train on a distribution it never sees at inference time.

## Estimated effort

- Code changes: ~30 minutes (new field + computation + rule swap + version bump + constant rename).
- Reclassify + verify: ~10 minutes.
- Doc updates: ~15 minutes.
- Total: ~1 hour.

## Confidence

High. Three independent research passes produce convergent evidence pointing at the same answer. The published literature is unanimous on the design pattern (per-night normalization). The only uncertainty is the exact cutoff (25th vs 30th vs other), which requires on-device tuning regardless. Starting at 25th is the most-cited choice and matches the closest published rule (Perez-Pozuelo's HypnosPy ECDF approach).

## What this leaves on the table for later

- **Decoupled `resting_hr` for strain/recovery** — the `baseline.resting_hr` semantics issue. Worth a session of its own. Probably right answer is "compute resting HR from a quiet-wake or post-onset window, not from the nightly minimum."
- **HR-only deep-learning approach (Sridhar 2020 style)** — Phase 2 might get more lift by feeding the raw RR series to an ML model and skipping rule-based gates entirely. Not for this iteration.
- **Athletic-population auto-detection** — eventually we could detect "user has very low resting HR" and pick rules accordingly, but the per-night percentile approach largely auto-handles this without explicit detection.
- **Hysteresis as an in-rule check** — if the median filter doesn't smooth aggressively enough on real data, revisit.
