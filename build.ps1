#Requires -Version 7.0
[CmdletBinding()]
param(
    [ValidatePattern('^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$')]
    [string]$Version = ([regex]::Match([IO.File]::ReadAllText((Join-Path $PSScriptRoot 'Cargo.toml')), '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value),
    [switch]$SkipTests
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (-not $IsWindows) { throw 'Build on Windows x64 with Rust MSVC, LLVM and .NET 10 SDK.' }
foreach ($tool in @('cargo', 'rustc', 'dotnet', 'lld-link')) {
    Get-Command $tool -ErrorAction Stop | Out-Null
}

function Invoke-Checked([string]$Program, [string[]]$Arguments) {
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Program failed (exit $LASTEXITCODE)." }
}

$root = $PSScriptRoot
$stage = Join-Path $root ('dist/staging/' + [guid]::NewGuid().ToString('N'))
$bundle = Join-Path $stage 'bundle'
$publish = Join-Path $stage 'manager'
$release = Join-Path $root "dist/releases/$Version"
$oldRustFlags = $env:RUSTFLAGS
$oldEncodedFlags = $env:CARGO_ENCODED_RUSTFLAGS
$oldTargetDir = $env:CARGO_TARGET_DIR
$oldVersion = $env:CLIENTPATCH_VERSION
Push-Location $root
try {
    & (Join-Path $root 'check-source.ps1')
    # Fixed output location; remap source paths in panic strings and compiler metadata.
    $env:CARGO_TARGET_DIR = Join-Path $root 'target'
    $env:RUSTFLAGS = $null
    $sysroot = (& rustc --print sysroot).Trim()
    if ($LASTEXITCODE -ne 0) { throw 'Could not locate the Rust toolchain.' }
    $userProfile = [Environment]::GetFolderPath('UserProfile')
    $env:CARGO_ENCODED_RUSTFLAGS = @(
        "--remap-path-prefix=$userProfile=build-user",
        "--remap-path-prefix=$($userProfile.Replace('\', '/'))=build-user",
        "--remap-path-prefix=$sysroot=rust-toolchain",
        "--remap-path-prefix=$($sysroot.Replace('\', '/'))=rust-toolchain",
        "--remap-path-prefix=$root=clientpatch",
        "--remap-path-prefix=$($root.Replace('\', '/'))=clientpatch"
    ) -join [char]0x1f
    $env:CLIENTPATCH_VERSION = $Version
    Invoke-Checked dotnet @('restore', 'manager/ClientpatchManager.sln', '-r', 'win-x64', '--configfile', 'NuGet.Config')
    if (-not $SkipTests) {
        Invoke-Checked cargo @('test', '--locked', '--target', 'x86_64-pc-windows-msvc')
        Invoke-Checked dotnet @('test', 'manager/ClientpatchManager.sln', '-c', 'Release', '-r', 'win-x64', '--no-restore')
        Invoke-Checked pwsh @('-NoProfile', '-File', 'tools/interopgen/check-interopgen.ps1')
        Invoke-Checked pwsh @('-NoProfile', '-File', 'tools/interopgen/check-deploy-bepinex.ps1')
    }
    Invoke-Checked cargo @('build', '--locked', '--release', '--target', 'x86_64-pc-windows-msvc')
    $assemblyVersion = ($Version -split '-')[0]
    Invoke-Checked dotnet @('publish', 'manager/src/ClientpatchManager.App/ClientpatchManager.App.csproj',
        '-c', 'Release', '-r', 'win-x64', '--self-contained', 'false', '--no-restore', '-o', $publish,
        "-p:Version=$assemblyVersion", "-p:InformationalVersion=$Version", '-p:DebugType=None',
        '-p:DebugSymbols=false', "-p:PathMap=$root=clientpatch", '-p:IncludeNativeLibrariesForSelfExtract=true')

    New-Item -ItemType Directory -Path $bundle -Force | Out-Null
    Copy-Item -LiteralPath 'target/x86_64-pc-windows-msvc/release/clientpatch.dll' -Destination (Join-Path $bundle 'version.dll')
    # The updater admits only manager.exe at the root. Reject publish sidecars that
    # would work after manual extraction but be silently omitted by self-update.
    foreach ($file in Get-ChildItem -LiteralPath $publish -Recurse -File) {
        if ([IO.Path]::GetRelativePath($publish, $file.FullName) -ne 'manager.exe') {
            throw "Manager publish must be a single executable; unexpected sidecar: $($file.Name)"
        }
    }
    Copy-Item -Path "$publish/*" -Destination $bundle -Recurse
    New-Item -ItemType Directory -Path (Join-Path $bundle 'clientpatch') | Out-Null
    Copy-Item -LiteralPath 'clientpatch.toml' -Destination (Join-Path $bundle 'clientpatch/clientpatch.example.toml')
    $Version | Set-Content -LiteralPath (Join-Path $bundle 'clientpatch/clientpatch.version') -Encoding ascii
    Copy-Item -LiteralPath 'README.md' -Destination (Join-Path $bundle 'clientpatch/README.md')
    # Notices must travel through the same allowlisted update path as their binaries.
    Copy-Item -LiteralPath 'LICENSE' -Destination (Join-Path $bundle 'clientpatch/LICENSE')
    & (Join-Path $root 'tools/package-notices.ps1') -Destination (Join-Path $bundle 'clientpatch')
    foreach ($required in @('version.dll', 'manager.exe', 'clientpatch/clientpatch.example.toml', 'clientpatch/LICENSE', 'clientpatch/README.md')) {
        if (-not (Test-Path -LiteralPath (Join-Path $bundle $required) -PathType Leaf)) {
            throw "Missing package file: $required"
        }
    }
    $rootEntries = @(Get-ChildItem -LiteralPath $bundle -Force | Select-Object -ExpandProperty Name | Sort-Object)
    if (($rootEntries -join '|') -cne 'clientpatch|manager.exe|version.dll') {
        throw "Unexpected package root entries: $($rootEntries -join ', ')"
    }
    & (Join-Path $root 'tools/check-version-proxy.ps1') -DllPath (Join-Path $bundle 'version.dll') -VersionedFile (Join-Path $bundle 'manager.exe')
    foreach ($file in Get-ChildItem $bundle -Recurse -File) {
        $bytes = [IO.File]::ReadAllBytes($file.FullName)
        foreach ($encoding in @([Text.Encoding]::UTF8, [Text.Encoding]::Unicode)) {
            $text = $encoding.GetString($bytes)
            foreach ($prefix in @($root, $root.Replace('\', '/'), $userProfile, $userProfile.Replace('\', '/'), $sysroot, $sysroot.Replace('\', '/'))) {
                if ($text.Contains($prefix, [StringComparison]::OrdinalIgnoreCase)) { throw "Build-machine path in package: $($file.Name)" }
            }
        }
        if ($file.LastWriteTime.Year -lt 1980) { $file.LastWriteTime = [datetime]'1980-01-01' }
    }
    # A fresh staging directory prevents stale files from entering the archive.
    $archive = Join-Path $stage "clientpatch-v$Version-win-x64.zip"
    Compress-Archive -Path "$bundle/*" -DestinationPath $archive
    New-Item -ItemType Directory -Path $release -Force | Out-Null
    foreach ($asset in @($archive)) {
        $name = Split-Path $asset -Leaf
        Copy-Item -LiteralPath $asset -Destination (Join-Path $release $name) -Force
        $hash = (Get-FileHash -LiteralPath $asset -Algorithm SHA256).Hash.ToLowerInvariant()
        "$hash  $name" | Set-Content -LiteralPath (Join-Path $release "$name.sha256") -Encoding ascii
    }
    Write-Host "Release assets: $release"
} finally {
    $env:RUSTFLAGS = $oldRustFlags
    $env:CARGO_ENCODED_RUSTFLAGS = $oldEncodedFlags
    $env:CARGO_TARGET_DIR = $oldTargetDir
    $env:CLIENTPATCH_VERSION = $oldVersion
    Pop-Location
}
