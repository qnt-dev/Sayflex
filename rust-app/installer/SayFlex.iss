#ifndef AppVersion
  #define AppVersion "0.8.11"
#endif

#ifdef TestNvidia
  #define AppName "SayFlex Test"
  #define AppDirectoryName "SayFlex-Test-Nvidia"
  #define AppIdentity "SayFlex.TestNvidia.Desktop"
  #define InstallerAppId "{{62070119-61B5-41BE-A387-381D775B526B}"
  #define InstallerOutput "..\dist\test-nvidia"
  #define InstallerName "SayFlex-" + AppVersion + "-nvidia-test-win-x64-setup"
  #ifndef AppSourceDir
    #define AppSourceDir "..\dist\test-nvidia\app"
  #endif
  #if !FileExists(AddBackslash(AppSourceDir) + "sayflex-test-channel")
    #error Test packages require the sayflex-test-channel marker in the payload.
  #endif
#else
  #define AppName "SayFlex"
  #define AppDirectoryName "SayFlex"
  #define AppIdentity "SayFlex.Desktop"
  #define InstallerAppId "{{BE541A7B-AF41-4F29-8D3F-D9F75AB68F66}"
  #define InstallerOutput "..\dist"
  #define InstallerName "SayFlex-" + AppVersion + "-win-x64-setup"
  #ifndef AppSourceDir
    #define AppSourceDir "..\dist\app"
  #endif
#endif
#define AppPublisher "SayFlex contributors"
#define AppExeName "SayFlex.exe"

