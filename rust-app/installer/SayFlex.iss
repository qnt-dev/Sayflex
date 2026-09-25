#ifndef AppVersion
  #define AppVersion "0.8.2"
#endif

#define AppName "SayFlex"
#define AppPublisher "SayFlex contributors"
#define AppExeName "SayFlex.exe"

[Setup]
AppId={{BE541A7B-AF41-4F29-8D3F-D9F75AB68F66}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
DefaultDirName={localappdata}\Programs\SayFlex
UsePreviousAppDir=yes
DefaultGroupName=SayFlex
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\dist
OutputBaseFilename=SayFlex-{#AppVersion}-win-x64-setup
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no
AppMutex=Local\SayFlex.Desktop,Local\StyleFlex.Desktop,Local\RustVoice.Desktop
UninstallDisplayIcon={app}\{#AppExeName}
SetupLogging=yes
SetupIconFile=..\..\resources\icons\sayflex.ico

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"
Name: "ukrainian"; MessagesFile: "compiler:Languages\Ukrainian.isl"
Name: "spanish"; MessagesFile: "compiler:Languages\Spanish.isl"
Name: "brazilianportuguese"; MessagesFile: "compiler:Languages\BrazilianPortuguese.isl"
Name: "german"; MessagesFile: "compiler:Languages\German.isl"
Name: "french"; MessagesFile: "compiler:Languages\French.isl"
Name: "italian"; MessagesFile: "compiler:Languages\Italian.isl"
Name: "korean"; MessagesFile: "compiler:Languages\Korean.isl"

[CustomMessages]
english.AutostartTask=Start SayFlex with Windows
russian.AutostartTask=Запускать SayFlex вместе с Windows
ukrainian.AutostartTask=Запускати SayFlex разом із Windows
spanish.AutostartTask=Iniciar SayFlex con Windows
brazilianportuguese.AutostartTask=Iniciar o SayFlex com o Windows
german.AutostartTask=SayFlex mit Windows starten
french.AutostartTask=Démarrer SayFlex avec Windows
italian.AutostartTask=Avvia SayFlex con Windows
korean.AutostartTask=Windows 시작 시 SayFlex 실행
english.DesktopIconTask=Create a desktop shortcut
russian.DesktopIconTask=Создать ярлык на рабочем столе
ukrainian.DesktopIconTask=Створити ярлик на робочому столі
spanish.DesktopIconTask=Crear un acceso directo en el escritorio
brazilianportuguese.DesktopIconTask=Criar um atalho na área de trabalho
german.DesktopIconTask=Desktopverknüpfung erstellen
french.DesktopIconTask=Créer un raccourci sur le bureau
italian.DesktopIconTask=Crea un collegamento sul desktop
korean.DesktopIconTask=바탕 화면 바로 가기 만들기
english.LaunchTask=Launch SayFlex
russian.LaunchTask=Запустить SayFlex
ukrainian.LaunchTask=Запустити SayFlex
spanish.LaunchTask=Iniciar SayFlex
brazilianportuguese.LaunchTask=Iniciar o SayFlex
german.LaunchTask=SayFlex starten
french.LaunchTask=Lancer SayFlex
italian.LaunchTask=Avvia SayFlex
korean.LaunchTask=SayFlex 실행

[Tasks]
Name: "autostart"; Description: "{cm:AutostartTask}"; Flags: unchecked
Name: "desktopicon"; Description: "{cm:DesktopIconTask}"; Flags: unchecked

[InstallDelete]
Type: files; Name: "{autodesktop}\Rust Voice.lnk"
Type: files; Name: "{userstartup}\Rust Voice.lnk"
Type: files; Name: "{group}\Rust Voice.lnk"
Type: files; Name: "{autodesktop}\StyleFlex.lnk"
Type: files; Name: "{userstartup}\StyleFlex.lnk"
Type: files; Name: "{group}\StyleFlex.lnk"
; Remove files from the short-lived development build that accidentally
; bundled an unrelated experimental product into SayFlex.
Type: files; Name: "{app}\SayFlex.Pet.exe"
Type: filesandordirs; Name: "{app}\pet-assets"

[Files]
Source: "..\dist\app\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
; The same identity the application declares for itself. A shortcut without it
; is a different application as far as the taskbar is concerned, so pinning one
; would leave a second button beside the running one.
Name: "{group}\SayFlex"; Filename: "{app}\{#AppExeName}"; AppUserModelID: "SayFlex.Desktop"
Name: "{autodesktop}\SayFlex"; Filename: "{app}\{#AppExeName}"; Tasks: desktopicon; AppUserModelID: "SayFlex.Desktop"
Name: "{userstartup}\SayFlex"; Filename: "{app}\{#AppExeName}"; Parameters: "--background"; Tasks: autostart; AppUserModelID: "SayFlex.Desktop"

[Run]
Filename: "{app}\{#AppExeName}"; Description: "{cm:LaunchTask}"; Flags: nowait postinstall skipifsilent

[Code]
procedure RemoveLegacyApplicationFiles;
var
  LegacyDir: String;
begin
  LegacyDir := ExpandConstant('{localappdata}\Programs\RustVoice');
  DeleteFile(LegacyDir + '\rust-desktop.exe');
  DeleteFile(LegacyDir + '\rust-input-host.exe');
  DeleteFile(LegacyDir + '\rust-speech-worker-cpu.exe');
  DeleteFile(LegacyDir + '\rust-speech-worker-cuda.exe');
  DeleteFile(LegacyDir + '\rust-translation-worker.exe');
  DeleteFile(LegacyDir + '\LICENSE.txt');
  DeleteFile(LegacyDir + '\THIRD_PARTY_NOTICES.md');
  DeleteFile(LegacyDir + '\SECURITY.md');
  DeleteFile(LegacyDir + '\unins000.exe');
  DeleteFile(LegacyDir + '\unins000.dat');
  DeleteFile(LegacyDir + '\unins000.msg');
  RemoveDir(LegacyDir);

  { The former interim product name used the same AppId. Keep this cleanup
    explicit so an upgrade cannot leave two autostart entries or stale workers. }
  LegacyDir := ExpandConstant('{localappdata}\Programs\StyleFlex');
  DeleteFile(LegacyDir + '\StyleFlex.exe');
  DeleteFile(LegacyDir + '\StyleFlex.InputHost.exe');
  DeleteFile(LegacyDir + '\StyleFlex.Speech.Cpu.exe');
  DeleteFile(LegacyDir + '\StyleFlex.Speech.Cuda.exe');
  DeleteFile(LegacyDir + '\StyleFlex.Translation.exe');
  DeleteFile(LegacyDir + '\LICENSE.txt');
  DeleteFile(LegacyDir + '\THIRD_PARTY_NOTICES.md');
  DeleteFile(LegacyDir + '\SECURITY.md');
  DelTree(LegacyDir + '\pet-assets', True, True, True);
  DeleteFile(LegacyDir + '\unins000.exe');
  DeleteFile(LegacyDir + '\unins000.dat');
  DeleteFile(LegacyDir + '\unins000.msg');
  RemoveDir(LegacyDir);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
    RemoveLegacyApplicationFiles;
end;
