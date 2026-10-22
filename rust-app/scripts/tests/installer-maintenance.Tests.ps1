$ErrorActionPreference = 'Stop'

Describe 'Installer maintenance decisions and language requests' {
    It 'runs the compiled Pascal helpers without a wizard, installation, or profile access' {
        $compiler = Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe'
        $installer = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\installer\SayFlex.iss'))
        foreach ($testChannel in @($false, $true)) {
            $fixture = Join-Path $TestDrive ([Guid]::NewGuid().ToString('N'))
            New-Item -ItemType Directory -Path $fixture | Out-Null
            Set-Content -LiteralPath (Join-Path $fixture 'SayFlex.exe') -Value 'fixture'
            Set-Content -LiteralPath (Join-Path $fixture 'sayflex-test-channel') -Value 'test-nvidia'
            $processed = Join-Path $fixture 'policy.iss'
            $wrapper = Join-Path $fixture 'preprocess.iss'
            $define = if ($testChannel) { '#define TestNvidia' } else { '' }
            @"
$define
#define AppSourceDir "$fixture"
[Setup]
SourceDir=$(Split-Path -Parent $installer)
#include "$installer"
#expr SaveToFile("$processed")
"@ | Set-Content -LiteralPath $wrapper -Encoding UTF8
            $output = & $compiler /Q "/O$fixture" /O- $wrapper 2>&1
            if ($LASTEXITCODE -ne 0) { throw "Installer preprocessing failed: $output" }
            $policy = Get-Content -LiteralPath $processed -Raw
            $code = [regex]::Match($policy, '(?ms)^\[Code\]\r?\n(.*)\z').Groups[1].Value
            $messages = [regex]::Match($policy, '(?ms)^\[CustomMessages\]\r?\n(.*?)(?=^\[|\z)').Groups[1].Value
            $languages = [regex]::Match($policy, '(?ms)^\[Languages\]\r?\n(.*?)(?=^\[|\z)').Groups[1].Value
            $result = Join-Path $fixture 'result.txt'
            $harness = Join-Path $fixture 'harness.iss'
            @"
[Setup]
AppName=Installer policy test
AppVersion=1
DefaultDirName=$fixture\unused
CreateAppDir=no
Uninstallable=no
PrivilegesRequired=lowest
ShowLanguageDialog=no
CloseApplications=no
OutputDir=$fixture
OutputBaseFilename=policy-test
[Languages]
$languages
[CustomMessages]
$messages
[Code]
$code
procedure Report(Value: String);
begin
  if not SaveStringToFile('$result', UTF8Encode(Value + #13#10), True) then
    RaiseException('Cannot write test output');
end;
function InitializeSetup: Boolean;
var
  FirstId, SecondId, Summary: String;
begin
  Report(OwnUninstallKey);
  Report(InstallationAction(False, '', '0.8.11'));
  Report(InstallationAction(True, '0.8.10', '0.8.11'));
  Report(InstallationAction(True, '0.8.11', '0.8.11'));
  Report(InstallationAction(True, '0.8.11.0', '0.8.11'));
  Report(InstallationAction(True, '0.8.12', '0.8.11'));
  Report(InstallationAction(True, 'unknown', '0.8.11'));
  Report(IntToStr(Ord(ShouldApplyInstallerLanguage(True, True, False))));
  Report(IntToStr(Ord(ShouldApplyInstallerLanguage(True, True, True))));
  Report(IntToStr(Ord(ShouldApplyInstallerLanguage(True, False, False))));
  Report(IntToStr(Ord(ShouldApplyInstallerLanguage(False, True, False))));
  FirstId := NewInstallationId;
  SecondId := NewInstallationId;
  Report(FirstId);
  Report(SecondId);
  Report(InstallerLanguageRequest('russian', FirstId));
  Report(InstallerLanguageRequest('brazilianportuguese', SecondId));
  Report(InstallerLanguageRequest('english', FirstId));
  Report(InstallerLanguageRequest('ukrainian', FirstId));
  Report(InstallerLanguageRequest('spanish', FirstId));
  Report(InstallerLanguageRequest('german', FirstId));
  Report(InstallerLanguageRequest('french', FirstId));
  Report(InstallerLanguageRequest('italian', FirstId));
  Report(InstallerLanguageRequest('korean', FirstId));
  ExistingInstallation := True;
  ExistingVersion := '0.8.10';
  ExistingDirectory := 'C:\Fixture\Existing';
  ExplicitInstallerLanguage := True;
  Summary := MaintenanceSummary;
  StringChangeEx(Summary, #13#10, '|', True);
  Report(Summary);
  Report(InstallerLanguageDisplayName);
  { Returning False aborts before InitializeWizard, files, registry or Run. }
  Result := False;
end;
"@ | Set-Content -LiteralPath $harness -Encoding UTF8
            $output = & $compiler /Q $harness 2>&1
            if ($LASTEXITCODE -ne 0) { throw "Installer policy harness failed to compile: $output" }
            $language = if ($testChannel) { 'russian' } else { 'english' }
            $run = Start-Process -FilePath (Join-Path $fixture 'policy-test.exe') -ArgumentList "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /LANG=$language" -WindowStyle Hidden -PassThru -Wait
            $run.ExitCode | Should Be 1 # InitializeSetup intentionally returns False.
            $lines = @(Get-Content -LiteralPath $result -Encoding UTF8)
            $id = if ($testChannel) { '{62070119-61B5-41BE-A387-381D775B526B}' } else { '{BE541A7B-AF41-4F29-8D3F-D9F75AB68F66}' }
            $lines[0] | Should Be "Software\Microsoft\Windows\CurrentVersion\Uninstall\${id}_is1"
            ($lines[1..6] -join ',') | Should Be 'InstallAction,UpdateAction,ReinstallAction,ReinstallAction,ReplaceAction,ReplaceAction'
            ($lines[7..10] -join ',') | Should Be '0,1,1,1'
            $lines[11] | Should Match '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
            $lines[12] | Should Match '^[0-9a-f-]{36}$'
            $lines[11] | Should Not Be $lines[12]
            $request = $lines[13] | ConvertFrom-Json
            $request.language | Should Be 'russian'
            $request.installation_id | Should Be $lines[11]
            ($lines[14] | ConvertFrom-Json).language | Should Be 'brazilianportuguese'
            [Text.Encoding]::UTF8.GetByteCount($lines[14]) | Should BeLessThan 257
            $requests = @($lines[13..21] | ForEach-Object { $_ | ConvertFrom-Json })
            ($requests.language -join ',') | Should Be 'russian,brazilianportuguese,english,ukrainian,spanish,german,french,italian,korean'
            $lines[22] | Should Match '0\.8\.10'
            $lines[22] | Should Match 'C:\\Fixture\\Existing'
            $lines[22] | Should Not Match '%n|%[123]'
            if ($testChannel) {
                $lines[22] | Should Match 'Настройки, история, скачанные модели'
                $lines[22] | Should Match 'Язык приложения после установки: Русский'
                $lines[23] | Should Be 'Русский'
            } else {
                $lines[22] | Should Match 'settings, history, downloaded models'
                $lines[22] | Should Match 'Application language after installation: English'
                $lines[23] | Should Be 'English'
            }
            (Test-Path -LiteralPath (Join-Path $fixture 'unused')) | Should Be $false
        }
    }
}
