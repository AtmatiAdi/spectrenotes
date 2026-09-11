# ADR 0005 — Renderer produkcyjny: Direct2D zamiast wgpu

**Status:** przyjęte · 2026-09-11 · zmienia ADR 0001 w części dotyczącej renderera

## Kontekst

ADR 0001 zakładał wgpu jako renderer, głównie ze względu na przenośność
(Vulkan/Metal w przyszłości), i jednocześnie przewidywał zejście niżej,
gdyby Etap 0 wykazał, że kontrola nad prezentacją jest niewystarczająca.

Etap 0 dał odwrotny wynik od zakładanego: demo na **Direct2D** na flip-model
swapchainie DXGI osiągnęło 266 Hz próbkowania i odczucie ocenione jako dobre —
zanim wgpu w ogóle został dotknięty.

## Decyzja

Renderer produkcyjny to **Direct2D + DXGI** przez `windows-rs`, ta sama ścieżka
co w demo. wgpu wypada z planu.

Powody, w kolejności wagi:

1. **Ścieżka prezentacji jest zwalidowana.** Frame latency 1, tearing dla mokrego
   atramentu, render sterowany zdarzeniami — to działa i jest zmierzone. wgpu
   wymagałby ponownej walidacji z niepewnym wynikiem, bo abstrahuje swapchain.
2. **DirectWrite za darmo.** UI notatnika (lista notatek, tytuły, HUD) potrzebuje
   tekstu. W wgpu to osobna biblioteka i osobny atlas glifów; w D2D to jedno wywołanie
   z antyaliasingiem w skali szarości, którego wymaga panel OLED (docs/04).
3. **Czas kompilacji.** Cały workspace z D2D buduje się w sekundy; wgpu dodawał
   minuty do pierwszego builda i sekundy do każdej iteracji.
4. **Kafle to bitmapy.** Cache kafli (Etap 2) w D2D to `ID2D1Bitmap1` z opcją
   TARGET — dokładnie to, czego użyto już do warstwy suchej.

## Co to zmienia w przenośności

`spectre-render` staje się crate'em **Windows-only**, obok `spectre-shell-win`.
Rdzeń (`proto`, `core`, `ink`, `sync`) pozostaje bez zależności od platformy —
port na Androida to napisanie `spectre-render-*` i `spectre-shell-*`, czyli
dokładnie tyle samo, ile trzeba by napisać dla powłoki niezależnie od renderera.

Realny koszt to kilka tysięcy linii renderera przy porcie, którego dziś nie ma
w planie. To dobra wymiana za zwalidowaną latencję i prostszy stack dziś.

## Konsekwencje

- `docs/01-ARCHITEKTURA.md`: crate'y platformowe to `spectre-render` i `spectre-shell-win`.
- Przewijanie warstwy suchej jest przyrostowe (przesunięcie bitmapy + dorysowanie
  odsłoniętego pasa) — to pokrywa 90% korzyści z cache'u kafli przy canvasie
  pionowym. Pełny cache kafli wraca do rozważenia dopiero, gdy zoom lub bardzo
  duże notatki pokażą, że pas odsłaniany przy przewijaniu jest za drogi.
