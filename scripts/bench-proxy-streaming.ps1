<#
.SYNOPSIS
    Measures modeld proxy memory and latency while streaming a large blob.

.DESCRIPTION
    Acceptance harness for ROADMAP A2:
      * peak RSS growth of the proxy process during a full download
      * /health p95 latency while 32 clients probe it under download load

    The script seeds a store with one large file, starts "modeld proxy start",
    downloads the blob end-to-end, and samples the server process.

.PARAMETER SizeMb
    Size of the test blob in MiB (default 512).

.PARAMETER HealthSamples
    Number of /health requests issued during the download (default 200).

.PARAMETER HealthConcurrency
    How many /health requests run at once (default 32).

.PARAMETER DownloadConcurrency
    How many parallel downloads keep the server busy during probing (default 4).

.PARAMETER MaxRssGrowthMb
    Fail when peak RSS growth exceeds this many MiB (default 100).

.PARAMETER MaxP95Ms
    Fail when /health p95 exceeds this many milliseconds (default 50).

.EXAMPLE
    pwsh -File scripts/bench-proxy-streaming.ps1 -SizeMb 1024
#>
param(
    [int]$SizeMb = 512,
    [int]$HealthSamples = 200,
    [int]$HealthConcurrency = 32,
    [int]$DownloadConcurrency = 4,
    [int]$MaxRssGrowthMb = 100,
    [double]$MaxP95Ms = 50,
    [string]$Modeld = "target/release/modeld.exe"
)

$ErrorActionPreference = "Stop"

# A sink that counts bytes without storing them, so the harness can verify the
# transfer without buffering half a gigabyte in the client process.
if (-not ("CountingNullStream" -as [type])) {
    Add-Type -TypeDefinition @"
using System;
using System.IO;
public sealed class CountingNullStream : Stream {
    public long Count;
    public override bool CanRead { get { return false; } }
    public override bool CanSeek { get { return false; } }
    public override bool CanWrite { get { return true; } }
    public override long Length { get { return Count; } }
    public override long Position { get { return Count; } set { throw new NotSupportedException(); } }
    public override void Flush() { }
    public override int Read(byte[] buffer, int offset, int count) { throw new NotSupportedException(); }
    public override long Seek(long offset, SeekOrigin origin) { throw new NotSupportedException(); }
    public override void SetLength(long value) { throw new NotSupportedException(); }
    public override void Write(byte[] buffer, int offset, int count) { Count += count; }
}
"@
}

function Fail($message) {
    Write-Host "FAIL: $message" -ForegroundColor Red
    exit 1
}

if (-not (Test-Path $Modeld)) {
    Fail "modeld binary not found at $Modeld (run: cargo build --release -p modeld-cli)"
}
$Modeld = (Resolve-Path $Modeld).Path

$root = Join-Path (Get-Location) "target/bench"
$dataDir = Join-Path $root "data"
$store = Join-Path $root "store"
$blob = Join-Path $dataDir "big.safetensors"

if (Test-Path $root) { Remove-Item -Recurse -Force $root }
New-Item -ItemType Directory -Force -Path $dataDir | Out-Null

# ── 1. Create the payload ───────────────────────────────────────────────────
Write-Host "Creating $SizeMb MiB payload ..." -ForegroundColor Cyan
$chunk = [byte[]]::new(1MB)
[System.Random]::new(42).NextBytes($chunk)
$fs = [System.IO.File]::Create($blob)
try {
    for ($i = 0; $i -lt $SizeMb; $i++) { $fs.Write($chunk, 0, $chunk.Length) }
} finally {
    $fs.Dispose()
}
$blobBytes = (Get-Item $blob).Length

# ── 2. Ingest it into a fresh store ─────────────────────────────────────────
Write-Host "Ingesting into store ..." -ForegroundColor Cyan
& $Modeld scan $dataDir --store $store *> (Join-Path $root "scan.log")
if ($LASTEXITCODE -ne 0) { Fail "modeld scan failed (see $root/scan.log)" }

# ── 3. Start the proxy ──────────────────────────────────────────────────────
$listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
$listener.Start()
$port = $listener.LocalEndpoint.Port
$listener.Stop()

$proxyLog = Join-Path $root "proxy.log"
$proxyErrLog = Join-Path $root "proxy.err.log"
$proxyArgs = @("proxy", "start", "--bind", "127.0.0.1", "--port", "$port", "--store", $store, "--allow-anonymous")
$proc = Start-Process -FilePath $Modeld -PassThru -NoNewWindow -RedirectStandardOutput $proxyLog -RedirectStandardError $proxyErrLog -ArgumentList $proxyArgs

$baseUrl = "http://127.0.0.1:$port"
$healthUrl = "$baseUrl/health"
$sessions = @()

function Start-Download($url) {
    $client = [System.Net.Http.HttpClient]::new()
    $client.Timeout = [TimeSpan]::FromMinutes(30)
    $resp = $client.GetAsync($url, [System.Net.Http.HttpCompletionOption]::ResponseHeadersRead).Result
    if (-not $resp.IsSuccessStatusCode) { throw "download failed: $($resp.StatusCode)" }
    $stream = $resp.Content.ReadAsStreamAsync().Result
    $sink = [CountingNullStream]::new()
    $task = $stream.CopyToAsync($sink)
    return @{ Client = $client; Response = $resp; Stream = $stream; Sink = $sink; Task = $task }
}

