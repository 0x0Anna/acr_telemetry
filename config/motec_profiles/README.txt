MoTeC LD export profiles (TOML)

Select in acr_recorder.toml:

  [export.motec]
  profile = "rally"

Files in this folder (or next to acr_export.exe as motec_profiles/) define
which channels are written to .ld and how sim fields map to them.

Built-in profiles (also embedded in the binary if files are missing):
  rbr.toml   - RBR / sim-style channel names (default)
  rally.toml - MoTeC Rally Basic / ADL names

Each [[channels]] entry:
  name     - channel id in the .ld file (must match your i2 workspace)
  unit     - MoTeC unit string
  source   - sim field id (see motec_profile.rs ChannelSource::parse)
  scale    - optional multiplier (default 1)
  offset   - optional offset (default 0)
  graphics - if true, channel is only written when a .graphics.rkyv sidecar exists
