#Requires -Version 7.0
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
$files = @(& git -c "safe.directory=$root" -C $root ls-files --cached --others --exclude-standard) | Sort-Object -Unique
if ($LASTEXITCODE -ne 0) { throw 'Could not enumerate repository source files.' }
$issues = [System.Collections.Generic.List[string]]::new()
foreach ($relative in $files) {
    $path = Join-Path $root $relative
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { continue }
    if ([IO.Path]::GetExtension($path) -notin @('.rs', '.cs', '.csproj', '.props', '.targets', '.md', '.toml', '.ps1', '.html', '.yml', '.resx', '.xaml', '.config')) { continue }
    $content = [IO.File]::ReadAllText($path)
    if ($content -match '(?i)(?<![A-Za-z])[A-Za-z]:[/\\]+(?:work|users)[/\\]|/(?:home|Users)/') {
        $issues.Add("Workstation path: $relative")
    }
    # Absolute Windows test inputs are deliberate; production source must derive paths.
    if ($relative -match '^(src/.*\.rs|manager/src/.*\.cs)$') {
        $production = ($content -split '#\[cfg\(test\)\]', 2)[0]
        if ($production -match '(?<![A-Za-z])[A-Za-z]:[/\\](?!/)') { $issues.Add("Absolute production path: $relative") }
    }
    $references = @()
    if ($relative.EndsWith('.csproj')) {
        [xml]$project = $content
        $references += @($project.SelectNodes('//ProjectReference/@Include | //HintPath') | ForEach-Object { $_.InnerText })
    }
    if ($relative.EndsWith('.md')) {
        $prose = [regex]::Replace($content, '(?ms)^```.*?^```\s*', '')
        $references += @([regex]::Matches($prose, '\]\(([^)]+)\)') | ForEach-Object { $_.Groups[1].Value.Split('#')[0] })
    }
    foreach ($reference in $references) {
        if (-not $reference -or $reference -match '^[a-z][a-z0-9+.-]*://') { continue }
        $resolved = [IO.Path]::GetFullPath((Join-Path (Split-Path $path -Parent) $reference))
        if (-not $resolved.StartsWith($root + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
            $issues.Add("External file reference: $relative -> $reference")
        } elseif (-not (Test-Path -LiteralPath $resolved) -and $relative -notmatch '^tools/smokeplugin/') {
            $issues.Add("Missing file reference: $relative -> $reference")
        }
    }
}
if ($issues.Count) { throw ($issues -join "`n") }
Write-Host "Source hygiene passed ($($files.Count) repository files; synthetic test paths retained)."
