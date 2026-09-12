<#
.SYNOPSIS
    Verify an installed Subordinate MSI on a machine that has never built the
    project (TASK-110).

.DESCRIPTION
    The Windows half of scripts/fresh-install-check.sh. Nothing here compiles
    and nothing here reaches into a source tree: every command runs out of
    %ProgramFiles%\Subordinate, and the only inputs are the sample project and
    its media.

    The first thing it does is replace PATH with the installed bin directory
    and Windows itself. A package that only works because the machine that
    built it still has C:\gstreamer on PATH is the failure this script exists
    to catch, so the search path is rebuilt rather than trusted, and the
    GStreamer environment variables the official installer sets are cleared.

    Output: a log on stdout and <Out>\facts.txt, a key=value record of the
    machine, the package and the result.

.PARAMETER Bin
    The installed bin directory. Defaults to %ProgramFiles%\Subordinate\bin.

.PARAMETER Project
    The sample project (.sub) to open, with its media beside it.

.PARAMETER Out
    Where to write facts.txt and the render.

.PARAMETER Preset
    Export preset. Defaults to mezzanine: H.264 in MKV with FLAC audio, so no
    machine is failed for want of an AAC encoder.

.PARAMETER Sequence
    Which sequence to render. demo.sub carries two, so one has to be named.

.PARAMETER Range
    Frames of the sequence timebase, the out point exclusive.

.PARAMETER NoGui
    Skip opening the editor window.

.PARAMETER Strict
    Fail if this machine has GStreamer of its own. For a genuinely clean
    machine, not for a developer box or the user's own desktop.
