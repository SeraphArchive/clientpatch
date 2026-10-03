#Requires -Version 7.0
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$fixture = Join-Path ([IO.Path]::GetTempPath()) ('clientpatch-deploy-test-' + [Guid]::NewGuid().ToString('N'))
$game = Join-Path $fixture 'game'
$payload = Join-Path $fixture 'payload'
$buildId = '0123456789abcdef'
$generated = Join-Path $game "clientpatch/interop/$buildId/interop"
$deploy = Join-Path $PSScriptRoot 'deploy_bepinex.ps1'
$global:ClientpatchDeployTestRunning = $false
function Get-Process { if ($global:ClientpatchDeployTestRunning) { [pscustomobject]@{ ProcessName = 'HeavenBurnsRed' } } }
function Write-Fixture([string]$Path, [string]$Text) {
    [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($Path)) | Out-Null
    [IO.File]::WriteAllText($Path, $Text)
}
function Assert-File([string]$Path, [string]$Expected) {
    if ([IO.File]::ReadAllText($Path).Trim() -ne $Expected) { throw "Unexpected content at $Path" }
}
try {
    foreach ($relative in @('winhttp.dll', 'doorstop_config.ini', '.doorstop_version', 'BepInEx/core/BepInEx.Core.dll', 'BepInEx/core/BepInEx.Unity.IL2CPP.dll', 'dotnet/runtime.dll')) {
        Write-Fixture (Join-Path $payload $relative) 'old payload'
    }
    foreach ($name in @('Assembly-CSharp.dll', 'UnityEngine.CoreModule.dll', 'xref.db')) { Write-Fixture (Join-Path $generated $name) 'old interop' }
    Write-Fixture (Join-Path $generated 'DONE') (@{ build_id = $buildId } | ConvertTo-Json)
    Write-Fixture (Join-Path $game 'clientpatch/interop/latest.json') (@{ build_id = $buildId } | ConvertTo-Json)
    & $deploy -GameDir $game -BepInExSource $payload
    Assert-File (Join-Path $game 'doorstop.dll') 'old payload'
    Assert-File (Join-Path $game 'BepInEx/interop/Assembly-CSharp.dll') 'old interop'
    if (Test-Path -LiteralPath (Join-Path $game 'winhttp.dll')) { throw 'Deployed an autoload Doorstop copy.' }
    $cfg = Join-Path $game 'BepInEx/config/BepInEx.cfg'
    Write-Fixture $cfg "[IL2CPP]`nUpdateInteropAssemblies = true`nKeep = yes`n[Other]`nUpdateInteropAssemblies = true`n"
    Write-Fixture (Join-Path $game 'BepInEx/interop/stale.dll') 'stale'
    Write-Fixture (Join-Path $payload 'winhttp.dll') 'new payload'
    foreach ($name in @('Assembly-CSharp.dll', 'UnityEngine.CoreModule.dll', 'xref.db')) { Write-Fixture (Join-Path $generated $name) 'new interop' }

    $lockedTarget = Join-Path $game 'BepInEx/interop/UnityEngine.CoreModule.dll'
    $held = [IO.File]::Open($lockedTarget, 'Open', 'Read', 'Read')
    try {
        $failed = $false
        try { & $deploy -GameDir $game -BepInExSource $payload -Update } catch { $failed = $true }
        if (-not $failed) { throw 'Locked destination was accepted.' }
        if (Test-Path -LiteralPath (Join-Path $game 'BepInEx/interop/clientpatch-interop.json')) { throw 'Failed recovery retained a current marker.' }
        Assert-File (Join-Path $game 'doorstop.dll') 'old payload'
        Assert-File (Join-Path $game 'BepInEx/interop/Assembly-CSharp.dll') 'old interop'
        if (-not (Test-Path -LiteralPath (Join-Path $game 'clientpatch/.native-install-backup/journal.json'))) { throw 'Interrupted recovery lost its journal.' }
    } finally { $held.Dispose() }
    & $deploy -GameDir $game -BepInExSource $payload -Update
    Assert-File (Join-Path $game 'doorstop.dll') 'new payload'
    Assert-File (Join-Path $game 'BepInEx/interop/Assembly-CSharp.dll') 'new interop'
    if (Test-Path -LiteralPath (Join-Path $game 'BepInEx/interop/stale.dll')) { throw 'Stale interop was retained.' }
    if (Test-Path -LiteralPath (Join-Path $game 'BepInEx/BepInEx')) { throw 'Payload directories were nested.' }
    $text = [IO.File]::ReadAllText($cfg)
    if (-not $text.Contains("[Other]`nUpdateInteropAssemblies = true") -or -not $text.Contains('Keep = yes') -or -not $text.Contains('UpdateInteropAssemblies = false')) { throw 'Config preservation failed.' }
    $global:ClientpatchDeployTestRunning = $true
    $failed = $false
    try { & $deploy -GameDir $game -BepInExSource $payload -Update } catch { $failed = $true }
    if (-not $failed) { throw 'Deployment accepted a running game.' }
    $global:ClientpatchDeployTestRunning = $false
    $lock = [IO.File]::Open((Join-Path $game 'clientpatch/.manager-operation.lock'), 'Open', 'ReadWrite', 'None')
    try {
        $failed = $false
        try { & $deploy -GameDir $game -BepInExSource $payload -Update } catch { $failed = $true }
        if (-not $failed) { throw 'Deployment ignored the manager lock.' }
    } finally { $lock.Dispose() }
    $original = Join-Path $game 'BepInEx'
    $outside = Join-Path $fixture 'outside'
    [IO.Directory]::Move($original, $outside)
    cmd /c mklink /J $original $outside | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Cannot create junction fixture.' }
    try {
        $failed = $false
        try { & $deploy -GameDir $game -BepInExSource $payload -Update } catch { $failed = $true }
        if (-not $failed) { throw 'Deployment followed a junction.' }
        Assert-File (Join-Path $outside 'interop/Assembly-CSharp.dll') 'new interop'
    } finally { [IO.Directory]::Delete($original) }
    Write-Host 'BepInEx deploy passed: install/update, locked rollback and recovery, marker ordering, config preservation, game/manager locks, and junction rejection.'
} finally {
    $global:ClientpatchDeployTestRunning = $false
    $resolved = [IO.Path]::GetFullPath($fixture)
    $tempPrefix = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    if (-not $resolved.StartsWith($tempPrefix, [StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe fixture cleanup.' }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}