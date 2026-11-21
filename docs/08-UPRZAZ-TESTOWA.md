# Uprząż testowa (`crates/spectre-harness`)

Testy obrazowe i pomiary pióra bez okna i bez pióra w ręku. Powstała jako etap E0 trybu 3D:
każdy etap 3D sprawdza nią, że płaszczyzna zerowa (tryb 2D) się nie zmieniła i że pióro
nie zwolniło.

## Co jest w środku
| Moduł | Co robi |
|---|---|
| `pen` | zapisany ruch pióra: kreski jako próbki ekranu (x, y, nacisk, czas); plik tekstowy, generator pisma 266 Hz, odtwarzanie paczkami jak `WM_POINTER` |
| `scene` | ta sama droga rysowania co okno (`App::feed`, `end_stroke`, `render`): `StrokeBuilder`, wypalanie co `WET_BURN_SEGS`, czubek na wierzch, `repaint` po kresce, przewijanie przyrostowe, kafelki stron |
| `png` | zapis i odczyt PNG (kompresja z `spectre-app/src/deflate.rs`), różnica pikseli, skrót obrazu |
| `notes` | 10 notatek wzorcowych testu T1: tła, zoom, PDF (kafelki syntetyczne), gęsta notatka, przewijanie, gumka i cofanie, mokra kreska |
| `stats` | percentyle |

Renderer ma do tego dwie rzeczy:
- `Renderer::new_offscreen(w, h, Device)` z `Device::create_warp()` (WARP, obraz jak na każdej maszynie) albo `Device::create()` (karta, jak w aplikacji). `present` rysuje wtedy do bitmapy, a `read_pixels` ją oddaje.
- `Renderer::presents()` liczy klatki oddane przez `present`, także w oknie (licznik klatek, T5.3). `gpu_wait()` czeka, aż GPU skończy klatkę.

## Polecenia
```
cargo build --release -p spectre-harness
target\release\spectre-harness t1 <katalog> [--png]     obrazy, pliki .ops, index.txt
target\release\spectre-harness t1-compare <a> <b>       T1.1 piksel w piksel i T1.2 bajt w bajt
target\release\spectre-harness latency [--warp] [--per-msg N] [--strokes N] [--rec plik]
target\release\spectre-harness record <plik>            nagranie syntetyczne do edycji
target\release\spectre-harness idle                     klatki w spoczynku
```

`latency` odtwarza pismo (6920 próbek) nad gęstą notatką 2880x1800 i mierzy dla każdego
komunikatu pióra czas od jego przyjęcia do oddania klatki (`present`, CPU) i do klatki
gotowej na GPU. Bez `Present` do DWM: to koszt aplikacji, nie kolejki ekranu.

## Wzorzec v0.4.5
`golden/t1-v0.4.5.txt` to skróty obrazów (WARP) i plików `.ops` notatek wzorcowych
z v0.4.5 (commit E0). `cargo test -p spectre-harness` sprawdza bajty plików zawsze,
a obrazy przy `SPECTRENOTES_GOLDEN_IMG=1` (maszyna odniesienia; WARP innej wersji
Windows może różnić się w antyaliasingu). Pełne porównanie z przebiegiem v0.4.5:
zbudować uprząż na commicie E0 w osobnym katalogu, `t1` w obu i `t1-compare`.

Pomiar bazowy v0.4.5 (laptop odniesienia, Intel Arc, release, 2026-11-21):

| Paczka | Do `present` p50 / p99 | Do klatki gotowej p50 / p99 |
|---|---|---|
| 1 próbka | 0,073 / 2,319 ms | 0,803 / 3,422 ms |
| 2 próbki, pusta notatka | 0,063 / 0,304 ms | 0,709 / 1,857 ms |

Spoczynek: 0 klatek przez 2 s po ostatniej kresce.
