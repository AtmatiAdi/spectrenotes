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
- [x] **Mokra kreska = zatwierdzona kreska** (16 IX 2026). Od wprowadzenia realizacji
      mokra kreska szła `DrawLine` per odcinek: każda kapsuła antyaliasowana osobno,
      sąsiednie zachodziły na piksele krawędzi 2–3 razy i alfa się sumowała — kreska
      w trakcie była ~0,5 px grubsza niż po puszczeniu i „chudła" przy oderwaniu
      rysika. Teraz mokra kreska to ta sama wstęga (`geometry::build`) co w dokumencie:
      odcinki ostateczne wypalane do warstwy suchej porcjami po 96 (`WET_BURN_SEGS`),
      reszta + czubek rysowana co klatkę jako jedna figura. Zmierzone: 1,24 → 0,88 px
      przed, 1,0 = 1,0 po; gruba 3,02 → 2,51 przed, 3,0 = 3,0 po
- [x] **Cienkie kreski bez realizacji** (`d2d::Shape`): realizacja D2D antyaliasuje
      obwódką mesha i poniżej ~1,5 px na ekranie obwódki obu krawędzi nachodzą na siebie
      (kreska 0,94 px dawała 1,06, a na 60-px odcinku przed łukiem 1,53 — geometria
      była poprawna, sprawdzone testem: winding 1, bez samoprzecięć). Kreski cieńsze niż
      `THIN_PX` idą `FillGeometry` (pokrycie analityczne, dokładne) z luźniejszym
      uproszczeniem 0,15 px. Koszt: rebuild 456 widocznych cienkich kresek 9,5 → 46 ms
      (tylko zoom/resize); scroll 0,8 ms, repaint 1,5 ms bez zmian. Bench ma przypadek
      „grubość 1.2" i czyści cache między przypadkami (wcześniej mierzył stary cache)
- [ ] Zoom bez pełnego rebuildu co ząbek kółka: podgląd skalowaniem warstwy suchej,
      rebuild po ustaniu gestu — zdejmie z zoomu zarówno 46 ms cienkich, jak i zimne
      budowanie obrysów (0,7 s na gęstej stronie przy każdym kroku 1,5×)
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
      „dopasuj szerokość" = 1,0). Notatka otwiera się dopasowana do okna; zmiana rozmiaru
      okna zoomu nie zmienia (mniejsze okno = mniej widocznego canvasu), `0` dopasowuje
      na żądanie. Kolumna węższa niż okno jest wyśrodkowana; szersza — przewijana
      w poziomie przyciskiem bocznym (zakres poszerzony o treść poza kolumną, żeby
      wszystko było osiągalne)
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
- [x] Okno bez systemowej ramki (WM_NCCALCSIZE/NCHITTEST): **canvas od samej góry**,
      tytuł notatki i przyciski okna w dwóch zakładkach nad canvasem; pasek narzędzi
      zadokowany u góry wchłania oba pola; snap i Win+strzałki zachowane
- [x] Pasek narzędzi dokowalny do 4 krawędzi (przeciąganie za uchwyt), orientacja
      pozioma dla góra/dół; dok i położenie okna w `config.txt`. Uchwyt siedzi na
      **samym końcu paska**, za kłódką komputera (2026-09-15) — na początku, przy
      menu, łapał się przypadkiem; w doku u góry koniec paska trzyma zapas od
      uchwytu przesuwania okna, żeby dwa ⠿ nie stały obok siebie
- [ ] Więcej narzędzi (zaznaczanie, kształty) — po Etapie 5, wymaga `StrokeTransform`
- [x] Odrzucanie `PT_TOUCH` na wejściu (Z10), przewijanie przyciskiem bocznym rysika
- [x] Menu (☰ na pasku narzędzi, `M`) z nagłówkiem konta (avatar z GitHuba, nazwa,
      wiek synchronizacji, przycisk sync): lista notatek w folderach (folder = `Meta`
      notatki, LWW; cache `<space>/.cache/meta`), przenoszenie między folderami,
      nowa notatka/folder; Ustawienia i Konto działają (Etap 5)
- [x] Autostart z systemem: wpis w kluczu `Run` użytkownika z `--tray` (start schowany do traya,
      okno na `Win+Shift+N`); rejestr jest źródłem prawdy, przełącznik w Ustawienia → System
