; ACR Recorder – Inno Setup 6 installer
; Stage payload first:  pwsh install\build.ps1
; Compile only:         ISCC.exe /DMyAppVersion=0.1.0 install\ACR_Recorder.iss

#ifndef MyAppVersion
  #define MyAppVersion "0.0.6"
#endif

#define MyAppName "ACR Recorder"
#define MyAppPublisher "ACR Telemetry"
#define MyAppExeName "acr_recorder.exe"

[Setup]
AppId={{A3F8C2E1-9B4D-4A6E-8F2C-1D5E7A9B0C3D}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppVerName={#MyAppName} {#MyAppVersion}
AppPublisher={#MyAppPublisher}
DefaultDirName=C:\tools\acr_telemetry
DefaultGroupName=ACR Recorder
DisableProgramGroupPage=yes
DisableDirPage=no
OutputDir=..\target\install
OutputBaseFilename=ACR_Recorder_{#MyAppVersion}_setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=lowest
UninstallDisplayIcon={app}\{#MyAppExeName}
MinVersion=10.0

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "german"; MessagesFile: "compiler:Languages\German.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "launchrecorder"; Description: "Start ACR Recorder after installation"; GroupDescription: "After installation:"; Flags: unchecked
Name: "launchbridge"; Description: "Start Telemetry Bridge after installation"; GroupDescription: "After installation:"; Flags: unchecked

[Dirs]
Name: "{app}\telemetry_raw"; Permissions: users-modify
Name: "{app}\timing\runs"; Permissions: users-modify
Name: "{app}\reference_tracks"; Permissions: users-modify
Name: "{app}\assets\split_sounds"; Permissions: users-modify

[Files]
Source: "staging\acr_recorder.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "staging\acr_export.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "staging\acr_motec.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "staging\acr_telemetry_bridge.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "staging\acr_analysis_export.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "staging\acr_track_match.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "staging\acr_timing.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "staging\acr_rtss_osd.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "staging\acr_recorder.toml"; DestDir: "{app}"; Flags: onlyifdoesntexist ignoreversion
Source: "staging\acr_timing.toml"; DestDir: "{app}"; Flags: onlyifdoesntexist ignoreversion
Source: "staging\acr_track_match.toml"; DestDir: "{app}"; Flags: onlyifdoesntexist ignoreversion
Source: "staging\acr_telemetry_bridge.toml"; DestDir: "{app}"; Flags: onlyifdoesntexist ignoreversion
Source: "staging\telemetry_color.toml"; DestDir: "{app}"; Flags: onlyifdoesntexist ignoreversion
Source: "staging\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "staging\README.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "staging\batch\*"; DestDir: "{app}\batch"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "staging\docs\*"; DestDir: "{app}\docs"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "staging\config-examples\*"; DestDir: "{app}\config-examples"; Flags: onlyifdoesntexist ignoreversion
Source: "staging\motec_profiles\*"; DestDir: "{app}\motec_profiles"; Flags: onlyifdoesntexist ignoreversion
Source: "staging\timing\*"; DestDir: "{app}\timing"; Flags: ignoreversion recursesubdirs createallsubdirs; Excludes: "timing.db,timing.db-wal,timing.db-shm"
Source: "staging\reference_tracks\*"; DestDir: "{app}\reference_tracks"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "staging\assets\*"; DestDir: "{app}\assets"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "staging\voices\*"; DestDir: "{app}\voices"; Flags: ignoreversion recursesubdirs createallsubdirs; Excludes: "*.md"

[Icons]
Name: "{group}\ACR Recorder"; Filename: "{app}\{#MyAppExeName}"; WorkingDir: "{app}"
Name: "{group}\ACR MoTeC (live .ld)"; Filename: "{app}\acr_motec.exe"; WorkingDir: "{app}"
Name: "{group}\ACR Export"; Filename: "{app}\acr_export.exe"; WorkingDir: "{app}"
Name: "{group}\ACR Telemetry Bridge"; Filename: "{app}\acr_telemetry_bridge.exe"; WorkingDir: "{app}"
Name: "{group}\ACR Track Match"; Filename: "{app}\acr_track_match.exe"; WorkingDir: "{app}"; Parameters: "--live"
Name: "{group}\{#MyAppName} README"; Filename: "{app}\README.txt"
Name: "{group}\{cm:UninstallProgram,{#MyAppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\ACR Recorder"; Filename: "{app}\{#MyAppExeName}"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "Start ACR Recorder"; Flags: nowait postinstall skipifsilent; Tasks: launchrecorder; WorkingDir: "{app}"
Filename: "{app}\acr_telemetry_bridge.exe"; Description: "Start Telemetry Bridge"; Flags: nowait postinstall skipifsilent; Tasks: launchbridge; WorkingDir: "{app}"

[Code]
procedure InitializeWizard;
begin
  WizardForm.DirEdit.Hint := 'C:\tools\acr_telemetry';
end;
