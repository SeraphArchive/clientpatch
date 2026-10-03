#Requires -Version 7.0
param([Parameter(Mandatory)][string]$Destination)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$output = Join-Path $Destination 'licenses'
New-Item -ItemType Directory -Path $output -Force | Out-Null
$index = [Collections.Generic.List[string]]::new()
$index.Add('Third-party dependencies retain their own licenses. This inventory includes build dependencies as well as linked code.')
$mit = [IO.File]::ReadAllText((Join-Path $root 'LICENSE'))
$mitTerms = $mit.Substring($mit.IndexOf('Permission is hereby'))
function Copy-Notices([string]$Source, [string]$Target) {
    $files = @(Get-ChildItem -LiteralPath $Source -Recurse -File | Where-Object {
        $_.Name -match '^(?i:LICEN[SC]E|COPYING|NOTICE|COPYRIGHT|THIRD[-_]?PARTY[-_]?NOTICES)(?:$|[-_.])'
    })
    foreach ($file in $files) {
        $dest = Join-Path $Target ([IO.Path]::GetRelativePath($Source, $file.FullName))
        New-Item -ItemType Directory -Path (Split-Path $dest -Parent) -Force | Out-Null
        Copy-Item -LiteralPath $file.FullName -Destination $dest
    }
    return $files.Count
}
$json = & cargo metadata --locked --offline --format-version 1 --filter-platform x86_64-pc-windows-msvc
if ($LASTEXITCODE -ne 0) { throw 'Cargo dependency inventory failed.' }
$metadata = $json | ConvertFrom-Json
foreach ($package in $metadata.packages | Sort-Object name) {
    if ($package.name -eq 'clientpatch') { continue }
    $target = Join-Path $output "rust/$($package.name)-$($package.version)"
    New-Item -ItemType Directory -Path $target -Force | Out-Null
    $source = Split-Path $package.manifest_path -Parent
    $count = Copy-Notices $source $target
    $index.Add("Rust: $($package.name) $($package.version) | $($package.license) | $($package.repository)")
    if (-not $count) {
        if ($package.license -ne 'MIT') { throw "No bundled license for $($package.name). Review the dependency." }
        "Package authors: $($package.authors -join ', ')`n`n$mitTerms" | Set-Content (Join-Path $target 'LICENSE-MIT.txt')
    }
    # The vendored disassembler keeps its original MIT notices in C source headers.
    if ($package.name -eq 'libudis86-sys') {
        $headers = Get-ChildItem (Join-Path $source 'libudis86') -File | ForEach-Object {
            $text = [IO.File]::ReadAllText($_.FullName)
            $match = [regex]::Match($text, '(?s)^/\*.*?\*/')
            if ($match.Success -and $match.Value.Contains('Copyright')) { $match.Value }
        }
        $headers | Select-Object -Unique | Set-Content (Join-Path $target 'udis86-notices.txt')
    }
}
$assets = Get-Content (Join-Path $root 'manager/src/ClientpatchManager.App/obj/project.assets.json') -Raw | ConvertFrom-Json
foreach ($library in $assets.libraries.PSObject.Properties | Sort-Object Name) {
    if ($library.Value.type -ne 'package') { continue }
    $source = $null
    foreach ($folder in $assets.packageFolders.PSObject.Properties.Name) {
        $candidate = Join-Path $folder $library.Value.path
        if (Test-Path -LiteralPath $candidate) { $source = $candidate; break }
    }
    if (-not $source) { throw "Missing package: $($library.Name)" }
    [xml]$nuspec = Get-Content (Get-ChildItem $source -Filter '*.nuspec' | Select-Object -First 1).FullName
    $meta = $nuspec.SelectSingleNode("//*[local-name()='metadata']")
    $id = $meta.SelectSingleNode("*[local-name()='id']").InnerText
    $version = $meta.SelectSingleNode("*[local-name()='version']").InnerText
    $license = $meta.SelectSingleNode("*[local-name()='license']").InnerText
    $authors = $meta.SelectSingleNode("*[local-name()='authors']").InnerText
    $copyrightNode = $meta.SelectSingleNode("*[local-name()='copyright']")
    $copyright = if ($copyrightNode) { $copyrightNode.InnerText } else { "Package authors: $authors" }
    $target = Join-Path $output "nuget/$id-$version"
    New-Item -ItemType Directory -Path $target -Force | Out-Null
    $count = Copy-Notices $source $target
    $override = Join-Path $PSScriptRoot "license-overrides/$id-$version.txt"
    if (Test-Path -LiteralPath $override) { Copy-Item -LiteralPath $override -Destination (Join-Path $target 'LICENSE.txt') }
    elseif ($license -eq 'MIT') { "$copyright`n`n$mitTerms" | Set-Content (Join-Path $target 'LICENSE-MIT.txt') }
    elseif (-not $count) { throw "Review and supply the license for $id $version ($license)." }
    $index.Add("NuGet: $id $version | $license | $copyright")
}
$index | Set-Content (Join-Path $Destination 'THIRD-PARTY-NOTICES.txt')
