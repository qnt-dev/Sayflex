$ErrorActionPreference = 'Stop'
$module = Join-Path $PSScriptRoot '..\test-nvidia-packaging.psm1'
if (Test-Path -LiteralPath $module) { Import-Module $module -Force }

Describe 'Local NVIDIA test source provenance' {
    BeforeEach {
        $repo = Join-Path $TestDrive ([Guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $repo | Out-Null
        & git -C $repo init --quiet
        Set-Content -LiteralPath (Join-Path $repo '.gitignore') -Value 'target/'
        Set-Content -LiteralPath (Join-Path $repo 'source.rs') -Value 'initial source'
        & git -C $repo add .
        & git -C $repo -c user.name=PackagingTest -c user.email=test@example.invalid commit --quiet -m fixture
    }

    It 'records dirty and untracked source bytes and catches a changed source after compilation' {
        Set-Content -LiteralPath (Join-Path $repo 'source.rs') -Value 'edited source'
        Set-Content -LiteralPath (Join-Path $repo 'new.rs') -Value 'new module'
        $before = Get-TestNvidiaSourceSnapshot -RepositoryPath $repo
        $before.dirty | Should Be $true
        @($before.files | Where-Object path -eq 'new.rs').Count | Should Be 1
        $row = $before.files | Where-Object path -eq 'source.rs'
        $row.sha256 | Should Be (Get-FileHash (Join-Path $repo 'source.rs')).Hash.ToLowerInvariant()
        { Assert-TestNvidiaSourceUnchanged -RepositoryPath $repo -Snapshot $before } | Should Not Throw
        Set-Content -LiteralPath (Join-Path $repo 'source.rs') -Value 'changed during build'
        { Assert-TestNvidiaSourceUnchanged -RepositoryPath $repo -Snapshot $before } | Should Throw 'changed during'
    }

    It 'ignores build outputs but notices a deleted tracked source' {
        $before = Get-TestNvidiaSourceSnapshot -RepositoryPath $repo
        New-Item -ItemType Directory -Path (Join-Path $repo 'target') | Out-Null
        Set-Content -LiteralPath (Join-Path $repo 'target\app.exe') -Value 'build output'
        { Assert-TestNvidiaSourceUnchanged -RepositoryPath $repo -Snapshot $before } | Should Not Throw
        Remove-Item -LiteralPath (Join-Path $repo 'source.rs')
        { Assert-TestNvidiaSourceUnchanged -RepositoryPath $repo -Snapshot $before } | Should Throw 'changed during'
    }
}

Describe 'Local NVIDIA test payload boundary' {
    BeforeEach {
        $app = Join-Path $TestDrive ([Guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $app | Out-Null
        Set-Content -LiteralPath (Join-Path $app 'sayflex-test-channel') -Value 'test-nvidia'
        Set-Content -LiteralPath (Join-Path $app 'SayFlex.Speech.Cuda.exe') -Value 'worker fixture importing cudart64_13.dll'
        $manifest = [pscustomobject]@{
            product = 'SayFlex'; channel = 'test-nvidia'; official_release = $false
            source_snapshot = [pscustomobject]@{ sha256 = ('a' * 64); files = @('fixture') }
        }
    }

    It 'requires the isolation marker and an explicitly unofficial source snapshot' {
        { Assert-TestNvidiaPayload -AppDirectory $app -Manifest $manifest } | Should Not Throw
        Remove-Item -LiteralPath (Join-Path $app 'sayflex-test-channel')
        { Assert-TestNvidiaPayload -AppDirectory $app -Manifest $manifest } | Should Throw 'marker'
        Set-Content -LiteralPath (Join-Path $app 'sayflex-test-channel') -Value 'test-nvidia'
        $manifest.official_release = $true
        { Assert-TestNvidiaPayload -AppDirectory $app -Manifest $manifest } | Should Throw 'unofficial'
    }

    It 'rejects NVIDIA runtime DLLs anywhere in the payload while allowing the CUDA worker' {
        $nested = Join-Path $app 'other\nested'
        New-Item -ItemType Directory -Path $nested | Out-Null
        Set-Content -LiteralPath (Join-Path $nested 'cublas64_13.dll') -Value 'not allowed'
        { Assert-TestNvidiaPayload -AppDirectory $app -Manifest $manifest } | Should Throw 'NVIDIA'
        Remove-Item -LiteralPath (Join-Path $nested 'cublas64_13.dll')
        { Assert-TestNvidiaPayload -AppDirectory $app -Manifest $manifest } | Should Not Throw
    }
}

Describe 'Compiled local test installer policy' {
    It 'compiles an isolated test package without autostart or legacy deletion actions' {
        $compiler = Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe'
        $installer = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\installer\SayFlex.iss'))
        $fixture = Join-Path $TestDrive 'inno-fixture'
        New-Item -ItemType Directory -Path $fixture | Out-Null
        Set-Content -LiteralPath (Join-Path $fixture 'SayFlex.exe') -Value 'payload fixture'
        Set-Content -LiteralPath (Join-Path $fixture 'sayflex-test-channel') -Value 'test-nvidia'
        $wrapper = Join-Path $TestDrive 'compile-test.iss'
        $processed = Join-Path $TestDrive 'compiled-policy.iss'
        @"
#define TestNvidia
#define AppSourceDir "$fixture"
[Setup]
SourceDir=$(Split-Path -Parent $installer)
#include "$installer"
#expr SaveToFile("$processed")
"@ | Set-Content -LiteralPath $wrapper
        $compilerOutput = & $compiler /Q "/O$TestDrive" /Ftest-policy $wrapper 2>&1
        if ($LASTEXITCODE -ne 0) { throw "Test installer compilation failed: $compilerOutput" }
        (Join-Path $TestDrive 'test-policy.exe') | Should Exist
        $policy = Get-Content -LiteralPath $processed -Raw
        # These are the preprocessor's emitted installer directives consumed by
        # Inno, not source-code substring assertions on the template.
        # The wrapper's SourceDir occupies its own Setup section.
        $setup = ([regex]::Matches($policy, '(?ms)^\[Setup\]\r?\n(.*?)(?=^\[|\z)') |
            ForEach-Object { $_.Groups[1].Value }) -join "`n"
        $setup | Should Match '(?m)^AppName=SayFlex Test\r?$'
        $setup | Should Match '(?m)^DefaultDirName=\{localappdata\}\\Programs\\SayFlex-Test-Nvidia\r?$'
        $setup | Should Not Match 'BE541A7B-AF41-4F29-8D3F-D9F75AB68F66'
        $setup | Should Match '(?m)^AppMutex=Local\\SayFlex\.TestNvidia\.Desktop\r?$'
        $setup | Should Match '(?m)^DisableDirPage=yes\r?$'
        $setup | Should Match '(?m)^UsePreviousAppDir=no\r?$'
        $tasks = [regex]::Match($policy, '(?ms)^\[Tasks\]\r?\n(.*?)(?=^\[|\z)').Groups[1].Value
        $tasks | Should Not Match 'autostart'
        $icons = [regex]::Match($policy, '(?ms)^\[Icons\]\r?\n(.*?)(?=^\[|\z)').Groups[1].Value
        $icons | Should Match '\{group\}\\SayFlex Test'
        $icons | Should Not Match '\{userstartup\}'
        $policy | Should Not Match '(?m)^\[InstallDelete\]'
    }

    It 'preserves the normal installer upgrade identity and directory reuse by default' {
        $compiler = Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe'
        $installer = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\installer\SayFlex.iss'))
        $fixture = Join-Path $TestDrive 'inno-normal-fixture'
        New-Item -ItemType Directory -Path $fixture | Out-Null
        Set-Content -LiteralPath (Join-Path $fixture 'SayFlex.exe') -Value 'payload fixture'
        $wrapper = Join-Path $TestDrive 'compile-normal.iss'
        $processed = Join-Path $TestDrive 'normal-policy.iss'
        @"
#define AppSourceDir "$fixture"
[Setup]
SourceDir=$(Split-Path -Parent $installer)
#include "$installer"
#expr SaveToFile("$processed")
"@ | Set-Content -LiteralPath $wrapper
        $compilerOutput = & $compiler /Q "/O$TestDrive" /Fnormal-policy $wrapper 2>&1
        if ($LASTEXITCODE -ne 0) { throw "Normal installer compilation failed: $compilerOutput" }
        $policy = Get-Content -LiteralPath $processed -Raw
        $setup = ([regex]::Matches($policy, '(?ms)^\[Setup\]\r?\n(.*?)(?=^\[|\z)') |
            ForEach-Object { $_.Groups[1].Value }) -join "`n"
        $setup | Should Match '(?m)^AppName=SayFlex\r?$'
        $setup | Should Match '(?m)^AppId=\{\{BE541A7B-AF41-4F29-8D3F-D9F75AB68F66\}\r?$'
        $setup | Should Match '(?m)^DefaultDirName=\{localappdata\}\\Programs\\SayFlex\r?$'
        $setup | Should Match '(?m)^UsePreviousAppDir=yes\r?$'
        $setup | Should Not Match '(?m)^UsePreviousAppDir=no\r?$'
        $tasks = [regex]::Match($policy, '(?ms)^\[Tasks\]\r?\n(.*?)(?=^\[|\z)').Groups[1].Value
        $tasks | Should Match 'autostart'
        $policy | Should Match '(?m)^\[InstallDelete\]'
    }

    It 'refuses to compile the test installer without the isolation marker' {
        $compiler = Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe'
        $installer = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\installer\SayFlex.iss'))
        $fixture = Join-Path $TestDrive 'inno-unmarked-fixture'
        New-Item -ItemType Directory -Path $fixture | Out-Null
        Set-Content -LiteralPath (Join-Path $fixture 'SayFlex.exe') -Value 'payload fixture'
        $previousPreference = $ErrorActionPreference
        try {
            # Windows PowerShell turns expected native stderr into an error
            # record; inspect the compiler's nonzero exit and message below.
            $ErrorActionPreference = 'Continue'
            $compilerOutput = & $compiler /Q /DTestNvidia "/DAppSourceDir=$fixture" "/O$TestDrive" /Funmarked-policy $installer 2>&1
            $compilerExit = $LASTEXITCODE
        } finally { $ErrorActionPreference = $previousPreference }
        $compilerExit | Should Not Be 0
        ($compilerOutput -join ' ') | Should Match 'sayflex-test-channel marker'
        (Test-Path (Join-Path $TestDrive 'unmarked-policy.exe')) | Should Be $false
    }
}

Describe 'Test packaging cannot masquerade as a public release' {
    It 'rejects signature-required test publishing before starting a build' {
        $publish = Join-Path $PSScriptRoot '..\publish.ps1'
        { & $publish -TestNvidia -RequireSignature } | Should Throw 'local test'
    }

    It 'requires the explicit test validator for a payload with an isolation marker' {
        $app = Join-Path $TestDrive 'marker-validation'
        New-Item -ItemType Directory -Path $app | Out-Null
        Set-Content -LiteralPath (Join-Path $app 'sayflex-test-channel') -Value 'test-nvidia'
        $validate = Join-Path $PSScriptRoot '..\validate-release.ps1'
        { & $validate -AppDirectory $app } | Should Throw 'TestNvidia'
    }
}
