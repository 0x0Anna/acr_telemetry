AcrGameClockMod — optional UE4SS Lua mod
=========================================

WARNING — USE AT YOUR OWN RISK
------------------------------
UE4SS injects code into the game process. That may violate the game's EULA,
break after updates, or be disallowed on some platforms. The acr_telemetry
authors are not liable for bans, crashes, or license issues.

The mod Lua source is not shipped in the public repository. Obtain main.lua
locally (see Scripts/README.txt) and copy this folder into:

  Assetto Corsa Rally\acr\Binaries\Win64\ue4ss\Mods\AcrGameClockMod\

UE4SS itself is NOT included — install from https://github.com/UE4SS-RE/RE-UE4SS

Output while driving:

  %APPDATA%\acr_telemetry\acr_game_clock.jsonl

acr_timing.toml:

  [game_clock]
  enabled = true
  sector_splits = true   # optional
