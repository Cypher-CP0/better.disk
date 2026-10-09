<#
.SYNOPSIS
  Runs the Stage 0 measurements on this Windows machine and collects every
  report in stage0-out\.

.DESCRIPTION
  Builds the spikes in release mode, creates a disposable fixture and a
  recycle sandbox under -Work, and runs:

    1. the enumeration spike on the fixture (native reader, full
       verification, manifest check), plus the FileFullDirectoryInfo and
       std readers for comparison;
    2. a worker-count sweep and a cancellation-latency run;
    3. the query benchmark at 100,000 and 1,000,000 entries;
    4. the recycle spike on the sandbox (unless -SkipRecycle);
    5. optionally a read-only scan of the system drive (-ScanSystemDrive),
       with paths redacted.

  Nothing outside -Work is modified. The system-drive scan only reads
  metadata.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts\stage0.ps1
  powershell -ExecutionPolicy Bypass -File scripts\stage0.ps1 -BulkFiles 100000 -BulkBytes 4GB -ScanSystemDrive
#>
[CmdletBinding()]
param(
  [string]$Work = (Join-Path $env:TEMP "bd-stage0"),
  [string]$Out = (Join-Path (Split-Path -Parent $PSScriptRoot) "stage0-out"),
  [int]$BulkFiles = 100000,
  [string]$BulkBytes = "2GB",
  [switch]$SkipBuild,
  [switch]$SkipRecycle,
  [switch]$ScanSystemDrive
)

$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
$bin = Join-Path $repo "target\release"
$results = [ordered]@{}

function Invoke-Step([string]$Name, [scriptblock]$Body) {
  Write-Host ""
  Write-Host "=== $Name ===" -ForegroundColor Cyan
  & $Body
  $code = $LASTEXITCODE
  $results[$Name] = $code
  if ($code -ne 0) { Write-Warning "$Name exited with $code" }
}

Push-Location $repo
try {
  if (-not $SkipBuild) {
    cargo build --release --locked -p bd-fixtures -p bd-spike-scan -p bd-spike-query -p bd-spike-recycle
    if ($LASTEXITCODE -ne 0) { throw "build failed" }
  }
  New-Item -ItemType Directory -Force -Path $Out, $Work | Out-Null
  $fixture = Join-Path $Work "fixture"
  $sandbox = Join-Path $Work "sandbox"
  foreach ($p in @($fixture, $sandbox)) {
    if (Test-Path -LiteralPath $p) { & "$bin\bd-fixtures.exe" clean $p }
  }

  Invoke-Step "create fixture" {
    & "$bin\bd-fixtures.exe" create $fixture --bulk-files $BulkFiles --bulk-bytes $BulkBytes
  }
  Invoke-Step "scan: native reader, verify all, manifest" {
    & "$bin\bd-spike-scan.exe" $fixture --verify all --quiet --json (Join-Path $Out "scan-fixture-native.report.json")
  }
  Invoke-Step "scan: FileFullDirectoryInfo reader" {
    & "$bin\bd-spike-scan.exe" $fixture --reader full --quiet --json (Join-Path $Out "scan-fixture-full.report.json")
  }
  Invoke-Step "scan: std reader baseline" {
    & "$bin\bd-spike-scan.exe" $fixture --reader std --no-manifest --quiet --json (Join-Path $Out "scan-fixture-std.report.json")
  }
  Invoke-Step "scan: worker sweep" {
    & "$bin\bd-spike-scan.exe" $fixture --workers 1,2,4,8 --repeat 3 --no-manifest --quiet --json (Join-Path $Out "scan-fixture-workers.report.json")
  }
  Invoke-Step "scan: cancellation latency" {
    & "$bin\bd-spike-scan.exe" $fixture --cancel-after-ms 300 --repeat 5 --no-manifest --quiet --json (Join-Path $Out "scan-fixture-cancel.report.json")
  }
  Invoke-Step "query: 100,000 entries" {
    & "$bin\bd-spike-query.exe" --files 100000 --json (Join-Path $Out "query-100k.report.json")
  }
  Invoke-Step "query: 1,000,000 entries" {
    & "$bin\bd-spike-query.exe" --files 1000000 --json (Join-Path $Out "query-1m.report.json")
  }

  if (-not $SkipRecycle) {
    Invoke-Step "create recycle sandbox" {
      & "$bin\bd-fixtures.exe" recycle-sandbox $sandbox
    }
    Invoke-Step "recycle: strict profile" {
      & "$bin\bd-spike-recycle.exe" $sandbox --json (Join-Path $Out "recycle-strict.report.json")
    }
  }

  if ($ScanSystemDrive) {
    $drive = "$env:SystemDrive\"
    Invoke-Step "scan: system drive (read-only, paths redacted)" {
      & "$bin\bd-spike-scan.exe" $drive --verify 5000 --quiet --json (Join-Path $Out "scan-system-drive.report.json")
    }
  }

  & "$bin\bd-fixtures.exe" clean $fixture | Out-Null
  Write-Host ""
  Write-Host "=== Summary ===" -ForegroundColor Cyan
  $results.GetEnumerator() | ForEach-Object {
    $state = if ($_.Value -eq 0) { "ok" } else { "exit $($_.Value)" }
    Write-Host ("{0,-50} {1}" -f $_.Key, $state)
  }
  Write-Host ""
  Write-Host "Reports: $Out"
  Write-Host "The recycle sandbox is left at $sandbox for inspection; remove it with:"
  Write-Host "  $bin\bd-fixtures.exe clean `"$sandbox`""
}
finally {
  Pop-Location
}