- [x] Ochrona AMOLED po bezczynności (Z7): fale przyciemnienia — patrz Etap 2
- [x] Ustawienia ochrony AMOLED: **przełącznik włącz/wyłącz** całej ochrony oraz
      **tylko na ekranie laptopa** — panel wbudowany rozpoznawany po typie złącza
      (`QueryDisplayConfig`, `OUTPUT_TECHNOLOGY_INTERNAL`), nie po „monitor główny";
      na zewnętrznym odliczanie trwa, ochrona startuje po przeniesieniu okna.
      Zweryfikowane: na `DISPLAY1` (panel) fale startują, na `DISPLAY6` nie
- [x] **Współpraca ze Spectre** (osobne repo `C:\Projects\Spectre`): widoczna notatka na
      panelu = prośba o wstrzymanie czarnej nakładki Spectre, jako **dzierżawa** 30 s
      odnawiana co 10 s (`RegisterWindowMessage("Spectre.ShieldHold")` → okno
      `SpectreShield`); `0` zwalnia przy ukryciu/zamknięciu. Zabity SpectreNotes nigdy
      nie zostawia panelu bez ochrony. Zweryfikowane end-to-end (log Spectre + jasność
      panelu). Spectre: `BEHAVIOR.md` S10, D-11
- [x] Bez okna konsoli (`windows_subsystem = "windows"`); `--console` i tryby wierszowe
      dołączają się do konsoli terminala
