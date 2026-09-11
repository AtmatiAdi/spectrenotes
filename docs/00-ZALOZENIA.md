# SpectreNotes — założenia produktowe

> Dokument źródłowy. Wszystkie decyzje techniczne w `docs/01-ARCHITEKTURA.md`
> i w `docs/adr/` muszą dać się uzasadnić którymś punktem stąd.

## Kontekst

Zamiennik Samsung Notes na Windows. Powód: Samsung już raz odciął
nie-samsungowe komputery od tej aplikacji i może to zrobić ponownie —
notatki muszą leżeć w formacie i infrastrukturze, nad którą mam pełną kontrolę.

## Sprzęt referencyjny (target #1)

| | |
|---|---|
| Maszyna | HP Spectre x360 16-aa0xxx (2-in-1) |
| CPU | Intel Core Ultra 7 155H (16C / 22T) |
| GPU | Intel Arc iGPU (zintegrowany) + NVIDIA RTX 4050 Laptop (hybryda) |
| Ekran | 2880×1800 **120 Hz Samsung OLED** (panel `SDC41A6`) |
| Digitizer | ELAN2513, „HID-compliant pen" — **MPP 2.0** |
| RAM | 32 GB |
| Sieć | Tailscale zainstalowany |

Konsekwencje: budżet klatki 8,33 ms, prawdziwy AMOLED (czerń = piksel zgaszony),
hybrydowe GPU (musimy *świadomie* zostać na iGPU — patrz `docs/04-ENERGIA-I-AMOLED.md`).

## Twarde założenia

### Z1. Wydajność i niski pobór energii
- Rendering **wyłącznie sterowany zdarzeniami** — brak pętli klatkowej gdy nic się nie dzieje.
- Idle (okno widoczne, brak interakcji): **0% CPU**, 0 wybudzeń GPU.
- Świadomy wybór iGPU — nigdy nie budzimy dedykowanej karty dla notatnika.
- Budżet: rysowanie na baterii ma kosztować mniej niż odtwarzanie wideo 1080p.

### Z2. Rezydentność w tle, natychmiastowe otwarcie
- Proces żyje cały czas w tray'u; okno jest tworzone raz i tylko chowane.
- Urządzenie GPU, pipeline'y i indeks notatek zainicjalizowane z góry.
- **Cel: <30 ms od hotkeya do pierwszej klatki.** Aplikacja ma sprawiać wrażenie
  części systemu, nie programu który się „uruchamia".
- RAM w tle: **cel ≤ 20 MB working set** (po `Trim()` + zwolnieniu cache kafli).

### Z3. Natychmiastowa synchronizacja + pełna historia
- Warstwa **live**: P2P po Tailscale/LAN, opóźnienie 1–5 ms, bez serwera pośredniczącego.
- Warstwa **trwała**: git. Space = prywatne repo GitHub. Historia = commity.
- **Odrzucone: „branch = notatka"** — uzasadnienie w `docs/adr/0003-git-jako-warstwa-trwala.md`.
  Przyjęte: space = repo, notatka = katalog, op-log rozbity per (użytkownik, urządzenie),
  dzięki czemu konflikt merge jest **strukturalnie niemożliwy**.
- Praca offline jest stanem normalnym, nie błędem.

### Z4. Realtime współpraca na jednym canvasie
- Dwie (lub więcej) osób rysuje jednocześnie na tej samej notatce.
- Transport: QUIC po Tailscale (zdalnie) lub mDNS + QUIC (LAN, działa bez internetu).
- Model danych to CRDT — zbieżność stanu bez serwera-arbitra i bez blokad.
- Obecność (kursor, kolor, pióro drugiej osoby) jest **efemeryczna** — nigdy nie ląduje w gicie.

### Z5. Zero-latency pióro MPP 2.0
- `WM_POINTER` + **`GetPointerPenInfoHistory`** (bez tego gubimy próbki między klatkami).
- Współrzędne z **`ptHimetricLocationRaw`**, nie z pikselowego `ptPixelLocation` —
  inaczej stroke jest schodkowany niezależnie od rozdzielczości.
- Wątek wejścia odseparowany od renderu; próbki przez ring buffer bez blokad.
- Present w trybie immediate/tearing dla „mokrego" stroke'a.
- Szczegóły i pełny łańcuch latencji: `docs/02-PIORO-I-LATENCJA.md`.

### Z6. Feeling rysowania, nie „rozpoznawania pisma"
- **Zero wygładzania. Zero denoise. Zero korekty kształtu.** Krzywa idzie dokładnie
  przez zaraportowane próbki.
- Rozróżnienie, które jest tu kluczowe (i mylone przez większość aplikacji):
  - *smoothing / filtr* (średnia krocząca, one-euro) — **zmienia** kształt → **NIE**
  - *predykcja / ekstrapolacja* — rysuje przed piórem, daje „haczyki" na zwrotach → **domyślnie NIE** (przełącznik do testów)
  - *interpolacja* (centripetal Catmull-Rom **przez** próbki) — nic nie zmienia,
    tylko wypełnia dziury między próbkami przy szybkim ruchu → **TAK**
- Nacisk → szerokość (krzywa konfigurowalna). Tilt → opcjonalnie eliptyczna stalówka.
- Odrzucanie dłoni: dotyk ignorowany gdy pióro jest w zasięgu (hover).

