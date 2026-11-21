# Tryb 3D

Wymagania: `briefs/spectrenotes-3d-wymagania.md` w repozytorium Wegi (W1–W19, O1–O18, T1–T7; późniejsze sekcje mają pierwszeństwo).

## Jak to działa dla użytkownika
- **Przycisk 3D** w pasku (podświetlony na zielono, gdy włączony) zmienia kamerę z ortograficznej na perspektywę (płynnie, ta sama skala w miejscu, na które się patrzy) i pokazuje kostkę widoku. Nic więcej.
- **Wyjście z 3D:** przycisk 3D, „2D” pod kostką albo blokada widoku. Wraca dokładny stan 2D sprzed wejścia: kamera, zoom, miejsce, blokada, płaszczyzna zerowa. Kliknięcie 3D przy włączonej blokadzie ją zdejmuje.
- **Kostka** (prawy górny róg): przeciągnięcie obraca całą przestrzeń, klik w ścianę ustawia widok z animacją, klik w narożnik daje izometrię, ⌂ wraca do widoku startowego.
- **Kompas** (przycisk w pasku, tylko w 3D) stoi na środku płaszczyzny rysowania:
  - pierścień X (czerwony) obraca wokół osi u płaszczyzny, pierścień Y (zielony) wokół pionu;
  - dwie szare strzałki przesuwają środek po płaszczyźnie (płaszczyzna ta sama, O16), niebieska przesuwa wzdłuż normalnej;
  - przeciągnięcie środka przesuwa go po płaszczyźnie bez kroków;
  - krok zależy od odległości pióra od środka: 45°, 15°, 5°, 1°, 0,1° (przesuwanie 100, 10, 1, 0,1, 0,01 j.); granice stref widać w trakcie, krok stoi przy piórze;
  - przy środku wartości bezwzględne: obrót X i Y, położenie środka.
- **Płaszczyzny powstają same:** pierwsza kreska po obróceniu albo przesunięciu płaszczyzny rysowania tworzy nową płaszczyznę. Kreska na płaszczyźnie, która już jest (ta sama nieskończona płaszczyzna), dołącza do niej.
- **Lista płaszczyzn** (przycisk w pasku): nazwa (pisana piórem, W19, albo nadana sama: „Pozioma 1”, „Pionowa 2”), współrzędne, oko. Klik w wiersz ustawia płaszczyznę rysowania; kamera stoi. Klik w nazwę otwiera pole pisania nazwy.
- **Widać tylko płaszczyznę rysowania:** siatka z 2D leży na niej i zanika ku brzegom; bez siatki – przerywana ramka 360×240 j. Inne płaszczyzny pokazują tylko kreski, bez tła i bez zasłaniania.
- **Pióro** rysuje na płaszczyźnie rysowania; gumka działa na jej kreskach. Przycisk na rysiku przesuwa kamerę, kółko przybliża do kursora. Zaznaczanie w 3D jeszcze nie działa.
- **Zapis widoku:** zamknięcie okna albo zmiana notatki w 3D zapisuje kamerę i płaszczyznę rysowania w notatce (metadana `view3d.<autor>`, każdy autor osobno). Notatka otwiera się w trybie, w którym była zamknięta.

## Jak to działa w kodzie
| Część | Gdzie |
|---|---|
| Format: `PlaneAdd`, `StrokeOn`, `PlaneProp` (tagi 4–6); kreski płaszczyzny zerowej bez zmian | `spectre-proto/src/codec.rs`, `types.rs` |
| Płaszczyzny w dokumencie, samo tworzenie, cofanie | `spectre-core/src/document.rs` (`add_stroke_on`, `find_plane`) |
| Geometria: `Pose`, `Camera3`, rzut, rzut odwrotny, homografia, kompas | `spectre-core/src/space.rs` |
| Renderer: pieczenie płaszczyzn, składanie z perspektywą, siatka | `spectre-render/src/d2d/scene3d.rs` |
| Okno: stan 3D, kostka, kompas, lista, zapis widoku | `spectre-app/src/three.rs`, wpięcie w `app.rs` |

Pióro w 3D: punkty przechodzą przez rzut odwrotny na płaszczyznę (jedno mnożenie na próbkę), mokra kreska jest rzutowana na ekran i dalej idzie drogą 2D (warstwa sucha = złożona scena). Scena składa się tylko po zmianie (kamera, płaszczyzna rysowania, kreska); w spoczynku nie ma klatek.

## Testy
- `cargo test`: T1.2 (bajty), T1.1 (powtarzalność; obraz przy `SPECTRENOTES_GOLDEN_IMG=1`), T2.1, T3.1–T3.4, T4 (płaszczyzny, migotanie), T6.1, T6.2, T6.5, T6.6, T7.
- Uprząż (`docs/08`): `t1`/`t1-compare`, `t2-write`/`t2-check` z binarką E0, `t4` (zrzuty pokoju co 90°), `t5`/`t51` (wydajność na karcie).
- `spectrenotes --ui3d-png <katalog>`: zrzuty interfejsu 3D przy 100–175% bez okna.
