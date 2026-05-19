Timing blame voice clips (WAV). Basename must match token + ".wav".
Played when a sector split is slower than PB (after correlation hints).

Required minimum:
  TimingSectorSlow.wav     — sector was slower than best

Optional per-factor (played after TimingSectorSlow, top 1–2):
  TimingExitSpeedLow.wav   — exit speed lower than PB pattern
  TimingExitSpeedHigh.wav
  TimingEntrySpeedLow.wav
  TimingEntrySpeedHigh.wav
  TimingThrottleHigh.wav
  TimingThrottleLow.wav
  TimingSlipAngleHigh.wav
  TimingSlipAngleLow.wav
  TimingSlipHigh.wav
  TimingSlipLow.wav
  TimingMinSlipLow.wav
  TimingMinSlipHigh.wav
  TimingDistanceLong.wav
  TimingDistanceShort.wav

Missing clips are skipped (logged to stderr).

Copilot crash / high-G (optional gimmick):
  CopilotAreYouOkGoGoGo.wav  — "Are you ok? Then GO GO GO!"
  High-G only: |g_force| >= 4g for 3s arms clip; plays on first 1–10 km/h crawl (45s cooldown).
  Position reset: HTML warning only, no copilot clip.
