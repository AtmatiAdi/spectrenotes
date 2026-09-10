# Pióro MPP 2.0 i łańcuch latencji

> Cel: rysowanie nieodróżnialne od Samsung Notes. To jest dokument
> najważniejszy dla odczucia produktu — reszta aplikacji może być przeciętna,
> ta warstwa nie może.

## Łańcuch pen-to-photon

Pełna droga od dotknięcia rysikiem do zapalenia piksela, z realnym budżetem
na tym sprzęcie (ELAN2513 MPP 2.0, panel 120 Hz):

| # | Etap | Koszt | Czy kontrolujemy |
|---|------|-------|------------------|
| 1 | Digitizer próbkuje pióro (~180–240 Hz) | 4–6 ms | nie |
| 2 | Transport HID + sterownik ELAN | 1–3 ms | nie |
| 3 | Stos wejścia Windows → `WM_POINTER` do okna | 1–4 ms | **częściowo** |
| 4 | Nasza obróbka: parsowanie, teselacja, zapis | <1 ms | **tak** |
| 5 | Render GPU mokrego stroke'a | <1 ms | **tak** |
| 6 | `Present` → DWM → skanowanie panelu | **8–25 ms** | **tak, i tu leży cała gra** |

Etapy 1–2 to ok. 5–9 ms, których nie ruszymy. **Cała realna praca to etap 6**,
gdzie naiwna aplikacja traci 2–3 klatki, a poprawna jedną lub mniej.

## Etap 3 — pozyskanie próbek bez strat

### `WM_POINTER`, nie `WM_MOUSE`
```
EnableMouseInPointer(TRUE)
→ WM_POINTERENTER / WM_POINTERDOWN / WM_POINTERUPDATE / WM_POINTERUP / WM_POINTERLEAVE
→ GetPointerType() → PT_PEN
→ GetPointerPenInfo() → POINTER_PEN_INFO { pressure 0..1024, tiltX/tiltY -90..90, rotation, penFlags }
```
`WM_MOUSEMOVE` daje jedną próbkę na klatkę, bez nacisku i bez tiltu — jest bezużyteczny.

### `GetPointerPenInfoHistory` — bez tego nic nie działa
Pióro raportuje ~240 Hz, komunikaty przychodzą ~120 Hz. Bez odczytania historii
**gubimy co drugą próbkę**, a stroke robi się wielokątem przy szybkim ruchu.

```
GetPointerPenInfoHistory(pointerId, &count, nullptr)   // ile próbek czeka
GetPointerPenInfoHistory(pointerId, &count, buffer)    // wszystkie, najnowsza pierwsza
```
Zawartość trzeba odwrócić — API zwraca od najnowszej do najstarszej.

### `ptHimetricLocationRaw` — najczęściej pomijany szczegół
`POINTER_INFO.ptPixelLocation` to **liczba całkowita pikseli ekranu**. Przy 2880×1800
to raster grubszy niż rozdzielczość digitizera i to on odpowiada za „schodkowanie"
w wielu aplikacjach.

Poprawnie:
```
GetPointerDeviceRects(device, &displayRect, &himetricRect)
x_px = (himetric.x - himetricRect.left) / himetricRect.width  * displayRect.width
```
Digitizer ma zwykle rozdzielczość rzędu 10× gęstszą niż piksele — dostajemy
prawdziwe współrzędne subpikselowe.

### `penFlags` i przyciski
`PEN_FLAG_BARREL` (przycisk boczny), `PEN_FLAG_INVERTED` (odwrócone = gumka w hoverze),
`PEN_FLAG_ERASER` (gumka dotyka ekranu). Mapujemy: inverted → narzędzie gumka.

### Wyłączenie wizualnego feedbacku systemu
Windows domyślnie rysuje kółko przy dotknięciu piórem i „press-and-hold" → prawy klik.
Oba psują odczucie i dodają percepcyjne opóźnienie:
```
SetWindowFeedbackSetting(hwnd, FEEDBACK_PEN_TAP,          0, sizeof(BOOL), &FALSE)
SetWindowFeedbackSetting(hwnd, FEEDBACK_PEN_DOUBLETAP,    0, sizeof(BOOL), &FALSE)
SetWindowFeedbackSetting(hwnd, FEEDBACK_PEN_PRESSANDHOLD, 0, sizeof(BOOL), &FALSE)
SetWindowFeedbackSetting(hwnd, FEEDBACK_PEN_RIGHTTAP,     0, sizeof(BOOL), &FALSE)
SetWindowFeedbackSetting(hwnd, FEEDBACK_PEN_BARRELVISUALIZATION, 0, sizeof(BOOL), &FALSE)
```

