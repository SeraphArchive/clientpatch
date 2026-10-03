#Requires -Version 7.0
$ErrorActionPreference = 'Stop'
$fixtureRoot = Join-Path ([IO.Path]::GetTempPath()) ('clientpatch-interopgen-test-' + [Guid]::NewGuid().ToString('N'))
$gameDir = Join-Path $fixtureRoot 'game'
$dumpRoot = Join-Path $gameDir 'clientpatch/interop'
$buildId = '0123456789abcdef'
$buildDir = Join-Path $dumpRoot $buildId
$outputDir = Join-Path $buildDir 'interop'
$dumperDir = Join-Path $fixtureRoot 'dumper'
$cliDll = Join-Path $fixtureRoot 'Il2CppInterop.CLI.dll'
$global:ClientpatchInteropTestMode = 'fail'

# Model external tool success/failure with real staged files and real directory promotion.
function dotnet {
    if ([IO.Path]::GetFileName($args[0]) -eq 'Il2CppDumper.dll') {
        $dummy = Join-Path $args[3] 'DummyDll'
        New-Item -ItemType Directory -Force -Path $dummy | Out-Null
        Set-Content -LiteralPath (Join-Path $dummy 'mscorlib.dll') -Value 'dummy'
        $global:LASTEXITCODE = 0
        return
    }
    $generated = $args[[Array]::IndexOf($args, '--output') + 1]
    New-Item -ItemType Directory -Force -Path $generated | Out-Null
    Set-Content -LiteralPath (Join-Path $generated 'Assembly-CSharp.dll') -Value 'new'
    if ($global:ClientpatchInteropTestMode -eq 'success') {
        Set-Content -LiteralPath (Join-Path $generated 'UnityEngine.CoreModule.dll') -Value 'new'
    }
    $global:LASTEXITCODE = $(if ($global:ClientpatchInteropTestMode -eq 'fail') { 1 } else { 0 })
}

try {
    New-Item -ItemType Directory -Force -Path $outputDir, $dumperDir | Out-Null
    Set-Content -LiteralPath (Join-Path $dumperDir 'Il2CppDumper.dll') -Value 'fixture'
    Set-Content -LiteralPath $cliDll -Value 'fixture'
    @{ build_id = $buildId } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $dumpRoot 'latest.json')
    foreach ($name in @('Assembly-CSharp.dll', 'UnityEngine.CoreModule.dll', 'old-only.dll', 'DONE')) {
        Set-Content -LiteralPath (Join-Path $outputDir $name) -Value 'old'
    }
    foreach ($mode in @('fail', 'incomplete', 'success')) {
        $global:ClientpatchInteropTestMode = $mode
        $failed = $false
        $failure = $null
        try {
            & (Join-Path $PSScriptRoot 'interopgen.ps1') -GameDir $gameDir -DumperDir $dumperDir -CliDll $cliDll -Force
        } catch { $failed = $true; $failure = $_ }
        if ($failed -ne ($mode -ne 'success')) { throw "Unexpected generation result for ${mode}: $failure" }
        $expected = $(if ($mode -eq 'success') { 'new' } else { 'old' })
        foreach ($name in @('Assembly-CSharp.dll', 'UnityEngine.CoreModule.dll')) {
            if ((Get-Content -LiteralPath (Join-Path $outputDir $name) -Raw).Trim() -ne $expected) { throw "$mode damaged $name" }
        }
        if ($mode -ne 'success' -and (Get-Content -LiteralPath (Join-Path $outputDir 'DONE') -Raw).Trim() -ne 'old') { throw "$mode damaged prior DONE" }
        $lock = [IO.File]::Open((Join-Path $gameDir 'clientpatch/.manager-operation.lock'), 'Open', 'ReadWrite', 'None')
        $lock.Dispose()
    }
    if (Test-Path -LiteralPath (Join-Path $outputDir 'old-only.dll')) { throw 'Promotion retained a stale assembly.' }
    if ((Get-Content -LiteralPath (Join-Path $outputDir 'DONE') -Raw | ConvertFrom-Json).build_id -ne $buildId) { throw 'Missing completion marker.' }
    $outside = Join-Path $fixtureRoot 'outside'
    $clientpatchDir = Join-Path $gameDir 'clientpatch'
    [IO.Directory]::Move($clientpatchDir, $outside)
    cmd /c mklink /J $clientpatchDir $outside | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Cannot create junction fixture.' }
    try {
        $rejected = $false
        try {
            & (Join-Path $PSScriptRoot 'interopgen.ps1') -GameDir $gameDir -DumperDir $dumperDir -CliDll $cliDll -Force
        } catch { $rejected = $true }
        if (-not $rejected) { throw 'Generation followed a linked clientpatch directory.' }
    } finally { [IO.Directory]::Delete($clientpatchDir) }
    Write-Host 'Interop wrapper passed: failed/incomplete forced regeneration preserves previous set; successful promotion removes stale assemblies.'
} finally {
    $resolvedFixture = [IO.Path]::GetFullPath($fixtureRoot)
    $tempPrefix = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    if (-not $resolvedFixture.StartsWith($tempPrefix, [StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe test cleanup path.' }
    Remove-Item -LiteralPath $resolvedFixture -Recurse -Force
}
