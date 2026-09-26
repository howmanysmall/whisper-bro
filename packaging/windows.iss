#define AppName "Whisper Bro"

; The version and binary path come from Cargo.toml and the build directory via
; scripts/package-windows-installer.ps1 (mise run package:windows). These
; fallbacks are for manual ISCC runs; 0.0.0 marks an unstamped build.
#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef AppBinary
  #define AppBinary "..\target\release\whisper-bro.exe"
#endif

[Setup]
AppId=dev.whisper-bro
AppName={#AppName}
AppVersion={#AppVersion}
DefaultDirName={localappdata}\Programs\Whisper Bro
DefaultGroupName={#AppName}
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir=..\dist
OutputBaseFilename=whisper-bro-windows-x64-setup
Compression=lzma2
SolidCompression=yes
UninstallDisplayIcon={app}\whisper-bro.exe
CloseApplications=yes

[Files]
Source: "{#AppBinary}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "Configure Whisper Bro.cmd"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Whisper Bro"; Filename: "{app}\whisper-bro.exe"; Parameters: "run"
Name: "{group}\Configure Whisper Bro"; Filename: "{app}\Configure Whisper Bro.cmd"

[Tasks]
Name: "autostart"; Description: "Start Whisper Bro when I sign in"; Flags: unchecked

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Whisper Bro"; ValueData: """{app}\whisper-bro.exe"" run"; Tasks: autostart; Flags: uninsdeletevalue

[Run]
Filename: "{app}\Configure Whisper Bro.cmd"; Description: "Configure API keys"; Flags: postinstall skipifsilent shellexec
