<#
.SYNOPSIS
  Srodowisko deweloperskie SpectreNotes.

.DESCRIPTION
  Bez parametrow: sprawdza toolchain i dodaje cargo do PATH biezacej sesji.
  -App       uruchamia SpectreNotes (release)
  -Demo      uruchamia demo odczucia piora (release)
  -Install   doinstalowuje brakujace elementy (rustup, VS Build Tools C++)
  -Persist   dopisuje .cargo\bin do PATH uzytkownika na stale

.EXAMPLE
  .\dev.ps1 -Demo
#>
param(
    [switch]$Demo,
    [switch]$App,
    [switch]$Install,
    [switch]$Persist
)

$ErrorActionPreference = 'Stop'
$root     = $PSScriptRoot
$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'

function Write-Status($ok, $label, $detail) {
    $mark = if ($ok) { '[ OK ]' } else { '[BRAK]' }
    $color = if ($ok) { 'Green' } else { 'Yellow' }
    Write-Host ("{0} {1,-18} {2}" -f $mark, $label, $detail) -ForegroundColor $color
}

# --- PATH na czas sesji ------------------------------------------------------
if ($env:Path -notlike "*$cargoBin*") {
    $env:Path = "$cargoBin;$env:Path"
}

if ($Persist) {
    $user = [Environment]::GetEnvironmentVariable('Path', 'User')
    if ($user -notlike "*\.cargo\bin*") {
        [Environment]::SetEnvironmentVariable('Path', "$user;$cargoBin", 'User')
        Write-Host "Dodano $cargoBin do PATH uzytkownika (nowe terminale to zobacza)." -ForegroundColor Green
    } else {
        Write-Host "PATH uzytkownika juz zawiera .cargo\bin." -ForegroundColor Green
    }
}

# --- Diagnostyka -------------------------------------------------------------
$haveCargo = [bool](Get-Command cargo -ErrorAction SilentlyContinue)
$cargoVer  = if ($haveCargo) { (cargo --version) } else { 'rustup nie zainstalowany' }
Write-Status $haveCargo 'cargo' $cargoVer

$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$vsPath  = $null
if (Test-Path $vswhere) {
    $vsPath = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null
}
$haveMsvc = [bool]$vsPath
Write-Status $haveMsvc 'MSVC (link.exe)' $(if ($haveMsvc) { $vsPath } else { 'VS Build Tools z workloadem C++' })

$sdkLib = "${env:ProgramFiles(x86)}\Windows Kits\10\Lib"
$sdkVer = if (Test-Path $sdkLib) { (Get-ChildItem $sdkLib | Sort-Object Name | Select-Object -Last 1).Name } else { $null }
Write-Status ([bool]$sdkVer) 'Windows SDK' $(if ($sdkVer) { $sdkVer } else { 'brak' })

# --- Instalacja brakow -------------------------------------------------------
if ($Install) {
    if (-not $haveMsvc) {
        Write-Host "`nInstaluje VS Build Tools (workload C++)..." -ForegroundColor Cyan
        winget install --id Microsoft.VisualStudio.2022.BuildTools -e `
            --accept-package-agreements --accept-source-agreements --disable-interactivity `
            --override "--quiet --wait --norestart --nocache --add Microsoft.VisualStudio.Workload.VCTools --add Microsoft.VisualStudio.Component.VC.Tools.x86.x64 --add Microsoft.VisualStudio.Component.Windows11SDK.26100"
    }
    if (-not $haveCargo) {
        Write-Host "`nInstaluje rustup + stable-msvc..." -ForegroundColor Cyan
        $tmp = Join-Path $env:TEMP 'rustup-init.exe'
        Invoke-WebRequest -Uri 'https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe' -OutFile $tmp
        & $tmp -y --default-toolchain stable-x86_64-pc-windows-msvc --profile default --no-modify-path
        $env:Path = "$cargoBin;$env:Path"
    }
    Write-Host "`nGotowe. Uruchom ponownie: .\dev.ps1 -Demo" -ForegroundColor Green
    return
}

if (-not $haveCargo -or -not $haveMsvc) {
    Write-Host "`nBrakuje elementow toolchaina. Uruchom:  .\dev.ps1 -Install" -ForegroundColor Yellow
    return
}

# --- Uruchomienie ---------------------------------------------------------------
if ($App) {
    Push-Location $root
    try { cargo run --release -p spectre-app } finally { Pop-Location }
    return
}
if ($Demo) {
    Push-Location $root
    try {
        cargo run --release -p inkdemo
    } finally {
        Pop-Location
    }
    return
}

Write-Host "`nSrodowisko gotowe w tej sesji.  Demo:  .\dev.ps1 -Demo   albo   cargo run --release -p inkdemo" -ForegroundColor Green