### Z7. AMOLED — praca całymi dniami
- Tło **#000000** (fizycznie zgaszone piksele = zero poboru na tym obszarze).
- Atrament nie jest czysto biały (peak luminance = wypalanie) — domyślnie ok. `#D8D8D8`.
- **Pixel shift**: powolne przesuwanie całego canvasu o kilka pikseli.
  Na nieskończonym canvasie to jest darmowe — to zwykły offset kamery.
- Brak statycznego chrome: UI samo się chowa; cokolwiek zostaje na ekranie — jeździ i przygasa.
- Tryb fullscreen jest **trybem podstawowym**, nie dodatkiem.

### Z8. Dystrybucja i aktualizacje przez git
- Update: aplikacja sprawdza tagi/Releases w repo, pobiera artefakt, atomowo podmienia przy restarcie.
- Rozdawanie innym: pojedynczy przenośny `.exe`, bez instalatora, bez runtime'u.
- Uwaga do rozwiązania: SmartScreen (podpis kodu) — `docs/06-DYSTRYBUCJA.md`.

### Z9. Canvas: nieskończony w pionie, stała szerokość
Notatka jest jak długa rolka papieru — przewijasz w dół, szerokość jest stała.

Konsekwencje:
- kamera ma **jeden** stopień swobody plus zoom, więc nawigacja, cache kafli
  i eksport do PDF są znacznie prostsze niż przy canvasie nieograniczonym w obu osiach;
- rysowanie szerokich schematów „w bok" jest wykluczone — to świadomy koszt;
- pixel shift (Z7) działa nadal, bo tło jest czarne i krawędzie „kartki" nie są
  widoczne: przesuwamy zawartość, a nie ramkę. Poziomy dryf wymaga tylko tego,
  żeby renderer nie zakładał, że kolumna treści jest przyklejona do krawędzi okna.
- do ustalenia przy Etapie 2: szerokość kolumny w jednostkach canvasu i to,
  jak zachowuje się przy zmianie zoomu oraz na monitorze zewnętrznym.

### Z10. Dotyk wyłączony w trybie notatki
Rysuje **wyłącznie** pióro. Dotyk jest ignorowany całkowicie, nie warunkowo.

Konsekwencje:
- odrzucanie dłoni przestaje być problemem — nie ma heurystyk, nie ma strojenia,
  nie ma przypadkowych kresek od nadgarstka. Odrzucamy `PT_TOUCH` na wejściu i koniec;
- znika cały rozpoznawacz gestów, co jest sporą oszczędnością złożoności;
- decyzja jest **bezwarunkowa**: dotyk na laptopie jest zbyt frustrujący, żeby
  utrzymywać go jako opcję, skoro jest precyzyjny rysik. Nie ma trybu „dotyk
  włączony" — nie projektujemy pod niego nawet w przyszłości;
- **nawigacja bez dotyku**: kandydatem jest **przycisk boczny rysika** —
  trzymany przełącza rysik na przewijanie, gumka na wymazywanie. Przyciski
  działają w każdym momencie: wciśnięcie w trakcie kreski kończy ją i od razu
  zaczyna nową rolę od tej samej próbki (model „przycisk trzymany = funkcja"). `tools/inkdemo` implementuje
  to i pokazuje w HUD, czy sterownik w ogóle raportuje ten przycisk
  (`PEN_FLAG_BARREL` albo `POINTER_FLAG_SECONDBUTTON`). Jeśli tak — temat
  zamknięty. Jeśli nie — pasek przewijania przy krawędzi obsługiwany piórem.

### Z11. Narzędzia v1: pióro, gumka, kilka kolorów
Nic więcej. Bez zakreślacza, bez lassa, bez obrazków.

Konsekwencje:
- renderer potrzebuje **jednej** warstwy atramentu, bez blendowania półprzezroczystego
  pod spodem — upraszcza to projekt cache'u kafli;
- operacja `StrokeTransform` nie jest w v1 potrzebna, więc **cały CRDT jest w v1
  czysto przemienny** i nie ma ani jednego miejsca z LWW (por. `adr/0004`).
  Format zostawia na nią miejsce, żeby lasso dało się dołożyć bez migracji;
- paleta kolorów dobierana pod AMOLED: niskie luminancje, brak czystej bieli.

## Nie-cele (świadomie poza zakresem v1)

- OCR / rozpoznawanie pisma odręcznego
- Import z Samsung Notes (`.sdocx`) — do rozważenia później, patrz `docs/05-ROADMAPA.md`
- Edycja tekstu bogatego, tabele, PDF-annotacje
- Android / iOS (rdzeń ma być przenośny, ale port to osobny etap)
- Chmura firm trzecich jako *jedyne* źródło prawdy

## Kryteria akceptacji v1

1. Rysowanie piórem nieodróżnialne w odczuciu od Samsung Notes na tym samym ekranie.
2. Hotkey → widoczne okno z notatką w <30 ms, mierzone.
3. 100 000 stroke'ów w notatce, pan/zoom nadal 120 fps.
4. Dwie maszyny po Tailscale rysują na jednej notatce; rozjazd wizualny <20 ms.
5. Wyłączenie zasilania w trakcie rysowania nie gubi więcej niż 1 s pracy.
6. Working set w tle ≤ 20 MB.
