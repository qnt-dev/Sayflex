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
UninstallDeleteModels=Also delete the downloaded speech and translation models and the optional runtimes?%n%nThey can take several gigabytes in %1. You can download them again later.
russian.UninstallDeleteModels=Удалить также скачанные модели речи и перевода и дополнительные компоненты?%n%nОни могут занимать несколько гигабайт в %1. Позже их можно скачать снова.
ukrainian.UninstallDeleteModels=Видалити також завантажені моделі мовлення й перекладу та додаткові компоненти?%n%nВони можуть займати кілька гігабайтів у %1. Пізніше їх можна завантажити знову.
spanish.UninstallDeleteModels=¿Eliminar también los modelos de voz y traducción descargados y los componentes opcionales?%n%nPueden ocupar varios gigabytes en %1. Podrás descargarlos de nuevo más tarde.
brazilianportuguese.UninstallDeleteModels=Excluir também os modelos de fala e tradução baixados e os componentes opcionais?%n%nEles podem ocupar vários gigabytes em %1. Você pode baixá-los novamente depois.
german.UninstallDeleteModels=Auch die heruntergeladenen Sprach- und Übersetzungsmodelle und die optionalen Laufzeitkomponenten löschen?%n%nSie können in %1 mehrere Gigabyte belegen. Sie lassen sich später erneut herunterladen.
french.UninstallDeleteModels=Supprimer aussi les modèles vocaux et de traduction téléchargés et les composants facultatifs ?%n%nIls peuvent occuper plusieurs gigaoctets dans %1. Vous pourrez les télécharger à nouveau.
italian.UninstallDeleteModels=Eliminare anche i modelli vocali e di traduzione scaricati e i componenti facoltativi?%n%nPossono occupare diversi gigabyte in %1. Potrai scaricarli di nuovo in seguito.
korean.UninstallDeleteModels=다운로드한 음성 및 번역 모델과 선택 구성 요소도 삭제할까요?%n%n%1에서 몇 기가바이트를 차지할 수 있습니다. 나중에 다시 다운로드할 수 있습니다.
UninstallDeletePersonalData=Also delete your dictation and translation history, settings and saved service keys?%n%nEverything left in %1 will be removed. This cannot be undone.
russian.UninstallDeletePersonalData=Удалить также историю диктовки и перевода, настройки и сохранённые ключи сервисов?%n%nВсё, что осталось в %1, будет удалено. Это нельзя отменить.
ukrainian.UninstallDeletePersonalData=Видалити також історію диктування й перекладу, налаштування та збережені ключі сервісів?%n%nУсе, що залишилося в %1, буде видалено. Це не можна скасувати.
spanish.UninstallDeletePersonalData=¿Eliminar también el historial de dictado y traducción, la configuración y las claves de servicio guardadas?%n%nSe eliminará todo lo que quede en %1. No se puede deshacer.
brazilianportuguese.UninstallDeletePersonalData=Excluir também o histórico de ditado e tradução, as configurações e as chaves de serviço salvas?%n%nTudo o que restar em %1 será removido. Isso não pode ser desfeito.
german.UninstallDeletePersonalData=Auch den Diktat- und Übersetzungsverlauf, die Einstellungen und gespeicherte Dienstschlüssel löschen?%n%nAlles, was in %1 verbleibt, wird entfernt. Das lässt sich nicht rückgängig machen.
french.UninstallDeletePersonalData=Supprimer aussi l'historique de dictée et de traduction, les paramètres et les clés de service enregistrées ?%n%nTout ce qui reste dans %1 sera supprimé. Cette action est irréversible.
italian.UninstallDeletePersonalData=Eliminare anche la cronologia di dettatura e traduzione, le impostazioni e le chiavi dei servizi salvate?%n%nTutto ciò che rimane in %1 verrà rimosso. L'operazione non può essere annullata.
korean.UninstallDeletePersonalData=받아쓰기 및 번역 기록, 설정, 저장된 서비스 키도 삭제할까요?%n%n%1에 남아 있는 모든 항목이 제거됩니다. 이 작업은 되돌릴 수 없습니다.
ProcessorTooOld=SayFlex needs a processor with SSE4.2 instructions (Intel or AMD, 2009 or newer). This processor does not have them, so setup will close.
russian.ProcessorTooOld=SayFlex нужен процессор с инструкциями SSE4.2 (Intel или AMD, 2009 года и новее). У этого процессора их нет, поэтому установка будет закрыта.
ukrainian.ProcessorTooOld=SayFlex потрібен процесор з інструкціями SSE4.2 (Intel або AMD, 2009 року й новіший). Цей процесор їх не має, тому встановлення буде закрито.
spanish.ProcessorTooOld=SayFlex necesita un procesador con instrucciones SSE4.2 (Intel o AMD, de 2009 o posterior). Este procesador no las tiene, así que la instalación se cerrará.
brazilianportuguese.ProcessorTooOld=O SayFlex precisa de um processador com instruções SSE4.2 (Intel ou AMD, de 2009 ou mais recente). Este processador não as tem, então a instalação será fechada.
german.ProcessorTooOld=SayFlex benötigt einen Prozessor mit SSE4.2-Befehlen (Intel oder AMD, ab 2009). Dieser Prozessor hat sie nicht, daher wird die Installation beendet.
french.ProcessorTooOld=SayFlex nécessite un processeur avec les instructions SSE4.2 (Intel ou AMD, 2009 ou plus récent). Ce processeur ne les a pas : l'installation va se fermer.
italian.ProcessorTooOld=SayFlex richiede un processore con istruzioni SSE4.2 (Intel o AMD, dal 2009 in poi). Questo processore non le ha, quindi l'installazione verrà chiusa.
korean.ProcessorTooOld=SayFlex에는 SSE4.2 명령을 지원하는 프로세서(2009년 이후 Intel 또는 AMD)가 필요합니다. 이 프로세서는 지원하지 않으므로 설치를 종료합니다.
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
; Earlier universal installers bundled NVIDIA cuBLAS here. No installer ships
; NVIDIA files any more; the app downloads them on the user's request.
Type: filesandordirs; Name: "{app}\runtimes\cuda"
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

