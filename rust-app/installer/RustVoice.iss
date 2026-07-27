#ifndef AppVersion
  #define AppVersion "0.5.5"
#endif

#define AppName "Rust Voice"
#define AppPublisher "Rust Voice contributors"
#define AppExeName "rust-desktop.exe"

[Setup]
AppId={{BE541A7B-AF41-4F29-8D3F-D9F75AB68F66}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
DefaultDirName={localappdata}\Programs\RustVoice
DefaultGroupName=Rust Voice
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\dist
OutputBaseFilename=RustVoice-{#AppVersion}-win-x64-setup
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no
AppMutex=Local\RustVoice.Desktop
UninstallDisplayIcon={app}\{#AppExeName}
SetupLogging=yes
SetupIconFile=..\..\resources\icons\keyboop.ico

[Tasks]
Name: "autostart"; Description: "Start Rust Voice with Windows"; Flags: unchecked
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked

[Files]
Source: "..\dist\app\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\Rust Voice"; Filename: "{app}\{#AppExeName}"
Name: "{autodesktop}\Rust Voice"; Filename: "{app}\{#AppExeName}"; Tasks: desktopicon
Name: "{userstartup}\Rust Voice"; Filename: "{app}\{#AppExeName}"; Parameters: "--background"; Tasks: autostart

[Run]
Filename: "{app}\{#AppExeName}"; Description: "Launch Rust Voice"; Flags: nowait postinstall skipifsilent
