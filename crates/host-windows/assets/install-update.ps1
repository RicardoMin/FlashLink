# SPDX-License-Identifier: GPL-3.0-only
# Embedded in FlashLink. The parent supplies the digest from its verified release manifest.
$ErrorActionPreference = 'Stop'
$target = [IO.Path]::GetFullPath($env:FLASHLINK_UPDATE_TARGET)
$root = [IO.Path]::GetDirectoryName($target)
$marker = [IO.Path]::Combine($root, '.flashlink-update.pending')
$payload = $env:FLASHLINK_UPDATE_FILE
$recover = $env:FLASHLINK_UPDATE_RECOVER -eq '1'
$work = $null
$lock = $null
$archive = $null
$inputFile = $null
$child = $null
$ready = $null
$armed = $false
$committed = $false

function Assert-RegularPath([string]$path) {
    if (([IO.File]::Exists($path) -or [IO.Directory]::Exists($path)) -and
        (([IO.File]::GetAttributes($path) -band [IO.FileAttributes]::ReparsePoint) -ne 0)) {
        throw "更新路径中存在重解析点：$path"
    }
}

function Member([string]$base, [string]$name) {
    # Only release-owned paths can be installed, removed or restored from a journal.
    if ($name -cnotmatch '^(FlashLink\.exe|LICENSE|NOTICE\.txt|使用说明\.txt|package-files\.json|(licenses|Sunshine)/.+)$' -or
        $name -match '^Sunshine/config(/|$)' -or $name -match '[\\:<>|?*"\x00-\x1f]') {
        throw "无效的发行包路径：$name"
    }
    $path = $base
    foreach ($part in ($name -split '/')) {
        if (!$part -or $part.EndsWith('.') -or $part.EndsWith(' ') -or
            $part -match '^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])(\.|$)') { throw "无效的文件名：$part" }
        $path = [IO.Path]::Combine($path, $part)
        Assert-RegularPath $path
    }
    return $path
}

function Destination([string]$name) {
    if ($name -ceq 'FlashLink.exe') { Assert-RegularPath $target; return $target }
    return Member $root $name
}

function Ensure-Parent([string]$path) {
    [void][IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($path))
}

function Put-File([string]$from, [string]$to) {
    Ensure-Parent $to
    if ([IO.File]::Exists($to)) {
        [IO.File]::Replace($from, $to, [Management.Automation.Language.NullString]::Value)
    } else {
        [IO.File]::Move($from, $to)
    }
}

function Same-File([string]$first, [string]$second) {
    if (![IO.File]::Exists($second) -or
        [IO.FileInfo]::new($first).Length -ne [IO.FileInfo]::new($second).Length) { return $false }
    $sha = [Security.Cryptography.SHA256]::Create()
    try {
        $a = [IO.File]::OpenRead($first)
        try { $left = [BitConverter]::ToString($sha.ComputeHash($a)) } finally { $a.Dispose() }
        $b = [IO.File]::OpenRead($second)
        try { return $left -eq [BitConverter]::ToString($sha.ComputeHash($b)) } finally { $b.Dispose() }
    } finally { $sha.Dispose() }
}

function Restore-Previous {
    $plan = ConvertFrom-Json -InputObject ([IO.File]::ReadAllText([IO.Path]::Combine($work, 'plan.json')))
    foreach ($change in $plan) {
        $dest = Destination $change.name
        if ($change.existed) {
            $original = Member ([IO.Path]::Combine($work, 'old')) $change.name
            if (Same-File $original $dest) { continue }
            # Keep backups intact if recovery itself is interrupted.
            $copy = [IO.Path]::Combine($work, 'restore.tmp')
            [IO.File]::Copy($original, $copy, $true)
            Put-File $copy $dest
        } elseif ([IO.File]::Exists($dest)) {
            [IO.File]::Delete($dest)
        }
    }
    [IO.File]::Delete($marker)
}

function Start-HostApp([string]$token) {
    $start = [Diagnostics.ProcessStartInfo]::new($target, $env:FLASHLINK_UPDATE_ARGS)
    $start.UseShellExecute = $false
    $start.WorkingDirectory = [Environment]::CurrentDirectory
    foreach ($key in @($start.EnvironmentVariables.Keys)) {
        if ($key -like 'FLASHLINK_UPDATE_*') { $start.EnvironmentVariables.Remove($key) }
    }
    if ($token) { $start.EnvironmentVariables['FLASHLINK_UPDATE_BOOT'] = $token }
    return [Diagnostics.Process]::Start($start)
}

