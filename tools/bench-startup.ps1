<#
.SYNOPSIS
  Benchmark startu i pamieci SpectreNotes - jedna binarka albo kilka wersji obok siebie.

.DESCRIPTION
  Dla kazdej binarki (`-Exes`), na kopii profilu testowego (`-Profile`, katalog
  %APPDATA% z SpectreNotes\...; nigdy profil uzytkownika):

    1. "swieza binarka": kopia exe pod nowa nazwa i jedno uruchomienie - tak
       wyglada pierwszy start po aktualizacji (Defender skanuje nowy plik),
    2. `-Runs` uruchomien "na cieplo": czas od CreateProcess do widocznego okna
       i do pierwszej niepustej klatki (PrintWindow, piksele paska narzedzi),
    3. po ostatnim uruchomieniu: RAM (working set, private) i czas CPU po 4 s
       bezczynnosci, RAM po schowaniu do traya (WM_CLOSE) po 2 s.

  Czasy mierzy pomocnik C# w tym samym procesie (Stopwatch od Process.Start),
  wiec narzut PowerShella nie wchodzi w wynik. Kazda binarka dostaje osobna
  kopie profilu, zeby wersje nie zostawialy sobie cache'ow (miniatury).

  Wymaga SPECTRENOTES_TEST_INPUT (ustawiane tu) - inaczej druga instancja
  pokazalaby okno tej dzialajacej u uzytkownika zamiast wystartowac.

.EXAMPLE
  .\tools\bench-startup.ps1 -Exes target\release\spectrenotes.exe -Profile ..\appdata-thumbs
  .\tools\bench-startup.ps1 -Exes a.exe,b.exe -Profile ..\appdata-thumbs -Runs 7 -Csv docs\bench.csv
#>
param(
    [Parameter(Mandatory)][string[]]$Exes,
    [Parameter(Mandatory)][string]$Profile,
    [int]$Runs = 5,
    [string]$Csv,
    [string]$Work = (Join-Path $env:TEMP "spectre-bench")
)

Add-Type -AssemblyName System.Drawing
$refs = if ($PSVersionTable.PSVersion.Major -ge 6) { 'System.Drawing.Common', 'System.Drawing.Primitives', 'System.ComponentModel.Primitives', 'System.Diagnostics.Process', 'System.Collections', 'System.Threading.Thread', 'System.Runtime.InteropServices' } else { 'System.Drawing' }
Add-Type -ReferencedAssemblies $refs -ErrorAction Stop -TypeDefinition @"
using System; using System.Diagnostics; using System.Runtime.InteropServices; using System.Text;
public static class SB {
  [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint f);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr a, int x, int y, int cx, int cy, uint f);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  delegate bool EnumProc(IntPtr h, IntPtr l);
  public static IntPtr Find(int pid) {
    IntPtr found = IntPtr.Zero;
    EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p); if ((int)p != pid || !IsWindowVisible(h)) return true;
      var sb = new StringBuilder(64); GetClassNameW(h, sb, 64); if (sb.ToString() != "SpectreNotes") return true; found = h; return false; }, IntPtr.Zero);
    return found;
  }
  // Start procesu i czas do widocznego okna [ms]; zwraca (pid, hwnd, msVisible).
  public static object[] Launch(string exe, string appdata) {
    var psi = new ProcessStartInfo(exe) { UseShellExecute = false, RedirectStandardError = true, RedirectStandardOutput = true };
    psi.Environment["APPDATA"] = appdata; psi.Environment["SPECTRENOTES_TEST_INPUT"] = "1"; psi.Environment["SPECTRENOTES_NO_LAN"] = "1";
    var sw = Stopwatch.StartNew(); var p = Process.Start(psi); p.BeginErrorReadLine(); p.BeginOutputReadLine(); IntPtr h = IntPtr.Zero;
    while (sw.ElapsedMilliseconds < 20000) { h = Find(p.Id); if (h != IntPtr.Zero) break; System.Threading.Thread.Sleep(1); }
    double vis = sw.Elapsed.TotalMilliseconds;
    return new object[] { p.Id, h, vis, sw };
  }
}
"@

# Pierwsza klatka: PrintWindow az w lewym pasku (60 px) pojawi sie jasny piksel.
# (W PowerShellu, nie w C#: System.Drawing pod pwsh 7 nie da sie zreferowac z Add-Type.)
function FirstPaint([IntPtr]$h, [Diagnostics.Stopwatch]$sw) {
    $r = New-Object SB+RECT; [SB]::GetClientRect($h, [ref]$r) | Out-Null
    $w = [Math]::Max(1, $r.R - $r.L); $hh = [Math]::Max(1, $r.B - $r.T)
    $bmp = New-Object System.Drawing.Bitmap $w, $hh
    try {
        while ($sw.ElapsedMilliseconds -lt 20000) {
            $g = [System.Drawing.Graphics]::FromImage($bmp); $dc = $g.GetHdc(); [SB]::PrintWindow($h, $dc, 2) | Out-Null; $g.ReleaseHdc($dc); $g.Dispose()
            for ($y = 0; $y -lt $hh; $y += 6) { for ($x = 0; $x -lt [Math]::Min(60, $w); $x += 3) { $c = $bmp.GetPixel($x, $y); if ($c.R + $c.G + $c.B -gt 60) { return $sw.Elapsed.TotalMilliseconds } } }
            Start-Sleep -Milliseconds 2
        }
        return -1.0
    } finally { $bmp.Dispose() }
}

