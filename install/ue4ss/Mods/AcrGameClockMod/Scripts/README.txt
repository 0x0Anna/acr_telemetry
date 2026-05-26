AcrGameClockMod — Scripts (local only)
======================================

The Lua mod source (main.lua) is NOT in the public Git repository.

For local development, place main.lua here manually. A backup may exist under:

  secret/ue4ss_mod_backup/

Or restore from the local Git branch (never push to origin):

  git show local/backup-with-gameclock-mod:install/ue4ss/Mods/AcrGameClockMod/Scripts/main.lua > main.lua

After copying, enable the mod in your game UE4SS mods.txt:

  AcrGameClockMod : 1

See docs/UE4SS_SETUP.md when available in your tree, or secret/local_snapshot_*/docs/UE4SS_SETUP.md.
