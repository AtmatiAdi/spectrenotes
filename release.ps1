<#
.SYNOPSIS
  Wydanie SpectreNotes: wersja -> build -> tag -> GitHub Release (ostatnie 3).

.DESCRIPTION
  Jeden krok od "to jest gotowe" do wydania, ktore zainstalowane aplikacje
  zobaczą jako aktualizacje (docs/06-DYSTRYBUCJA.md):

    1. podnosi `version` w [workspace.package] w Cargo.toml,
    2. `cargo test --workspace` (chyba ze -SkipTests) i `cargo build --release`,
    3. liczy SHA256SUMS.txt dla spectrenotes.exe i SpectreNotes-Setup.exe,
    4. commit "Wydanie vX.Y.Z", tag vX.Y.Z, push,
    5. `gh release create` z trzema zasobami o stalych nazwach,
    6. kasuje wydania starsze niz -Keep ostatnich (tagi zostaja w repo).

  Zasoby maja stale nazwy, wiec adres najnowszego instalatora nie zmienia sie:
    https://github.com/<owner>/<repo>/releases/latest/download/SpectreNotes-Setup.exe

  Wymaga: gh (zalogowany), cargo, czyste drzewo robocze na main.

.PARAMETER Version
  Pelny numer, np. 0.2.0. Alternatywa: -Bump.
.PARAMETER Bump
  patch | minor | major - podnosi biezacy numer z Cargo.toml.
.PARAMETER Notes
  Opis wydania (markdown). Domyslnie: lista commitow od poprzedniego tagu v*.
.PARAMETER Keep
  Ile ostatnich wydan zostawic na GitHubie (domyslnie 3).
.PARAMETER Repo
  Repozytorium wydan `owner/repo`. Domyslnie RELEASES_REPO z spectre-update
  (publiczne repo tylko na wydania - zrodla zostaja prywatne).
.PARAMETER DryRun
  Wszystko lokalnie (wersja, build, sumy), bez commitu, pushu i release'u.

.EXAMPLE
  .\release.ps1 -Bump patch
  .\release.ps1 -Version 0.2.0 -Notes "Pelny ekran, panel menu obok paska"
