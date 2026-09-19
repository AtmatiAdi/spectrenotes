# 07 — Benchmark wersji

Jedno miejsce na liczby, które mają nie rosnąć: rozmiar binarki, czas startu,
pamięć. Każde wydanie dopisuje wiersz do [`bench/startup.csv`](bench/startup.csv)
(skrypt `tools/bench-startup.ps1`), a poniższa tabela to jego czytelna kopia.

## Jak mierzymy

`tools/bench-startup.ps1 -Exes <exe…> -Profile <APPDATA testowe> -Runs 7 -Csv docs/bench/startup.csv`

- **Maszyna odniesienia:** laptop z panelem 2880×1800 (Intel Arc, Windows 11 Pro),
  profil testowy = kopia 6 notatek użytkownika (największa 324 kB), okno
  1400×900 na monitorze pomocniczym, LAN wyłączony (`SPECTRENOTES_NO_LAN`).
- **świeża** — kopia exe pod nową nazwą uruchomiona raz: tak wygląda pierwszy
  start po aktualizacji (Defender skanuje nowy plik). Jedna próba, duży rozrzut.
- **okno** — od `CreateProcess` do widocznego okna głównego (Stopwatch w C#,
  bez narzutu PowerShella); **klatka** — do pierwszej niepustej klatki
  (`PrintWindow`, jasny piksel w pasku narzędzi). Mediana i minimum z `-Runs`.
- **RAM** — working set / private bytes po 4 s bezczynności z otwartym oknem
  wyboru notatki; **tray** — working set 2 s po schowaniu (X).
- **CPU** — łączny czas procesora po tych 4 s (start + miniatury + bezczynność).
- Wewnętrzne kamienie milowe (`startup:` w stderr, od wejścia do `main`):
  okno utworzone, lista notatek, notatka otwarta, renderer gotowy, pierwsza
  klatka; `renderer:` rozbija urządzenie D3D / swapchain / D2D / DWrite.

Szum: ±10 ms na czasach, ±50 ms na CPU, ±2 MB na RAM — różnice mniejsze od
tego nie są zmianą.

## Wyniki (2026-09-19, ten sam dzień, ta sama maszyna)

| wersja | exe kB | świeża okno/klatka | okno med (min) | klatka med (min) | RAM MB (priv) | tray MB | CPU ms |
|---|---:|---:|---:|---:|---:|---:|---:|
| 0.3.0 | 2710 | 276 / 483 | 148 (115) | 170 (150) | 93,3 (80,1) | 14,2 | 344 |
| 0.3.3 | 2745 | 241 / 283 | 122 (115) | 161 (152) | 94,8 (82,0) | 14,3 | 281 |
| 0.3.4 | 2758 | 270 / 305 | 129 (113) | 150 (138) | 92,9 (81,8) | 13,2 | 281 |
| 0.3.5 | 2764 | 262 / 306 | 122 (113) | 147 (132) | 92,9 (83,4) | 13,3 | 297 |
| 0.3.6 | 2788 | 568 / 625 | 126 (119) | 156 (148) | 87,4 (70,0) | 14,2 | 141 |
| 0.3.7-dev | 2836 | 245 / 312 | 105 (98) | 134 (130) | 88,8 (71,0) | 14,0 | 219* |

\* CPU dla 0.3.7-dev w powtórkach: 109–219 ms — szum, nie regresja.

Wewnątrz procesu (0.3.7-dev, 5 startów na ciepło): okno 5 ms, notatka otwarta
10 ms, urządzenie D3D 46–51 ms, swapchain 3, D2D 1, DWrite 1, renderer gotowy
62–69 ms, **pierwsza klatka 81–91 ms**. Przed przeniesieniem urządzenia D3D do
wątku (0.3.6): D3D 60–74 ms, renderer 75–92, pierwsza klatka 97–122.

## Co z tego wynika

- **Start na ciepło nie zwolnił** między 0.3.0 a 0.3.6 — wszystkie wersje w
  paśmie 122–148 ms do okna i 147–170 do klatki, różnice w granicach szumu.
- **Pierwszy start po aktualizacji jest wolniejszy** (świeża binarka: +100 do
  +300 ms) — to Defender skanujący nowy plik, nie aplikacja; przy pięciu
  wydaniach w dwa dni każdy start „po aktualizacji" tak wygląda. Nie da się
  tego obejść z poziomu aplikacji.
- **0.3.6 zdjęło 10 % RAM i połowę CPU startu**: miniatury z cache'u PNG zamiast
  rysowania każdej notatki przy każdym starcie.
- **0.3.7-dev: urządzenie D3D powstaje w tle** od pierwszej linijki `main`
  (`spectre_render::Device::create_in_background`), równolegle z tworzeniem okna
  i czytaniem notatek — na ciepło −15–20 ms do pierwszej klatki, na zimno więcej
  (ładowanie sterownika z dysku nakłada się na czytanie notatek z dysku).
- Reszta startu to ~10 ms własnej pracy; 60–70 % czasu to `D3D11CreateDevice`
  (sterownik Intel), na co nie mamy wpływu.

## Skalowanie z liczbą notatek (profil 300 notatek, 0.3.7-dev)

- lista notatek: 27 ms na ciepło, **780 ms po restarcie systemu** (300 małych
  plików meta z zimnego dysku) — jedyny koszt startu rosnący z liczbą notatek;
  przy tysiącach notatek do zrobienia jeden plik indeksu na space.
- miniatury: wątek `thumbs` — na zimno 7,5 s pracy przy odpowiedzi okna
  0,44 ms średnio / 2,9 ms max, widoczne kafelki po ~0,6 s; na ciepło (PNG)
  150 ms, wszystkie 300 ~170 ms po pierwszej klatce.
