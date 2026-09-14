# Architektura

## Wybór stacku

**Rust + wgpu, powłoka Win32 przez `windows-rs`.**

| Wymaganie | Dlaczego Rust to spełnia |
|---|---|
| Z1 wydajność, Z5 zero-latency | brak GC → brak losowych pauz na ścieżce pióra; pełna kontrola nad alokacją |
| Z2 ≤20 MB w tle | brak runtime'u; statyczna binarka ~8–12 MB |
| Z3/Z4 sync i CRDT | `git2` (libgit2 w binarce, ADR 0006; `gix` odłożony), GitHub REST przez WinHTTP, live po TCP + multicast (`std::net`, ADR 0007; `quinn` odłożony), własne kodowanie (ADR 0004) |
| Z8 dystrybucja | jeden `.exe`, zero zależności u odbiorcy |
| przenośność rdzenia | `wgpu` → D3D12 dziś, Vulkan/Metal gdy przyjdzie Linux/Android |

Odrzucone: Electron (RAM i latencja dyskwalifikują), C# (GC na ścieżce atramentu),
C++ (te same wyniki, dużo wolniejszy rozwój pozostałych 80% aplikacji).
Pełne uzasadnienie: `docs/adr/0001-stack.md`.

## Podział na crate'y

```
crates/
  spectre-core      # model dokumentu, op-log CRDT, undo, geometria, kamera
  spectre-ink       # próbkowanie, krzywe nacisku, interpolacja, teselacja wstęgi
  spectre-render    # Direct2D na DXGI flip-model (ADR 0005), warstwa mokra/sucha, AMOLED
  spectre-sync      # git (libgit2, ADR 0006) + budzet ruchu + live P2P (TCP + multicast, ADR 0007; udostepnianie per notatka z haslem, ADR 0008)
  spectre-proto     # format on-disk i wire (jedno źródło prawdy dla obu)
  spectre-shell-win # okno Win32, WM_POINTER, tray, global hotkey, DXGI/DPI
  spectre-app       # binarka: spina wszystko, konfiguracja, updater
tools/
  inkdemo           # samodzielne demo do oceny odczucia pióra
```

Zależności idą **tylko w dół**:
```
spectre-app
   ├── spectre-shell-win ──┐
   ├── spectre-render ─────┤
   ├── spectre-sync ───────┼── spectre-core ── spectre-proto
   └── spectre-ink ────────┘
```
`spectre-core`, `-ink`, `-proto`, `-sync` **nie mają ani jednej zależności od Windows**.
To jest cała przenośność, o którą chodzi w Z-platformy: port = napisanie nowego
`shell-*` i `render-*` (renderer jest Windows-only od ADR 0005).

## Model wątków

```
┌─ Input thread ──────────────┐  własna pompa komunikatów, ABOVE_NORMAL
│  WM_POINTER + history       │  parsuje próbki, NIC nie renderuje
│  → lock-free ring buffer    │
└──────────┬──────────────────┘
           │
┌──────────▼─ Render thread ──┐  MMCSS "Games" tylko gdy rysujemy
│  drenuje ring, teseluje     │
│  mokry stroke → present     │
│  suchy stroke → tile cache  │
└──────────┬──────────────────┘
           │ (kanał, nigdy nie blokuje renderu)
┌──────────▼─ Sync thread ────┐  git: commit na idle, fetch/merge/push
│  co N s / na idle: git      │  (libgit2, sekundy - nigdy nie blokuje renderu)
└─────────────────────────────┘
           │
┌──────────▼─ Live thread ────┐  TCP do peerów (LAN / Tailscale); płyną tylko udostępnione
│                             │  i otwarte notatki (ADR 0007, 0008)
│  op-log → peers             │  mokra kreska co komunikat pióra,
│  peers → via-*.ops → okno   │  zdalne operacje na dysk i do dokumentu
└─────────────────────────────┘
```

Zasada nadrzędna: **żadna operacja I/O, sieciowa ani gitowa nie może dotknąć
wątku wejścia ani renderu.** Sync widzi tylko gotowe operacje przez kanał.

## Model dokumentu

Notatka to **nieskończony canvas** + uporządkowany log operacji.

```rust
enum OpKind {
    StrokeAdd { id: StrokeId, tool: Tool, color: Rgba, width: f32, samples: Vec<Sample> },
    StrokeErase { id: StrokeId },                    // tombstone
    StrokeTransform { id: StrokeId, xf: Affine },    // LWW po (lamport, author)
    PageMeta { .. },
}

struct Op {
    author:  AuthorId,   // (użytkownik, urządzenie) — patrz 03
    lamport: u64,
    kind:    OpKind,
}
```

Kolejność renderowania = sortowanie po `(lamport, author)`. Jest deterministyczna,
więc każdy peer widzi tę samą kolejność warstw bez uzgadniania.

Dlaczego **własny op-log CRDT zamiast Yjs/Automerge**: atrament to zbiór rosnący
z nagrobkami, a nie edytowany tekst. Wszystkie operacje są przemienne z definicji
(dodanie dwóch stroke'ów w dowolnej kolejności daje ten sam rysunek), więc nie potrzebujemy
maszynerii do rozstrzygania pozycji w sekwencji. Efekt: kilkaset linii zamiast biblioteki,
mniejszy zapis na dysk i brak narzutu metadanych na próbkę.
Automerge zostaje na później dla notatek tekstowych, gdyby doszły.

## Rendering

**Warstwa sucha** — kafle 512×512 w przestrzeni canvasu, tekstury GPU, LRU.
Przerysowywane tylko przy pan/zoom przekraczającym próg. Koszt klatki niezależny
od liczby stroke'ów w notatce.

**Warstwa mokra** — bieżący stroke, teselowany przyrostowo (dokładamy czworokąty
na końcu bufora, nie przeliczamy całości), rysowany na wierzchu, present immediate.

**Teselacja** — wstęga: na segment dwa trójkąty o szerokości z nacisku, plus
okrągłe złącza. Wykonywana na CPU przyrostowo; przy realnych gęstościach próbek
jest to ułamek kosztu wysłania danych na GPU.

**AA** — MSAA 4× lub analityczny AA w shaderze, zawsze w skali szarości.

## Rezydentność w tle (Z2)

Jeden proces, tray, okno tworzone raz:

| Stan | Co robi |
|---|---|
| Widoczne | pełny pipeline, render na zdarzenia |
| Ukryte (`SW_HIDE`) | `IDXGIDevice3::Trim()`, zwolnienie cache kafli, `EmptyWorkingSet()`, wątki parkują na zdarzeniach — **0% CPU** |
| Wywołanie (`RegisterHotKey`) | `ShowWindow` + present pierwszej klatki z zachowanego snapshotu |

Urządzenie D3D, pipeline'y i indeks notatek żyją przez cały czas — dlatego cel <30 ms
jest realny: nie ma czego inicjalizować, jest tylko odsłonięcie okna i jeden present.

## Konfiguracja

`%APPDATA%\SpectreNotes\config.toml` — jawny, edytowalny plik.
Ustawienia wpływające na odczucie (krzywa nacisku, tearing, interpolacja, predykcja)
są przełączalne w locie, żeby dało się je porównywać na żywo, a nie przez restart.