- [x] **Jedna instancja na space** (2026-09-16): drugie uruchomienie pokazuje okno
      działającej (także z traya; `--tray` przy działającej kończy się po cichu),
      inny space = osobny proces, `--new-instance` dla debugowania i testów.
      Rozpoznanie po własności okna (`SetPropW`/`GetPropW`, skrót ścieżki space'u),
      bez muteksów i IPC; `spectre-shell-win::instance`
- [x] **Interfejs po angielsku** — menu, HUD, komunikaty synchronizacji i błędów;
      dokumentacja i komentarze po polsku. Bez warstwy i18n (jeden język, prosto)

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
      wylogowanie. Wymaga `client_id` aplikacji OAuth (`github.rs::CLIENT_ID`).
      Logowania hasłem GitHub już nie obsługuje — Device Flow jest odpowiednikiem
- [x] **Avatar konta** w nagłówku menu: pobrany raz z GitHuba (WIC → BGRA, pędzel
      bitmapowy D2D), zapisany w `avatar.img`, po restarcie z cache (bez ruchu sieci)
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
      — **po Etapie 6½**: jej kształt zależy od decyzji o GUI
- [ ] Snapshoty i przycinanie HEAD — razem z Etapem 3

## Etap 6 — Realtime P2P  ◐ DZIAŁA W LAN

- [x] Transport **TCP + `TCP_NODELAY`** z wykrywaniem multicastem w LAN (ADR 0007;
      nie QUIC/`quinn` — tokio+rustls bez powodu, nie mDNS — beacon `SPCTLV1` co 2 s).
      Łączy instancja o mniejszym id; rozgłaszanie tylko przy widocznym oknie
- [x] **Mokra kreska u drugiej osoby w trakcie rysowania**: paczka próbek na każdy
      komunikat pióra, odbiorca prowadzi ten sam `StrokeBuilder`; rysik peera jako
      kropka. Ustawienie *Sieć lokalna → Rysowanie na żywo* (`live=` w `config.txt`)
- [x] Dołączenie w trakcie: `Summary` (notatka, autor → ostatni lamport) i różnica;
      po merge'u gita dosłanie peerom tego, co przyszło z GitHuba
- [x] Zdalne operacje w plikach `via-<odbiorca>-*.ops` — jeden pisarz na plik,
      merge gita nadal bezkonfliktowy; instancja bez logowania ma kopię przez zalogowaną
- [x] **Kryterium <20 ms: spełnione z zapasem** — dwie instancje na jednej maszynie
      (różny `COMPUTERNAME`), mokra kreska 0,08–0,3 ms od wysłania do odbioru (HUD),
      kreska widoczna u drugiej strony w trakcie (zrzuty co 150 ms). Dwie fizyczne
      maszyny w LAN — do potwierdzenia na sprzęcie (Windows Firewall musi przepuścić
      `spectrenotes.exe`; prompt przy pierwszym starcie)
- [x] **Udostępnianie per notatka, z hasłem lub bez (ADR 0008)** — protokół v2:
      połączenie nic nie replikuje, po `Hello` lista `Shared`, notatka płynie dopiero
      po `Open` → `Opened(ok)`; dowód hasła SHA-256 (klucz pochodny + nonce'y z `Hello`),
      hasło nie idzie siecią ani na dysk. Nazwa space'u bez znaczenia. Minimum GUI:
      *Notatki → This note on LAN* (udostępnij, hasło) i *Shared on LAN* (cudze, kłódka,
      otwórz/zamknij). `lan-<space>.txt` w danych aplikacji. Zweryfikowane dwiema
      instancjami: złe hasło odrzucone, dobre otwiera, kreska A u B w trakcie
- [x] Peer przez Tailscale bez multicastu: adresy `host:port` w *Konto → Sieć lokalna*,
      węzeł łączy się sam i ponawia co 10 s (`Job::Peers`); deduplikacja po instancji
- [x] Heartbeat: `Ping` po 5 s ciszy, zerwane po 12 s (test z milczącym klientem)
- [ ] Szyfrowanie treści bez Tailscale (dziś: hasło chroni przed otwarciem, nie przed
      podsłuchem — jawny TCP w LAN)
- [ ] Przycinanie `via-*`, gdy plik autora w gicie już to pokrywa (razem ze snapshotami)
- [ ] GUI udostępniania docelowo (Etap 6½): kto ma notatkę otwartą, lista peerów,
      udostępnianie z listy notatek (nie tylko bieżącej), nazwy zamiast `user@computer`

## Etap 6½ — Określenie wymagań i zmiany GUI  ☐ DO SPISANIA Z UŻYTKOWNIKIEM

Rdzeń (pióro, format, git, live) stoi; **brakuje wymagań dla sporej części
funkcji użytkowych** — do tej pory GUI rosło przyrostowo, funkcja po funkcji.
Zanim dojdzie przeglądarka wersji i kolejne narzędzia, jedna sesja na spisanie,
co aplikacja ma robić i jak ma wyglądać. Wynik: aktualizacja `00-ZALOZENIA.md`
(nowe Z-ki albo doprecyzowanie istniejących) i lista zmian GUI tutaj.

Konkretne zgłoszenia (do zrobienia niezależnie od reszty):
- [x] **Stan synchronizacji w menu, obok informacji „synced"**: nagłówek pokazuje
      `synced 4:37 ago · 12:04:11` (bez logowania: `saved …`) — wiek co do sekundy,
      **tykający na żywo** (timer 1 s tylko przy otwartym menu), plus dokładna
      godzina. Zakładka Account, sekcja Sync, rozdziela trzy zegary: `saved to disk`,
      `on GitHub`, `with LAN peer` (`sync::Mark` = Instant + unix s). Zweryfikowane
      zrzutami: 0:00 → 0:03 po 3 s, ta sama godzina; dwie instancje — wiersz peera
      wypełnia się po wymianie operacji
- [x] **Pasek jako pasek tytułowy** (2026-09-15): puste miejsce paska w każdym doku
      przesuwa okno (`HTCAPTION`); drugi uchwyt ⠿ przy przyciskach okna.
- [x] **Panel menu omija pasek narzędzi** (2026-09-15): z lewej staje obok niego,
      u góry/u dołu za nim; ☰ zostaje klikalny (drugie dotknięcie zamyka), a otwarte
      menu trzyma pasek na ekranie — widać, co robią ustawienia doku i przypięcia.
      Miniatura dostała też zapas nad treścią: kreska leżąca dokładnie na górnej
      krawędzi treści wypadała za kadrem i kafelek wyglądał na pusty.
- [x] **Przycisk pełnego ekranu ⛶** przy minimalizuj/maksymalizuj/zamknij
      (2026-09-15). W pełnym ekranie zakładki (tytuł i przyciski) chodzą razem
      z paskiem — wjeżdżają przy krawędzi, znikają po bezczynności — żeby dało się
      z niego wyjść bez `F11`; uchwyty przesuwania okna są tam wygaszone.
- [x] **Pasek zadań pod oknem** (2026-09-15): zgłoszone jako „małe okno zasłania pasek
      zadań". Przyczyna w Spectre — przyciemniacz paska jest oknem własnościowym
      `Shell_TrayWnd` i podnosił się skokiem przez `HWND_NOTOPMOST`, co zdejmowało
      topmost także właścicielowi; naprawione tam (`HWND_TOP`). U nas: wejście
      i wyjście z pełnego ekranu melduje się powłoce przez
      `ITaskbarList2::MarkFullscreenWindow`, więc pasek ustępuje od razu, a nie po
      heurystyce. Zmierzone: z działającym Spectre pasek trzyma `WS_EX_TOPMOST`,
      a okno przesunięte na jego pas już go nie zakrywa.
- [x] **Pełny ekran a maksymalizacja** (2026-09-15): pełny ekran jest trybem nad
      stanem okna. Wejście zapamiętuje `WINDOWPLACEMENT` (a więc i maksymalizację,
      i prostokąt przywrócenia), wyjście oddaje dokładnie ten stan; przycisk
      maksymalizacji w pełnym ekranie najpierw z niego wychodzi i pokazuje stan,
      do którego wróci; do `config.txt` idzie położenie sprzed pełnego ekranu.
      Zmierzone: zmaksymalizowane → F11 → F11 wraca na -9,-9-2889,1749
      (`IsZoomed`), ❐ w pełnym ekranie daje z powrotem okno 1400x1000.
- [x] **Kłódka widoku**: zablokowany = kolumna wyśrodkowana (bezwzględny środek
      notatki), zoom wolno, scroll w bok nie; odblokowany = canvas nieskończony
      w osi X. Naprawia „odblokowanie" scrolla w bok przez rysowanie po oddaleniu.
      Zweryfikowane: ta sama kropka po przeciągnięciu zostaje (locked) / przesuwa
      się o 200 px (unlocked).
- [x] Lupa −/+ na pasku (zoom wokół środka okna), procent = dopasuj szerokość.
- [x] Grubość pióra co 0,1 px (pasek i `[` `]`).
- [x] Kłódka komputera (`LockWorkStation`, jak `Win+L`) na samym końcu paska.
- [x] Elementy paska kurczą się proporcjonalnie, gdy pasek jest krótszy niż one
      (900×700: wszystko widoczne, nic nie nachodzi na kłódkę).
- [x] Przyciski ▲▼＋ (notatki) zdjęte z paska (2026-09-15) — nawigacja przez menu,
      `PgUp/PgDn`, `Ctrl+N`.
- [x] Tytuł w pasku u góry: szerokość i środek jak zakładka nad canvasem, bez
      rozciągania na wolne miejsce; przy 2400 px środek napisu = środek okna.
- [x] UI skaluje się do DPI Windows (2026-09-15): wymiary w `ui.rs`/`menu.rs` są
      w pikselach logicznych (96 DPI), a `layout` liczy z nich fizyczne piksele
      raz, mnożąc przez skalę monitora (`window::dpi_scale`, `WM_DPICHANGED`
      przelicza po przeniesieniu okna). Nic nie jest skalowane w trakcie
      rysowania — każdy prostokąt i glif ma policzone piksele (wydajność,
      ostrość), czcionki DWrite tworzone w docelowym rozmiarze. Canvas z tym nie
      ma nic wspólnego (kreski w swoich jednostkach). Grubość paska = pasek zadań
      Windows (`BAR_THICK` 48 log.), panele tytułu/kontrolek o ~10 % mniejsze niż
      w pierwszym podejściu (`TAB_H` 36, `WIN_BTN_W` 50). Przyciski okna znów w
      prawym górnym rogu (błąd: układ w pikselach zamiast logicznych wypychał je
      do ~3/4 szerokości).
- [x] Wejście testowe bez prawdziwej myszy: `SPECTRENOTES_TEST_INPUT=1` +
      `WM_COPYDATA` z tekstem `down X Y [barrel|eraser]` / `move X Y` / `up` /
      `hover X Y`; skrypt testu nie rusza kursora ani fokusu użytkownika.

- [x] Ochrona AMOLED liczy bezczynność całego systemu (2026-09-15, bug): okno
      nieaktywne nie widzi wejścia, więc fale i pełny ekran wchodziły podczas
      pisania w innej aplikacji. `GetLastInputInfo` + `waves_delay`.

- [x] Lista notatek jako kafelki z miniaturami (2026-09-15): miniatura strony
      z tytułem i datą na pasku; renderowane raz do bitmapy (`Renderer::build_thumb`,
      `UiPrim::Thumb`), budowane po kolei z budżetem na klatkę, bo wczytanie
      cudzej notatki to odczyt z dysku. Notatki z LAN mają te same kafelki,
      miniaturę po otwarciu (wcześniej nie mamy treści).
      Od 2026-09-16 budowane **od startu w tle** (ten sam timer i budżet,
      rysik w zasięgu wstrzymuje), nie przy pierwszym otwarciu menu — 12 notatek
      gotowych 260 ms po starcie. Bieżąca notatka odświeża się po lamporcie przy
      otwarciu menu; notatki zmienione przez sync i opuszczona notatka rysowana
      przy zamkniętym menu wracają do kolejki.

Do rozstrzygnięcia (lista otwarta — dopisywać):
- [ ] Menu i nawigacja: czy panel boczny zostaje, co z zakładkami nad canvasem,
      gdzie ląduje przeglądarka wersji, jak wygląda przenoszenie notatek i foldery
- [ ] Narzędzia rysowania (Z11 vs „Więcej narzędzi" z Etapu 4): zaznaczanie, kształty,
      zakreślacz, lasso — które w v1, jak wybierane rysikiem
- [ ] Praca z wieloma osobami (Etap 6): jak pokazywać, kto jest w notatce, kolory
      autorów, „podążaj za drugą osobą", co z kursorem na innej notatce
- [ ] Ustawienia: które trwałe (`config.txt`), które na skróty klawiszowe, czy HUD
      zostaje jako narzędzie diagnostyczne
- [ ] Notatki: szablony tła (linie, kratka), rozmiar/kolumna, eksport, kosz/archiwum
- [ ] Sprzęt: zachowanie na zewnętrznym monitorze (ochrona AMOLED już tylko na panelu),
      dok/undock, tryb tabletu (klawiatura schowana)
- [ ] Onboarding: pierwszy start, logowanie, pierwszy space, druga maszyna

## Etap 7 — Dystrybucja

- [x] Wydania: `release.ps1` (wersja → testy → build → tag → GitHub Release, 3 ostatnie)
- [x] Instalator `SpectreNotes-Setup.exe` (pobiera najnowsze wydanie, SHA-256, `--install`)
- [x] Updater w aplikacji: sprawdzanie w tle, Download z postępem, „Install and restart"
      (podmiana przez rename działającej binarki — 06-DYSTRYBUCJA)
- [x] Publiczne repozytorium wydań `spectrenotes-releases` (instalacja i aktualizacje bez tokenu)
- Podpis kodu (patrz niżej — otwarty problem)
- Instrukcja dołączenia kogoś do space'u

## Poza v1

**Import z Samsung Notes (`.sdocx`)** — pierwszy na liście po v1. Nie blokuje
przesiadki: nowe notatki powstają w SpectreNotes, stare zostają do wglądu tam,
gdzie są. Format to zip z XML i binarnymi stroke'ami, więc jest wykonalny,
ale to praca na osobny etap i nie ma powodu robić jej przed działającą aplikacją.

Dalej: zakreślacz i lasso (`StrokeTransform` + LWW), wstawianie obrazków
(content-addressed store, pytanie o Git LFS), OCR, eksport PDF, szyfrowanie
at-rest, tryb prezentacji.

**Opcjonalnie: port na Androida (Samsung, S Pen).** Tylko Android — Apple odpada
(decyzja właściciela: konto deweloperskie, App Store, build na macOS). Stan
wyjściowy jest dobry: `spectre-proto`, `spectre-core`, `spectre-ink` i prawie cały
`spectre-sync` (git, budżet, live TCP, udostępnianie z hasłem) nie mają zależności
od Windows — to ~6,5 tys. linii rdzenia, które kompilują się na `aarch64-linux-android`
bez zmian (wyjątek: backend TLS libgit2 — na Androidzie inna feature flaga zamiast
WinHTTP). Do napisania na platformę: renderer (Vulkan/Skia zamiast Direct2D),
wejście S Pen (`MotionEvent` z naciskiem, tiltem i `getHistorical*` — odpowiednik
`GetPointerPenInfoHistory`), powłoka (Kotlin, FFI przez `uniffi`), z celem niskiej
latencji (`SurfaceView`, front-buffered rendering Samsunga). Ochrona AMOLED zbędna
(system ma własną). Kolejność, gdyby to ruszyło:
1. Przy okazji Etapu 6½ wyciągnąć z `spectre-app` przenośną logikę (menu, toolbar,
   kamera, stan aplikacji — dziś posplataną z `HWND`, `PostMessage`, timerami,
   `GetLocalTime`) do crate'u `spectre-ui` bez `windows::`; w `spectre-app` zostaje
   sam Win32. Tanie, porządkuje kod niezależnie od portu.
2. Rdzeń + `spectre-ui` jako biblioteka `.so` z bindingami `uniffi`.
3. Powłoka Android: renderer, S Pen, okno; pomiar latencji jak w Etapie 0.
Nie zaczynać przed domknięciem v1 na Windows i decyzjami GUI z 6½ — port skopiuje
każdą decyzję, która jeszcze nie zapadła.

## Otwarte problemy

- **SmartScreen.** Niepodpisana binarka dostanie ostrzeżenie u każdego, komu ją dasz.
  Certyfikat OV to koszt rzędu kilkuset zł rocznie, self-signed nie usuwa ostrzeżenia.
  Do rozstrzygnięcia dopiero przy etapie 7 — nie blokuje niczego wcześniej.
- **Multi-GPU przy doku.** Zachowanie przy przenoszeniu okna między iGPU a RTX 4080
  wymaga testu na realnym sprzęcie, nie da się tego zaplanować z góry.
