# Windows field report. Requires only the built-in Windows PowerShell 5.1.
[CmdletBinding()]
param(
    [string]$StateDir = (Join-Path $env:LOCALAPPDATA 'FlashLink\DemoHost'),
    [string]$OutputDir = (Join-Path ([Environment]::GetFolderPath('MyDocuments')) 'FlashLink\testdata'),
    [string]$HostExecutable
)
$ErrorActionPreference = 'Stop'
$utf8 = New-Object System.Text.UTF8Encoding($false)
try { [Console]::OutputEncoding = $utf8 } catch {}

$now = [DateTime]::UtcNow
$video = @()
$gpuStatus = 'collected'
try {
    $video = @(Get-CimInstance Win32_VideoController -OperationTimeoutSec 10 -ErrorAction Stop | ForEach-Object {
        [ordered]@{ model = $_.Name; driver = $_.DriverVersion
            desktop_width = $_.CurrentHorizontalResolution; desktop_height = $_.CurrentVerticalResolution
            desktop_refresh_hz = $_.CurrentRefreshRate }
    })
} catch { $gpuStatus = 'unavailable' }

function Read-EngineLog([string]$hostLog) {
    $engine = [ordered]@{ status = 'unavailable'; reason = 'host_log_missing' }
    if (Test-Path -LiteralPath $hostLog -PathType Leaf) {
        $lines = @(Get-Content -LiteralPath $hostLog -Tail 4000)
        $issueCounts = @{}
        $categories = [ordered]@{ encoder = 'encod|nvenc|amf|quicksync|h264|hevc'
            audio = 'audio|opus|wasapi|sound'; display = 'display|monitor|topology'
            capture = 'captur|dxgi|duplication'; connection = 'connect|socket|network|rtsp|port '
            identity = 'certificat|pairing|tls'; input = 'mouse|keyboard|input' }
        foreach ($line in $lines) {
            if ($line -notmatch '\b(Warning|Error|Fatal):') { continue }
            $kind = 'other'
            foreach ($key in $categories.Keys) {
                if ($line -match $categories[$key]) { $kind = $key; break }
            }
            $issueCounts[$kind] = 1 + [int]$issueCounts[$kind]
        }
        $engine = [ordered]@{ status = 'collected'; max_tail_lines = 4000
            issue_counts_including_startup_probes = $issueCounts
            last_write_utc = (Get-Item -LiteralPath $hostLog).LastWriteTimeUtc.ToString('o')
            counts = [ordered]@{
                connect_events = @($lines -cmatch 'CLIENT CONNECTED').Count
                disconnect_events = @($lines -cmatch 'CLIENT DISCONNECTED').Count
                opus_initializations = @($lines -cmatch 'Opus initialized:').Count
            }
        }
    }
    return $engine
}
$engine = Read-EngineLog (Join-Path $StateDir 'flashlink-sunshine.log')
$previous = Read-EngineLog (Join-Path $StateDir 'flashlink-sunshine-previous.log')
$events = @()
$privacyLog = Join-Path $StateDir 'privacy-events.log'
if (Test-Path -LiteralPath $privacyLog -PathType Leaf) {
    $events = @(foreach ($line in Get-Content -LiteralPath $privacyLog -Tail 256) {
        if ($line -cmatch '^(\d{1,12})\s+(private|lost|resumed|locking|locked|restored|restore_pending|display_reapplied|guard_(?:pipe_closed|display_changed|display_timeout|display_without_lease|desktop_unavailable|engine_exited|lease_expired|user_end|owner_unavailable|management_unavailable))$') {
            [ordered]@{ unix_seconds = [long]$Matches[1]; event = [string]$Matches[2] }
        }
    })
}
$displayLog = @()
$privacyDisplayLog = Join-Path $StateDir 'privacy-display.log'
if (Test-Path -LiteralPath $privacyDisplayLog -PathType Leaf) {
    $displayLog = @(foreach ($line in Get-Content -LiteralPath $privacyDisplayLog -Tail 256) {
        if ($line -cmatch '^\d{1,12}\s+phase=[a-z_]+(?:\s+[a-z_]+=[-0-9a-zA-Z_,/:]+)+$') {
            [string]$Matches[0]
        }
    })
}
$report = [ordered]@{
    schema_version = 1; mode = 'windows_host'; created_utc = $now.ToString('o')
    platform = 'Windows'; os_release = [Environment]::OSVersion.Version.ToString()
    graphics = [ordered]@{ status = $gpuStatus; adapters = $video }
    host = $engine; previous_stream_log = $previous; privacy_events = $events
    privacy_display_log = $displayLog
    physical_screen_observation = 'not_tested'; audible_playback_observation = 'not_tested'
}
if ($HostExecutable -and (Test-Path -LiteralPath $HostExecutable -PathType Leaf)) {
    $report['host_package_sha256'] = (Get-FileHash -LiteralPath $HostExecutable -Algorithm SHA256).Hash.ToLowerInvariant()
}
$null = [IO.Directory]::CreateDirectory($OutputDir)
$id = [Guid]::NewGuid().ToString('N').Substring(0, 8)
$filename = 'windows-' + $now.ToString('yyyyMMddTHHmmssZ') + '-' + $id + '.json'
$path = Join-Path $OutputDir $filename
[IO.File]::WriteAllText($path + '.tmp', ($report | ConvertTo-Json -Depth 6), $utf8)
[IO.File]::Move($path + '.tmp', $path)
if ([Console]::IsOutputRedirected) {
    $bytes = $utf8.GetBytes($path + "`n")
    $stdout = [Console]::OpenStandardOutput()
    $stdout.Write($bytes, 0, $bytes.Length)
    $stdout.Flush()
} else {
    [Console]::WriteLine($path)
}