function IsProcessorFeaturePresent(Feature: LongWord): LongBool;
  external 'IsProcessorFeaturePresent@kernel32.dll stdcall';

const
  PF_SSE4_2_INSTRUCTIONS_AVAILABLE = 38;

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

{ The oldest worker is built for SSE4.2 (every x86-64 processor since 2009).
  Refuse clearly, before any file is installed, instead of installing a
  program that cannot start. Empty when the processor is supported. }
function UnsupportedProcessorMessage: String;
begin
  if IsProcessorFeaturePresent(PF_SSE4_2_INSTRUCTIONS_AVAILABLE) then
    Result := ''
  else
    Result := CustomMessage('ProcessorTooOld');
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
  Result := UnsupportedProcessorMessage;
  if Result <> '' then
    Exit;
  { Also reject /DIR overrides: a marker must never enter the normal install. }
  if CompareText(CanonicalInstallPath(ExpandConstant('{app}')),
      CanonicalInstallPath(ExpandConstant('{localappdata}\Programs\SayFlex-Test-Nvidia'))) <> 0 then
    Result := 'SayFlex Test must be installed in its separate SayFlex-Test-Nvidia directory.';
end;
#else
function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := UnsupportedProcessorMessage;
end;

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

function UserDataDirectory: String;
begin
#ifdef TestNvidia
  Result := ExpandConstant('{localappdata}\SayFlex-Test-Nvidia');
#else
  Result := ExpandConstant('{localappdata}\SayFlex');
#endif
end;

{ Uninstalling used to leave every downloaded model (up to ~12 GB), the
  history in plain text and the saved keys behind without asking. Ask twice:
  models are large and easy to download again; history and keys are personal.
  A silent uninstall keeps everything, so an upgrade never loses data. }
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  DataDir: String;
begin
  if CurUninstallStep <> usPostUninstall then
    Exit;
  DataDir := UserDataDirectory;
  if not DirExists(DataDir) then
    Exit;
  if SuppressibleMsgBox(FmtMessage(CustomMessage('UninstallDeleteModels'), [DataDir]),
      mbConfirmation, MB_YESNO or MB_DEFBUTTON2, IDNO) = IDYES then begin
    DelTree(DataDir + '\models', True, True, True);
    DelTree(DataDir + '\translation-models', True, True, True);
    DelTree(DataDir + '\ocr-models', True, True, True);
    DelTree(DataDir + '\runtimes', True, True, True);
    DelTree(DataDir + '\runtime-downloads', True, True, True);
  end;
  if SuppressibleMsgBox(FmtMessage(CustomMessage('UninstallDeletePersonalData'), [DataDir]),
      mbConfirmation, MB_YESNO or MB_DEFBUTTON2, IDNO) = IDYES then
    DelTree(DataDir, True, True, True);
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
