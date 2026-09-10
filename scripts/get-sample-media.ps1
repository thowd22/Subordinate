<#
.SYNOPSIS
    Fetch the CC0 media the sample project in examples/sample-project plays.

.DESCRIPTION
    Windows equivalent of scripts/get-sample-media.sh. The media is never
    committed: every developer and CI runner downloads the same three files
    from Wikimedia Commons instead, each pinned by URL, byte size and SHA-256,
    so a file that changed upstream fails here rather than quietly changing
    what the sample project renders. Output lands in
    examples\sample-project\media (gitignored) beside a manifest.json listing
    every file, its licence and where it came from.

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
#>
[CmdletBinding()]
param(
    [string]$OutDir,
    [switch]$Force,
    [switch]$List,
    [switch]$DryRun
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $OutDir) { $OutDir = Join-Path $repoRoot 'examples\sample-project\media' }

# ------------------------------------------------------------------ catalogue
#
# `name` is the file name the sample project refers to, which is a slug rather
# than the Commons file name: the project file has to name it on Windows too,
# and the originals carry accents, spaces and parentheses.
#
# The URLs are upload.wikimedia.org content URLs, which are stable for the life
# of a revision; the SHA-256 pins the revision itself.
$licence = 'CC0-1.0'
$licenceUrl = 'https://creativecommons.org/publicdomain/zero/1.0/'

$catalogue = @(
    [pscustomobject]@{
        name   = 'porters-paris-1921.webm'
        bytes  = 930338L
        sha256 = 'a46b99a8d0de346fa999524c5462d64dec2cb3f3f2122b96f1190cfad2c2f02d'
        url    = 'https://upload.wikimedia.org/wikipedia/commons/7/7d/Ancienne_et_nouvelle_tenue_des_porteurs_des_Pompes_Fun%C3%A8bres_de_la_Ville_de_Paris_-_AI49294.webm'
        title  = 'Ancienne et nouvelle tenue des porteurs des Pompes Funebres de la Ville de Paris'
        author = 'Le Saint Lucien'
        source = 'https://commons.wikimedia.org/wiki/File:Ancienne_et_nouvelle_tenue_des_porteurs_des_Pompes_Fun%C3%A8bres_de_la_Ville_de_Paris_-_AI49294.webm'
    }
    [pscustomobject]@{
        name   = 'crowned-pigeon.webm'
        bytes  = 3595734L
        sha256 = '69ecc307be7b24521596ef489a952dca4145cf20f291f7b37a68c00ae10deabf'
        url    = 'https://upload.wikimedia.org/wikipedia/commons/0/00/Victorian_Crowned_Pigeon_fighting.webm'
        title  = 'Victorian Crowned Pigeon fighting'
        author = 'Designism'
        source = 'https://commons.wikimedia.org/wiki/File:Victorian_Crowned_Pigeon_fighting.webm'
    }
    [pscustomobject]@{
        name   = 'soneros-en-xalapa.webm'
        bytes  = 176564L
        sha256 = '0545e135408bd2e73a11db1e54d203fdab2432401584cdc4460052a6f352d3ec'
        url    = 'https://upload.wikimedia.org/wikipedia/commons/b/b3/Soneros_en_Xalapa.webm'
        title  = 'Soneros en Xalapa'
        author = 'Koffermejia'
        source = 'https://commons.wikimedia.org/wiki/File:Soneros_en_Xalapa.webm'
    }
)

if ($List) {
    $catalogue | Format-Table name, bytes, @{ Label = 'licence'; Expression = { $licence } }, title
    exit 0
}

# ------------------------------------------------------------------ downloads
# Wikimedia asks automated clients to identify themselves.
$userAgent = "Subordinate-sample-media/1.0 (scripts/get-sample-media.ps1; a video editor's sample project)"

function Get-Sha256 {
    param([string]$Path)
    (Get-FileHash -Path $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

# Downloads $Url to $Target and checks it against $Sha and $Bytes. A file that
# fails either check is deleted, so a rerun cannot mistake it for good.
function Save-Media {
    param([string]$Target, [string]$Url, [string]$Sha, [long]$Bytes)
    $part = "$Target.part"
    # Wikimedia rate-limits bursts from shared CI egress IPs with HTTP 429, and
    # the built-in retry backs off far too fast to clear one. Retry the whole
    # transfer with a growing delay instead. Keep in sync with the shell script.
    $delay = 5
    for ($attempt = 1; $attempt -le 5; $attempt++) {
        try {
            Invoke-WebRequest -Uri $Url -OutFile $part -UserAgent $userAgent `
                -MaximumRetryCount 2 -RetryIntervalSec 3
            break
        } catch {
            if (Test-Path $part) { Remove-Item -Force $part }
            if ($attempt -eq 5) {
                throw "get-sample-media: giving up on $(Split-Path -Leaf $Target) after 5 attempts: $_"
            }
            Write-Host "get-sample-media: download of $(Split-Path -Leaf $Target) failed, retrying in ${delay}s"
            Start-Sleep -Seconds $delay
            $delay *= 2
        }
    }
    $got = (Get-Item $part).Length
    if ($got -ne $Bytes) {
        Remove-Item -Force $part
        throw "get-sample-media: $(Split-Path -Leaf $Target) is $got bytes, expected $Bytes"
    }
    $gotSha = Get-Sha256 $part
    if ($gotSha -ne $Sha) {
        Remove-Item -Force $part
        throw ("get-sample-media: $(Split-Path -Leaf $Target) hashes to $gotSha, expected $Sha; " +
            'the file upstream is not the one this project was built against')
    }
    Move-Item -Force $part $Target
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$entries = foreach ($m in $catalogue) {
    $target = Join-Path $OutDir $m.name
    if ($DryRun) {
        Write-Host "get-sample-media: would download $($m.name) from $($m.url)"
    }
    elseif ((-not $Force) -and (Test-Path $target) -and ((Get-Sha256 $target) -eq $m.sha256)) {
        Write-Host "get-sample-media: keeping existing $($m.name)"
    }
    else {
        Write-Host "get-sample-media: downloading $($m.name) ($($m.bytes) bytes)"
        Save-Media -Target $target -Url $m.url -Sha $m.sha256 -Bytes $m.bytes
    }
    [pscustomobject]@{
        name        = $m.name
        bytes       = $m.bytes
        sha256      = $m.sha256
        url         = $m.url
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
    media     = @($entries)
} | ConvertTo-Json -Depth 4 | Set-Content -Path $manifestPath -Encoding utf8
Write-Host "get-sample-media: wrote $manifestPath"
