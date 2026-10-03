#Requires -Version 7.0
# Deploy a local BepInEx payload and generated interop as one recoverable transaction.
param(
    [Parameter(Mandatory=$true)][string]$GameDir,
    [string]$BepInExSource = '',
    [string]$SmokePlugin = '',
    [switch]$Update
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$GameDir = [IO.Path]::GetFullPath($GameDir)
if (-not $BepInExSource) { $BepInExSource = Join-Path $PSScriptRoot 'tools/BepInEx' }
if (-not $SmokePlugin) { $SmokePlugin = Join-Path $PSScriptRoot '../smokeplugin/bin/Release/net6.0/clientpatch-smoke.dll' }

function Assert-PlainPath([string]$Path) {
    $current = [IO.Path]::GetFullPath($Path)
    while ($current) {
        try {
            if ([IO.File]::GetAttributes($current) -band [IO.FileAttributes]::ReparsePoint) { throw "Linked installation path: $current" }
        } catch [IO.FileNotFoundException] {
        } catch [IO.DirectoryNotFoundException] {
        }
        $current = [IO.Path]::GetDirectoryName($current)
    }
}
function Assert-PlainTree([string]$Path) {
    Assert-PlainPath $Path
    foreach ($item in Get-ChildItem -LiteralPath $Path -Force) {
        Assert-PlainPath $item.FullName
        if ($item.PSIsContainer) { Assert-PlainTree $item.FullName }
    }
}
function Destination([string]$Relative) {
    if ([IO.Path]::IsPathRooted($Relative) -or $Relative.Contains(':') -or ($Relative -split '[\\/]' | Where-Object { $_ -in @('', '.', '..') })) { throw 'Unsafe deployment path.' }
    $path = [IO.Path]::GetFullPath((Join-Path $GameDir $Relative))
    if (-not $path.StartsWith($GameDir.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Deployment escaped the game directory.' }
    Assert-PlainPath $path
    if (Test-Path -LiteralPath $path -PathType Container) { throw "Expected a file: $path" }
    return $path
}
function Assert-GameStopped {
    if (Get-Process -Name HeavenBurnsRed -ErrorAction SilentlyContinue) { throw 'Close HeavenBurnsRed before deploying BepInEx.' }
}
function Replace-File([string]$Target, [AllowNull()][string]$Source) {
    Assert-PlainPath $Target
    if (-not $Source) { if (Test-Path -LiteralPath $Target) { [IO.File]::Delete($Target) }; return }
    Assert-PlainPath $Source
    [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($Target)) | Out-Null
    $temporary = Join-Path ([IO.Path]::GetDirectoryName($Target)) ('.cpm-deploy-' + [Guid]::NewGuid().ToString('N'))
    try {
        [IO.File]::Copy($Source, $temporary, $false)
        [IO.File]::Move($temporary, $Target, $true)
    } finally { if (Test-Path -LiteralPath $temporary) { [IO.File]::Delete($temporary) } }
}

$data = Join-Path $GameDir 'clientpatch'
$backup = Join-Path $data '.native-install-backup'
$journalPath = Join-Path $backup 'journal.json'
$gateRelative = 'BepInEx/interop/clientpatch-interop.json'
function Recover-Deployment {
    Assert-PlainPath $journalPath
    if (-not (Test-Path -LiteralPath $journalPath)) { return }
    $journal = Get-Content -LiteralPath $journalPath -Raw | ConvertFrom-Json
    if ($journal.PSObject.Properties.Name -contains 'files') {
        $rows = @($journal.files)
        $gate = $journal.gate
    } else {
        $rows = @($journal)
        $gate = $null
        foreach ($row in $rows) { if ([IO.Path]::GetFileName($row[0]) -eq 'clientpatch-interop.json') { $gate = $row[0] } }
    }
    if ($gate) {
        if (-not ($rows | Where-Object { $_[0] -eq $gate })) { throw 'Recovery gate is not in the snapshot.' }
        Replace-File (Destination $gate) $null
    }
    for ($i = 0; $i -lt $rows.Count; $i++) {
        Destination $rows[$i][0] | Out-Null
        if ($rows[$i][1]) {
            Assert-PlainPath (Join-Path $backup "$i")
            if (-not (Test-Path -LiteralPath (Join-Path $backup "$i") -PathType Leaf)) { throw 'Recovery backup is missing.' }
        }
    }
    $order = @()
    for ($i = 0; $i -lt $rows.Count; $i++) { if ($rows[$i][0] -ne $gate) { $order += $i } }
    for ($i = 0; $i -lt $rows.Count; $i++) { if ($rows[$i][0] -eq $gate) { $order += $i } }
    foreach ($i in $order) {
        $source = if ($rows[$i][1]) { Join-Path $backup "$i" } else { $null }
        Replace-File (Destination $rows[$i][0]) $source
    }
    [IO.File]::Delete($journalPath)
    Assert-PlainTree $backup
    Remove-Item -LiteralPath $backup -Recurse -Force
}

Assert-GameStopped
Assert-PlainPath $data
Assert-PlainTree $BepInExSource
foreach ($required in @('winhttp.dll', 'BepInEx/core/BepInEx.Core.dll', 'BepInEx/core/BepInEx.Unity.IL2CPP.dll', 'doorstop_config.ini', 'dotnet')) {
    if (-not (Test-Path -LiteralPath (Join-Path $BepInExSource $required))) { throw "Incomplete BepInEx payload: $required" }
}
$mutex = [Threading.Mutex]::new($false, 'Global\clientpatch-bepinex-setup')
$ownedMutex = $false
$operationLock = $null
$stage = $null
try {
    try { $ownedMutex = $mutex.WaitOne(0) } catch [Threading.AbandonedMutexException] { $ownedMutex = $true }
    if (-not $ownedMutex) { throw 'Another native setup operation is active.' }
    [IO.Directory]::CreateDirectory($data) | Out-Null
    $lockPath = Join-Path $data '.manager-operation.lock'
    Assert-PlainPath $lockPath
    $operationLock = [IO.File]::Open($lockPath, 'OpenOrCreate', 'ReadWrite', 'None')
    Assert-GameStopped
    Recover-Deployment
    $dumpRoot = Join-Path $data 'interop'
    Assert-PlainPath (Join-Path $dumpRoot 'latest.json')
    $latest = Get-Content -LiteralPath (Join-Path $dumpRoot 'latest.json') -Raw | ConvertFrom-Json
    if ($latest.build_id -notmatch '^[0-9a-f]{16}$') { throw 'Invalid dump build id.' }
    $interopSource = Join-Path $dumpRoot ($latest.build_id + '/interop')
    Assert-PlainTree $interopSource
    foreach ($required in @('DONE', 'Assembly-CSharp.dll', 'UnityEngine.CoreModule.dll')) {
        if (-not (Test-Path -LiteralPath (Join-Path $interopSource $required) -PathType Leaf)) { throw "Incomplete interop: $required" }
    }
    if ((Get-Content -LiteralPath (Join-Path $interopSource 'DONE') -Raw | ConvertFrom-Json).build_id -ne $latest.build_id) { throw 'Interop completion marker does not match latest dump.' }
    $stage = Join-Path $data ('.deploy-bepinex-' + [Guid]::NewGuid().ToString('N'))
    [IO.Directory]::CreateDirectory($stage) | Out-Null
    $files = [Collections.Generic.Dictionary[string,string]]::new([StringComparer]::OrdinalIgnoreCase)
    function Stage-File([string]$Relative, [string]$Source) {
        Destination $Relative | Out-Null
        Assert-PlainPath $Source
        $copy = Join-Path $stage ([Guid]::NewGuid().ToString('N'))
        [IO.File]::Copy($Source, $copy, $false)
        $files[$Relative] = $copy
    }
    foreach ($item in @('winhttp.dll', 'doorstop_config.ini', '.doorstop_version', 'BepInEx', 'dotnet')) {
        $relative = if ($item -eq 'winhttp.dll') { 'doorstop.dll' } else { $item }
        $source = Join-Path $BepInExSource $item
        if (-not (Test-Path -LiteralPath $source)) { continue }
        if (-not $Update -and (Test-Path -LiteralPath (Join-Path $GameDir $relative))) { continue }
        if (Test-Path -LiteralPath $source -PathType Container) {
            foreach ($file in Get-ChildItem -LiteralPath $source -Recurse -File -Force) {
                Stage-File ($relative + '/' + [IO.Path]::GetRelativePath($source, $file.FullName)) $file.FullName
            }
        } else { Stage-File $relative $source }
    }
    $files['winhttp.dll'] = $null
    Stage-File 'clientpatch/interopgen.ps1' (Join-Path $PSScriptRoot 'interopgen.ps1')
    $interopDestination = Join-Path $GameDir 'BepInEx/interop'
    if (Test-Path -LiteralPath $interopDestination) {
        Assert-PlainTree $interopDestination
        foreach ($file in Get-ChildItem -LiteralPath $interopDestination -File) {
            if ($file.Extension -in @('.dll', '.db')) { $files['BepInEx/interop/' + $file.Name] = $null }
        }
    }
    foreach ($file in Get-ChildItem -LiteralPath $interopSource -File) {
        if ($file.Extension -in @('.dll', '.db')) { Stage-File ('BepInEx/interop/' + $file.Name) $file.FullName }
    }
    $configRelative = 'BepInEx/config/BepInEx.cfg'
    $configPath = Destination $configRelative
    $text = if (Test-Path -LiteralPath $configPath) { [IO.File]::ReadAllText($configPath) } elseif ($files.ContainsKey($configRelative)) { [IO.File]::ReadAllText($files[$configRelative]) } else { '' }
    $lines = [Collections.Generic.List[string]]::new()
    $inSection = $false; $seenSection = $false; $seenKey = $false
    foreach ($line in ($text -split '\r?\n')) {
        $trimmed = $line.Trim().TrimStart([char]0xfeff)
        if ($trimmed -match '^\[.*\]$') {
            if ($inSection -and -not $seenKey) { $lines.Add('UpdateInteropAssemblies = false') }
            $inSection = $trimmed -ceq '[IL2CPP]'; $seenSection = $seenSection -or $inSection; $seenKey = $false
        }
        if ($inSection -and $trimmed -cmatch '^UpdateInteropAssemblies\s*=') { $lines.Add('UpdateInteropAssemblies = false'); $seenKey = $true } else { $lines.Add($line) }
    }
    if (-not $seenSection) { $lines.Add('[IL2CPP]'); $lines.Add('UpdateInteropAssemblies = false') } elseif ($inSection -and -not $seenKey) { $lines.Add('UpdateInteropAssemblies = false') }
    $configStage = Join-Path $stage 'config'
    [IO.File]::WriteAllText($configStage, ($lines -join "`n") + "`n")
    $files[$configRelative] = $configStage
    if (Test-Path -LiteralPath $SmokePlugin -PathType Leaf) { Stage-File ('BepInEx/plugins/' + [IO.Path]::GetFileName($SmokePlugin)) $SmokePlugin }
    $markerStage = Join-Path $stage 'marker'
    @{ build_id = $latest.build_id } | ConvertTo-Json | Set-Content -LiteralPath $markerStage
    $files[$gateRelative] = $markerStage

    # Snapshot every destination before publishing a native-compatible recovery journal.
    Assert-PlainPath $backup
    [IO.Directory]::CreateDirectory($backup) | Out-Null
    $saved = [Collections.Generic.List[object]]::new()
    foreach ($relative in $files.Keys) {
        $target = Destination $relative
        $existed = [IO.File]::Exists($target)
        if ($existed) {
            $backupFile = Join-Path $backup ($saved.Count.ToString())
            Assert-PlainPath $backupFile
            [IO.File]::Copy($target, $backupFile, $true)
        }
        $saved.Add(@($relative, $existed))
    }
    $journalStage = Join-Path $stage 'journal'
    $journalBytes = [Text.Encoding]::UTF8.GetBytes((@{ files = $saved.ToArray(); gate = $gateRelative } | ConvertTo-Json -Depth 8))
    $stream = [IO.File]::Open($journalStage, 'CreateNew', 'Write', 'None')
    try { $stream.Write($journalBytes); $stream.Flush($true) } finally { $stream.Dispose() }
    [IO.File]::Move($journalStage, $journalPath)
    try {
        Assert-GameStopped
        Replace-File (Destination $gateRelative) $null
        foreach ($relative in $files.Keys) {
            if ($relative -ne $gateRelative) { Replace-File (Destination $relative) $files[$relative] }
        }
        Replace-File (Destination $gateRelative) $files[$gateRelative]
        [IO.File]::Delete($journalPath)
    } catch {
        Recover-Deployment
        throw
    }
    Assert-PlainTree $backup
    Remove-Item -LiteralPath $backup -Recurse -Force
    Write-Host "BepInEx deployment complete for build $($latest.build_id)."
} finally {
    if ($stage -and (Test-Path -LiteralPath $stage)) {
        Assert-PlainTree $stage
        Remove-Item -LiteralPath $stage -Recurse -Force
    }
    if ($operationLock) { $operationLock.Dispose() }
    if ($ownedMutex) { $mutex.ReleaseMutex() }
    $mutex.Dispose()
}