#>
[CmdletBinding()]
param(
    [string]$Bin = "$env:ProgramFiles\Subordinate\bin",
    [Parameter(Mandatory = $true)][string]$Project,
    [string]$Out,
    [string]$Preset = 'mezzanine',
    [string]$Sequence = 'Main cut',
    [string]$Range = '0:25',
    [switch]$NoGui,
    [switch]$Strict
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if (-not $Out) { $Out = Join-Path (Get-Location) 'fresh-install' }
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$factsPath = Join-Path $Out 'facts.txt'
Set-Content -Path $factsPath -Value @() -Encoding utf8

function Add-Fact {
    param([string]$Key, [string]$Value)
    "$Key=$Value" | Tee-Object -FilePath $factsPath -Append | Write-Host
}

function Write-Banner {
    param([string]$Text)
    Write-Host ''
    Write-Host '=============================================================='
    Write-Host "== $Text"
    Write-Host '=============================================================='
}

function Stop-WithFailure {
    param([string]$Message, [string]$Stage)
    Write-Host "::error::$Message"
    Add-Fact result fail
    Add-Fact failed_at $Stage
    exit 1
}

# ------------------------------------------------------------- the machine --
Write-Banner 'the machine'
$os = Get-CimInstance Win32_OperatingSystem
Add-Fact os "$($os.Caption) $($os.Version)"
Add-Fact kernel "build $($os.BuildNumber)"
Add-Fact arch $env:PROCESSOR_ARCHITECTURE
$adapters = @(Get-CimInstance Win32_VideoController | ForEach-Object { $_.Name })
if ($adapters.Count -gt 0) { Add-Fact gpu ($adapters -join ', ') } else { Add-Fact gpu 'none reported' }

# Before PATH is rebuilt: what this machine had of its own.
foreach ($tool in 'gst-inspect-1.0.exe', 'gst-launch-1.0.exe', 'cargo.exe') {
    $found = Get-Command $tool -ErrorAction SilentlyContinue
    if ($found) {
        if ($Strict -and $tool -like 'gst-*') {
            Stop-WithFailure "this machine already has $tool ($($found.Source)); it is not a fresh machine" machine
        }
        Write-Host "note: this machine has its own $tool ($($found.Source)); the package is used exclusively anyway"
    } else {
        Write-Host "absent, as a fresh machine should be: $tool"
    }
}
if ($Strict -and (Test-Path 'C:\gstreamer')) {
    Stop-WithFailure 'C:\gstreamer exists on this machine; it is not a fresh machine' machine
}

# From here on: the installed package and Windows, and nothing else. The
# registry cache an earlier GStreamer may have written under the user profile
# goes too, so the bundled plugins are found by the install layout alone.
Remove-Item -Recurse -Force "$env:USERPROFILE\.cache\gstreamer-1.0" -ErrorAction SilentlyContinue
foreach ($name in 'GSTREAMER_1_0_ROOT_MSVC_X86_64', 'GST_PLUGIN_PATH', 'GST_PLUGIN_SYSTEM_PATH', 'PKG_CONFIG_PATH') {
    Remove-Item "Env:$name" -ErrorAction SilentlyContinue
}
$env:PATH = "$Bin;$env:SystemRoot\system32;$env:SystemRoot;$env:SystemRoot\System32\Wbem"
Write-Host "PATH is now: $env:PATH"

$editor = Join-Path $Bin 'subordinate.exe'
$cli = Join-Path $Bin 'subordinate-cli.exe'
# The MCP bridge is installed by the MSI beside the editor (TASK-142); an agent
# on a machine that only ever ran the installer has no other way to get one.
$mcp = Join-Path $Bin 'subordinate-mcp.exe'
$inspect = Join-Path $Bin 'gst-inspect-1.0.exe'
$discoverer = Join-Path $Bin 'gst-discoverer-1.0.exe'
foreach ($exe in $editor, $cli, $mcp, $inspect, $discoverer) {
    if (-not (Test-Path $exe)) { Stop-WithFailure "not installed: $exe" install }
}
Add-Fact package "MSI installed at $Bin"

# ------------------------------------------------------- the package starts --
Write-Banner 'the installed package starts'
& $editor --help | Out-File -Encoding utf8 (Join-Path $Out 'help.txt')
if ($LASTEXITCODE -ne 0) { Stop-WithFailure "the editor would not start (exit $LASTEXITCODE)" start }
$version = (& $inspect --version 2>&1 | Out-String)
if ($LASTEXITCODE -ne 0) { Stop-WithFailure 'the bundled gst-inspect-1.0 would not run' start }
Write-Host $version
$line = ($version -split "`r?`n" | Where-Object { $_ -match '^GStreamer\s' } | Select-Object -First 1)
Add-Fact gstreamer ("$line" -replace '^GStreamer\s+', '')

# The MSI ships gst-discoverer-1.0.exe, but not every package does: the
# AppImage did not, because on Ubuntu that command lives in
# gstreamer1.0-plugins-base-apps, which the build container did not install
# (found by this check, run 34641628065). Where the command is there it reads
# the export back from outside, which is an independent look at the file;
# where it is not, the render's own --verify probe -- the same discoverer,
# from the same bundle, in process -- is what the result rests on. Either way
# the check says which.
& $discoverer --help *> $null
$hasDiscoverer = ($LASTEXITCODE -eq 0)
if ($hasDiscoverer) { Add-Fact discoverer 'bundled as a command' }
else { Add-Fact discoverer 'library only; --verify probes in process' }

# A blacklisted plugin is a DLL whose dependencies did not come along.
$summary = (& $inspect 2>$null | Select-Object -Last 1)
Write-Host $summary
if ($summary -match 'blacklist') {
    Stop-WithFailure "the installed runtime blacklisted some of its own plugins: $summary" plugins
}

# ------------------------------------------------------- open the project ----
Write-Banner 'open the sample project'
$openPath = Join-Path $Out 'open.json'
& $cli open $Project | Out-File -Encoding utf8 $openPath
if ($LASTEXITCODE -ne 0) { Stop-WithFailure 'the project would not open' open }
$open = Get-Content -Raw $openPath
Write-Host $open
# `offline` lists every media file the project refers to that could not be
# found, or whose bytes are not the ones it was authored against. A relink
# prompt on a fresh machine is the classic packaging failure.
if ($open -notmatch '"offline":\s*\[\s*\]') {
    Stop-WithFailure 'the project opened with offline media; see open.json' open
}
Write-Host 'every media path resolved: no relink needed'

& $cli inspect $Project | Out-File -Encoding utf8 (Join-Path $Out 'inspect.json')
if ($LASTEXITCODE -ne 0) { Stop-WithFailure 'the project would not inspect' open }

# ------------------------------------------------------------ probe media ----
Write-Banner 'probe the media with the bundled discoverer'
$mediaDir = Join-Path (Split-Path -Parent $Project) 'media'
$clips = @(Get-ChildItem -Path $mediaDir -Filter *.webm -ErrorAction SilentlyContinue)
if ($clips.Count -eq 0) { Stop-WithFailure "no sample media beside $Project" probe }
$sawAudio = $false
foreach ($clip in $clips) {
    if (-not $hasDiscoverer) { $sawAudio = $true; continue }
    $report = (& $discoverer $clip.FullName 2>&1 | Out-String)
    if ($report -match 'ERROR') { Stop-WithFailure "the bundled discoverer reported an error on $($clip.Name)" probe }
    Write-Host "--- $($clip.Name)"
    Write-Host (($report -split "`r?`n" | Select-Object -First 14) -join "`n")
    if ($report -match 'audio') { $sawAudio = $true }
}
Add-Fact media_probed $clips.Count
if (-not $sawAudio) { Stop-WithFailure 'no audio stream in any sample clip' probe }
Write-Host 'audio streams present and readable'

# --------------------------------------------------------------- export -----
Write-Banner 'export with the best available encoder'
# sub_export's own selection order (crates/sub-export/src/encoder.rs): hardware
# first, software last. On a hosted runner only x264enc and mfh264enc survive,
# and mfh264enc without a GPU behind it is a software Media Foundation
# transform, so the render falls through to whichever one can actually encode.
$available = @()
foreach ($element in 'nvh264enc', 'amfh264enc', 'mfh264enc', 'x264enc') {
    & $inspect --exists $element
    if ($LASTEXITCODE -eq 0) { $available += $element; Write-Host "encoder present: $element" }
    else { Write-Host "encoder absent:  $element" }
}
Add-Fact encoders_available ($available -join ' ')
if ($available.Count -eq 0) { Stop-WithFailure 'the installed runtime offers no H.264 encoder at all' export }

$extension = if ($Preset -in 'mezzanine', 'h265-archive') { 'mkv' } else { 'mp4' }
$output = Join-Path $Out "fresh-install.$extension"
$used = ''
foreach ($element in $available) {
    Write-Host "--- rendering with $element"
    & $cli render $Project --sequence $Sequence --preset $Preset --encoder $element `
        --range $Range --out $output --verify 2>&1 |
        Tee-Object -FilePath (Join-Path $Out 'render.log') | Write-Host
    if ($LASTEXITCODE -eq 0) { $used = $element; break }
    # Present in the registry is not the same as usable: nvcodec and amfcodec
    # register elements that need a driver behind them, so falling through to
    # the next candidate is what "best *available*" means.
    Write-Host "$element could not render on this machine; trying the next encoder"
}
if (-not $used) { Stop-WithFailure 'no available encoder could render the sample project' export }
Add-Fact encoder_used $used

# --------------------------------------------------------------- validate ---
Write-Banner 'validate the export with the bundled discoverer'
if (-not (Test-Path $output)) { Stop-WithFailure 'the render reported success but wrote no file' validate }
$bytes = (Get-Item $output).Length
Add-Fact output_bytes $bytes
Write-Host "$output is $bytes bytes"
if ($bytes -lt 10240) { Stop-WithFailure "the rendered file is implausibly small ($bytes bytes)" validate }
# --verify made the package read its own output back before it exited, and it
# refuses a file with no video; that report is in the render log either way.
$renderLog = Get-Content -Raw (Join-Path $Out 'render.log')
if ($renderLog -notmatch '"probe"') {
    Stop-WithFailure 'the render did not probe what it wrote (--verify produced no report)' validate
}
Write-Host 'the package read its own export back before it exited'

if ($hasDiscoverer) {
    $report = (& $discoverer $output 2>&1 | Out-String)
    Write-Host $report
    if ($report -match 'ERROR') { Stop-WithFailure 'the discoverer reported an error on the export' validate }
    if ($report -notmatch 'video') { Stop-WithFailure 'the export carries no video stream' validate }
    if ($report -notmatch 'audio') { Stop-WithFailure 'the export carries no audio stream' validate }
    Write-Host 'and the bundled gst-discoverer-1.0 reads it back from outside: video and audio'
}

# ------------------------------------------------------------------- MCP ----
# The Windows half of the MCP stage in fresh-install-check.sh: the installed
# bridge, spoken to over its stdin and stdout, one JSON-RPC message per line.
# No editor is running, so the bridge starts the subordinate-cli.exe it finds
# beside itself -- which is what proves the MSI ships a bridge and a CLI that
# can find each other. A scratch SUBORDINATE_INSTANCE keeps the endpoint off
# the default one a user's editor would own.
Write-Banner 'drive the package from an agent (MCP)'
$mcpIn = Join-Path $Out 'mcp-in.jsonl'
$mcpOut = Join-Path $Out 'mcp-out.jsonl'
$mcpErr = Join-Path $Out 'mcp-err.txt'
# project.new leaves a project with no sequences, and timeline.get_state
# answers a domain error on one of those, so the round trip creates a sequence
# in between: a mutation goes in and the timeline that comes back is the one it
# produced.
$settings = '{"resolution":{"width":1920,"height":1080},"frame_rate":{"numerator":30,"denominator":1},"sample_rate":48000,"color":{"space":"rec709","transfer":"bt709","primaries":"bt709"}}'
$requests = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fresh-install-check","version":"1"}}}'
    '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"project_new","arguments":{"name":"Fresh install"}}}'
    '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"sequence_create","arguments":{"name":"Installed by MCP","settings":' + $settings + '}}}'
    '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"timeline_get_state","arguments":{}}}'
)
Set-Content -Encoding ASCII -Path $mcpIn -Value ($requests -join "`r`n")
$env:SUBORDINATE_INSTANCE = 'fresh-install-check'
$env:SUBORDINATE_LOG = 'info'
# Piping through cmd keeps stdin, stdout and stderr straight without
# Start-Process redirection games; the bridge exits when stdin reaches EOF.
cmd /c "type `"$mcpIn`" | `"$mcp`" > `"$mcpOut`" 2> `"$mcpErr`""
$mcpExit = $LASTEXITCODE
Write-Host '--- subordinate-mcp stderr (last 20 lines)'
Get-Content $mcpErr -Tail 20 -ErrorAction SilentlyContinue | Write-Host
Write-Host '--- subordinate-mcp stdout'
$lines = @(Get-Content $mcpOut -ErrorAction SilentlyContinue)
foreach ($line in $lines) {
    if ($line.Length -gt 400) { Write-Host ($line.Substring(0, 400) + ' ...') } else { Write-Host $line }
}
if ($mcpExit -ne 0) { Stop-WithFailure "the installed MCP bridge exited $mcpExit" mcp }

# One reply per request id, each a result rather than an error. `isError` is
# how MCP reports a tool that ran and refused, which a bare exit status does
# not show.
$seen = @{}
foreach ($line in $lines) {
    if (-not $line.Trim()) { continue }
    $message = $line | ConvertFrom-Json
    # Set-StrictMode makes a missing property an error, and a notification has
    # no id at all, so ask before reading.
    if ($message.PSObject.Properties.Name -contains 'id' -and $null -ne $message.id) {
        $seen[[int]$message.id] = $message
    }
}
$named = @{ 1 = 'initialize'; 2 = 'project.new'; 3 = 'sequence.create'; 4 = 'timeline.get_state' }
foreach ($id in 1, 2, 3, 4) {
    if (-not $seen.ContainsKey($id)) {
        Stop-WithFailure "the bridge never answered $($named[$id]) (request $id)" mcp
    }
    if ($seen[$id].PSObject.Properties.Name -contains 'error') {
        Stop-WithFailure "$($named[$id]) failed: $($seen[$id].error | ConvertTo-Json -Compress)" mcp
    }
    if ($seen[$id].PSObject.Properties.Name -notcontains 'result') {
        Stop-WithFailure "$($named[$id]) answered neither a result nor an error" mcp
    }
    if ($seen[$id].result.PSObject.Properties.Name -contains 'isError' -and $seen[$id].result.isError) {
        Stop-WithFailure "$($named[$id]) answered a tool error: $($seen[$id].result | ConvertTo-Json -Depth 6 -Compress)" mcp
    }
    Write-Host "$($named[$id]) round-tripped"
}
# The timeline that came back has to be the one the mutation made, not an empty
# answer that happens not to be an error.
if (-not (Select-String -Path $mcpOut -Pattern 'Installed by MCP' -Quiet)) {
    Stop-WithFailure 'timeline.get_state did not return the sequence sequence.create had just made' mcp
}
Add-Fact mcp 'round-trip ok (project.new, sequence.create, timeline.get_state)'
Write-Host "the installed package's own MCP bridge drove the installed package's own engine"

# ------------------------------------------------------------------ the UI --
if ($NoGui) {
    Add-Fact gui 'not requested'
} else {
    Write-Banner 'open the project in the editor'
    $uiLog = Join-Path $Out 'ui.log'
    & $editor --ui-smoke $Project --hold-seconds 5 2>&1 | Tee-Object -FilePath $uiLog | Write-Host
    if ($LASTEXITCODE -ne 0) { Stop-WithFailure "the editor exited $LASTEXITCODE opening the project" gui }
    if (-not (Select-String -Path $uiLog -Pattern 'ui-smoke ready' -Quiet)) {
        Stop-WithFailure 'the editor never reported its windows ready' gui
    }
    Add-Fact gui ok
    Write-Host 'the editor opened the project, painted and popped the viewer out'
}

Add-Fact result pass
Write-Banner "PASS -- the MSI installed and ran on $($os.Caption)"
