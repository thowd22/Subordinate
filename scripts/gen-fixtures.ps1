<#
.SYNOPSIS
    Generate the deterministic test media fixtures used by the Rust test suite.

.DESCRIPTION
    Windows equivalent of scripts/gen-fixtures.sh. Media binaries are never
    committed; every developer and CI runner synthesises an identical set with
    GStreamer instead. Output lands in fixtures/ (gitignored) together with a
    manifest.json describing every fixture.

    Requires gst-launch-1.0.exe on PATH (the official MSVC install puts it in
    <root>\bin; see docs/DEVELOPMENT.md). The lossy audio fixtures also want
    lamemp3enc, avenc_aac and vorbisenc; where one of those is absent that
    fixture is skipped rather than failing the run. Keep this script and
    scripts/gen-fixtures.sh in sync.

.PARAMETER OutDir
    Directory to write fixtures into. Defaults to <repo>\fixtures.

.PARAMETER Long
    Also generate the 10-minute long-GOP clip (slow; skipped in CI).

.PARAMETER Force
    Regenerate files that already exist.

.PARAMETER List
    List the fixture catalogue and exit.

.PARAMETER DryRun
    Print the pipelines instead of running them.
#>
[CmdletBinding()]
param(
    [string]$OutDir,
    [switch]$Long,
    [switch]$Force,
    [switch]$List,
    [switch]$DryRun
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $OutDir) { $OutDir = Join-Path $repoRoot 'fixtures' }

# ------------------------------------------------------------------ catalogue
#
# duration_ns is an exact integer count of nanoseconds and frame rates are exact
# rationals; nothing here is ever expressed as a float.
#
# lossy marks a fixture whose codec cannot reproduce its input samples. A lossy
# encoder also adds priming and padding frames, so duration_ns is the authored
# length, which the file itself only matches to within a few tens of
# milliseconds; tests give lossy fixtures a tolerance and keep the lossless
# ones exact.
#
# 29.97 drop-frame: 300 frames at 30000/1001 is 10_010_000_000 ns exactly.
# VFR: 90 frames at 30/1 (3 s) then 180 frames at 60/1 (3 s) = 6 s.
$catalogue = @(
    [pscustomobject]@{ name = 'bars_1080p_h264.mp4'; kind = 'video'; width = 1920; height = 1080
        duration_ns = 5000000000L; fps_num = 25; fps_den = 1; vfr = $false; lossy = $false; long = $false
        description = '1080p SMPTE colour bars, H.264, timecode burn-in'
    }
    [pscustomobject]@{ name = 'bars_2160p_h264.mp4'; kind = 'video'; width = 3840; height = 2160
        duration_ns = 5000000000L; fps_num = 25; fps_den = 1; vfr = $false; lossy = $false; long = $false
        description = '4K SMPTE colour bars, H.264, timecode burn-in'
    }
    [pscustomobject]@{ name = 'dropframe_2997_h264.mp4'; kind = 'video'; width = 1920; height = 1080
        duration_ns = 10010000000L; fps_num = 30000; fps_den = 1001; vfr = $false; lossy = $false; long = $false
        description = '29.97 drop-frame clip with drop-frame timecode burn-in'
    }
    [pscustomobject]@{ name = 'vfr_60_30.mkv'; kind = 'video'; width = 1280; height = 720
        duration_ns = 6000000000L; fps_num = 60; fps_den = 1; vfr = $true; lossy = $false; long = $false
        description = 'Variable-frame-rate clip: 3 s at 30 fps then 3 s at 60 fps'
    }
    [pscustomobject]@{ name = 'longgop_720p_10min.mp4'; kind = 'video'; width = 1280; height = 720
        duration_ns = 600000000000L; fps_num = 25; fps_den = 1; vfr = $false; lossy = $false; long = $true
        description = '10-minute long-GOP H.264 clip (250-frame GOP, B-frames)'
    }
    [pscustomobject]@{ name = 'tone_48k_stereo.wav'; kind = 'audio'; width = 0; height = 0
        duration_ns = 5000000000L; fps_num = 0; fps_den = 1; vfr = $false; lossy = $false; long = $false
        description = 'Audio only: 5 s 440 Hz sine, 48 kHz stereo, 16-bit WAV'
    }
    [pscustomobject]@{ name = 'tone_48k_stereo.flac'; kind = 'audio'; width = 0; height = 0
        duration_ns = 5000000000L; fps_num = 0; fps_den = 1; vfr = $false; lossy = $false; long = $false
        description = 'Audio only: 5 s 440 Hz sine, 48 kHz stereo, FLAC'
    }
    [pscustomobject]@{ name = 'tone_48k_stereo.mp3'; kind = 'audio'; width = 0; height = 0
        duration_ns = 5000000000L; fps_num = 0; fps_den = 1; vfr = $false; lossy = $true; long = $false
        description = 'Audio only: 5 s 440 Hz sine, 48 kHz stereo, MP3 at 192 kbit/s CBR'
    }
    [pscustomobject]@{ name = 'tone_48k_stereo.m4a'; kind = 'audio'; width = 0; height = 0
        duration_ns = 5000000000L; fps_num = 0; fps_den = 1; vfr = $false; lossy = $true; long = $false
        description = 'Audio only: 5 s 440 Hz sine, 48 kHz stereo, AAC-LC in MP4'
    }
    [pscustomobject]@{ name = 'tone_48k_stereo.ogg'; kind = 'audio'; width = 0; height = 0
        duration_ns = 5000000000L; fps_num = 0; fps_den = 1; vfr = $false; lossy = $true; long = $false
        description = 'Audio only: 5 s 440 Hz sine, 48 kHz stereo, Ogg Vorbis'
    }
)

