# Energia i AMOLED

Panel: **Samsung OLED 2880×1800 @120 Hz** (`SDC41A6`). To prawdziwy AMOLED —
czarny piksel jest **zgaszony**, a nie podświetlony na czarno. Ma to dwie
konsekwencje i obie działają na naszą korzyść.

## Pobór mocy

Na OLED pobór jest w przybliżeniu proporcjonalny do sumy luminancji zapalonych
pikseli. Notatka "jasny atrament na czerni" zapala pojedyncze procenty powierzchni
ekranu. Ta sama notatka jako "czarny atrament na białej kartce" zapala prawie wszystko.

Różnica jest rzędu **jednej dziesiątej poboru samego panelu**. Przy założeniu Z7
(wyświetlanie notatek całymi dniami) to nie jest kosmetyka — to jest najważniejsza
pojedyncza decyzja energetyczna w projekcie, o większym wpływie niż wszystko,
co zrobimy w kodzie.

Dlatego tło **#000000** jest domyślne i nie jest "trybem ciemnym" — jest trybem
podstawowym, a tryb jasny to opcja dla ekranów zewnętrznych (LCD), gdzie ta
zależność nie zachodzi.

## Ochrona przed wypaleniem

1. **Fale przyciemnienia zamiast pixel shiftu.** Pixel shift został zaimplementowany
   i odrzucony po teście na żywo: przeskok treści o 1 px jest wyczuwalny przy
   pisaniu. Zamiast tego po czasie bezczynności (domyślnie 3 min, w ustawieniach
   10 s – 10 min) przez ekran płyną skośne, sfalowane pasy ciemności — jak połysk
   zaczarowanego przedmiotu w Minecrafcie, tylko gaszący: trzy warstwy pasów pod
   różnymi kątami, o różnych okresach i prędkościach (14–80 px/s), główna gasi do
   zera, pozostałe dokładają migotanie. Pasy gaszą piksele tylko pod sobą; z czasem
   przechodzą przez każdy (test: 98 % siatki zgaszone w 5 min), obraz nie jest ani
   przesunięty, ani jednolicie przygaszony. Realizacja: maska krycia liczona na CPU
   w siatce co 12 px (~1 ms na 2880×1800, sama arytmetyka, bez libm) i rozciągana
   na okno jedną bitmapą z interpolacją kubiczną. Koszt: jedna tania klatka co
   60 ms, tylko gdy okno jest widoczne i tylko w bezczynności. Na czas ochrony
   notatka między pasami też przygasa (jasność z ustawień, domyślnie 30 %), UI znika
   (pasek narzędzi i tytuł to statyczny chrome), a okno przechodzi na pełny ekran
   z `HWND_TOPMOST`, żeby pasy przeszły przez cały panel; po pierwszym wejściu wraca
   poprzedni rozmiar. Pierwsza wersja (plamy z gradientem radialnym) odrzucona:
   wyglądała jak plamy, nie jak fale.
2. **Atrament nie jest biały.** Domyślnie `#D8D8D8`. Piksel prowadzony na pełnej
   bieli starzeje się nieproporcjonalnie szybciej, a przy pracy nocnej i tak oślepia.
3. **Brak statycznego chrome.** Toolbar chowa się po 3 s bezczynności. Cokolwiek
   musi zostać widoczne (wskaźnik sync, zegar) dryfuje razem z canvasem i przygasa.
4. **Bez globalnej rampy.** Jednolite przyciemnienie odrzucone: fale (pkt 1) chronią
   panel lokalnie, a reszta notatki pozostaje czytelna.
5. **Brak białych błysków.** Żadnych przejść przez jasne tło, żadnego splash screena.
6. **Budżet elementu statycznego.** Zasada projektowa: element, który potrafi stać
   w jednym miejscu dłużej niż 10 minut, musi albo dryfować, albo przygasać.
   Bez wyjątków.

## Antyaliasing a subpiksele

Panele OLED nie mają zwykłego układu RGB-stripe. Antyaliasing subpikselowy
(ClearType) daje na nich kolorowe obwódki wokół cienkich linii. Atrament
renderujemy **wyłącznie w skali szarości**: MSAA albo AA analityczny w shaderze.

## Wybór GPU — pułapka hybrydy

Maszyna ma Intel Arc (iGPU) i NVIDIA RTX 4050 (dGPU), a w doku jeszcze RTX 4080 SUPER.
Domyślna heurystyka DXGI potrafi wybrać adapter "wysokiej wydajności" i **wybudzić
dedykowaną kartę do rysowania notatek**. Koszt: kilkanaście watów bez żadnego zysku.

Zasada: **wybieramy adapter, który realnie steruje wyjściem, na którym stoi okno.**

```
IDXGIFactory6::EnumAdapterByGpuPreference(MINIMUM_POWER)
+ weryfikacja, czy adapter ma IDXGIOutput obejmujący monitor okna
```

Po przeniesieniu okna na monitor zewnętrzny podpięty do dGPU przełączamy się
świadomie, obsługując `WM_DISPLAYCHANGE`.

## Reszta budżetu energetycznego

- **Zero pętli klatkowej.** Klatka powstaje na zdarzenie. Idle to 0% CPU i brak
  wybudzeń GPU.
- **Prawie brak timerów.** Jedyny cykliczny to fale przyciemnienia: budzą się raz po
  3 min bezczynności, potem co 60 ms, i gasną przy pierwszym wejściu albo ukryciu
  okna — w tle proces nie budzi się w ogóle. Decyzja: pełna
  bezczynność to dokładnie ten moment, w którym ochrona przed wypaleniem ma sens,
  więc "zero timerów" ustąpiło "zero wybudzeń w tle".
- **MMCSS tylko w trakcie stroke'a.** Podnosimy priorytet wątku na czas rysowania
  i oddajemy natychmiast po `WM_POINTERUP`.
- **Świadomość zasilania.** `RegisterPowerSettingNotification`: na baterii obniżamy
  pułap odświeżania warstwy suchej. Warstwa mokra zostaje nietknięta —
  to jest nienegocjowalne (Z5).
- **W tle**: `IDXGIDevice3::Trim()`, zwolnienie cache kafli, `EmptyWorkingSet`,
  wątki zaparkowane na zdarzeniach.

## Do zweryfikowania pomiarowo

- Realny pobór panelu: ta sama notatka na czerni i na bieli, `powercfg /batteryreport`
  oraz odczyt szyny panelu w HWiNFO.
- Czy sterownik ELAN nie stosuje własnego wygładzania na poziomie firmware'u.
  Test: powolny ruch pióra wzdłuż liniału i porównanie surowych próbek z prostą.
  Jeżeli firmware wygładza, żadna praca po naszej stronie tego nie cofnie
  i trzeba to po prostu udokumentować jako sufit jakości na tym digitizerze.
