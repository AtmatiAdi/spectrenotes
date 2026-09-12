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

## Etap 2 — Realny renderer  ◐ PRAWIE ZAMKNIĘTY (zostaje test dGPU)

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
- [~] Cache kafli — niepotrzebny: po realizacjach geometrii i indeksie pasów
      przewijanie przyrostowe mieści się w budżecie z dużym zapasem
- [x] Ochrona AMOLED po bezczynności: **fale lokalnego przyciemnienia** (`amoled.rs`) —
      skośne, sfalowane pasy ciemności (3 warstwy pod różnymi kątami, jak połysk
      zaczarowanego przedmiotu w Minecrafcie) płyną przez ekran i gaszą notatkę do zera
      tylko pod sobą; maska liczona na CPU co 12 px, rysowana jedną bitmapą. Każde
      wejście gasi je natychmiast (hover dopiero po ruchu ≥ 12 px), `W` włącza na stałe
      (podgląd), czas bezczynności w ustawieniach 10 s – 10 min, jasność między pasami
      w ustawieniach. Na czas ochrony: UI schowane, pełny ekran (TOPMOST), po wejściu
      powrót. Timer gaśnie przy ukryciu okna. Zweryfikowane zrzutami ekranu (skrypt: start po 10 s, gaszenie
      klawiszem, powrót po 10 s; pokrycie 93 % próbek w 2 min). Pixel shift, globalna
      rampa i plamy z gradientem radialnym **odrzucone po teście**
- [x] Szerokość kolumny (Z9): `COLUMN_W = 2880` jednostek (na docelowym panelu zoom
      „dopasuj szerokość" = 1,0). Domyślnie dopasowanie do okna, trzymane przy zmianie
      rozmiaru do pierwszego ręcznego zoomu; `0` wraca. Kolumna węższa niż okno jest
      wyśrodkowana; szersza — przewijana w poziomie przyciskiem bocznym (zakres
      poszerzony o treść poza kolumną, żeby wszystko było osiągalne)
- [x] **Kryterium 100 000 kresek: spełnione.** Indeks pasów po Y w `Document`
      (`visible_in` czyta kilka pasów zamiast skanować wszystko — skan kosztował ~5 ms
      na klatkę i dominował nad rysowaniem). Bench: rebuild 1,6 ms, scroll +2 px
      0,014 ms, repaint 0,009 ms przy 100 000 kresek (~190 widocznych)
- [x] Cache realizacji geometrii czyszczony przy zmianie notatki — `StrokeId` (autor,
      licznik) jest unikalny tylko w obrębie notatki; bez tego F11 rysował białe pasy
      z notatki testowej w innych notatkach (odtworzone i naprawione, zrzuty)
- [x] Pełny ekran = `HWND_TOPMOST`: przy kilku monitorach klik w inny ekran nie wyciąga
      paska zadań nad notatkę (do potwierdzenia na sprzęcie)
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
- [x] Ochrona AMOLED po bezczynności (Z7): fale przyciemnienia — patrz Etap 2

## Etap 5 — Git jako warstwa trwała  ◐ PRAWIE ZAMKNIĘTY

- [x] Backend `spectre-sync::git` na **`libgit2` w binarce** (ADR 0006; nie `gix`: brak
      push i uwierzytelniania; nie proces `git`: zależność od instalacji): init, commit,
      fetch, merge niezależnych historii z `FileFavor::Union` dla `folders.txt`, push,
      historia notatki, klasyfikacja błędów (limit ruchu / token / inne). Test: dwa
      „urządzenia" przez lokalne repo bare — pliki obu autorów po obu stronach
- [x] Wątek sync w aplikacji (`spectre-app::sync`): commit 10 s po ostatniej zmianie,
      pełny cykl przy ukryciu i pokazaniu okna oraz na start; render nigdy nie czeka
- [x] **Repozytorium automatyczne**: `spectrenotes-<space>` wykrywane albo zakładane
      przez GitHub REST API po zalogowaniu; użytkownik nie widzi adresów
- [x] Logowanie GitHub Device Flow (kod + przeglądarka, token w DPAPI) albo wklejony PAT;
      wylogowanie. Wymaga `client_id` aplikacji OAuth (`github.rs::CLIENT_ID`)
- [x] **Budżet ruchu** (`spectre-sync::budget`): księga połączeń, odstęp min. 30 s,
      progi 120/h, 1200/dobę, 500 MB/dobę, predykcja dobowa; odmowa serwera → odczekanie
      10 min ×2 do 2 h z ostrzeżeniem w Konto i HUD, commity lokalne w tym czasie
- [x] Po merge: lista notatek i cache metadanych odświeżone, bieżąca notatka
      przeładowana (po zakończeniu kreski, jeśli trwa)
- [x] **Zweryfikowane end-to-end na GitHubie**: dwie instancje (różny `COMPUTERNAME`,
      ten sam space) — założenie repo, push, pobranie, merge niezależnych historii,
      fast-forward z powrotem
- [ ] Rejestracja aplikacji OAuth na GitHubie i wpisanie `CLIENT_ID` (właściciel projektu)
- [ ] Przeglądarka wersji notatki (`log_note` jest; brak UI i odtwarzania stanu z commita)
- [ ] Snapshoty i przycinanie HEAD — razem z Etapem 3

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