if ($List) {
    $catalogue | Format-Table name, kind,
    @{ Label = 'ms'; Expression = { $_.duration_ns / 1000000 } }, vfr, lossy, long, description
    exit 0
}

# ------------------------------------------------------------------ preflight
function Invoke-Pipeline {
    param([string[]]$Arguments)
    if ($DryRun) {
        Write-Host "gst-launch-1.0 $($Arguments -join ' ')"
        return
    }
    & gst-launch-1.0 -q -e @Arguments
    if ($LASTEXITCODE -ne 0) { throw "gst-launch-1.0 failed with exit code $LASTEXITCODE" }
}

if (-not $DryRun) {
    if (-not (Get-Command gst-launch-1.0 -ErrorAction SilentlyContinue)) {
        throw 'gen-fixtures: gst-launch-1.0 not found on PATH; see docs/DEVELOPMENT.md'
    }
    $required = @('videotestsrc', 'audiotestsrc', 'timecodestamper', 'timeoverlay', 'capssetter',
        'concat', 'audioconvert', 'x264enc', 'h264parse', 'mp4mux', 'matroskamux', 'wavenc', 'flacenc')
    $missing = @($required | Where-Object { & gst-inspect-1.0 --exists $_; $LASTEXITCODE -ne 0 })
    if ($missing.Count -gt 0) {
        throw "gen-fixtures: missing GStreamer elements: $($missing -join ' ')"
    }
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

function Test-Needed {
    param([string]$Name)
    $target = Join-Path $OutDir $Name
    if ((-not $Force) -and (Test-Path $target) -and ((Get-Item $target).Length -gt 0)) {
        Write-Host "gen-fixtures: keeping existing $Name"
        return $false
    }
    Write-Host "gen-fixtures: generating $Name"
    return $true
}

# Colour bars with a burnt-in timecode. The GOP is one second of frames, so
# these clips are short-GOP and cheap to seek in.
function New-BarsFixture {
    param([string]$Name, [int]$Width, [int]$Height, [int]$Frames, [int]$FpsNum, [int]$FpsDen)
    $fontSize = [int]($Height / 16)
    $keyInt = [int]($FpsNum / $FpsDen)
    Invoke-Pipeline @(
        'videotestsrc', 'pattern=smpte', "num-buffers=$Frames",
        '!', "video/x-raw,format=I420,width=$Width,height=$Height,framerate=$FpsNum/$FpsDen",
        '!', 'timecodestamper', 'post-messages=false',
        '!', 'timeoverlay', 'time-mode=time-code', 'halignment=center', 'valignment=bottom',
        "font-desc=Monospace $fontSize",
        '!', 'x264enc', 'bitrate=12000', "key-int-max=$keyInt", 'speed-preset=veryfast',
        '!', 'h264parse', '!', 'mp4mux', '!', 'filesink', "location=$(Join-Path $OutDir $Name)"
    )
}

function New-VideoFixture {
    param([string]$Name)
    switch ($Name) {
        'bars_1080p_h264.mp4' { New-BarsFixture $Name 1920 1080 125 25 1 }
        'bars_2160p_h264.mp4' { New-BarsFixture $Name 3840 2160 125 25 1 }
        # timecodestamper flags the timecode drop-frame automatically for
        # 30000/1001, which is what makes this a genuine 29.97 DF fixture.
        'dropframe_2997_h264.mp4' { New-BarsFixture $Name 1920 1080 300 30000 1001 }
        # Two segments with genuinely different frame spacing are concatenated
        # and relabelled to a single caps, so the encoder sees one stream while
        # the muxer records buffer timestamps 33.3 ms apart for the first three
        # seconds and 16.6 ms apart for the last three. Matroska stores those
        # per-frame timestamps, giving a real variable-frame-rate file.
        'vfr_60_30.mkv' {
            Invoke-Pipeline @(
                'concat', 'name=c',
                '!', 'capssetter', 'replace=true',
                'caps=video/x-raw,format=I420,width=1280,height=720,framerate=60/1',
                '!', 'x264enc', 'bitrate=6000', 'key-int-max=60', 'speed-preset=veryfast',
                '!', 'h264parse', '!', 'matroskamux',
                '!', 'filesink', "location=$(Join-Path $OutDir $Name)",
                'videotestsrc', 'pattern=smpte', 'num-buffers=90',
                '!', 'video/x-raw,format=I420,width=1280,height=720,framerate=30/1', '!', 'c.',
                'videotestsrc', 'pattern=ball', 'num-buffers=180',
                '!', 'video/x-raw,format=I420,width=1280,height=720,framerate=60/1', '!', 'c.'
            )
        }
        'longgop_720p_10min.mp4' {
            Invoke-Pipeline @(
                'videotestsrc', 'pattern=ball', 'num-buffers=15000',
                '!', 'video/x-raw,format=I420,width=1280,height=720,framerate=25/1',
                '!', 'timecodestamper', 'post-messages=false',
                '!', 'timeoverlay', 'time-mode=time-code', 'halignment=center', 'valignment=bottom',
                'font-desc=Monospace 45',
                '!', 'x264enc', 'bitrate=4000', 'key-int-max=250', 'bframes=3', 'speed-preset=veryfast',
                '!', 'h264parse', '!', 'mp4mux', '!', 'filesink', "location=$(Join-Path $OutDir $Name)"
            )
        }
        default { throw "gen-fixtures: no pipeline for '$Name'" }
    }
}

# The encoder tail of an audio fixture's pipeline. The lossless formats need a
# single encoder; the lossy ones need an encoder and, for AAC, a parser and a
# container. '!' separates elements, as in a gst-launch description.
function Get-AudioEncoderChain {
    param([string]$Name)
    switch ([System.IO.Path]::GetExtension($Name)) {
        '.wav' { , @('wavenc') }
        '.flac' { , @('flacenc') }
        '.mp3' { , @('lamemp3enc', 'target=bitrate', 'bitrate=192', 'cbr=true') }
        '.m4a' { , @('avenc_aac', 'bitrate=192000', '!', 'aacparse', '!', 'mp4mux') }
        '.ogg' { , @('vorbisenc', 'quality=0.6', '!', 'oggmux') }
        default { throw "gen-fixtures: no pipeline for '$Name'" }
    }
}

# True when every element an audio fixture needs is installed. MP3, AAC and Ogg
# Vorbis encoders live in plugin sets a minimal install may not carry, so a
# missing one skips that fixture instead of failing the whole run.
function Test-AudioEncoderAvailable {
    param([string]$Name)
    if ($DryRun) { return $true }
    foreach ($word in (Get-AudioEncoderChain $Name)) {
        if ($word -eq '!' -or $word.Contains('=')) { continue }
        & gst-inspect-1.0 --exists $word | Out-Null
        if ($LASTEXITCODE -ne 0) { return $false }
    }
    return $true
}

# 5 s at 48 kHz with 4800 samples per buffer is exactly 50 buffers. Lossy
# encoders add priming and padding around those 240000 frames, so the file is
# a little longer than the catalogue's authored duration.
function New-AudioFixture {
    param([string]$Name)
    Invoke-Pipeline (@(
            'audiotestsrc', 'wave=sine', 'freq=440', 'samplesperbuffer=4800', 'num-buffers=50',
            '!', 'audio/x-raw,format=S16LE,rate=48000,channels=2',
            '!', 'audioconvert', '!'
        ) + (Get-AudioEncoderChain $Name) + @(
            '!', 'filesink', "location=$(Join-Path $OutDir $Name)"
        ))
}

# ------------------------------------------------------------------- generate
$entries = foreach ($f in $catalogue) {
    $generated = $true
    if ($f.long -and (-not $Long)) {
        Write-Host "gen-fixtures: skipping $($f.name) (pass -Long to generate it)"
        $generated = $false
    }
    elseif (($f.kind -eq 'audio') -and (-not (Test-AudioEncoderAvailable $f.name))) {
        Write-Host "gen-fixtures: skipping $($f.name) (its encoder is not installed)"
        $generated = $false
    }
    elseif (Test-Needed $f.name) {
        if ($f.kind -eq 'video') { New-VideoFixture $f.name } else { New-AudioFixture $f.name }
    }
    $target = Join-Path $OutDir $f.name
    if ((-not $DryRun) -and $generated -and (-not (Test-Path $target))) {
        throw "gen-fixtures: $($f.name) was not written"
    }
    [pscustomobject]@{
        name        = $f.name
        kind        = $f.kind
        width       = $f.width
        height      = $f.height
        duration_ns = $f.duration_ns
        fps_num     = $f.fps_num
        fps_den     = $f.fps_den
        vfr         = $f.vfr
        lossy       = $f.lossy
        generated   = $generated
        description = $f.description
    }
}

if ($DryRun) {
    Write-Host 'gen-fixtures: dry run, manifest not written'
    exit 0
}

$manifestPath = Join-Path $OutDir 'manifest.json'
[pscustomobject]@{
    version   = 1
    generator = 'scripts/gen-fixtures.ps1'
    fixtures  = @($entries)
} | ConvertTo-Json -Depth 4 | Set-Content -Path $manifestPath -Encoding utf8
Write-Host "gen-fixtures: wrote $manifestPath"
