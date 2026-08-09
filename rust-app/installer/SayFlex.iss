#ifndef AppVersion
  #define AppVersion "0.5.23"
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
UsePreviousAppDir=no
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

[Tasks]
Name: "autostart"; Description: "Start SayFlex with Windows"; Flags: unchecked
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked

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
Name: "{group}\SayFlex"; Filename: "{app}\{#AppExeName}"
Name: "{autodesktop}\SayFlex"; Filename: "{app}\{#AppExeName}"; Tasks: desktopicon
Name: "{userstartup}\SayFlex"; Filename: "{app}\{#AppExeName}"; Parameters: "--background"; Tasks: autostart

[Run]
Filename: "{app}\{#AppExeName}"; Description: "Launch SayFlex"; Flags: nowait postinstall skipifsilent

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
