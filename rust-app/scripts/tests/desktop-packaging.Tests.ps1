$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot '..\release-validation.psm1') -Force

Describe 'Desktop build feature isolation' {
    It 'builds the desktop alone after the workspace and before staging any binary' {
        $errors = $null
        $ast = [System.Management.Automation.Language.Parser]::ParseFile(
            (Join-Path $PSScriptRoot '..\publish.ps1'), [ref]$null, [ref]$errors)
        $errors | Should BeNullOrEmpty
        $builds = @($ast.FindAll({ param($node)
            $node -is [System.Management.Automation.Language.CommandAst] -and
            $node.CommandElements[0] -is [System.Management.Automation.Language.VariableExpressionAst] -and
            $node.CommandElements[0].VariablePath.UserPath -eq 'cargoPath'
        }, $true))
        $workspace = $builds | Where-Object { @($_.CommandElements.Value) -contains '--workspace' }
        $workspace.Extent.Text | Should Match '--workspace\s+--exclude\s+sayflex-desktop'
        $desktop = @($builds | Where-Object {
            @($_.CommandElements.Value) -contains 'sayflex-desktop' -and
            @($_.CommandElements.Value) -notcontains '--workspace'
        })
        $desktop.Count | Should Be 1
        $args = @($desktop[0].CommandElements | ForEach-Object {
            if ($_ -is [System.Management.Automation.Language.VariableExpressionAst]) {
                '$' + $_.VariablePath.UserPath
            } elseif ($_ -is [System.Management.Automation.Language.CommandParameterAst]) {
                '-' + $_.ParameterName
            } else { $_.Value }
        })
        ($args -join ' ') | Should Be '$cargoPath build --locked --target-dir $cpuTarget --profile $profile -p sayflex-desktop'
        $copy = $ast.FindAll({ param($node)
            $node -is [System.Management.Automation.Language.CommandAst] -and
            $node.GetCommandName() -eq 'Copy-Item'
        }, $true) | Select-Object -First 1
        ($desktop[0].Extent.StartOffset -gt $workspace.Extent.EndOffset) | Should Be $true
        ($desktop[0].Extent.EndOffset -lt $copy.Extent.StartOffset) | Should Be $true
    }
}

# Minimal PE32+ images exercise the import table reader, including RVA mapping.
# No test executable is launched, and an unrelated matching byte string is not
# mistaken for a DLL import.
function New-DesktopPeFixture([string]$Path, [string[]]$Imports) {
    $bytes = New-Object byte[] 2048
    $stream = New-Object IO.MemoryStream(,$bytes)
    $writer = New-Object IO.BinaryWriter($stream)
    try {
        $writer.Write([uint16]0x5a4d)
        $stream.Position = 0x3c; $writer.Write([uint32]0x80)
        $stream.Position = 0x80; $writer.Write([uint32]0x4550)
        $writer.Write([uint16]0x8664); $writer.Write([uint16]1)
        $stream.Position = 0x94; $writer.Write([uint16]240)
        $stream.Position = 0x98; $writer.Write([uint16]0x20b)
        $stream.Position = 0x104; $writer.Write([uint32]16)
        $stream.Position = 0x110; $writer.Write([uint32]0x1000); $writer.Write([uint32](20 * ($Imports.Count + 1)))
        $stream.Position = 0x190; $writer.Write([uint32]1024); $writer.Write([uint32]0x1000)
        $writer.Write([uint32]1024); $writer.Write([uint32]512)
        for ($index = 0; $index -lt $Imports.Count; $index++) {
            $stream.Position = 512 + $index * 20 + 12
            $writer.Write([uint32](0x1200 + $index * 64))
            $stream.Position = 1024 + $index * 64
            $writer.Write([Text.Encoding]::ASCII.GetBytes($Imports[$index] + [char]0))
        }
        $stream.Position = 1700
        $writer.Write([Text.Encoding]::ASCII.GetBytes('DirectML.dll'))
        [IO.File]::WriteAllBytes($Path, $bytes)
    } finally { $writer.Dispose(); $stream.Dispose() }
}

Describe 'Desktop packaged PE import boundary' {
    It 'allows ordinary UI imports and ignores unreferenced strings in the executable' {
        $path = Join-Path $TestDrive 'ui.exe'
        New-DesktopPeFixture $path @('KERNEL32.dll', 'opengl32.dll')
        @(Get-DesktopInferenceImportFailures -DesktopPath $path) | Should BeNullOrEmpty
    }

    It 'rejects an actual DirectML D3D12 or ONNX Runtime import in the desktop' {
        foreach ($dependency in @('DirectML.dll', 'D3D12.dll', 'onnxruntime.dll')) {
            $path = Join-Path $TestDrive 'inference-ui.exe'
            New-DesktopPeFixture $path @('KERNEL32.dll', $dependency)
            $failures = @(Get-DesktopInferenceImportFailures -DesktopPath $path)
            $failures.Count | Should Be 1
            $failures[0] | Should Match ([regex]::Escape($dependency))
        }
    }

    It 'fails closed when a purported desktop cannot be inspected as a PE image' {
        $path = Join-Path $TestDrive 'invalid.exe'
        Set-Content -LiteralPath $path -Value 'not an executable'
        @(Get-DesktopInferenceImportFailures -DesktopPath $path).Count | Should Be 1
    }
}
