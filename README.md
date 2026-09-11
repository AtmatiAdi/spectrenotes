# SpectreNotes

Notatnik rysunkowy dla Windows na ekrany AMOLED i pióra MPP 2.0.
Własny zamiennik Samsung Notes — bez zależności od cudzej chmury i cudzej decyzji
o tym, które komputery są wystarczająco właściwe, żeby uruchomić aplikację.

**Stan: Etapy 0–1 zamknięte, 2–4 w większości.** 266 Hz próbkowania, pozytywny werdykt
odczucia (tag `v0.1-latency-baseline`), trwałe notatki, pasek narzędzi, rezydentność
w tray'u: 2 MB w tle, 20 ms do klatki po hotkeyu (`docs/05-ROADMAPA.md`).

## Uruchomienie

```
.\dev.cmd -App      # SpectreNotes
.\dev.cmd -Demo     # demo odczucia pióra (Etap 0, narzędzie diagnostyczne)
```

`dev.cmd` (wrapper na `dev.ps1`) sprawdza toolchain, dodaje `cargo` do PATH sesji
i odpala wybraną binarkę. `.\dev.cmd -Install` doinstalowuje braki (rustup, VS Build
Tools), `.\dev.cmd -Persist` dopisuje `.cargo\bin` do PATH użytkownika na stałe.
Gdy `cargo` jest już w PATH: `cargo run --release -p spectre-app`.

### Aplikacja

Notatki leżą w `%APPDATA%\SpectreNotes\spaces\default` (inna ścieżka: pierwszy
argument). Każda kreska trafia do pliku autora natychmiast po zakończeniu,
`fsync` po 400 ms ciszy — wyrwanie zasilania gubi co najwyżej ostatnie pociągnięcie.

Aplikacja **żyje w tray'u**: `Esc`, zamknięcie okna i `Win+Shift+N` chowają ją
(w tle ~2 MB), `Win+Shift+N` lub klik w ikonę odsłania w ~20 ms. Zakończenie:
`Ctrl+Q` albo prawy klik na ikonie → Zakończ.

**Pasek narzędzi** wyjeżdża, gdy rysik zbliży się do lewej krawędzi, i chowa się
po 2,5 s bezczynności (Z7). Tytuł notatki: tap w nagłówek u góry, wpisz, `Enter`.

| Sterowanie | Działanie |
|---|---|
| pióro | rysowanie |
| rysik przy lewej krawędzi | pasek: pióro, gumka, kolory, grubość, undo/redo, notatki |
| `Ctrl+kółko` | zoom wokół kursora |
| **przycisk gumki (trzymany)** | gumka kresek — usuwa całe kreski, które dotknie |
| **przycisk boczny (trzymany)** | przewijanie |
| kółko myszy, `Home` | przewijanie, powrót na górę |
| `1`–`6` | kolor (paleta pod AMOLED) |
| `E` | gumka z klawiatury |
| `[` `]` | grubość |
| `Ctrl+Z` / `Ctrl+Y` | cofnij / ponów (cofnięcie wymazania odtwarza kreskę) |
| `PgUp` / `PgDn` | poprzednia / następna notatka |
| `Ctrl+N` | nowa notatka |
| `F11`, `H`, `V`, `T` | pełny ekran, HUD (domyślnie wyłączony), vsync, tryb przewijania |
| `Esc` / `Ctrl+Q` | ukryj do tray / zakończ |

Przyciski rysika działają w każdym momencie: wciśnięcie w trakcie kreski kończy
ją i od razu zaczyna nową rolę. Dotyk palcem jest ignorowany całkowicie (Z10).

### Demo (Etap 0)

Czarny canvas, rysuj piórem. `F11` = pełny ekran, `H` chowa HUD.

| Klawisz | Działanie |
|---|---|
| **przycisk boczny (trzymany)** | przewijanie — także w trakcie kreski: kończy ją i przewija |
| **przycisk gumki (trzymany)** | wymazywanie — także w trakcie kreski |
| kółko myszy | przewijanie |
| `Ctrl+Z` | cofnij ostatnią kreskę |
| `I` | interpolacja centripetal Catmull-Rom |
| `S` | filtr wygładzający 1-Euro (domyślnie wył.) |
| `P` | predykcja: 0 → 4 ms → 8 ms |
| `1` `2` `3` | nacisk→szerokość: stały / liniowy / gamma |
| `[` `]` | grubość pióra |
| `V` | vsync ↔ immediate + tearing |
| `C` | wyczyść |
| `F11` / `Esc` | pełny ekran / wyjście |

