#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif

#define AppName "Keyboop"
#define AppPublisher "Keyboop contributors"
#define AppExeName "Keyboop.UI.exe"

[Setup]
AppId={{6E0B5850-6980-4A10-89A5-AE89DFE58284}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
DefaultDirName={localappdata}\Programs\Keyboop
DefaultGroupName=Keyboop
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\dist
OutputBaseFilename=Keyboop-{#AppVersion}-win-x64-setup
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no
AppMutex=Local\Keyboop.UI
UninstallDisplayIcon={app}\{#AppExeName}
SetupLogging=yes
SetupIconFile=..\resources\icons\keyboop.ico

[Tasks]
Name: "autostart"; Description: "Запускать Keyboop вместе с Windows"; Flags: unchecked
Name: "desktopicon"; Description: "Создать ярлык на рабочем столе"; Flags: unchecked

[Files]
Source: "..\dist\app\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\Keyboop"; Filename: "{app}\{#AppExeName}"
Name: "{autodesktop}\Keyboop"; Filename: "{app}\{#AppExeName}"; Tasks: desktopicon
Name: "{userstartup}\Keyboop"; Filename: "{app}\{#AppExeName}"; Tasks: autostart

[Run]
Filename: "{app}\{#AppExeName}"; Description: "Запустить Keyboop"; Flags: nowait postinstall skipifsilent
