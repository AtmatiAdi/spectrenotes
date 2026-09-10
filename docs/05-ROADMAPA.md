# Roadmapa

Kolejność jest podporządkowana jednej zasadzie: **najpierw rozstrzygamy rzeczy,
które mogą wywrócić projekt.** Odczucie pióra jest takim ryzykiem — jeśli na tym
digitizerze nie da się osiągnąć feelingu Samsung Notes, to reszta planu nie ma znaczenia.
Sync i UI są ryzykiem znanym i rozwiązywalnym, więc idą później.

## Etap 0 — Weryfikacja odczucia pióra  ← JESTEŚMY TUTAJ

`tools/inkdemo`, samodzielna binarka.

- [x] Okno Win32, borderless fullscreen, czarne tło
- [x] `WM_POINTER` + `GetPointerPenInfoHistory` + współrzędne himetric
- [x] Renderer wgpu, teselacja wstęgi, present immediate/tearing
- [x] HUD: Hz pióra, próbki/klatkę, czas klatki, nacisk, tilt
- [x] Przełączniki na żywo: interpolacja, wygładzanie, predykcja, vsync
- [ ] **Werdykt: czy to jest dobre? Pomiar kamerą 240 fps.**

Wyjście z etapu: świadoma decyzja, czy surowy `WM_POINTER` wystarcza, czy trzeba
schodzić do DirectComposition z niezależną warstwą mokrego atramentu.

## Etap 1 — Rdzeń dokumentu

- `spectre-proto`: format operacji, kodowanie próbek, CRC, wersjonowanie
- `spectre-core`: op-log CRDT, zegar Lamporta, undo/redo, kamera, hit-test gumki
- `spectre-ink`: krzywe nacisku, interpolacja centripetal, teselacja przyrostowa
- Testy własności: przemienność i idempotencja operacji (proptest)

## Etap 2 — Realny renderer

- Cache kafli 512×512, LRU, warstwa sucha vs mokra
- Pan/zoom z inercją, kryterium: 100 000 stroke'ów przy 120 fps
- Wybór adaptera GPU (MINIMUM_POWER + weryfikacja wyjścia)
- Pixel shift, rampa przygaszania, paleta AMOLED

## Etap 3 — Trwałość lokalna

- Zapis/odczyt `.ops`, snapshoty, przycinanie
- Odzyskiwanie po utracie zasilania (obcięcie uszkodzonego ogona)
- Indeks notatek, ładowanie leniwe
- **Kryterium: wyrwanie zasilania w trakcie rysowania gubi maksymalnie 1 s**

## Etap 4 — Powłoka i rezydentność

- Tray, `RegisterHotKey`, chowanie okna zamiast zamykania
- `Trim()` + `EmptyWorkingSet` przy ukryciu
- **Kryteria: <30 ms hotkey→klatka, ≤20 MB working set w tle**
- Minimalne UI: przełącznik narzędzi, lista notatek, wszystko auto-chowane

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

Import z Samsung Notes (`.sdocx`), OCR, PDF, port na Androida (tablet + S Pen),
szyfrowanie at-rest, tryb prezentacji.

## Otwarte problemy

- **SmartScreen.** Niepodpisana binarka dostanie ostrzeżenie u każdego, komu ją dasz.
  Certyfikat OV to koszt rzędu kilkuset zł rocznie, self-signed nie usuwa ostrzeżenia.
  Do rozstrzygnięcia dopiero przy etapie 7 — nie blokuje niczego wcześniej.
- **Multi-GPU przy doku.** Zachowanie przy przenoszeniu okna między iGPU a RTX 4080
  wymaga testu na realnym sprzęcie, nie da się tego zaplanować z góry.
