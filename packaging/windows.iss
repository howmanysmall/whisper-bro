#define AppName "Yap"

; The version and binary path come from Cargo.toml and the build directory via
; scripts/package-windows-installer.ps1 (mise run package:windows). These
; fallbacks are for manual ISCC runs; 0.0.0 marks an unstamped build.
#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef AppVersionNumeric
  #define AppVersionNumeric "0.0.0"
#endif
#ifndef AppBinary
  #define AppBinary "..\target\release\yap.exe"
#endif

[Setup]
AppId=dev.yap
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher=HowManySmall
AppPublisherURL=https://github.com/howmanysmall/yap
AppSupportURL=https://github.com/howmanysmall/yap/issues
AppUpdatesURL=https://github.com/howmanysmall/yap/releases
VersionInfoVersion={#AppVersionNumeric}
SetupIconFile=yap.ico
DefaultDirName={localappdata}\Programs\Yap
DefaultGroupName={#AppName}
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir=..\dist
OutputBaseFilename=yap-windows-x64-setup
Compression=lzma2
SolidCompression=yes
UninstallDisplayIcon={app}\yap.exe
CloseApplications=yes

[Files]
Source: "{#AppBinary}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "Configure Yap.cmd"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Yap"; Filename: "{app}\yap.exe"; Parameters: "run"
Name: "{group}\Configure Yap"; Filename: "{app}\Configure Yap.cmd"

[Tasks]
Name: "autostart"; Description: "Start Yap when I sign in"; Flags: unchecked

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Yap"; ValueData: """{app}\yap.exe"" run"; Tasks: autostart; Flags: uninsdeletevalue

[Run]
Filename: "{app}\yap.exe"; Parameters: "run"; Description: "Launch Yap"; Flags: postinstall nowait skipifsilent
Filename: "{app}\Configure Yap.cmd"; Description: "Set the OpenRouter API key"; Flags: postinstall skipifsilent shellexec
