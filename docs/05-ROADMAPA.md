# Roadmapa

Kolejność jest podporządkowana jednej zasadzie: **najpierw rozstrzygamy rzeczy,
które mogą wywrócić projekt.** Odczucie pióra jest takim ryzykiem — jeśli na tym
digitizerze nie da się osiągnąć feelingu Samsung Notes, to reszta planu nie ma znaczenia.
Sync i UI są ryzykiem znanym i rozwiązywalnym, więc idą później.

## Etap 0 — Weryfikacja odczucia pióra  ✔ ZAMKNIĘTY

`tools/inkdemo`, samodzielna binarka.

- [x] Okno Win32, borderless fullscreen, czarne tło
- [x] `WM_POINTER` + `GetPointerPenInfoHistory` + współrzędne himetric
- [x] Renderer Direct2D, kapsuły per segment, present immediate/tearing
- [x] HUD: Hz pióra, próbki/klatkę, czas klatki, nacisk, tilt
- [x] Przełączniki na żywo: interpolacja, wygładzanie, predykcja, vsync
- [x] **Werdykt: 266 Hz próbkowania, odczucie ocenione jako dobre.** Przycisk boczny
      i gumka raportowane przez sterownik ELAN — nawigacja rysikiem potwierdzona.
- [ ] Pomiar kamerą 240 fps — do zrobienia przy okazji, nie blokuje

Wynik: surowy `WM_POINTER` + flip-model wystarcza. DirectComposition niepotrzebne.
Decyzja pochodna: renderer produkcyjny to Direct2D (ADR 0005), nie wgpu.

## Etap 1 — Rdzeń dokumentu  ✔ ZAMKNIĘTY

- [x] `spectre-proto`: format operacji, kodowanie próbek (~7 B/próbkę), CRC, wersjonowanie
- [x] `spectre-core`: op-log CRDT, zegar Lamporta, undo/redo, kamera, hit-test gumki
- [x] `spectre-ink`: krzywe nacisku, interpolacja centripetal, podział na czubek i część domkniętą
- [x] Testy własności: przemienność (losowe permutacje, 3 autorów) i idempotencja (proptest)

## Etap 2 — Realny renderer  ◐ W TOKU

- [x] Direct2D na DXGI flip-model (ADR 0005), warstwa sucha vs mokra
- [x] Przewijanie przyrostowe: przesunięcie bitmapy + dorysowanie odsłoniętego pasa
- [x] Wybór adaptera GPU (MINIMUM_POWER) — potwierdzone: Intel Arc, nie RTX
- [x] Paleta AMOLED (6 kolorów, bez czystej bieli)
- [x] Zoom `Ctrl+kółko` wokół kursora (kamera + UI)
- [x] Obrys kreski jako `ID2D1GeometryRealization` budowany raz i cache'owany:
      rebuild 453 widocznych kresek 257 → 3,8 ms (ciepły), repaint 6,4 → 0,5 ms,
      scroll 3,4 → 1,4 ms; zimny rebuild ~0,4 ms/kreskę (podłoga teselacji D2D,
      płacona raz — przy otwarciu notatki). Pen-up przerysowuje prostokąt kreski
      z tej samej geometrii; undo/redo to repaint regionu, nie pełny rebuild
- [ ] Cache kafli — dopiero gdy przewijanie przyrostowe okaże się za wolne
- [x] Pixel shift: ±4 px po torze Lissajous (61 s / 89 s), krok co 4 s tylko w bezczynności
      rysika, całe piksele; treść paska tytułowego dryfuje razem z canvasem
- [x] Rampa przygaszania: po 3 min bez wejścia płynnie (2 s) do 40 %, powrót przy
      pierwszym ruchu; czarna warstwa w `present`, nie jasność panelu. Timery gasną
      przy ukryciu okna (Z2: zero wybudzeń w tle)
- [ ] Szerokość kolumny (Z9) w jednostkach canvasu i zachowanie przy zmianie zoomu
- [ ] Kryterium: 100 000 stroke'ów przy 120 fps — do zmierzenia
- [ ] Weryfikacja wyjścia okna przy przenoszeniu na monitor z dGPU

## Etap 3 — Trwałość lokalna  ◐ W TOKU

