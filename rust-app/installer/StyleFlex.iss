#ifndef AppVersion
  #define AppVersion "0.5.19"
#endif

#define AppName "StyleFlex"
#define AppPublisher "StyleFlex contributors"
#define AppExeName "StyleFlex.exe"

[Setup]
AppId={{BE541A7B-AF41-4F29-8D3F-D9F75AB68F66}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
DefaultDirName={localappdata}\Programs\StyleFlex
UsePreviousAppDir=no
DefaultGroupName=StyleFlex
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\dist
OutputBaseFilename=StyleFlex-{#AppVersion}-win-x64-setup
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no
AppMutex=Local\StyleFlex.Desktop,Local\RustVoice.Desktop
UninstallDisplayIcon={app}\{#AppExeName}
SetupLogging=yes
SetupIconFile=..\..\resources\icons\styleflex.ico

[Tasks]
Name: "autostart"; Description: "Start StyleFlex with Windows"; Flags: unchecked
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked

[InstallDelete]
Type: files; Name: "{autodesktop}\Rust Voice.lnk"
Type: files; Name: "{userstartup}\Rust Voice.lnk"
Type: files; Name: "{group}\Rust Voice.lnk"

[Files]
Source: "..\dist\app\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\StyleFlex"; Filename: "{app}\{#AppExeName}"
Name: "{autodesktop}\StyleFlex"; Filename: "{app}\{#AppExeName}"; Tasks: desktopicon
Name: "{userstartup}\StyleFlex"; Filename: "{app}\{#AppExeName}"; Parameters: "--background"; Tasks: autostart

[Run]
Filename: "{app}\{#AppExeName}"; Description: "Launch StyleFlex"; Flags: nowait postinstall skipifsilent

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
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
    RemoveLegacyApplicationFiles;
end;