try {
    $ready = $false
    for ($i = 0; $i -lt 100; $i++) {
        try {
            $null = Invoke-RestMethod -Uri $healthUrl -TimeoutSec 2
            $ready = $true
            break
        } catch {
            Start-Sleep -Milliseconds 200
        }
    }
    if (-not $ready) { Fail "proxy did not become ready (see $proxyLog)" }

    $models = Invoke-RestMethod -Uri "$baseUrl/v1/models" -TimeoutSec 10
    $hash = $models.models[0].hash
    if (-not $hash) { Fail "no model was ingested" }
    $blobUrl = "$baseUrl/v1/blobs/$hash"

    $proc.Refresh()
    $baselineRss = $proc.WorkingSet64

    # ── 4. Full download while sampling RSS ─────────────────────────────────
    # The read loop runs inside .NET (CopyToAsync) because a PowerShell-level
    # read loop is far slower than the server and would never stress it.
    Write-Host "Downloading $SizeMb MiB and sampling RSS ..." -ForegroundColor Cyan
    $one = Start-Download $blobUrl
    $peakRss = $baselineRss
    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    while (-not $one.Task.IsCompleted) {
        $proc.Refresh()
        if ($proc.WorkingSet64 -gt $peakRss) { $peakRss = $proc.WorkingSet64 }
        Start-Sleep -Milliseconds 5
    }
    $timer.Stop()
    $one.Task.Wait()
    $downloaded = $one.Sink.Count
    $one.Stream.Dispose(); $one.Response.Dispose(); $one.Client.Dispose()
    if ($downloaded -ne $blobBytes) { Fail "downloaded $downloaded bytes, expected $blobBytes" }

    $seconds = $timer.Elapsed.TotalSeconds
    $mbPerSec = [math]::Round(($downloaded / 1MB) / $seconds, 1)

    # ── 5. /health latency under concurrent download load ───────────────────
    Write-Host "Probing /health with $HealthConcurrency concurrent clients under load ..." -ForegroundColor Cyan
    $load = @()
    for ($i = 0; $i -lt $DownloadConcurrency; $i++) { $load += Start-Download $blobUrl }

    $latencies = 1..$HealthSamples | ForEach-Object -Parallel {
        $c = [System.Net.Http.HttpClient]::new()
        $c.Timeout = [TimeSpan]::FromSeconds(10)
        try {
            $t = [System.Diagnostics.Stopwatch]::StartNew()
            $r = $c.GetAsync($using:healthUrl).Result
            $t.Stop()
            if ($r.IsSuccessStatusCode) { $t.Elapsed.TotalMilliseconds } else { 10000 }
        } catch {
            10000
        } finally {
            $c.Dispose()
        }
    } -ThrottleLimit $HealthConcurrency

    foreach ($session in $load) {
        $session.Task.Wait()
        $session.Stream.Dispose(); $session.Response.Dispose(); $session.Client.Dispose()
    }

    $proc.Refresh()
    $settledRss = $proc.WorkingSet64

    $sorted = @($latencies | Sort-Object)
    $p50 = $sorted[[int]([math]::Floor($sorted.Count * 0.50))]
    $p95 = $sorted[[int]([math]::Floor($sorted.Count * 0.95))]
    $worst = $sorted[$sorted.Count - 1]
    $rssGrowthMb = [math]::Round(($peakRss - $baselineRss) / 1MB, 1)

    Write-Host ""
    Write-Host "-------- proxy streaming benchmark --------" -ForegroundColor Cyan
    Write-Host ("  blob size          : {0:N0} MiB" -f ($blobBytes / 1MB))
    Write-Host ("  download           : {0:N0} MiB in {1:N2} s ({2} MiB/s)" -f ($downloaded / 1MB), $seconds, $mbPerSec)
    Write-Host ("  RSS baseline       : {0:N1} MiB" -f ($baselineRss / 1MB))
    Write-Host ("  RSS peak           : {0:N1} MiB" -f ($peakRss / 1MB))
    Write-Host ("  RSS settled        : {0:N1} MiB" -f ($settledRss / 1MB))
    Write-Host ("  RSS growth         : {0:N1} MiB (limit {1} MiB)" -f $rssGrowthMb, $MaxRssGrowthMb)
    Write-Host ("  /health p50        : {0:N1} ms" -f $p50)
    Write-Host ("  /health p95        : {0:N1} ms (limit {1} ms)" -f $p95, $MaxP95Ms)
    Write-Host ("  /health worst      : {0:N1} ms over {1} samples" -f $worst, $sorted.Count)
    Write-Host ("  load window        : {0} parallel downloads" -f $DownloadConcurrency)
    Write-Host "-------------------------------------------" -ForegroundColor Cyan

    $failed = $false
    if ($rssGrowthMb -gt $MaxRssGrowthMb) {
        Write-Host "FAIL: RSS grew by $rssGrowthMb MiB" -ForegroundColor Red
        $failed = $true
    }
    if ($p95 -gt $MaxP95Ms) {
        Write-Host "FAIL: /health p95 was $p95 ms" -ForegroundColor Red
        $failed = $true
    }
    if ($failed) { exit 1 }
    Write-Host "PASS" -ForegroundColor Green
} finally {
    foreach ($session in $sessions) {
        try { $session.Stream.Dispose(); $session.Response.Dispose(); $session.Client.Dispose() } catch { }
    }
    if ($proc -and -not $proc.HasExited) {
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    }
}
