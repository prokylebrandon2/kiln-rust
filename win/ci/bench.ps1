# Speed comparison: kiln-win vs 7-Zip on one data set (best of N runs; the summary table goes to
# the GitHub job summary). Informational: it never fails the job on a slow result.
param(
    [Parameter(Mandatory)][string]$Exe,
    [Parameter(Mandatory)][string]$Data,
    [int]$Runs = 3
)

$ErrorActionPreference = 'Stop'
$sevenZip = 'C:\Program Files\7-Zip\7z.exe'
if (-not (Test-Path -LiteralPath $sevenZip)) { throw '7-Zip not found on this runner' }

$tmp = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
$work = Join-Path $tmp 'kiln-bench'
if (Test-Path -LiteralPath $work) { Remove-Item -LiteralPath $work -Recurse -Force }
New-Item -ItemType Directory -Path $work | Out-Null

$files = Get-ChildItem -LiteralPath $Data -Recurse -File -Force |
    Where-Object { $_.FullName -notmatch '\\\.git\\' }
$bytes = ($files | Measure-Object Length -Sum).Sum
$summary = New-Object System.Collections.Generic.List[string]
$summary.Add("Data set: $($files.Count) files, $([math]::Round($bytes / 1MB, 1)) MiB, $([Environment]::ProcessorCount) cores")
$summary.Add('')
$summary.Add('| Format | Tool | Pack (s) | Size (MiB) | Unpack (s) |')
$summary.Add('|---|---|---:|---:|---:|')

function Run([string]$Tool, [string[]]$ArgList) {
    & $Tool @ArgList | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "$Tool failed with exit code $LASTEXITCODE" }
}

function Best([scriptblock]$Block, [scriptblock]$Reset) {
    $best = [double]::MaxValue
    for ($i = 0; $i -lt $Runs; $i++) {
        & $Reset
        $t = (Measure-Command { & $Block }).TotalSeconds
        if ($t -lt $best) { $best = $t }
    }
    $best
}

function Measure-Case([string]$Format, [string]$Tool, [string]$Archive, [scriptblock]$Pack, [scriptblock]$Unpack) {
    $out = Join-Path $work 'extract'
    $packSec = Best $Pack { Remove-Item -LiteralPath $Archive -Force -ErrorAction SilentlyContinue }
    $size = (Get-Item -LiteralPath $Archive).Length / 1MB
    $unpackSec = Best $Unpack { Remove-Item -LiteralPath $out -Recurse -Force -ErrorAction SilentlyContinue }
    $row = '| {0} | {1} | {2:N2} | {3:N1} | {4:N2} |' -f $Format, $Tool, $packSec, $size, $unpackSec
    Write-Host $row
    $summary.Add($row)
}

$z1 = Join-Path $work 'kiln.zip'
$z7 = Join-Path $work '7z.zip'
$k1 = Join-Path $work 'kiln.kiln'
$s7 = Join-Path $work '7z.7z'
$out = Join-Path $work 'extract'

Measure-Case 'zip, level 6' 'kiln-win' $z1 `
    { Run $Exe @('a', $z1, $Data, '-l', '6', '-y') } `
    { Run $Exe @('x', $z1, '-o', $out, '-y') }
Measure-Case 'zip, level 6' '7-Zip' $z7 `
    { Run $sevenZip @('a', '-tzip', '-mx=6', '-mmt=on', $z7, $Data) } `
    { Run $sevenZip @('x', '-y', "-o$out", $z7) }
Measure-Case '.kiln (adaptive) / 7z (LZMA2, -mx5)' 'kiln-win' $k1 `
    { Run $Exe @('a', $k1, $Data, '-l', '5', '-y') } `
    { Run $Exe @('x', $k1, '-o', $out, '-y') }
Measure-Case '.kiln (adaptive) / 7z (LZMA2, -mx5)' '7-Zip' $s7 `
    { Run $sevenZip @('a', '-t7z', '-mx=5', '-mmt=on', $s7, $Data) } `
    { Run $sevenZip @('x', '-y', "-o$out", $s7) }

# Thread sweep (kiln-win only): is it faster with more workers than cores? Creating files waits on
# the file system and the virus scanner, so extra threads can hide that wait.
$cores = [Environment]::ProcessorCount
foreach ($workers in ($cores * 2), ($cores * 4)) {
    Measure-Case "zip, level 6, -t $workers" 'kiln-win' $z1 `
        { Run $Exe @('a', $z1, $Data, '-l', '6', '-y', '-t', "$workers") } `
        { Run $Exe @('x', $z1, '-o', $out, '-y', '-t', "$workers") }
    Measure-Case ".kiln (adaptive), -t $workers" 'kiln-win' $k1 `
        { Run $Exe @('a', $k1, $Data, '-l', '5', '-y', '-t', "$workers") } `
        { Run $Exe @('x', $k1, '-o', $out, '-y', '-t', "$workers") }
}

if ($env:GITHUB_STEP_SUMMARY) { $summary | Add-Content -LiteralPath $env:GITHUB_STEP_SUMMARY }
$global:LASTEXITCODE = 0
