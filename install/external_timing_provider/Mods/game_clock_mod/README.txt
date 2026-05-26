Game clock mod — optional external timing provider
==================================================

WARNING — USE AT YOUR OWN RISK
------------------------------
An external timing provider may inject code into the game process. That may
violate the game's EULA, break after updates, or be disallowed on some platforms.
The acr_telemetry authors are not liable for bans, crashes, or license issues.

The mod source (main.lua) is not shipped in the public repository. Obtain it
locally (see Scripts/README.txt) and install per your provider's documentation.

Output while driving (when configured):

  %APPDATA%\acr_telemetry\acr_game_clock.jsonl

acr_timing.toml:

  [game_clock]
  enabled = true
  sector_splits = true   # optional

RTSS: minimal preset shows "Timer ready" when JSONL is fresh, else "Timing ready".
