<#
.SYNOPSIS
    Fetch the CC0 media the sample project in examples/sample-project plays.

.DESCRIPTION
    Windows equivalent of scripts/get-sample-media.sh. The media is never
    committed: every developer and CI runner downloads the same three files
    instead, each pinned by byte size and SHA-256, so a file that changed fails
    here rather than quietly changing what the sample project renders. Output
    lands in examples\sample-project\media (gitignored) beside a manifest.json
    listing every file, its licence and where it came from.

    The files are served from this project's own GitHub release
    (https://github.com/thowd22/Subordinate/releases/tag/sample-media-v1), not
    from Wikimedia Commons where they were originally published: Wikimedia
    rate-limits bursts from shared CI egress with HTTP 429, and a third-party
    site is a single point of failure for every CI run. The Commons page and
    the original upload URL stay in the catalogue and in the manifest for
    provenance, and -Upstream fetches from them, but nothing in CI does.

    Everything in the catalogue is CC0 1.0 (public domain dedication): no
    attribution is required, and examples/sample-project/README.md credits each
    author anyway. Keep this script and scripts/get-sample-media.sh in sync.

.PARAMETER OutDir
    Directory to write media into. Defaults to <repo>\examples\sample-project\media.

.PARAMETER Force
    Re-download files that already exist.

.PARAMETER List
    List the media catalogue and exit.

.PARAMETER DryRun
    Print what would be downloaded instead of downloading it.

.PARAMETER Upstream
    Download from the original Wikimedia Commons URLs instead of this project's
    release assets. Never used by CI.
#>
[CmdletBinding()]
param(
    [string]$OutDir,
    [switch]$Force,
    [switch]$List,
    [switch]$DryRun,
    [switch]$Upstream
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $OutDir) { $OutDir = Join-Path $repoRoot 'examples\sample-project\media' }

# ------------------------------------------------------------------ catalogue
#
# `name` is the file name the sample project refers to, which is a slug rather
# than the Commons file name: the project file has to name it on Windows too,
# and the originals carry accents, spaces and parentheses. It is also the name
# of the release asset, so the download URL is $releaseBase/$name.
#
# `origin` is the upload.wikimedia.org content URL the file was taken from and
# `source` its Commons description page. Both are provenance only: they are
# recorded in the manifest and are contacted only under -Upstream.
$licence = 'CC0-1.0'
$licenceUrl = 'https://creativecommons.org/publicdomain/zero/1.0/'

# Where the files are served from. The tag is immutable, so the asset URLs are
# stable for the life of the release; the SHA-256 pins the bytes.
$releaseTag = 'sample-media-v1'
$releaseBase = "https://github.com/thowd22/Subordinate/releases/download/$releaseTag"
# Published next to the assets: one "<sha256>  <name>" line per file.
$checksumsUrl = "$releaseBase/SHA256SUMS"

$catalogue = @(
    [pscustomobject]@{
        name   = 'porters-paris-1921.webm'
        bytes  = 930338L
        sha256 = 'a46b99a8d0de346fa999524c5462d64dec2cb3f3f2122b96f1190cfad2c2f02d'
        origin = 'https://upload.wikimedia.org/wikipedia/commons/7/7d/Ancienne_et_nouvelle_tenue_des_porteurs_des_Pompes_Fun%C3%A8bres_de_la_Ville_de_Paris_-_AI49294.webm'
        title  = 'Ancienne et nouvelle tenue des porteurs des Pompes Funebres de la Ville de Paris'
        author = 'Le Saint Lucien'
        source = 'https://commons.wikimedia.org/wiki/File:Ancienne_et_nouvelle_tenue_des_porteurs_des_Pompes_Fun%C3%A8bres_de_la_Ville_de_Paris_-_AI49294.webm'
    }
    [pscustomobject]@{
        name   = 'crowned-pigeon.webm'
        bytes  = 3595734L
        sha256 = '69ecc307be7b24521596ef489a952dca4145cf20f291f7b37a68c00ae10deabf'
        origin = 'https://upload.wikimedia.org/wikipedia/commons/0/00/Victorian_Crowned_Pigeon_fighting.webm'
        title  = 'Victorian Crowned Pigeon fighting'
        author = 'Designism'
        source = 'https://commons.wikimedia.org/wiki/File:Victorian_Crowned_Pigeon_fighting.webm'
    }
    [pscustomobject]@{
        name   = 'soneros-en-xalapa.webm'
        bytes  = 176564L
        sha256 = '0545e135408bd2e73a11db1e54d203fdab2432401584cdc4460052a6f352d3ec'
        origin = 'https://upload.wikimedia.org/wikipedia/commons/b/b3/Soneros_en_Xalapa.webm'
        title  = 'Soneros en Xalapa'
        author = 'Koffermejia'
        source = 'https://commons.wikimedia.org/wiki/File:Soneros_en_Xalapa.webm'
    }
)

# The URL a file is fetched from, honouring -Upstream.
function Get-DownloadUrl {
    param([pscustomobject]$Media)
    if ($Upstream) { $Media.origin } else { "$releaseBase/$($Media.name)" }
}

if ($List) {
    $catalogue | Format-Table name, bytes, @{ Label = 'licence'; Expression = { $licence } }, title
    exit 0
}

# ------------------------------------------------------------------ downloads
# GitHub and Wikimedia both ask automated clients to identify themselves.
$userAgent = "Subordinate-sample-media/2.0 (scripts/get-sample-media.ps1; a video editor's sample project)"

function Get-Sha256 {
    param([string]$Path)
    (Get-FileHash -Path $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

# Fetches $Url to $Target, retrying the whole transfer with a growing delay. A
# hosted runner can still meet a transient 5xx or a reset connection, and the
# built-in retry backs off too fast to clear one. Keep in sync with the shell
# script.
#
# The retry is this loop's and nothing else's: `-MaximumRetryCount` and
# `-RetryIntervalSec` arrived with PowerShell 7, and Windows PowerShell 5.1 -
# which is what `shell: powershell` is, and what a self-hosted Windows runner
# without pwsh has - refuses the whole call with "a parameter cannot be found
# that matches parameter name 'MaximumRetryCount'". That turned every attempt
# into an immediate failure and the script gave up after five of them
# (TASK-143, yodaddy, run 34679496173).
function Save-Url {
    param([string]$Target, [string]$Url)
    $delay = 5
    for ($attempt = 1; $attempt -le 5; $attempt++) {
        try {
            Invoke-WebRequest -Uri $Url -OutFile $Target -UserAgent $userAgent
            return
        } catch {
            if (Test-Path $Target) { Remove-Item -Force $Target }
            if ($attempt -eq 5) {
                throw "get-sample-media: giving up on $Url after 5 attempts: $_"
            }
            Write-Host "get-sample-media: download of $Url failed, retrying in ${delay}s"
            Start-Sleep -Seconds $delay
            $delay *= 2
        }
    }
}

# Downloads the release SHA256SUMS once and checks it against the pins in this
# script. It is a cross-check on the release, not the authority: the pins here
# are what a file is accepted against, so a tampered or regenerated checksum
# file fails the fetch instead of widening what is accepted. Skipped under
# -Upstream, where the release is not the source.
$script:ChecksumsChecked = $false
function Test-ReleaseChecksums {
    if ($script:ChecksumsChecked) { return }
    $script:ChecksumsChecked = $true
    if ($Upstream) { return }
    $sums = Join-Path ([System.IO.Path]::GetTempPath()) ([System.IO.Path]::GetRandomFileName())
    try {
        Save-Url -Target $sums -Url $checksumsUrl
        $published = @{}
        foreach ($line in Get-Content -Path $sums) {
            $fields = $line -split '\s+', 2
            if ($fields.Count -eq 2) {
                $published[$fields[1].Trim().TrimStart('*')] = $fields[0].Trim().ToLowerInvariant()
            }
        }
        foreach ($m in $catalogue) {
            if (-not $published.ContainsKey($m.name)) {
                throw "get-sample-media: $($m.name) is not listed in $checksumsUrl"
            }
            if ($published[$m.name] -ne $m.sha256) {
                throw ("get-sample-media: release lists $($m.name) as $($published[$m.name]), " +
                    "this script pins $($m.sha256); the release is not the one this project " +
                    'was built against')
            }
        }
    } finally {
        if (Test-Path $sums) { Remove-Item -Force $sums }
    }
}

# Downloads $Url to $Target and checks it against $Sha and $Bytes. A file that
# fails either check is deleted, so a rerun cannot mistake it for good.
function Save-Media {
    param([string]$Target, [string]$Url, [string]$Sha, [long]$Bytes)
    $part = "$Target.part"
    Save-Url -Target $part -Url $Url
    $got = (Get-Item $part).Length
    if ($got -ne $Bytes) {
        Remove-Item -Force $part
        throw "get-sample-media: $(Split-Path -Leaf $Target) is $got bytes, expected $Bytes"
    }
    $gotSha = Get-Sha256 $part
    if ($gotSha -ne $Sha) {
        Remove-Item -Force $part
        throw ("get-sample-media: $(Split-Path -Leaf $Target) hashes to $gotSha, expected $Sha; " +
            'the file served is not the one this project was built against')
    }
    Move-Item -Force $part $Target
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$entries = foreach ($m in $catalogue) {
    $target = Join-Path $OutDir $m.name
    $url = Get-DownloadUrl -Media $m
    if ($DryRun) {
        Write-Host "get-sample-media: would download $($m.name) from $url"
    }
    elseif ((-not $Force) -and (Test-Path $target) -and ((Get-Sha256 $target) -eq $m.sha256)) {
        # A warm cache reaches nothing over the network at all: the checksum
        # file is only fetched when a file actually has to be downloaded.
        Write-Host "get-sample-media: keeping existing $($m.name)"
    }
    else {
        Test-ReleaseChecksums
        Write-Host "get-sample-media: downloading $($m.name) ($($m.bytes) bytes) from $url"
        Save-Media -Target $target -Url $url -Sha $m.sha256 -Bytes $m.bytes
    }
    [pscustomobject]@{
        name        = $m.name
        bytes       = $m.bytes
        sha256      = $m.sha256
        url         = $url
        origin      = $m.origin
        title       = $m.title
        author      = $m.author
        source      = $m.source
        licence     = $licence
        licence_url = $licenceUrl
    }
}

if ($DryRun) {
    Write-Host 'get-sample-media: dry run, manifest not written'
    exit 0
}

$manifestPath = Join-Path $OutDir 'manifest.json'
[pscustomobject]@{
    version   = 1
    generator = 'scripts/get-sample-media.ps1'
    release   = $releaseTag
    media     = @($entries)
} | ConvertTo-Json -Depth 4 | Set-Content -Path $manifestPath -Encoding utf8
Write-Host "get-sample-media: wrote $manifestPath"
