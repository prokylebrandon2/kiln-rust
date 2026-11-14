# Round-trip checks for kiln-win.exe on a throw-away tree: .zip and .kiln, selective extract,
# overwrite protection, and ZIP interoperability with PowerShell and 7-Zip (both directions).
param([Parameter(Mandatory)][string]$Exe)

$ErrorActionPreference = 'Stop'

$tmp = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
$work = Join-Path $tmp 'kiln-smoke'
if (Test-Path -LiteralPath $work) { Remove-Item -LiteralPath $work -Recurse -Force }
New-Item -ItemType Directory -Path $work | Out-Null
Push-Location $work

function Run([string]$Tool, [string[]]$ArgList) {
    & $Tool @ArgList
    if ($LASTEXITCODE -ne 0) {
        throw "$Tool $($ArgList -join ' ') failed with exit code $LASTEXITCODE"
    }
}

function Get-Tree([string]$Root) {
    $full = (Resolve-Path -LiteralPath $Root).Path
    Get-ChildItem -LiteralPath $full -Recurse -Force | ForEach-Object {
        $rel = $_.FullName.Substring($full.Length).TrimStart('\')
        if ($_.PSIsContainer) {
            "D $rel"
        } else {
            "F $rel $((Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash)"
        }
    } | Sort-Object
}

function Assert-Same([string]$Expected, [string]$Actual, [string]$What) {
    $a = Get-Tree $Expected
    $b = Get-Tree $Actual
    $diff = Compare-Object $a $b
    if ($diff) {
        $diff | Select-Object -First 20 | Format-Table -AutoSize | Out-String | Write-Host
        throw "$What: extracted tree differs from the source"
    }
    Write-Host "ok: $What ($($a.Count) items)"
}

# ---- the test tree -------------------------------------------------------------------------
$src = Join-Path $work 'src'
New-Item -ItemType Directory -Force -Path "$src\docs\deep\er", "$src\empty dir", "$src\many" | Out-Null
Set-Content -LiteralPath "$src\a.txt" -Value 'hello kiln' -Encoding ascii
[IO.File]::WriteAllBytes("$src\zero.bin", [byte[]]@())
$line = 'The quick brown fox jumps over the lazy dog. ' * 4
[IO.File]::WriteAllText("$src\big.txt", ($line + "`n") * 40000)
$random = New-Object byte[] (3MB)
(New-Object System.Random 42).NextBytes($random)
[IO.File]::WriteAllBytes("$src\random.bin", $random)
$unicode = "caf$([char]0xE9) $([char]0x65E5)$([char]0x672C).txt"
Set-Content -LiteralPath "$src\$unicode" -Value 'unicode name' -Encoding utf8
Set-Content -LiteralPath "$src\docs\deep\er\n.txt" -Value 'deep file' -Encoding ascii
for ($i = 0; $i -lt 200; $i++) {
    Set-Content -LiteralPath "$src\many\f$i.txt" -Value ("file $i " * (10 + $i)) -Encoding ascii
}

# ---- kiln-win round trips ------------------------------------------------------------------
foreach ($ext in 'zip', 'kiln') {
    Write-Host "== .$ext =="
    $arc = Join-Path $work "t.$ext"
    Run $Exe @('a', $arc, 'src')
    Run $Exe @('l', $arc)

    $out = Join-Path $work "out-$ext"
    Run $Exe @('x', $arc, '-o', $out)
    Assert-Same $src (Join-Path $out 'src') "kiln-win round trip .$ext"

    # An existing archive must not be replaced without -y.
    & $Exe a $arc src 2>$null
    if ($LASTEXITCODE -eq 0) { throw "adding to an existing .$ext without -y should fail" }
    $global:LASTEXITCODE = 0
    Run $Exe @('a', $arc, 'src', '-y')

    # Selective extract: one folder, nothing else.
    $part = Join-Path $work "part-$ext"
    Run $Exe @('x', $arc, 'src/docs', '-o', $part)
    if (-not (Test-Path -LiteralPath "$part\src\docs\deep\er\n.txt")) { throw "selective extract .$ext lost the file" }
    if (Test-Path -LiteralPath "$part\src\a.txt") { throw "selective extract .$ext took more than asked" }
    Write-Host "ok: selective extract .$ext"
}

# ---- ZIP interoperability --------------------------------------------------------------------
$zip = Join-Path $work 't.zip'

$ps = Join-Path $work 'out-powershell'
Expand-Archive -LiteralPath $zip -DestinationPath $ps
Assert-Same $src (Join-Path $ps 'src') 'PowerShell Expand-Archive reads our zip'

$sevenZip = 'C:\Program Files\7-Zip\7z.exe'
if (Test-Path -LiteralPath $sevenZip) {
    Run $sevenZip @('t', $zip)
    $o7 = Join-Path $work 'out-7z'
    Run $sevenZip @('x', '-y', "-o$o7", $zip)
    Assert-Same $src (Join-Path $o7 'src') '7-Zip reads our zip'

    foreach ($mx in 0, 1, 9) {
        $z7 = Join-Path $work "by7z-$mx.zip"
        Run $sevenZip @('a', '-tzip', "-mx=$mx", '-mcu=on', $z7, $src)
        $back = Join-Path $work "back-$mx"
        Run $Exe @('x', $z7, '-o', $back)
        Assert-Same $src (Join-Path $back 'src') "we read a 7-Zip zip (-mx=$mx)"
    }
} else {
    Write-Warning '7-Zip not found on this runner; skipped the 7-Zip interoperability checks'
}

Pop-Location
$global:LASTEXITCODE = 0
Write-Host 'smoke test passed'
