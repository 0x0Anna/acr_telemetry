; ACR Recorder – Inno Setup script
; Build: iscc installer\ACR_Recorder.iss  (from repo root, after cargo build --release)

#define MyAppName "ACR Recorder"
#define MyAppVersion "0.1.0"
#define MyAppPublisher "ACR"
#define MyAppURL "https://github.com/your-repo/acc"
#define MyAppExeName "acr_recorder.exe"
#define MyAppBridgeExe "acr_telemetry_bridge.exe"

[Setup]
AppId={{A1B2C3D4-E5F6-7890-ABCD-EF1234567890}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
AppUpdatesURL={#MyAppURL}
DefaultDirName={autopf}\ACR_Recorder
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
; Output in repo root: installer\Output\ or custom
OutputDir=..\target\installer
OutputBaseFilename=ACR_Recorder_{#MyAppVersion}_setup
SetupIconFile=
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "german"; MessagesFile: "compiler:Languages\German.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "quicklaunchicon"; Description: "{cm:CreateQuickLaunchIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
; Binaries (build first: cargo build --release)
Source: "..\target\release\acr_recorder.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\release\acr_export.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\release\acr_telemetry_bridge.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\release\acr_analysis_export.exe"; DestDir: "{app}"; Flags: ignoreversion
; Config: installer template (relative paths) – only if not already present
Source: "..\config-examples\acr_recorder.installer.toml"; DestDir: "{app}"; DestName: "acr_recorder.toml"; Flags: ignoreversion onlyifdoesntexist uninsneveruninstall
Source: "..\config-examples\acr_telemetry_bridge.toml"; DestDir: "{app}"; Flags: ignoreversion onlyifdoesntexist uninsneveruninstall
Source: "..\config-examples\telemetry_color.toml"; DestDir: "{app}"; Flags: ignoreversion onlyifdoesntexist uninsneveruninstall
; Batch helpers (notes dir is configurable; user may put these elsewhere)
Source: "..\batch\acr_stop.bat"; DestDir: "{app}\batch"; Flags: ignoreversion
Source: "..\batch\acr_marker_good.bat"; DestDir: "{app}\batch"; Flags: ignoreversion
Source: "..\batch\acr_marker_bad.bat"; DestDir: "{app}\batch"; Flags: ignoreversion
Source: "..\batch\acr_note_aborted.bat"; DestDir: "{app}\batch"; Flags: ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Comment: "Start telemetry recorder"
Name: "{group}\ACR Telemetry Bridge"; Filename: "{app}\{#MyAppBridgeExe}"; Comment: "Live dashboard (HTTP)"
Name: "{group}\Uninstall {#MyAppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon
Name: "{userappdata}\Microsoft\Internet Explorer\Quick Launch\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: quicklaunchicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(MyAppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent
Filename: "{app}\{#MyAppBridgeExe}"; Description: "Start Telemetry Bridge (live dashboard)"; Flags: nowait postinstall skipifsilent unchecked

[Messages]
; German: optional welcome/finish lines
; (WizardImageFile etc. can be added for a "schöner" look)

[Code]
// Optional: copy acr_recorder.toml from installer template only if missing (already done via onlyifdoesntexist)
// Could add a "Open config folder" button in Finish page if needed.