- [x] Zapis/odczyt `.ops`, chunki per autor z rolowaniem po 2 MB
- [x] Odzyskiwanie po utracie zasilania (obcięcie uszkodzonego ogona) — test w `opsfile.rs`
- [x] Space = katalog, notatka = katalog ULID, lista i tworzenie notatek
- [x] `fsync` po 400 ms ciszy — **kryterium 1 s spełnione konstrukcyjnie**
- [ ] Snapshoty i przycinanie HEAD (potrzebne dopiero przy dużych notatkach)
- [x] Tytuł i folder notatki w `Meta` (jedna reguła LWW dla obu; UI: pasek tytułowy i menu)

## Etap 4 — Powłoka i rezydentność  ◐ W TOKU

- [x] Tray (klik = pokaż/ukryj, prawy = menu), `Win+Shift+N`, `Esc`/zamknięcie chowa zamiast kończyć
- [x] `Trim()` + `EmptyWorkingSet` przy ukryciu
- [x] **Zmierzone: 2 MB w tle (cel ≤20), 20,1 ms hotkey→klatka (cel <30)**
- [x] Pasek na lewej krawędzi, rysowany w D2D, auto-chowany po 2,5 s: pióro, gumka,
      kolory, grubość, undo/redo, poprzednia/następna/nowa notatka
- [x] Tytuł notatki: tap w nagłówek, edycja z klawiatury, `Enter`/`Esc`
- [x] Zoom `Ctrl+kółko` wokół kursora
- [x] Okno bez systemowej ramki (WM_NCCALCSIZE/NCHITTEST): własny pasek tytułowy
      z tytułem notatki i przyciskami okna; snap i Win+strzałki zachowane
- [x] Pasek narzędzi dokowalny do 4 krawędzi (przeciąganie za uchwyt), orientacja
      pozioma dla góra/dół; dok i położenie okna w `config.txt`
- [ ] Więcej narzędzi (zaznaczanie, kształty) — po Etapie 5, wymaga `StrokeTransform`
- [x] Odrzucanie `PT_TOUCH` na wejściu (Z10), przewijanie przyciskiem bocznym rysika
- [x] Menu (☰ w pasku tytułowym i na pasku narzędzi, `M`): lista notatek w folderach
      (folder = `Meta` notatki, LWW; cache `<space>/.cache/meta`), przenoszenie między
      folderami, nowa notatka/folder; zakładki Ustawienia i Konto jako wydmuszki
      (ustawienia przełączają to, co już jest; logowanie czeka na Etap 5)
- [ ] Autostart z systemem (klucz Run) — dopiero gdy stabilne
- [x] Pixel shift i rampa przygaszania (Z7) — patrz Etap 2

## Etap 5 — Git jako warstwa trwała

- `gix`: init, commit, fetch, merge, push
- Space = prywatne repo GitHub, uwierzytelnianie
- Automatyczny commit na idle, kolejka offline
- Odtworzenie stanu z historii, przeglądarka wersji notatki

## Etap 6 — Realtime P2P

- QUIC (`quinn`) po Tailscale, discovery mDNS w LAN
- Strumień operacji + kanał obecności (kursor drugiej osoby)
- Dołączenie w trakcie sesji (snapshot + ogon)
- **Kryterium: dwie maszyny, rozjazd wizualny <20 ms**

## Etap 7 — Dystrybucja

- Updater z GitHub Releases, atomowa podmiana przy restarcie
- Podpis kodu (patrz niżej — otwarty problem)
- Instrukcja dołączenia kogoś do space'u

## Poza v1

**Import z Samsung Notes (`.sdocx`)** — pierwszy na liście po v1. Nie blokuje
przesiadki: nowe notatki powstają w SpectreNotes, stare zostają do wglądu tam,
gdzie są. Format to zip z XML i binarnymi stroke'ami, więc jest wykonalny,
ale to praca na osobny etap i nie ma powodu robić jej przed działającą aplikacją.

Dalej: zakreślacz i lasso (`StrokeTransform` + LWW), wstawianie obrazków
(content-addressed store, pytanie o Git LFS), OCR, eksport PDF, port na Androida
(tablet + S Pen), szyfrowanie at-rest, tryb prezentacji.

## Otwarte problemy

- **SmartScreen.** Niepodpisana binarka dostanie ostrzeżenie u każdego, komu ją dasz.
  Certyfikat OV to koszt rzędu kilkuset zł rocznie, self-signed nie usuwa ostrzeżenia.
  Do rozstrzygnięcia dopiero przy etapie 7 — nie blokuje niczego wcześniej.
- **Multi-GPU przy doku.** Zachowanie przy przenoszeniu okna między iGPU a RTX 4080
  wymaga testu na realnym sprzęcie, nie da się tego zaplanować z góry.
