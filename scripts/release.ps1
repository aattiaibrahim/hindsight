# Builds a signed installer and publishes it as a GitHub release that
# existing installs will auto-update to.
#
#   1. Bump the version in src-tauri/tauri.conf.json (and Cargo.toml / package.json).
#   2. powershell -ExecutionPolicy Bypass -File scripts\release.ps1 -Notes "What changed"
#
# Needs: the private signing key at $env:USERPROFILE\.tauri\rewind-updater.key
# (never commit it; keep a backup, or existing installs can't be updated), and gh logged in.
param(
    [string]$Notes = "Bug fixes and improvements.",
    [string]$KeyPath = "$env:USERPROFILE\.tauri\rewind-updater.key"
)
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

$conf = Get-Content "src-tauri\tauri.conf.json" -Raw | ConvertFrom-Json
$version = $conf.version
$tag = "v$version"
$repo = "aattiaibrahim/hindsight"
if (-not (Test-Path $KeyPath)) { throw "Signing key not found at $KeyPath" }
$ErrorActionPreference = "Continue"
gh release view $tag --repo $repo *> $null
$exists = ($LASTEXITCODE -eq 0)
$ErrorActionPreference = "Stop"
if ($exists) { throw "Release $tag already exists. Bump the version first." }

Write-Host "Building Hindsight $version (signed)..."
$env:TAURI_SIGNING_PRIVATE_KEY = Get-Content $KeyPath -Raw
# The key has no password. Windows PowerShell can't hold an empty variable
# ($env:X = "" deletes it), so the signer used to stop and ask; with --ci it signs
# with an empty password instead. Set TAURI_SIGNING_PRIVATE_KEY_PASSWORD for a key that has one.
$ci = if ($env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD) { @() } else { @("--ci") }
pnpm tauri build @ci
if ($LASTEXITCODE -ne 0) { throw "Build failed" }
Remove-Item Env:\TAURI_SIGNING_PRIVATE_KEY, Env:\TAURI_SIGNING_PRIVATE_KEY_PASSWORD -ErrorAction SilentlyContinue

# The build is "Hindsight_<ver>_x64-setup.exe" (builds before 0.1.32 were
# named "Rewind_…" and are copied to the new name; the signature covers the
# contents, so the name doesn't matter to it).
$exeName = "Hindsight_${version}_x64-setup.exe"
$exe = "src-tauri\target\release\bundle\nsis\$exeName"
$built = $exe
$old = "src-tauri\target\release\bundle\nsis\Rewind_${version}_x64-setup.exe"
if (-not (Test-Path $exe) -and (Test-Path $old)) { $built = $old; Copy-Item $old $exe -Force }
$sig = Get-Content "$built.sig" -Raw
$manifest = [ordered]@{
    version   = $version
    notes     = $Notes
    pub_date  = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    platforms = [ordered]@{
        "windows-x86_64" = [ordered]@{
            signature = $sig.Trim()
            url       = "https://github.com/$repo/releases/download/$tag/$exeName"
        }
    }
}
$latest = "src-tauri\target\release\bundle\nsis\latest.json"
[IO.File]::WriteAllText((Join-Path $root $latest), ($manifest | ConvertTo-Json -Depth 5), (New-Object Text.UTF8Encoding $false))

$notesFile = New-TemporaryFile
@"
$Notes

### Install
Download **$exeName** below and run it. Already have Hindsight? It updates itself.
Windows SmartScreen may warn because the app isn't code-signed yet: click **More info → Run anyway**.
"@ | Set-Content $notesFile

& powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "package-streamdeck.ps1")
$plugin = "streamdeck\dist\Hindsight.streamDeckPlugin"
if (-not (Test-Path $plugin)) { throw "Stream Deck plugin wasn't built at $plugin" }

Write-Host "Publishing $tag..."
gh release create $tag $exe $latest $plugin --repo $repo --title "Hindsight $version" --notes-file $notesFile --latest
$published = $LASTEXITCODE -eq 0
Remove-Item $notesFile
# GitHub sometimes answers 500: don't claim success, and say how to retry without rebuilding.
if (-not $published) {
    throw "Publishing failed (GitHub error). The build is fine; retry with: gh release create $tag $exe $latest $plugin --repo $repo --title `"Hindsight $version`" --latest"
}
Write-Host "Done: https://github.com/$repo/releases/tag/$tag"
