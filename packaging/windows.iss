#define AppName "Whisper Bro"
#define AppVersion "0.1.0"

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
Source: "..\target\release\whisper-bro.exe"; DestDir: "{app}"; Flags: ignoreversion
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