try {
    $parentProcess = Get-Process -Id ([int]$env:FLASHLINK_UPDATE_PARENT) -ErrorAction SilentlyContinue
    if ($parentProcess -and !$parentProcess.WaitForExit(60000)) { throw '原主机尚未退出。' }
    Assert-RegularPath $root
    Assert-RegularPath $marker
    $lockPath = [IO.Path]::Combine($root, 'install.lock')
    Assert-RegularPath $lockPath
    $lock = [IO.File]::Open($lockPath, 'OpenOrCreate', 'ReadWrite', 'None')
    if ($recover) {
        $workName = [IO.File]::ReadAllText($marker)
        if ($workName -cnotmatch '^\.flashlink-update-[a-f0-9]{32}$') { throw '更新恢复记录无效。' }
        $work = [IO.Path]::Combine($root, $workName)
        Assert-RegularPath $work
        Assert-RegularPath ([IO.Path]::Combine($work,'old'))
        Assert-RegularPath ([IO.Path]::Combine($work,'plan.json'))
        Restore-Previous
        $committed = $true
        [void](Start-HostApp '')
        return
    }
    if ([IO.File]::Exists($marker)) { throw '请重新打开 FlashLink，完成上次更新恢复。' }
    $token = [Guid]::NewGuid().ToString('N')
    $workName = ".flashlink-update-$token"
    $work = [IO.Path]::Combine($root, $workName)
    $fresh = [IO.Path]::Combine($work, 'new')
    $backup = [IO.Path]::Combine($work, 'old')
    [void][IO.Directory]::CreateDirectory($fresh)
    [void][IO.Directory]::CreateDirectory($backup)
    $files = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    # Hold one read-only shared handle from SHA verification through extraction.
    $inputFile = [IO.File]::Open($payload, 'Open', 'Read', 'Read')
    $sha = [Security.Cryptography.SHA256]::Create()
    try { $digest = [BitConverter]::ToString($sha.ComputeHash($inputFile)).Replace('-', '') }
    finally { $sha.Dispose() }
    if ($digest -ne $env:FLASHLINK_UPDATE_HASH -or $inputFile.Length -ne [long]$env:FLASHLINK_UPDATE_SIZE) {
        throw '更新文件的长度或 SHA-256 校验失败。'
    }
    $inputFile.Position = 0
    if ($env:FLASHLINK_UPDATE_SINGLE -eq '1') {
        $file = [IO.File]::Create((Member $fresh 'FlashLink.exe'))
        try { $inputFile.CopyTo($file) } finally { $file.Dispose() }
        [void]$files.Add('FlashLink.exe')
    } else {
        Add-Type -AssemblyName System.IO.Compression
        $archive = [IO.Compression.ZipArchive]::new($inputFile, [IO.Compression.ZipArchiveMode]::Read, $true)
        $expanded = 0L
        foreach ($entry in $archive.Entries) {
            $name = $entry.FullName
            $dest = Member $fresh $name
            $expanded += $entry.Length
            if ($expanded -gt 2GB -or !$files.Add($name)) { throw '便携包过大或包含重复文件。' }
            Ensure-Parent $dest
            $file = [IO.File]::Open($dest, 'CreateNew')
            try { $stream = $entry.Open(); try { $stream.CopyTo($file) } finally { $stream.Dispose() } }
            finally { $file.Dispose() }
        }
        foreach ($required in @('FlashLink.exe','Sunshine/sunshine.exe','package-files.json')) {
            if (!$files.Contains($required)) { throw "便携包缺少 $required" }
        }
    }
    $managed = [Collections.Generic.HashSet[string]]::new($files, [StringComparer]::OrdinalIgnoreCase)
    $inventory = Member $root 'package-files.json'
    if ($env:FLASHLINK_UPDATE_SINGLE -ne '1' -and [IO.File]::Exists($inventory)) {
        foreach ($item in (ConvertFrom-Json -InputObject ([IO.File]::ReadAllText($inventory)))) {
            [void](Member $root $item)
            [void]$managed.Add($item)
        }
    }
    $plan = foreach ($name in $managed) {
        $dest = Destination $name
        $exists = [IO.File]::Exists($dest)
        if ($exists) {
            $old = Member $backup $name
            Ensure-Parent $old
            [IO.File]::Copy($dest, $old)
        }
        [pscustomobject]@{name = $name; existed = $exists}
    }
    [IO.File]::WriteAllText([IO.Path]::Combine($work, 'plan.json'), (ConvertTo-Json -InputObject @($plan) -Compress))
    $pending = [IO.Path]::Combine($work, 'pending')
    [IO.File]::WriteAllText($pending, $workName)
    [IO.File]::Move($pending, $marker)
    $armed = $true
    foreach ($name in $managed) {
        $dest = Destination $name
        if ($files.Contains($name)) { Put-File (Member $fresh $name) $dest }
        elseif ([IO.File]::Exists($dest)) { [IO.File]::Delete($dest) }
    }
    $ready = [Threading.EventWaitHandle]::new($false, [Threading.EventResetMode]::ManualReset, "Local\FlashLinkUpdate-$token")
    $child = Start-HostApp $token
    $deadline = [Diagnostics.Stopwatch]::StartNew()
    while (!$ready.WaitOne(250)) {
        if ($child.HasExited -or $deadline.Elapsed.TotalSeconds -gt 100) { throw '新版主机未能启动，将恢复原版本。' }
    }
    [IO.File]::Delete($marker)
    $committed = $true
} catch {
    $detail = $_.Exception.Message
    try {
        if ($armed -and !$committed) {
            if ($child -and !$child.HasExited) { $child.Kill(); $child.WaitForExit() }
            Restore-Previous
        }
        if (!$recover -and $lock -and ![IO.File]::Exists($marker)) { [void](Start-HostApp '') }
    } catch { $detail += [Environment]::NewLine + $_.Exception.Message }
    Add-Type -AssemblyName System.Windows.Forms
    [void][Windows.Forms.MessageBox]::Show($detail, 'FlashLink 更新失败')
} finally {
    if ($archive) { $archive.Dispose() }
    if ($inputFile) { $inputFile.Dispose() }
    if ($ready) { $ready.Dispose() }
    if ($lock) { $lock.Dispose() }
    if (!$recover -and $payload) { try { [IO.File]::Delete($payload) } catch {} }
    if ($work -and ![IO.File]::Exists($marker)) {
        try { [IO.Directory]::Delete($work, $true) } catch {}
    }
}