Odwrócenie pióra (gumka) przełącza na wymazywanie automatycznie. Dotyk palcem jest
ignorowany całkowicie (Z10). HUD pokazuje na żywo stan przycisku bocznego —
jeśli przy wciśniętym przycisku wciąż pokazuje `---`, sterownik nie raportuje
ani `PEN_FLAG_BARREL`, ani `POINTER_FLAG_SECONDBUTTON` i trzeba szukać innej drogi.

### Co obserwować

HUD pokazuje to, co realnie decyduje o odczuciu:

- **Hz pióra** — powinno być wyraźnie więcej niż odświeżanie ekranu (rzędu 180–240).
  Jeśli pokazuje ~120, znaczy że historia próbek nie działa i gubimy połowę danych.
- **próbki/komunikat** i **z historii** — dowód, że `GetPointerPenInfoHistory`
  faktycznie dokłada próbki ponad tę jedną z komunikatu.
- **wejście→present** — od znacznika czasu najnowszej próbki do wywołania `Present`.
  To jest ta część łańcucha latencji, na którą mamy wpływ.
- **GPU** — musi pokazywać iGPU. Jeśli pokazuje RTX, wybór adaptera jest zepsuty
  i notatnik wybudza dedykowaną kartę.

Bezwzględnej latencji pen-to-photon HUD nie zmierzy — to się robi kamerą 240 fps
(telefon), licząc klatki między dotknięciem rysika a pojawieniem się piksela.

### Porównania, które warto zrobić od razu

1. `S` włączony vs. wyłączony — czy wygładzanie faktycznie przeszkadza, czy tylko
   teoretycznie (założenie Z6 mówi, że przeszkadza; to jest test tego założenia).
2. `P` 0 vs. 8 ms — czy niższa odczuwalna latencja jest warta „haczyków" na zwrotach.
3. `V` vsync vs. tearing — czy różnica jednej klatki jest wyczuwalna przy pisaniu.
4. To samo w oknie i na pełnym ekranie — pełny ekran powinien być zauważalnie lepszy,
   bo DWM wypada z łańcucha prezentacji.

## Struktura

```
crates/
  spectre-proto      format .ops: varint, CRC32, kodowanie probek, odzysk po awarii  [dziala]
  spectre-core       op-log CRDT, zegar Lamporta, undo/redo, hit-test, kamera      [dziala]
  spectre-ink        probki -> krzywa nacisku -> interpolacja -> geometria         [dziala]
  spectre-sync       lokalny store (space/notatka/autor); git i QUIC: Etapy 5-6    [lokalnie]
  spectre-render     Direct2D na DXGI flip-model, przewijanie przyrostowe          [dziala]
  spectre-shell-win  okno Win32, WM_POINTER, DPI, feedback piora, pelny ekran      [dziala]
  spectre-app        binarka `spectrenotes`                                       [dziala]
tools/
  inkdemo            demo odczucia piora (Etap 0)                                 [dziala]
docs/                zalozenia, architektura, ADR-y
```

`spectre-proto`, `-core`, `-ink` i `-sync` nie mają żadnej zależności od Windows.
To jest cała przenośność rdzenia: port polega na napisaniu nowego `spectre-shell-*`
i `spectre-render-*` (ADR 0005).

Testy: `cargo test --workspace` — w tym testy własności (proptest) dla formatu
(roundtrip, obcięte bajty) i dla CRDT (przemienność dla losowych permutacji,
idempotencja).

## Dokumentacja

| Dokument | O czym |
|---|---|
| [`docs/00-ZALOZENIA.md`](docs/00-ZALOZENIA.md) | założenia produktowe, kryteria akceptacji |
| [`docs/01-ARCHITEKTURA.md`](docs/01-ARCHITEKTURA.md) | podział na crate'y, model wątków, rezydentność |
| [`docs/02-PIORO-I-LATENCJA.md`](docs/02-PIORO-I-LATENCJA.md) | łańcuch pen-to-photon, gdzie giną klatki |
| [`docs/03-FORMAT-I-SYNC.md`](docs/03-FORMAT-I-SYNC.md) | format `.ops`, model repo, warstwa live |
| [`docs/04-ENERGIA-I-AMOLED.md`](docs/04-ENERGIA-I-AMOLED.md) | pobór mocy, wypalanie, wybór GPU |
| [`docs/05-ROADMAPA.md`](docs/05-ROADMAPA.md) | kolejność prac i dlaczego taka |
| [`docs/06-DYSTRYBUCJA.md`](docs/06-DYSTRYBUCJA.md) | aktualizacje, rozdawanie, SmartScreen |
| [`docs/adr/`](docs/adr/) | decyzje architektoniczne wraz z odrzuconymi wariantami |

## Wymagania budowania

- Rust stable (MSVC), Windows 10 1809+ / Windows 11
- VS Build Tools z workloadem C++ oraz Windows SDK 10

## Licencja

MIT
