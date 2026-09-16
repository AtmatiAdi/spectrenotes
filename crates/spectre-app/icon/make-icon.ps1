#Requires -Version 5.1
<#
Ikona SpectreNotes - generowana, nie rysowana recznie, zeby dala sie odtworzyc
i poprawic jedna liczba. Motyw: to, co widzi uzytkownik - biala kreska o zmiennej
grubosci (nacisk) na czarnym canvasie, z akcentem UI (115,160,140) jako kropla
mokrego tuszu na koncu kreski.

Wyjscie: crates\spectre-app\icon.ico (rozmiary 16..256, wpisy PNG), opcjonalnie
podglad PNG 256 px (-Preview). Uzycie: .\crates\spectre-app\icon\make-icon.ps1
#>
param(
    [string]$Out = (Join-Path $PSScriptRoot '..\icon.ico'),
    [string]$Preview = ''
)
Add-Type -AssemblyName System.Drawing

# Kolory z ui.rs: BG (18,18,18), LINE (60,60,60), tusz jak FG na canvasie, ACCENT.
$bgColor = [System.Drawing.Color]::FromArgb(255, 20, 20, 22)
$rimColor = [System.Drawing.Color]::FromArgb(255, 70, 70, 74)
$inkColor = [System.Drawing.Color]::FromArgb(255, 236, 236, 236)
$accent = [System.Drawing.Color]::FromArgb(255, 115, 160, 140)

function Bezier([double[]]$p, [double]$t) {
    # $p = x0,y0, x1,y1, x2,y2, x3,y3 (krzywa kubiczna)
    $u = 1 - $t
    $x = $u*$u*$u*$p[0] + 3*$u*$u*$t*$p[2] + 3*$u*$t*$t*$p[4] + $t*$t*$t*$p[6]
    $y = $u*$u*$u*$p[1] + 3*$u*$u*$t*$p[3] + 3*$u*$t*$t*$p[5] + $t*$t*$t*$p[7]
    ,@($x, $y)
}

function Stamp([System.Drawing.Graphics]$g, [System.Drawing.Brush]$ink, [double[]]$p, [double]$s, [int]$n, [scriptblock]$width) {
    # Kreska jako gesty ciag kolek (stempel okraglego pedzla): bez wstegi nie ma
    # problemu z petla brzegu w ciasnym zakrecie, a nakladanie sie kolek na
    # kryjacym tle nic nie psuje.
    for ($i = 0; $i -le $n; $i++) {
        $t = $i / $n
        $c = Bezier $p $t
        $w = (& $width $t) * $s
        $g.FillEllipse($ink, [float]($c[0]*$s - $w), [float]($c[1]*$s - $w), [float](2*$w), [float](2*$w))
    }
}

function Draw([int]$size) {
    $bmp = New-Object System.Drawing.Bitmap($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
    $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.Clear([System.Drawing.Color]::Transparent)
    $s = $size / 100.0   # wspolrzedne projektu w 0..100

    # Kafelek: zaokraglony kwadrat z cienka obwodka, zeby czytal sie na ciemnym pasku.
    $r = 22 * $s; $m = 2 * $s; $d = $size - 2*$m
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $path.AddArc($m, $m, 2*$r, 2*$r, 180, 90)
    $path.AddArc($m + $d - 2*$r, $m, 2*$r, 2*$r, 270, 90)
    $path.AddArc($m + $d - 2*$r, $m + $d - 2*$r, 2*$r, 2*$r, 0, 90)
    $path.AddArc($m, $m + $d - 2*$r, 2*$r, 2*$r, 90, 90)
    $path.CloseFigure()
    $g.FillPath((New-Object System.Drawing.SolidBrush($bgColor)), $path)
    $rimW = [Math]::Max(1.0, 1.6 * $s)
    $g.DrawPath((New-Object System.Drawing.Pen($rimColor, $rimW)), $path)

    # Kreska: jeden pociagniecie "S" - lekko na wejsciu, pelny nacisk w brzuchu,
    # unoszone pioro na wyjsciu. Krzywa i profil w jednostkach projektu.
    $curve = @(80, 26,  -16, 0,  116, 100,  20, 76)
    # Male rozmiary (pasek zadan, tray) dostaja grubsza kreske i wieksza krople -
    # przy 16 px cienka koncowka to pol piksela szarosci, nie kreska.
    $boost = if ($size -le 24) { 1.7 } elseif ($size -le 40) { 1.3 } else { 1.0 }
    $pressure = { param($t) $boost * (1.6 + 6.2 * [Math]::Pow([Math]::Sin([Math]::PI * [Math]::Pow($t, 0.85)), 1.3)) }.GetNewClosure()
    $ink = New-Object System.Drawing.SolidBrush($inkColor)
    Stamp $g $ink $curve $s ([Math]::Max(120, 3 * $size)) $pressure
    # Kropla mokrego tuszu w akcencie - tam, gdzie pioro oderwalo sie od kartki.
    $e = Bezier $curve 1.0
    $dropR = 6.5 * $s * [Math]::Sqrt($boost)
    $g.FillEllipse((New-Object System.Drawing.SolidBrush($accent)), [float]($e[0]*$s - $dropR), [float]($e[1]*$s - $dropR), [float](2*$dropR), [float](2*$dropR))

    $g.Dispose()
    ,$bmp
}

function PngBytes([System.Drawing.Bitmap]$bmp) {
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    ,$ms.ToArray()
}

$sizes = 16, 20, 24, 32, 40, 48, 64, 128, 256
$images = @()
foreach ($sz in $sizes) {
    $bmp = Draw $sz
    $images += ,@($sz, (PngBytes $bmp))
    if ($Preview -and $sz -eq 256) { $bmp.Save($Preview, [System.Drawing.Imaging.ImageFormat]::Png) }
    $bmp.Dispose()
}

# ICO: ICONDIR + ICONDIRENTRY[n] + dane PNG (Vista+ czyta wpisy PNG w kazdym rozmiarze).
$ms = New-Object System.IO.MemoryStream
$bw = New-Object System.IO.BinaryWriter($ms)
$bw.Write([uint16]0); $bw.Write([uint16]1); $bw.Write([uint16]$images.Count)
$offset = 6 + 16 * $images.Count
foreach ($im in $images) {
    $sz = $im[0]; $bytes = $im[1]
    $dim = if ($sz -ge 256) { 0 } else { $sz }
    $bw.Write([byte]$dim); $bw.Write([byte]$dim)
    $bw.Write([byte]0); $bw.Write([byte]0)
    $bw.Write([uint16]1); $bw.Write([uint16]32)
    $bw.Write([uint32]$bytes.Length); $bw.Write([uint32]$offset)
    $offset += $bytes.Length
}
foreach ($im in $images) { $bw.Write($im[1]) }
$bw.Flush()
$dir = Split-Path -Parent $Out
if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir | Out-Null }
[System.IO.File]::WriteAllBytes($Out, $ms.ToArray())
"ikona: $Out ($($ms.Length) B, rozmiary $($sizes -join ','))"
