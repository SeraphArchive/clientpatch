# interopgen — generate BepInEx interop assemblies from a clientpatch runtime dump
#
# Chain:
#   1. clientpatch interopdump module dumps <GameDir>\clientpatch\interop\<build_id>\
#      {GameAssembly.dll, global-metadata.dat} from the live process (run the game
#      once with the module enabled; restore_handles must be OFF — the dumper
#      wants the raw handle form).
#   2. Il2CppDumper turns the dump into DummyDll with
#      FieldOffset attributes. Its IsDumped mode auto-engages (live ImageBase).
#   3. Il2CppInterop.CLI generate turns DummyDll into interop assemblies.
#
# Usage:
#   powershell -File interopgen.ps1 [-GameDir <dir>] [-Force]
#
# The result lands in <DumpRoot>\<build_id>\interop\.
# Cached per build via an interop\DONE marker; -Force regenerates.

param(
    [Parameter(Mandatory=$true)][string]$GameDir,
    [string]$DumpRoot = "",
    [string]$DumperDir = "",
    [string]$CliDll = "",
    [switch]$Force
)

$ErrorActionPreference = "Stop"

function Assert-PlainPath([string]$Path) {
    $current = [IO.Path]::GetFullPath($Path)
    while ($current) {
        try {
            $attributes = [IO.File]::GetAttributes($current)
            if ($attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Linked tool/output path is not supported: $current" }
        } catch [IO.FileNotFoundException] {
        } catch [IO.DirectoryNotFoundException] {
        }
        $current = [IO.Path]::GetDirectoryName($current)
    }
}

function Assert-PlainTree([string]$Path) {
    $item = Get-Item -LiteralPath $Path -Force
    if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Linked tool/output path is not supported: $Path" }
    if ($item.PSIsContainer) {
        foreach ($child in Get-ChildItem -LiteralPath $Path -Force) { Assert-PlainTree $child.FullName }
    }
}

if (-not $DumpRoot) { $DumpRoot = Join-Path $GameDir "clientpatch\interop" }
if (-not $DumperDir) { $DumperDir = Join-Path $PSScriptRoot "tools\Il2CppDumper" }
if (-not $CliDll) { $CliDll = Join-Path $PSScriptRoot "tools\Il2CppInterop.CLI\Il2CppInterop.CLI.dll" }
if (-not (Test-Path -LiteralPath (Join-Path $DumperDir "Il2CppDumper.dll"))) { throw "Place the HBR-compatible dumper in $DumperDir or pass -DumperDir." }
if (-not (Test-Path -LiteralPath $CliDll)) { throw "Place Il2CppInterop.CLI at $CliDll or pass -CliDll." }
# Serialize against manager/native cache maintenance for the entire input lifetime.
$dataDir = Join-Path $GameDir "clientpatch"
Assert-PlainPath $dataDir
Assert-PlainPath $DumpRoot
New-Item -ItemType Directory -Force -Path $dataDir | Out-Null
$operationLockPath = Join-Path $dataDir '.manager-operation.lock'
Assert-PlainPath $operationLockPath
$operationLock = [IO.File]::Open($operationLockPath, [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
$previousRollForward = [Environment]::GetEnvironmentVariable('DOTNET_ROLL_FORWARD', 'Process')
try {
$env:DOTNET_ROLL_FORWARD = "LatestMajor"
$latest = Get-Content -LiteralPath (Join-Path $DumpRoot "latest.json") | ConvertFrom-Json
if ($latest.build_id -notmatch '^[0-9a-f]{16}$') { throw "Invalid dump build id." }
$build = Join-Path $DumpRoot $latest.build_id
Assert-PlainPath $build
$ga = Join-Path $build "GameAssembly.dll"
$meta = Join-Path $build "global-metadata.dat"
$out = Join-Path $build "interop"
Assert-PlainPath $out
$doneMarker = Join-Path $out "DONE"

if ((Test-Path $doneMarker) -and (Test-Path (Join-Path $out "Assembly-CSharp.dll")) -and (Test-Path (Join-Path $out "UnityEngine.CoreModule.dll")) -and -not $Force) {
    Write-Host "interop assemblies already generated for $($latest.build_id) (use -Force)"
    exit 0
}

Write-Host "== build $($latest.build_id): Il2CppDumper =="
$generationStage = Join-Path $build (".interopgen-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $generationStage | Out-Null
try {
$dumperOut = Join-Path $generationStage "dumper_out"
$generatedOut = Join-Path $generationStage "interop"
# Il2CppDumper only recognizes an output dir that ALREADY EXISTS (else it
# silently dumps into its own exe dir).
New-Item -ItemType Directory -Force $dumperOut | Out-Null
$privateDumper = Join-Path $build (".dumper-tool-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $privateDumper | Out-Null
Copy-Item -Path (Join-Path $DumperDir "*") -Destination $privateDumper -Recurse -Force
try {
    $dumperConfigPath = Join-Path $privateDumper "config.json"
    if (Test-Path -LiteralPath $dumperConfigPath) {
        $dumperConfig = Get-Content -LiteralPath $dumperConfigPath -Raw | ConvertFrom-Json
        $dumperConfig | Add-Member -NotePropertyName RequireAnyKey -NotePropertyValue $false -Force
        $dumperConfig | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath $dumperConfigPath
    }
    dotnet (Join-Path $privateDumper "Il2CppDumper.dll") $ga $meta $dumperOut
    if ($LASTEXITCODE -ne 0) { throw "Il2CppDumper failed ($LASTEXITCODE)" }
} finally {
    $resolvedPrivateDumper = [IO.Path]::GetFullPath($privateDumper)
    $resolvedBuildPrefix = [IO.Path]::GetFullPath($build).TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    if ($resolvedPrivateDumper.StartsWith($resolvedBuildPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        Assert-PlainPath $resolvedPrivateDumper
        Assert-PlainTree $resolvedPrivateDumper
        Remove-Item -LiteralPath $resolvedPrivateDumper -Recurse -Force
    }
}
if (-not (Test-Path (Join-Path $dumperOut "DummyDll\mscorlib.dll"))) { throw "Il2CppDumper produced no DummyDll" }

Write-Host "== build $($latest.build_id): Il2CppInterop.CLI generate =="
dotnet $CliDll generate --input (Join-Path $dumperOut "DummyDll") --output $generatedOut --game-assembly $ga
if ($LASTEXITCODE -ne 0) { throw "Il2CppInterop.CLI failed ($LASTEXITCODE)" }
if (-not (Test-Path (Join-Path $generatedOut "Assembly-CSharp.dll")) -or -not (Test-Path (Join-Path $generatedOut "UnityEngine.CoreModule.dll"))) { throw "Generator produced incomplete interop." }

@{ build_id = $latest.build_id; generated_unix = [DateTimeOffset]::Now.ToUnixTimeSeconds() } |
    ConvertTo-Json | Set-Content -LiteralPath (Join-Path $generatedOut "DONE")
# Publish a complete directory, so failed -Force runs never damage a prior DONE set.
$previousOut = Join-Path $generationStage "previous-interop"
$promotionSucceeded = $false
if (Test-Path -LiteralPath $out) {
    Assert-PlainTree $out
    [IO.Directory]::Move($out, $previousOut)
}
try {
    [IO.Directory]::Move($generatedOut, $out)
    $promotionSucceeded = $true
} catch {
    if (Test-Path -LiteralPath $previousOut) { [IO.Directory]::Move($previousOut, $out) }
    throw
}
$count = (Get-ChildItem $out -Filter *.dll).Count
Write-Host "== done: $count interop assemblies in $out =="
} finally {
    $resolvedStage = [IO.Path]::GetFullPath($generationStage)
    $buildPrefix = [IO.Path]::GetFullPath($build).TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    if (-not $resolvedStage.StartsWith($buildPrefix, [StringComparison]::OrdinalIgnoreCase)) { throw 'Generation cleanup escaped dump directory.' }
    # Retain a previous set if promotion and rollback both failed.
    if (-not (Test-Path -LiteralPath (Join-Path $generationStage 'previous-interop')) -or $promotionSucceeded) {
        Assert-PlainPath $resolvedStage
        Assert-PlainTree $resolvedStage
        Remove-Item -LiteralPath $resolvedStage -Recurse -Force
    }
}
} finally {
    [Environment]::SetEnvironmentVariable('DOTNET_ROLL_FORWARD', $previousRollForward, 'Process')
    $operationLock.Dispose()
}
