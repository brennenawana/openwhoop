Recommended approach
Option 3 (Within-night percentile)—or a closely related Option 4 (Within-night Z-score)—is the strongest path forward for your Phase 1 classifier. Your current absolute offset is structurally flawed because the nightly minimum HR is the nadir of Deep sleep. Anchoring a threshold to the absolute minimum restricts the gate to only the deepest fraction of Deep sleep, mathematically guaranteeing an underestimation for athletes with compressed HR floors.
By changing the rule to hr_mean < 30th percentile of this night's epoch HRs or z_score(hr_mean) < -0.5, you decouple sleep staging from the longitudinal baseline. This self-calibrates to night-to-night variance (e.g., an elevated HR curve after late meals or alcohol) while fully protecting WHOOP’s downstream Strain and Recovery algorithms that rely on the resting_hr variable. The risk of this metric degenerating on a night with zero true Deep sleep is heavily mitigated because your rule requires a logical AND with stillness, temporal position, and HF power.
Key evidence
Per-night normalization is standard: Consumer wearable algorithms (Oura, Fitbit) and state-of-the-art academic models heavily favor within-night relative features (Z-scores, rolling averages, or min-max normalization) to absorb baseline drift and fitness disparities rather than absolute thresholds[1][2].
HR is a weak independent classifier: Absolute HR distinguishes Wake/REM (higher, variable) from NREM (lower, stable) but struggles to separate Light (N2) from Deep (N3). The separation of N2 from N3 heavily relies on HRV (e.g., increased HF power) and respiratory stability[3][4].
Athletes have wider HR gaps: Highly trained endurance athletes experience profound parasympathetic dominance during slow-wave sleep. Their nocturnal minimums routinely drop 10–15+ BPM below their morning awake resting HR, making fixed offsets fundamentally restrictive for this population[5][6].
Per-question answers
Q1: In published wrist-based classifiers, how is the HR reference point defined?
It is rarely an absolute physiological baseline. Machine learning classifiers (like Random Forests or LSTMs) typically ingest rolling relative features (e.g., an epoch's delta from the trailing 10-minute moving average) or mathematically normalized distributions (Z-scores per night)[2][4].
Q2: Is there normative data on the relationship between nightly minimum HR and awake resting HR?
Yes. In normal adults, nightly minimum HR is typically 5–10 BPM lower than morning awake resting HR. In highly trained athletes, this gap widens significantly due to extreme vagal tone during slow-wave sleep, where the minimum can drop 10–20 BPM below the awake baseline[5]. This confirms your hypothesis that using the minimum as a global anchor breaks for athletic users.
Q3: How discriminating is HR alone for Deep vs Light sleep in wrist-based staging?
Very weak. Heart rate acts as a contextual gate—it drops from Wake to Sleep, and fluctuates wildly in REM. However, the absolute HR difference between Light (N2) and Deep (N3) is heavily overlapping[3]. Consumer staging models rely on HRV (specifically HF power/parasympathetic tone) and stillness to isolate Deep sleep[1][7].
Q4: Is there any published approach that uses "percentile of this night's HR distribution"?
Yes. While many deep-learning models use standard scaling (Z-scores), percentiles are mathematically equivalent for non-parametric normalization and are frequently used in rule-based physiological algorithms because they are immune to extreme outlier spikes (e.g., waking up to go to the bathroom)[4][8].
Consumer-wearable validation study comparisons
Wulterkens 2021 & Fonseca 2023 (Philips): These models use neural networks fed by instantaneous HR and body movement. They do not use absolute HR gates; they rely on the network to learn relative temporal dynamics (e.g., sustained drops in HR variance) and frequency-domain HRV[3][7].
Altini & Kinnunen 2021 (Oura): Use Random Forests on ANS-mediated features (HRV, temperature, movement). Features are standardized, emphasizing epoch-by-epoch deviations from the night's mean rather than longitudinal baselines[1].
Beattie 2017 (Fitbit): Extracted 60+ features per epoch using Linear Discriminant Analysis and Random Forests. They explicitly relied on rolling variances and normalized spectral power moving averages rather than absolute user baselines[2].
Kotzen 2022 / Zhai 2023 (MESA benchmarks): End-to-end deep learning where input signals are explicitly Z-score normalized per recording, removing absolute scale entirely[4].
MESA-style preprocessing conventions
In MESA benchmarks (such as the SLAMSS paper by Zhai et al., or SleepPPG-Net), it is standard convention to apply per-recording standardization (Z-score normalization) to heart rate and respiratory signals before feeding them to the classifier[4]. This explicitly removes the subject's baseline and scale, forcing the model to classify based on relative within-night architecture. If Phase 2 will train LightGBM on MESA, adopting within-night percentiles or Z-scores in Phase 1 directly aligns your rule-based feature space with the normalized arrays your future ML model will expect.
Athletic-population considerations
Athletes with sub-50 HRs exhibit compressed absolute variance during sleep but highly distinct autonomic nervous system shifts. Because their minimum HR is so low, their absolute HR curve sits flat near the floor, but their parasympathetic tone (HF power) spikes dramatically during N3[6]. Rigid absolute thresholds almost always fail on this demographic. The published literature strongly advocates for individualized, dynamic baselines (which your within-night percentile achieves) to prevent athletic users from seeing 0% Deep sleep[5].
If I chose differently
If you pick Option 1 (Widen the offset to +12/+15): You retain the architectural flaw. While it "fixes" the athlete's 48 BPM night by brute force, it will falsely label noisy Light sleep as Deep for a sedentary user whose HR fluctuates wildly, heavily degrading your specificity.
If you pick Option 2 (Redefine resting_hr): You risk corrupting downstream algorithms (Strain, Recovery, Sleep Need) that expect resting_hr to be the true physiological floor. Decoupling the sleep staging logic from global user variables is a much safer software design.
Uncertainties / what the literature can't tell me
The literature lacks explicit rule-based heuristic trees for modern consumer wearables, as they have all transitioned to proprietary ML models (Random Forests or Neural Networks). We have to infer their threshold logic from their feature engineering. Furthermore, whether to use Z-score or Percentiles is a matter of engineering preference: ML pipelines prefer Z-scores because they are differentiable, but percentiles are often vastly superior for rule-based systems because they are robust against extreme artifacts (like a 120 BPM spike when getting out of bed). The exact percentile cutoff (e.g., 25th vs 30th) will require empirical tuning against your specific distribution of Light vs Deep sleep.
Sources
help
nih.gov
nih.gov
dovepress.com
nih.gov
nih.gov
ultrahuman.com
tue.nl
nih.gov

# ChatGPT Deep Research

