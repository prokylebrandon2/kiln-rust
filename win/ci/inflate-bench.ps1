# Native DEFLATE decoder vs libdeflate, on zips made by 7-Zip (an encoder that is not ours): one of a
# source tree (many small text files) and one of binaries (big entries). `kiln-win bench-inflate`
# checks every entry's output against the other decoder and the CRC-32, so a wrong decode fails the
# job; a slow result never does. The tables go to the GitHub job summary.
param(
    [Parameter(Mandatory)][string]$Exe,
    [Parameter(Mandatory)][string]$Data,
    [Parameter(Mandatory)][string[]]$Binaries,
    [int]$Runs = 7
)

$ErrorActionPreference = 'Stop'
$sevenZip = 'C:\Program Files\7-Zip\7z.exe'
if (-not (Test-Path -LiteralPath $sevenZip)) { throw '7-Zip not found on this runner' }

$tmp = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
$work = Join-Path $tmp 'kiln-inflate-bench'
if (Test-Path -LiteralPath $work) { Remove-Item -LiteralPath $work -Recurse -Force }
New-Item -ItemType Directory -Path $work | Out-Null

$summary = New-Object System.Collections.Generic.List[string]
$summary.Add('### Native inflate vs libdeflate')
$summary.Add('')

function Bench([string]$Title, [string]$Zip) {
    $lines = & $Exe bench-inflate $Zip -n $Runs
    if ($LASTEXITCODE -ne 0) { throw "bench-inflate failed on $Title (exit code $LASTEXITCODE)" }
    $lines | ForEach-Object { Write-Host $_ }
    $summary.Add("**$Title**")
    $summary.Add('')
    foreach ($l in $lines) { $summary.Add([string]$l) }
    $summary.Add('')
}

$tree = Join-Path $work 'tree.zip'
Push-Location -LiteralPath $Data
try { & $sevenZip a -tzip -mx=6 -mmt=on '-xr!.git' $tree '.' | Out-Null } finally { Pop-Location }
if ($LASTEXITCODE -ne 0) { throw '7-Zip failed on the source tree' }
Bench 'Source tree, 7-Zip -mx6' $tree

$bin = Join-Path $work 'binaries.zip'
& $sevenZip a -tzip -mx=6 $bin @Binaries | Out-Null
if ($LASTEXITCODE -ne 0) { throw '7-Zip failed on the binaries' }
Bench 'Binaries, 7-Zip -mx6' $bin

if ($env:GITHUB_STEP_SUMMARY) { $summary | Add-Content -LiteralPath $env:GITHUB_STEP_SUMMARY }
$global:LASTEXITCODE = 0
