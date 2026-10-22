$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot '..\release-validation.psm1') -Force

Describe 'Durable installer locale handoff' {
    It 'compiles both channels with locale persistence independent of the optional app launch' {
        $compiler = Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe'
        $installer = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\installer\SayFlex.iss'))
        foreach ($testChannel in @($false, $true)) {
            $fixture = Join-Path $TestDrive ([Guid]::NewGuid().ToString('N'))
            New-Item -ItemType Directory -Path $fixture | Out-Null
            Set-Content -LiteralPath (Join-Path $fixture 'SayFlex.exe') -Value 'fixture'
            Set-Content -LiteralPath (Join-Path $fixture 'sayflex-test-channel') -Value 'test-nvidia'
            $wrapper = Join-Path $fixture 'locale.iss'
            $processed = Join-Path $fixture 'policy.iss'
            $define = if ($testChannel) { '#define TestNvidia' } else { '' }
            @"
$define
#define AppSourceDir "$fixture"
[Setup]
SourceDir=$(Split-Path -Parent $installer)
#include "$installer"
#expr SaveToFile("$processed")
"@ | Set-Content -LiteralPath $wrapper
            # /O- validates and compiles Pascal without generating an installer.
            $output = & $compiler /Q "/O$fixture" /O- $wrapper 2>&1
            if ($LASTEXITCODE -ne 0) { throw "Installer locale policy failed to compile: $output" }
            $policy = Get-Content -LiteralPath $processed -Raw
            $callback = [regex]::Match($policy, '(?si)procedure CurStepChanged\(.*?(?=\z)').Value
            $callback | Should Match 'ssPostInstall'
            $callback | Should Match 'SaveInstallerLanguage'
            $policy | Should Match 'InstallerLanguageRequest\(ActiveLanguage, NewInstallationId\)'
            $policy | Should Match '(?m)^ShowLanguageDialog=yes\r?$'
            $policy | Should Match '(?m)^DisableWelcomePage=no\r?$'
            $policy | Should Match '(?m)^DisableReadyPage=no\r?$'
            $uninstall = [regex]::Match($policy, '(?ms)^\[UninstallDelete\]\r?\n(.*?)(?=^\[|\z)').Groups[1].Value
            $uninstall | Should Match 'Name: "\{app\}\\installer-language"'
            @(Get-ChildItem -LiteralPath $fixture -Filter '*.exe').Count | Should Be 1
        }
    }
}

Describe 'Installed payload locale exception' {
    It 'excludes only the root installer-owned seed while retaining every other unexpected file' {
        $installed = Join-Path $TestDrive 'installed'
        New-Item -ItemType Directory -Path (Join-Path $installed 'nested') -Force | Out-Null
        foreach ($file in @('SayFlex.exe', 'build-manifest.json', 'installer-language', 'nested\installer-language', 'unexpected.dll')) {
            Set-Content -LiteralPath (Join-Path $installed $file) -Value 'fixture'
        }
        $paths = @(Get-InstalledPayloadFilePaths -AppDirectory $installed)
        ($paths -contains 'SayFlex.exe') | Should Be $true
        ($paths -contains 'build-manifest.json') | Should Be $true
        ($paths -contains 'nested/installer-language') | Should Be $true
        ($paths -contains 'unexpected.dll') | Should Be $true
        ($paths -contains 'installer-language') | Should Be $false
        $paths.Count | Should Be 4
    }
}