[Setup]
AppId={#InstallerAppId}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
DefaultDirName={localappdata}\Programs\{#AppDirectoryName}
#ifdef TestNvidia
UsePreviousAppDir=no
DisableDirPage=yes
#else
UsePreviousAppDir=yes
#endif
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
ShowLanguageDialog=yes
DisableWelcomePage=no
DisableReadyPage=no
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#InstallerOutput}
OutputBaseFilename={#InstallerName}
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
#ifdef TestNvidia
CloseApplications=no
#else
CloseApplications=yes
#endif
RestartApplications=no
#ifdef TestNvidia
AppMutex=Local\SayFlex.TestNvidia.Desktop
#else
AppMutex=Local\SayFlex.Desktop,Local\StyleFlex.Desktop,Local\RustVoice.Desktop
#endif
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
InstallAction=Install
UpdateAction=Update
ReinstallAction=Reinstall
ReplaceAction=Install this version
MaintenanceFound=Found %1 %2%nInstallation folder: %3
MaintenanceTarget=Version to install: %1
MaintenancePreserved=Your settings, history, downloaded models and runtimes will be kept.
MaintenanceLanguage=Application language after installation: %1
MaintenanceLanguageKept=Your current application language will be kept.
MaintenanceContinue=Choose Next to continue.
MaintenanceReadyPrompt=Choose %1 to continue, or Back to review the settings.
russian.InstallAction=Установить
russian.UpdateAction=Обновить
russian.ReinstallAction=Переустановить
russian.ReplaceAction=Установить эту версию
russian.MaintenanceFound=Обнаружена установленная версия: %1 %2%nПапка установки: %3
russian.MaintenanceTarget=Устанавливаемая версия: %1
russian.MaintenancePreserved=Настройки, история, скачанные модели и компоненты сохранятся.
russian.MaintenanceLanguage=Язык приложения после установки: %1
russian.MaintenanceLanguageKept=Текущий язык приложения сохранится.
russian.MaintenanceContinue=Нажмите «Далее», чтобы продолжить.
russian.MaintenanceReadyPrompt=Нажмите «%1», чтобы продолжить, или «Назад», чтобы проверить параметры.
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
#ifndef TestNvidia
Name: "autostart"; Description: "{cm:AutostartTask}"; Flags: unchecked
#endif
Name: "desktopicon"; Description: "{cm:DesktopIconTask}"; Flags: unchecked

#ifndef TestNvidia
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
#endif

[Files]
Source: "{#AppSourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[UninstallDelete]
; Written after installation, separately from the versioned application payload.
Type: files; Name: "{app}\installer-language"

[Icons]
; The same identity the application declares for itself. A shortcut without it
; is a different application as far as the taskbar is concerned, so pinning one
; would leave a second button beside the running one.
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExeName}"; AppUserModelID: "{#AppIdentity}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; Tasks: desktopicon; AppUserModelID: "{#AppIdentity}"
#ifndef TestNvidia
Name: "{userstartup}\SayFlex"; Filename: "{app}\{#AppExeName}"; Parameters: "--background"; Tasks: autostart; AppUserModelID: "SayFlex.Desktop"
#endif

[Run]
#ifdef TestNvidia
Filename: "{app}\{#AppExeName}"; Description: "Launch SayFlex Test"; Flags: nowait postinstall skipifsilent unchecked
#else
Filename: "{app}\{#AppExeName}"; Description: "{cm:LaunchTask}"; Flags: nowait postinstall skipifsilent
#endif

[Code]
type
  TInstallationGuid = record
    D1: LongWord;
    D2: Word;
    D3: Word;
    D4: array[0..7] of Byte;
  end;

function CoCreateGuid(var Guid: TInstallationGuid): Integer;
  external 'CoCreateGuid@{sys}\ole32.dll stdcall';
function StringFromGUID2(var Guid: TInstallationGuid; Buffer: String; MaxLength: Integer): Integer;
  external 'StringFromGUID2@{sys}\ole32.dll stdcall';

var
  ExistingInstallation: Boolean;
  ExistingVersion: String;
  ExistingDirectory: String;
  ExplicitInstallerLanguage: Boolean;
  OriginalNextButtonLeft: Integer;
  OriginalNextButtonWidth: Integer;
  OriginalBackButtonLeft: Integer;

function OwnUninstallKey: String;
begin
  Result := ExpandConstant('Software\Microsoft\Windows\CurrentVersion\Uninstall\{#InstallerAppId}_is1');
end;

function InstallationAction(Found: Boolean; OldVersion, NewVersion: String): String;
var
  OldPacked, NewPacked: Int64;
  Difference: Integer;
begin
  Result := 'InstallAction';
  if not Found then Exit;
  Result := 'ReplaceAction';
  if StrToVersion(OldVersion, OldPacked) and StrToVersion(NewVersion, NewPacked) then begin
    Difference := ComparePackedVersion(NewPacked, OldPacked);
    if Difference = 0 then Result := 'ReinstallAction'
    else if Difference > 0 then Result := 'UpdateAction';
  end;
end;

function ShouldApplyInstallerLanguage(Existing, Silent, ExplicitLanguage: Boolean): Boolean;
begin
  Result := (not Existing) or (not Silent) or ExplicitLanguage;
end;

function NewInstallationId: String;
var
  Guid: TInstallationGuid;
  Buffer: String;
begin
  if CoCreateGuid(Guid) <> 0 then
    RaiseException('Could not create the installation language request identifier.');
  SetLength(Buffer, 39);
  if StringFromGUID2(Guid, Buffer, 39) <> 39 then
    RaiseException('Could not format the installation language request identifier.');
  Result := Lowercase(Copy(Buffer, 2, 36));
end;

function InstallerLanguageRequest(Language, InstallationId: String): String;
begin
  { ActiveLanguage is one of the nine ASCII names in [Languages], and the ID
    is a native UUID. The resulting ASCII JSON is also valid UTF-8. }
  Result := '{"language":"' + Language + '","installation_id":"' + InstallationId + '"}';
end;

function InstallerLanguageDisplayName: String;
begin
  case ActiveLanguage of
    'russian': Result := 'Русский';
    'ukrainian': Result := 'Українська';
    'spanish': Result := 'Español';
    'brazilianportuguese': Result := 'Português (Brasil)';
    'german': Result := 'Deutsch';
    'french': Result := 'Français';
    'italian': Result := 'Italiano';
    'korean': Result := '한국어';
  else
    Result := 'English';
  end;
end;

function MaintenanceSummary: String;
begin
  Result := '';
  if ExistingInstallation then
    Result := FmtMessage(CustomMessage('MaintenanceFound'), ['{#AppName}', ExistingVersion,
      ExistingDirectory]) + #13#10 + #13#10;
  Result := Result + FmtMessage(CustomMessage('MaintenanceTarget'), ['{#AppVersion}']) + #13#10;
  if ExistingInstallation then
    Result := Result + CustomMessage('MaintenancePreserved') + #13#10;
  if ShouldApplyInstallerLanguage(ExistingInstallation, WizardSilent, ExplicitInstallerLanguage) then
    Result := Result + FmtMessage(CustomMessage('MaintenanceLanguage'), [InstallerLanguageDisplayName])
  else
    Result := Result + CustomMessage('MaintenanceLanguageKept');
end;

procedure InitializeWizard;
var
  Index: Integer;
  Key: String;
begin
  OriginalNextButtonLeft := WizardForm.NextButton.Left;
  OriginalNextButtonWidth := WizardForm.NextButton.Width;
  OriginalBackButtonLeft := WizardForm.BackButton.Left;
  { Both channels install per-user in 64-bit mode. Never inspect the other
    channel's AppId or use a production profile as a test-install detector. }
  Key := OwnUninstallKey;
  ExistingInstallation := RegQueryStringValue(HKCU64, Key, 'DisplayVersion', ExistingVersion);
  if ExistingInstallation then begin
    if not RegQueryStringValue(HKCU64, Key, 'Inno Setup: App Path', ExistingDirectory) then
      RegQueryStringValue(HKCU64, Key, 'InstallLocation', ExistingDirectory);
  end;
  ExplicitInstallerLanguage := False;
  for Index := 1 to ParamCount do
    if CompareText(Copy(ParamStr(Index), 1, 6), '/LANG=') = 0 then
      ExplicitInstallerLanguage := True;
  WizardForm.WelcomeLabel1.Caption :=
    CustomMessage(InstallationAction(ExistingInstallation, ExistingVersion, '{#AppVersion}')) + ' {#AppName}';
  WizardForm.WelcomeLabel2.Caption := MaintenanceSummary + #13#10 + #13#10 + CustomMessage('MaintenanceContinue');
end;

procedure CurPageChanged(CurPageID: Integer);
var
  Action: String;
  ExtraWidth: Integer;
begin
  { Keep Cancel fixed and preserve the gap between Back and the action.
    Restore the normal layout when returning to earlier wizard pages. }
  WizardForm.NextButton.Width := OriginalNextButtonWidth;
  WizardForm.NextButton.Left := OriginalNextButtonLeft;
  WizardForm.BackButton.Left := OriginalBackButtonLeft;
  if CurPageID = wpReady then begin
    Action := CustomMessage(InstallationAction(ExistingInstallation, ExistingVersion, '{#AppVersion}'));
    WizardForm.PageNameLabel.Caption := Action + ' {#AppName}';
    WizardForm.ReadyLabel.Caption := FmtMessage(CustomMessage('MaintenanceReadyPrompt'), [Action]);
    WizardForm.NextButton.Caption := Action;
    ExtraWidth := ScaleX(130) - OriginalNextButtonWidth;
    if ExtraWidth > 0 then begin
      WizardForm.NextButton.Width := OriginalNextButtonWidth + ExtraWidth;
      WizardForm.NextButton.Left := OriginalNextButtonLeft - ExtraWidth;
      WizardForm.BackButton.Left := OriginalBackButtonLeft - ExtraWidth;
    end;
  end;
end;

function UpdateReadyMemo(Space, NewLine, MemoUserInfoInfo, MemoDirInfo, MemoTypeInfo,
  MemoComponentsInfo, MemoGroupInfo, MemoTasksInfo: String): String;
begin
  Result := MaintenanceSummary + NewLine + NewLine + MemoDirInfo;
  if MemoTasksInfo <> '' then Result := Result + NewLine + NewLine + MemoTasksInfo;
end;

function CanonicalInstallPath(Path: String): String;
begin
  Result := RemoveBackslashUnlessRoot(PathNormalizeSlashes(ExpandFileName(Path)));
  while (Length(Result) > 0) and
    ((Result[Length(Result)] = '.') or (Result[Length(Result)] = ' ')) do
    Delete(Result, Length(Result), 1);
end;

#ifdef TestNvidia
function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';
  { Also reject /DIR overrides: a marker must never enter the normal install. }
  if CompareText(CanonicalInstallPath(ExpandConstant('{app}')),
      CanonicalInstallPath(ExpandConstant('{localappdata}\Programs\SayFlex-Test-Nvidia'))) <> 0 then
    Result := 'SayFlex Test must be installed in its separate SayFlex-Test-Nvidia directory.';
end;
#else
procedure RemoveLegacyApplicationFiles;
var
  LegacyDir: String;
  CurrentDir: String;
begin
  CurrentDir := CanonicalInstallPath(ExpandConstant('{app}'));
  LegacyDir := ExpandConstant('{localappdata}\Programs\RustVoice');
  if CompareText(CanonicalInstallPath(LegacyDir), CurrentDir) <> 0 then begin
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
  end else
    Log('Skipping RustVoice cleanup because it is the active install directory.');

  { The former interim product name used the same AppId. Keep this cleanup
    explicit so an upgrade cannot leave two autostart entries or stale workers. }
  LegacyDir := ExpandConstant('{localappdata}\Programs\StyleFlex');
  if CompareText(CanonicalInstallPath(LegacyDir), CurrentDir) <> 0 then begin
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
  end else
    Log('Skipping StyleFlex cleanup because it is the active install directory.');
end;
#endif

procedure SaveInstallerLanguage;
begin
  { A silent upgrade without /LANG must not override a user preference.
    Keep any earlier pending explicit request intact until the app consumes it. }
  if not ShouldApplyInstallerLanguage(ExistingInstallation, WizardSilent, ExplicitInstallerLanguage) then
    Exit;
  { Durable even when launch is skipped. Each successful installation gets a
    fresh ID; the app atomically acknowledges it with its saved locale once.
    Do not create app-data here: that could suppress legacy-profile migration. }
  if not SaveStringToFile(ExpandConstant('{app}\installer-language'),
      InstallerLanguageRequest(ActiveLanguage, NewInstallationId), False) then
    RaiseException('Could not save the selected application language.');
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then begin
    SaveInstallerLanguage;
#ifndef TestNvidia
    RemoveLegacyApplicationFiles;
#endif
  end;
end;
