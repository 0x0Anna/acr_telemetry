Optional WAV files for split feedback ([beep] / [cumulative_beep] in acr_timing.toml).

Set mode = "wav" or mode = "both", then point paths at your .wav files (relative to repo cwd when running acr_timing).

Example:
  faster_wav = "assets/split_sounds/fast.wav"
  slower_wav = "assets/split_sounds/slow.wav"

Per |Δ| tier (which *would* trigger 1 / 2 / 3 sine beeps — WAV plays once only):
  faster_wav_1, faster_wav_2, faster_wav_3
  slower_wav_1, slower_wav_2, slower_wav_3

Put any repetition inside the WAV file itself. Sine beeps still repeat 1–3× (mode sine/both).

If a tier file is missing, falls back to faster_wav / slower_wav, then sine (mode wav/both).