#>
param(
    [string]$Version,
    [ValidateSet('patch', 'minor', 'major')]
    [string]$Bump,
    [string]$Notes,
    [int]$Keep = 3,
    [string]$Repo,
    [switch]$SkipTests,
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
Set-Location $root
$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
if ($env:Path -notlike "*$cargoBin*") { $env:Path = "$cargoBin;$env:Path" }

function Step($msg) { Write-Host "`n== $msg" -ForegroundColor Cyan }
function Fail($msg) { Write-Host "BLAD: $msg" -ForegroundColor Red; exit 1 }

# --- Wersja ------------------------------------------------------------------
$cargoToml = Join-Path $root 'Cargo.toml'
$toml = Get-Content $cargoToml -Raw
$m = [regex]::Match($toml, '(?ms)^\[workspace\.package\]\s*\r?\nversion\s*=\s*"(\d+)\.(\d+)\.(\d+)"')
if (-not $m.Success) { Fail 'nie znalazlem version w [workspace.package]' }
$cur = "$($m.Groups[1].Value).$($m.Groups[2].Value).$($m.Groups[3].Value)"

if ($Bump) {
    $a = [int]$m.Groups[1].Value; $b = [int]$m.Groups[2].Value; $c = [int]$m.Groups[3].Value
    switch ($Bump) {
        'major' { $a++; $b = 0; $c = 0 }
        'minor' { $b++; $c = 0 }
        'patch' { $c++ }
    }
    $Version = "$a.$b.$c"
}
if (-not $Version) { Fail 'podaj -Version X.Y.Z albo -Bump patch|minor|major' }
if ($Version -notmatch '^\d+\.\d+\.\d+$') { Fail "wersja `"$Version`" nie jest X.Y.Z" }
$tag = "v$Version"

# Repozytorium WYDAN (publiczne) - inne niz zrodla. Jedno zrodlo prawdy:
# stala RELEASES_REPO w spectre-update, ta sama, ktora ma w sobie aplikacja.
if (-not $Repo) {
    $lib = Get-Content (Join-Path $root 'crates\spectre-update\src\lib.rs') -Raw
    $Repo = [regex]::Match($lib, '(?m)^pub const RELEASES_REPO: &str = "([^"]+)";').Groups[1].Value
}
$repo = $Repo
if (-not $repo) { Fail 'brak RELEASES_REPO w spectre-update' }

Write-Host "SpectreNotes $cur -> $Version  ($repo, tag $tag)" -ForegroundColor Green

# --- Warunki wstepne ----------------------------------------------------------
Step 'Warunki wstepne'
if (-not (Get-Command gh -ErrorAction SilentlyContinue)) { Fail 'brak gh (GitHub CLI)' }
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { Fail 'brak cargo' }
gh auth status 2>&1 | Out-Null
if ($LASTEXITCODE -ne 0) { Fail 'gh nie jest zalogowany (gh auth login)' }
$branch = (git rev-parse --abbrev-ref HEAD).Trim()
if ($branch -ne 'main' -and -not $DryRun) { Fail "wydania tylko z main (jestes na $branch)" }
$dirty = git status --porcelain
if ($dirty -and -not $DryRun) { Fail "drzewo robocze nie jest czyste:`n$dirty" }
if (-not $DryRun) {
    $existing = git tag -l $tag
    if ($existing) { Fail "tag $tag juz istnieje" }
}

# Instancja uruchomiona z target\release trzyma plik - poprosic ja o zakonczenie
# (WM_APP+5, patrz shell_win::install::WM_QUIT_APP), nie zabijac.
$targetDir = (Join-Path $root 'target').ToLower()
$running = Get-Process spectrenotes -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -and $_.Path.ToLower().StartsWith($targetDir) }
if ($running) {
    Write-Host "Zamykam instancje z target\ (pid $($running.Id -join ', '))..." -ForegroundColor Yellow
    Add-Type -Namespace Rel -Name W -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll", CharSet=System.Runtime.InteropServices.CharSet.Unicode)]
public static extern System.IntPtr FindWindowExW(System.IntPtr p, System.IntPtr a, string cls, string title);
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern bool PostMessageW(System.IntPtr h, uint msg, System.IntPtr w, System.IntPtr l);
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern uint GetWindowThreadProcessId(System.IntPtr h, out uint pid);
'@
    $h = [IntPtr]::Zero
    while (($h = [Rel.W]::FindWindowExW([IntPtr]::Zero, $h, 'SpectreNotes', $null)) -ne [IntPtr]::Zero) {
        $wpid = 0
        [Rel.W]::GetWindowThreadProcessId($h, [ref]$wpid) | Out-Null
        if ($running.Id -contains $wpid) { [Rel.W]::PostMessageW($h, 0x8000 + 5, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null }
    }
    $running | Wait-Process -Timeout 15 -ErrorAction SilentlyContinue
}

# --- Wersja do Cargo.toml -----------------------------------------------------
Step "Cargo.toml: version = `"$Version`""
$newToml = $toml.Substring(0, $m.Groups[1].Index) + $Version + $toml.Substring($m.Groups[3].Index + $m.Groups[3].Length)
[IO.File]::WriteAllText($cargoToml, $newToml, [Text.UTF8Encoding]::new($false))

# --- Testy i build ---------------------------------------------------------------
if (-not $SkipTests) {
    Step 'cargo test --workspace'
    cargo test --workspace --quiet
    if ($LASTEXITCODE -ne 0) { Fail 'testy nie przeszly' }
}
Step 'cargo build --release'
cargo build --release -p spectre-app -p spectrenotes-setup
if ($LASTEXITCODE -ne 0) { Fail 'build nie przeszedl' }

# --- Zasoby -----------------------------------------------------------------------
Step 'Zasoby wydania'
$dist = Join-Path $root 'target\release\dist'
New-Item -ItemType Directory -Force $dist | Out-Null
Get-ChildItem $dist | Remove-Item -Force
$assets = @('spectrenotes.exe', 'SpectreNotes-Setup.exe')
foreach ($a in $assets) {
    Copy-Item (Join-Path $root "target\release\$a") (Join-Path $dist $a)
}
$sums = $assets | ForEach-Object {
    $h = (Get-FileHash (Join-Path $dist $_) -Algorithm SHA256).Hash.ToLower()
    "$h  $_"
}
# LF, bez BOM - format sha256sum, czytany przez spectre_update::sum_for.
[IO.File]::WriteAllText((Join-Path $dist 'SHA256SUMS.txt'), (($sums -join "`n") + "`n"), [Text.UTF8Encoding]::new($false))
Get-ChildItem $dist | ForEach-Object { "{0,-24} {1,10:N0} B" -f $_.Name, $_.Length }

# --- Opis wydania -------------------------------------------------------------------
if (-not $Notes) {
    $prev = git tag -l 'v*' --sort=-v:refname | Where-Object { $_ -match '^v\d+\.\d+\.\d+$' } | Select-Object -First 1
    $range = if ($prev) { "$prev..HEAD" } else { 'HEAD' }
    $log = git log $range --no-merges --format='- %s'
    $Notes = if ($log) { ($log -join "`n") } else { "SpectreNotes $Version" }
}
$Notes += "`n`nInstalacja: pobierz **SpectreNotes-Setup.exe** i uruchom - sciaga te wersje i instaluje ja dla biezacego uzytkownika. Zainstalowana aplikacja sama zaproponuje kolejne wydania (Settings -> Application)."

if ($DryRun) {
    Step 'DryRun - koniec'
    Write-Host "Opis wydania:`n$Notes"
    git checkout -- Cargo.toml Cargo.lock 2>$null
    exit 0
}

# --- Commit, tag, push ---------------------------------------------------------------
Step "Commit i tag $tag"
git add Cargo.toml Cargo.lock
git diff --cached --quiet
if ($LASTEXITCODE -ne 0) {
    git commit -q -m "Wydanie $tag" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
}
git tag -a $tag -m "SpectreNotes $Version"
git push -q origin main
git push -q origin $tag
if ($LASTEXITCODE -ne 0) { Fail 'push nie przeszedl' }

# --- Release -------------------------------------------------------------------------
Step "gh release create $tag"
$notesFile = Join-Path $dist 'NOTES.md'
[IO.File]::WriteAllText($notesFile, $Notes, [Text.UTF8Encoding]::new($false))
gh release create $tag `
    (Join-Path $dist 'spectrenotes.exe') `
    (Join-Path $dist 'SpectreNotes-Setup.exe') `
    (Join-Path $dist 'SHA256SUMS.txt') `
    --repo $repo --title "SpectreNotes $Version" --notes-file $notesFile --latest
if ($LASTEXITCODE -ne 0) { Fail 'gh release create nie przeszedl' }
Remove-Item $notesFile

# --- Ostatnie N -------------------------------------------------------------------------
Step "Zostawiam $Keep ostatnich wydan"
$all = gh release list --repo $repo --limit 100 --json tagName,createdAt,isDraft | ConvertFrom-Json
$old = $all | Where-Object { -not $_.isDraft } | Sort-Object { [datetime]$_.createdAt } -Descending | Select-Object -Skip $Keep
foreach ($r in $old) {
    Write-Host "  usuwam wydanie $($r.tagName) (tag zostaje)"
    gh release delete $r.tagName --repo $repo --yes
}

Write-Host "`nWydano SpectreNotes $Version" -ForegroundColor Green
Write-Host "  https://github.com/$repo/releases/tag/$tag"
Write-Host "  instalator: https://github.com/$repo/releases/latest/download/SpectreNotes-Setup.exe"
