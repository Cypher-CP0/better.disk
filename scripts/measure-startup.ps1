<#
.SYNOPSIS
  Measures launch-to-first-paint of the shell spike over several runs.

.DESCRIPTION
  Starts the release build of the app repeatedly. Each run writes its
  startup report (process creation -> Rust main -> setup -> first painted
  frame) and exits right after the first paint. The first run after a
  reboot is the cold start; later runs are warm.

  Build first:
    cd app; npm ci; npm run build:exe

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts\measure-startup.ps1 -Runs 10
#>
[CmdletBinding()]
param(
  [int]$Runs = 10,
  [string]$Exe,
  [string]$Out
)

$ErrorActionPreference = "Stop"
# $PSScriptRoot can be empty while parameter defaults are evaluated (seen with
# Windows PowerShell 5.1 and -File), so paths that depend on it are resolved
# here instead.
$scriptDir = $PSScriptRoot
if (-not $scriptDir -and $MyInvocation.MyCommand.Path) {
  $scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
}
if (-not $scriptDir) { $scriptDir = Join-Path (Get-Location).Path "scripts" }
$repo = Split-Path -Parent $scriptDir
if (-not $Exe) { $Exe = Join-Path $repo "target\release\bd-app.exe" }
if (-not $Out) { $Out = Join-Path $repo "stage0-out" }

if (-not (Test-Path -LiteralPath $Exe)) {
  throw "No app build at $Exe. Run: cd app; npm ci; npm run build:exe"
}
New-Item -ItemType Directory -Force -Path $Out | Out-Null

$values = @()
for ($i = 1; $i -le $Runs; $i++) {
  $report = Join-Path $Out ("startup-{0:D2}.report.json" -f $i)
  if (Test-Path -LiteralPath $report) { Remove-Item -LiteralPath $report }
  $env:BD_STARTUP_REPORT = $report
  $env:BD_EXIT_AFTER_FIRST_PAINT = "1"
  try {
    $p = Start-Process -FilePath $Exe -PassThru
    if (-not $p.WaitForExit(30000)) {
      $p.Kill()
      Write-Warning "run $i did not exit within 30 s"
      continue
    }
  }
  finally {
    Remove-Item Env:\BD_STARTUP_REPORT, Env:\BD_EXIT_AFTER_FIRST_PAINT -ErrorAction SilentlyContinue
  }
  if (-not (Test-Path -LiteralPath $report)) {
    Write-Warning "run $i wrote no report"
    continue
  }
  $r = Get-Content -LiteralPath $report -Raw | ConvertFrom-Json
  $values += [double]$r.processToFirstPaintMs
  Write-Host ("run {0,2}: launch to first paint {1,7:N0} ms (main {2:N0} ms, setup {3:N0} ms)" -f `
      $i, $r.processToFirstPaintMs, $r.processToMainMs, $r.processToSetupMs)
  Start-Sleep -Milliseconds 500
}

if ($values.Count -gt 0) {
  $sorted = $values | Sort-Object
  $median = $sorted[[int][math]::Floor(($sorted.Count - 1) / 2)]
  $p95 = $sorted[[int][math]::Round(($sorted.Count - 1) * 0.95)]
  Write-Host ""
  Write-Host ("first run (cold if after reboot): {0:N0} ms" -f $values[0])
  Write-Host ("median {0:N0} ms, p95 {1:N0} ms, max {2:N0} ms over {3} runs; target <= 3000 ms" -f `
      $median, $p95, $sorted[-1], $values.Count)
}