### DPI awareness
`SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)`. Bez tego na 2880×1800 przy 200%
system skaluje bitmapę okna — atrament jest rozmyty, a współrzędne przeliczane z błędem.

### Osobny wątek wejścia
Okno wejściowe ma własną pompę komunikatów i własny wątek. Próbki lądują
w bezblokadowym ring bufferze. Dzięki temu **zacięcie renderu nigdy nie gubi próbki** —
w najgorszym razie stroke dorysuje się klatkę później, ale jego kształt zostanie wierny.

## Etap 6 — prezentacja, czyli gdzie giną klatki

### Flip model, nie blt
```
DXGI_SWAP_EFFECT_FLIP_DISCARD, BufferCount = 2..3
DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT
IDXGISwapChain2::SetMaximumFrameLatency(1)
```
Domyślne 3 klatki kolejki to na 120 Hz **25 ms** samego opóźnienia kolejkowania.

### Tearing dla mokrego atramentu
```
DXGI_FEATURE_PRESENT_ALLOW_TEARING → Present(0, DXGI_PRESENT_ALLOW_TEARING)
```
Rozdarcie obrazu przy rysowaniu cienkiej linii jest niewidoczne, a oszczędza
do pełnej klatki. Przełącznik w ustawieniach — do porównania na żywo.

### Independent flip
Borderless fullscreen o rozdzielczości dokładnie równej trybowi pulpitu, bez nakładek
i bez przezroczystości → DWM oddaje swapchain wprost do skanowania i **znika z łańcucha**.
To jest powód, dla którego tryb pełnoekranowy jest u nas trybem podstawowym (Z7),
a nie kosmetycznym dodatkiem.

### Render sterowany wejściem
Nie ma pętli `while(true) { render(); }`. Klatka powstaje **gdy przyszła próbka**
albo gdy coś się zmieniło. To jednocześnie najniższa latencja i zero poboru na idle (Z1).

## Mokry i suchy atrament

- **Mokry** — stroke aktualnie rysowany. Przerysowywany co klatkę, prezentowany natychmiast.
- **Suchy** — stroke zakończony. Wypalany do cache kafli (tile cache) i odtąd nieruszany.

Bez tego podziału notatka z 100 000 stroke'ów zabija framerate. Z nim koszt klatki
zależy tylko od długości *bieżącego* stroke'a.

## Czego świadomie NIE robimy (Z6)

| Technika | Co robi | Decyzja |
|---|---|---|
| Moving average / one-euro filter | uśrednia próbki, „uspokaja" linię | **NIE** — zmienia kształt, to już nie jest Twoja kreska |
| Fitting do krzywych Béziera z tolerancją | upraszcza stroke, gubi drobne drgania | **NIE** — drgania to informacja, nie szum |
| Predykcja / ekstrapolacja | rysuje przed piórem, maskuje latencję | **domyślnie NIE** — daje „haczyki" na zwrotach. Przełącznik do testów. |
| Korekta kształtów (prostowanie linii, koła) | zamienia rysunek na figurę | **NIE** |
| Centripetal Catmull-Rom **przez** próbki | wypełnia dziury między próbkami | **TAK** — nie zmienia żadnej próbki, tylko rysuje to co między nimi |

Rozróżnienie interpolacja/wygładzanie jest tu kluczowe i bywa mylone:
splajn centripetal **przechodzi dokładnie przez każdą zaraportowaną próbkę**.
Nie usuwa informacji — dodaje ciągłość tam, gdzie digitizer po prostu nie zdążył zmierzyć.

## Model stroke'a

```
Sample { x: f32, y: f32, pressure: f32 (0..1), tilt_x: f32, tilt_y: f32, t_us: u32 }
Stroke { id, tool, color, base_width, samples: Vec<Sample> }
```
- szerokość = `base_width * krzywa_nacisku(pressure)`; krzywa konfigurowalna (gamma / punkty kontrolne)
- teselacja: wstęga (ribbon) — dla każdego segmentu czworokąt o szerokości z nacisku,
  plus okrągłe zakończenia; tilt opcjonalnie spłaszcza stalówkę w elipsę
- antyaliasing: **grayscale**, nigdy ClearType/subpiksel — układ subpikseli paneli OLED
  nie jest zwykłym RGB-stripe i subpikselowy AA daje kolorowe obwódki (patrz `04-ENERGIA-I-AMOLED.md`)

## Jak to zmierzyć

`tools/inkdemo` wyświetla na żywo: częstotliwość próbkowania pióra [Hz], liczbę próbek
odebranych z historii vs. z komunikatów, czas obróbki klatki, czas present, nacisk i tilt.
Bezwzględną latencję pen-to-photon mierzy się kamerą 240 fps (telefon) — licząc klatki
między dotknięciem rysika a pojawieniem się piksela. To jedyny uczciwy pomiar.
