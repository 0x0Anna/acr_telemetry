MoTeC LD export profiles (TOML)

Select in acr_recorder.toml:

  [export.motec]
  profile = "rally"

Files in this folder (or next to acr_export.exe as motec_profiles/) define
which channels are written to .ld and how sim fields map to them.

Full guide (create / adapt workspaces): docs/MOTEC_PROFILES.md

Built-in profiles (also embedded in the binary if files are missing):
  rbr.toml   - RBR / sim-style channel names (default)
  rally.toml - MoTeC Rally Basic / ADL names
  all_data.toml - All currently supported MoTeC exporter sources

Each [[channels]] entry:
  name     - channel id in the .ld file (must match your i2 workspace)
  unit     - MoTeC unit string
  source   - sim field id (see docs/MOTEC_PROFILES.md)
  scale    - optional multiplier (default 1)
  offset   - optional offset (default 0)
  graphics - if true, channel is only written when a .graphics.rkyv sidecar exists
