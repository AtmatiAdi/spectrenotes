# ADR 0001 — Rust + wgpu, powłoka Win32

**Status:** przyjęte · 2026-09-11

## Kontekst

Aplikacja ma jednocześnie: rysować z minimalną latencją (Z5), siedzieć w tle
poniżej 20 MB (Z2), synchronizować się przez git i P2P (Z3, Z4) i dać się rozdać
jako jeden plik (Z8). Te wymagania mocno zawężają wybór.

## Rozważane opcje

**Electron / web.** Odrzucone bez dalszej analizy. Sam runtime przekracza budżet
RAM o rząd wielkości, a `PointerEvent` w przeglądarce jest odcięty od
`GetPointerPenInfoHistory` i od kontroli nad prezentacją. `getCoalescedEvents`
łagodzi pierwszy problem, ale nie drugi.

**C# / .NET 9 + Vortice.** Realna opcja i najszybsza w starcie — SDK był już
zainstalowany. Odrzucona z dwóch powodów. Po pierwsze GC: nawet krótka pauza
na ścieżce pióra jest widoczna jako zacięcie kreski, a walka z tym (pooling,
`SustainedLowLatency`, unikanie alokacji) sprowadza się do pisania C# tak,
jakby to był Rust, tylko bez gwarancji. Po drugie ekosystem: `gix`, `quinn`
i biblioteki CRDT nie mają w .NET odpowiedników tej jakości.

**C++20 + Direct2D/D3D12.** Daje ten sam wynik latencji co Rust, przy wyraźnie
wolniejszym rozwoju pozostałych 80% aplikacji (sync, git, CRDT, updater)
i przy zarządzaniu zależnościami przez vcpkg.

## Decyzja

**Rust**, renderer na **wgpu**, powłoka Win32 przez **windows-rs**.

- brak GC, więc brak losowych pauz na ścieżce atramentu;
- statyczna binarka bez runtime'u u odbiorcy;
- `gix` (git w czystym Ruście), `quinn` (QUIC), `blake3`, `bincode` — wszystko dojrzałe;
- `windows-rs` daje pełny, nieokrojony dostęp do `WM_POINTER`, DXGI i DirectComposition,
  więc wybór Rusta niczego nie odbiera po stronie API systemowego;
- wgpu mapuje się dziś na D3D12, a w przyszłości na Vulkan/Metal, co otwiera
  Linuxa i Androida bez przepisywania renderera.

## Konsekwencje

- Wymagany toolchain: rustup + VS Build Tools (workload C++). Zainstalowane.
- wgpu jest warstwą abstrakcji, więc odbiera trochę kontroli nad prezentacją.
  Gdyby etap 0 wykazał, że `PresentMode::Immediate` nie wystarcza, schodzimy
  do D3D12 przez windows-rs w samym `spectre-render` — reszta crate'ów tego nie zauważy.
  To jest główny powód, dla którego renderer jest osobnym crate'em.
- Dłuższy czas kompilacji niż w C#, co spowalnia iterację nad UI.
