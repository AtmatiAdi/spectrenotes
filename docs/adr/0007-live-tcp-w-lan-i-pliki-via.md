# ADR 0007 — Warstwa live: TCP + multicast w LAN, zdalne operacje w plikach `via-*`

**Status:** przyjęte · 2026-09-13

## Kontekst

Etap 6 (Z4): druga osoba ma widzieć kreskę **w trakcie** rysowania, nie po
`git push`. Plan z ADR 0001 i `docs/03` zakładał QUIC (`quinn`), datagramy na
obecność, mDNS w LAN. Przy wejściu w etap trzeba było rozstrzygnąć dwie rzeczy:
czym przesyłać i **gdzie odbiorca ma odkładać cudze operacje**, żeby nie
zepsuć niezmiennika z `docs/03` (jeden plik `.ops` = dokładnie jeden pisarz),
na którym stoi bezkonfliktowy merge gita.

## Decyzja

1. **TCP z `TCP_NODELAY`, nie QUIC.** `quinn` ciągnie tokio + rustls + ring —
   największa zależność w binarce, dla której nie ma tu uzasadnienia: w LAN
   TCP daje opóźnienie rzędu 0,1 ms (zmierzone: mokra kreska 0,08–0,3 ms na
   jednej maszynie), a przez Tailscale i tak jedzie w tunelu WireGuard, który
   sam radzi sobie ze stratami. Zero nowych ciężkich zależności; `std::net`
   plus `socket2` (tylko po to, żeby dwie instancje na jednej maszynie mogły
   dzielić port multicastu — `SO_REUSEADDR`, którego `std` nie wystawia).
   Obecność (rysik drugiej osoby) idzie tym samym strumieniem — na LAN nie ma
   czego gubić. QUIC wraca, gdy pojawi się realny problem z Internetem bez
   Tailscale; interfejs (`Msg`, ramki) jest od transportu niezależny.
2. **Wykrywanie: własny beacon multicast** `239.255.94.94:47941` co 2 s
   (`SPCTLV1`, id instancji, port TCP, nazwa space'u, autor). Nie mDNS —
   pełna implementacja mDNS/DNS-SD to setki linii dla tej samej informacji.
   Łączy instancja o **mniejszym** id, druga czeka na `accept` — nigdy dwóch
   połączeń między tą samą parą. Rozgłaszanie tylko przy widocznym oknie
   (Z2: w tle proces ma się nie budzić); istniejące połączenia zostają.
   Peerzy z Tailscale bez multicastu: `Job::Connect(adres)` — do podpięcia
   pod ustawienia, gdy będzie potrzebne.
3. **Replikacja po połączeniu**: obie strony wysyłają `Summary` — dla każdej
   pary (notatka, autor) ostatni lamport z plików na dysku. Odbiorca odsyła
   `Ops` ze wszystkim, czego nadawcy brakuje. **Niezmiennik prefiksu**: autor
   dopisuje rosnąco po lamporcie, a każdy posiadacz dostał to w tej kolejności,
   więc jedna liczba opisuje stan; operacje o lamporcie nie większym od znanego
   są duplikatami. Każdy węzeł pamięta, co peer ma (jego `Summary` + wszystko,
   co poszło w obie strony), i po merge'u gita dosyła różnicę — maszyna bez live
   (np. telefon w przyszłości) dociera przez GitHub, a stąd do peerów. Węzeł
   przekazuje dalej tylko to, co było dla niego nowe, więc topologia trzech
   peerów bez pełnej siatki zbiega się i pętla się kończy.
4. **Zdalne operacje lądują w `ops/<autor>/via-<ja>-000001.ops`** — pliku, który
   pisze wyłącznie ta maszyna (własne chunki autora pisze tylko on). Niezmiennik
   „jeden plik = jeden pisarz" zostaje, `git merge` dalej nie ma jak się
   skonfliktować, a wczytanie notatki (`NoteStore::open`, skan metadanych) czyta
   wszystkie `.ops` w katalogu autora i deduplikuje po `(autor, lamport)` —
   idempotencja CRDT z ADR 0004. Bonus: instancja **bez logowania** ma kopię na
   GitHubie przez tę zalogowaną. Odrzucone: dopisywanie do właściwego pliku
   autora (dwa pisarze, konflikt binarny przy rozjeździe prefiksów; własna
   strategia merge „unia rekordów" byłaby do napisania i utrzymania) oraz
   lokalny cache poza repo (praca peera bez logowania nie miałaby kopii).
5. **Mokra kreska**: próbki lecą paczką na każdy komunikat pióra (~4 ms) jako
   `Wet` z kolorem i grubością w każdej paczce; odbiorca prowadzi dla peera ten
   sam `StrokeBuilder`, co dla własnej kreski (odcinki ostateczne do warstwy
   suchej, czubek na wierzch). `StrokeAdd` na koniec zastępuje wersję mokrą
   (przerysowanie prostokąta z geometrii dokumentu — jak lokalnie). `Wet`
   niesie zegar nadawcy: na jednej maszynie to bezpośredni pomiar opóźnienia,
   pokazywany w HUD-zie.
6. **Bez szyfrowania i uwierzytelniania** w v1: granica zaufania to LAN /
   Tailscale (`docs/03` „Bezpieczeństwo"). Każde urządzenie w sieci ze
   space'em o tej samej nazwie dostanie notatki. Dwa węzły z tą samą nazwą
   autora (`user@komputer`) odmawiają sobie połączenia — po merge'u byłyby
   dwoma pisarzami jednego pliku.

## Konsekwencje

- Windows Firewall pyta przy pierwszym uruchomieniu o zgodę dla `spectrenotes.exe`
  (nasłuch TCP + multicast). Bez zgody działa tylko między instancjami na tej
  samej maszynie; peerzy z sieci nie połączą się. Prompt jest systemowy, nie da
  się go ominąć bez uprawnień administratora.
- Kreska narysowana przy dwóch połączonych maszynach leży w repo dwa razy
  (u autora i w `via-*` odbiorcy). Przy 1 MB/h rysowania to bez znaczenia dla v1;
  przycinanie `via-*`, gdy plik autora w gicie już to pokrywa — razem ze
  snapshotami (Etap 3/5).
- Zombie po wyrwaniu kabla: TCP zauważy to dopiero po swoim timeoucie; do tego
  czasu peer figuruje jako połączony. Nowa instancja ma nowe id, więc ponowne
  uruchomienie nie jest tym blokowane. Heartbeat — gdy zacznie przeszkadzać.
- Zweryfikowane dwiema instancjami na jednej maszynie (różny `COMPUTERNAME`,
  osobne `APPDATA`, osobne katalogi space'u o tej samej nazwie): wykrycie przez
  multicast, notatka założona po jednej stronie pojawia się po drugiej, kreska
  widoczna u drugiej **w trakcie** rysowania (zrzuty co 150 ms), rysik peera
  jako kropka, opóźnienie mokrej kreski 0,08–0,3 ms, po restarcie notatki
  z `via-*` czytane zwykłym `NoteStore`.