function Fmt($v) { if ($v -is [double]) { "{0:N0}" -f $v } else { "$v" } }

New-Item -ItemType Directory -Force $Work | Out-Null
$rows = @()
foreach ($exe in $Exes) {
    $exe = (Resolve-Path $exe).Path
    $name = [IO.Path]::GetFileNameWithoutExtension($exe)
    $ver = (Get-Item $exe).VersionInfo.ProductVersion
    $size = (Get-Item $exe).Length
    # Osobna kopia profilu na wersje.
    $app = Join-Path $Work "profile-$name"
    if (Test-Path $app) { Remove-Item -LiteralPath $app -Recurse -Force }
    Copy-Item $Profile $app -Recurse
    # 1. Swieza binarka (pierwszy start po aktualizacji).
    $fresh = Join-Path $Work ("fresh-{0}.exe" -f $name)   # stala nazwa: zapora pyta o nowa sciezke tylko raz
    Copy-Item $exe $fresh
    $r = [SB]::Launch($fresh, $app); $pid0 = $r[0]; $h = $r[1]; $freshVis = $r[2]
    $freshPaint = if ($h -ne [IntPtr]::Zero) { (FirstPaint $h $r[3]) } else { -1 }
    Start-Sleep -Milliseconds 1500; Stop-Process -Id $pid0 -Force -ErrorAction SilentlyContinue; Start-Sleep -Milliseconds 1500
    Remove-Item $fresh -Force -ErrorAction SilentlyContinue
    # 2. Na cieplo.
    $vis = @(); $paint = @(); $ws = 0; $priv = 0; $cpu = 0; $wsHidden = 0
    for ($i = 1; $i -le $Runs; $i++) {
        $r = [SB]::Launch($exe, $app); $procId = $r[0]; $h = $r[1]
        $vis += [double]$r[2]
        $paint += if ($h -ne [IntPtr]::Zero) { [double](FirstPaint $h $r[3]) } else { -1 }
        if ($i -eq $Runs) {
            # Okno poza monitorem glownym (nakladka), 4 s bezczynnosci, pomiar pamieci.
            Start-Sleep 4
            $p = Get-Process -Id $procId; $p.Refresh()
            $ws = $p.WorkingSet64; $priv = $p.PrivateMemorySize64; $cpu = $p.TotalProcessorTime.TotalMilliseconds
            [SB]::PostMessageW($h, 0x10, [IntPtr]0, [IntPtr]0) | Out-Null   # WM_CLOSE = schowaj do traya
            Start-Sleep 2
            $p.Refresh(); $wsHidden = $p.WorkingSet64
        } else {
            Start-Sleep -Milliseconds 800
        }
        Stop-Process -Id $procId -Force -ErrorAction SilentlyContinue; Start-Sleep -Milliseconds 1500
    }
    $visS = $vis | Sort-Object; $paintS = $paint | Sort-Object
    $row = [ordered]@{
        version = $ver; file = $name; date = (Get-Date -Format 'yyyy-MM-dd')
        size_kb = [math]::Round($size / 1KB)
        fresh_visible_ms = [math]::Round($freshVis); fresh_paint_ms = [math]::Round($freshPaint)
        visible_med_ms = [math]::Round($visS[[int]($visS.Count / 2)]); visible_min_ms = [math]::Round($visS[0])
        paint_med_ms = [math]::Round($paintS[[int]($paintS.Count / 2)]); paint_min_ms = [math]::Round($paintS[0])
        ws_mb = [math]::Round($ws / 1MB, 1); private_mb = [math]::Round($priv / 1MB, 1)
        ws_hidden_mb = [math]::Round($wsHidden / 1MB, 1); cpu_ms = [math]::Round($cpu)
        runs = $Runs
    }
    $rows += [pscustomobject]$row
    "{0,-22} {1,6} kB  swieza {2,5}/{3,5} ms  okno {4,4} ms (min {5,4})  klatka {6,4} ms (min {7,4})  RAM {8,6} MB (priv {9,6}, tray {10,6})  CPU {11,5} ms" -f `
        "$name ($ver)", $row.size_kb, $row.fresh_visible_ms, $row.fresh_paint_ms, $row.visible_med_ms, $row.visible_min_ms, $row.paint_med_ms, $row.paint_min_ms, $row.ws_mb, $row.private_mb, $row.ws_hidden_mb, $row.cpu_ms
}
if ($Csv) {
    $new = -not (Test-Path $Csv)
    $rows | Export-Csv -Path $Csv -Append -NoTypeInformation -Encoding UTF8
    if ($new) { "zapisano naglowek i wiersze: $Csv" } else { "dopisano wiersze: $Csv" }
}
$rows
