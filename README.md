# SpectreNotes

Notatnik rysunkowy dla Windows na ekrany AMOLED i pióra MPP 2.0.
Własny zamiennik Samsung Notes — bez zależności od cudzej chmury i cudzej decyzji
o tym, które komputery są wystarczająco właściwe, żeby uruchomić aplikację.

**Stan: Etapy 0–1 zamknięte, 2–5 w większości (git wbudowany, repo GitHub automatyczne, budżet ruchu).** 266 Hz próbkowania, pozytywny werdykt
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

Narzędzia diagnostyczne w tej samej binarce: `spectrenotes --bench` (pomiary renderera)
i `spectrenotes --fill-white [space]` (notatka testowa: cała kolumna zamalowana bielą,
do oglądania fal AMOLED).

## Instalacja i aktualizacje

Gotowa aplikacja: [najnowsze wydanie](https://github.com/AtmatiAdi/spectrenotes-releases/releases/latest)
→ `SpectreNotes-Setup.exe`. Instalator sam nic nie zawiera — pobiera bieżącą
wersję, sprawdza sumę SHA-256 i instaluje ją dla użytkownika
(`%LOCALAPPDATA%\SpectreNotes\app`, skrót w menu Start, wpis w „Zainstalowane
aplikacje"; notatki są osobno w `%APPDATA%\SpectreNotes` i odinstalowanie ich nie rusza).

Zainstalowana aplikacja sprawdza wydania 5 s po starcie i co 10 min; gdy jest nowsza,
Settings → **Application** pokazuje „version X available" z przyciskiem Download,
paskiem postępu i „Install and restart". Nic nie pobiera się ani nie podmienia bez
kliknięcia. `spectrenotes.exe --install` / `--uninstall` robią to samo z linii poleceń.

Wydanie: `.\release.ps1 -Bump patch` (wersja → testy → build → tag → GitHub Release,
zostają 3 ostatnie). Szczegóły: [`docs/06-DYSTRYBUCJA.md`](docs/06-DYSTRYBUCJA.md).

### Aplikacja

Notatki leżą w `%APPDATA%\SpectreNotes\spaces\default` (inna ścieżka: pierwszy
argument). Każda kreska trafia do pliku autora natychmiast po zakończeniu,
`fsync` po 400 ms ciszy — wyrwanie zasilania gubi co najwyżej ostatnie pociągnięcie.

Aplikacja **żyje w tray'u**: `Esc`, zamknięcie okna i `Win+Shift+N` chowają ją
(w tle ~2 MB), `Win+Shift+N` lub klik w ikonę odsłania w ~20 ms. Zakończenie:
`Ctrl+Q` albo prawy klik na ikonie → Quit. **Interfejs jest po angielsku** (menu,
HUD, komunikaty); dokumentacja i komentarze w kodzie zostają po polsku.

**Jedna instancja na space**: ponowne uruchomienie (ikona, skrót, autostart) przy
działającej aplikacji pokazuje jej okno i kończy się — dwa procesy pisałyby do tego
samego op-logu. Inny katalog space'u to osobna instancja; `--new-instance` (debug,
testy) wyłącza sprawdzanie. Kod: `spectre-shell-win::instance` (własność okna
`SetPropW` ze skrótem ścieżki space'u, komunikat `WM_SHOW_APP`).

Binarka jest aplikacją okienkową — **bez okna konsoli**. Argumenty: `--tray` (start
schowany do traya; tak startuje autostart), `--new-instance` (patrz wyżej), `--console`
(dołącz do konsoli terminala, z którego uruchomiono — logi), `--bench`, `--fill-white`
(tryby wierszowe, same dołączają konsolę).

**Współpraca ze Spectre** (`C:\Projects\Spectre` — presety ekranów i czarna nakładka
AMOLED na panel laptopa): gdy SpectreNotes jest widoczne na panelu, ma włączoną własną
ochronę **i jest zmaksymalizowane albo w pełnym ekranie** (domyślnie; ustawienie
*Maximized only* wyłącza ten warunek), co 10 s prosi Spectre o wstrzymanie nakładki (dzierżawa 30 s przez komunikat
`Spectre.ShieldHold`). Spectre po wygaśnięciu dzierżawy wraca do ochrony sam — zamknięty
czy zawieszony SpectreNotes nigdy nie zostawia panelu bez ochrony. HUD (`H`) mówi, co
naprawdę poszło i dlaczego (`Spectre: shield held off, last ping 3 s ago` / `not holding -
window on \\.\DISPLAY2, not the laptop panel` / `Spectre not running`), a przejścia tego
stanu lądują w `%APPDATA%\SpectreNotes\partner.log`; Spectre loguje każde zakrycie
i odkrycie panelu z powodem i stanem dzierżawy (`%APPDATA%\Spectre\spectre.log`).
Przyciemniacz paska zadań w Spectre ustępuje aplikacji na pełnym ekranie — naszej do
15 IX 2026 nie ustępował, bo predykat „to moje okno" dopasowywał **prefiks** nazwy klasy
`Spectre`, a nasza klasa nazywa się `SpectreNotes`. Naprawione po stronie Spectre:
`WindowFilter.IsOwn` pyta o tożsamość procesu, nie o nazwę.
Tam też siedziała przyczyna zgłoszenia „małe okno SpectreNotes zasłania pasek zadań":
przyciemniacz Spectre jest **oknem własnościowym** paska (owner = `Shell_TrayWnd`),
a podnosił się skokiem `HWND_NOTOPMOST` → `HWND_TOPMOST`. Win32 wiąże te stany
jednostronnie — zdjęcie topmost z okna własnościowego zdejmuje je też z właściciela,
a podniesienie z powrotem właściciela już nie podnosi. Pasek zadań tracił więc
`WS_EX_TOPMOST` przy pierwszym sprawdzeniu z-order i od tej chwili zasłaniało go
dowolne okno. Naprawione po stronie Spectre (`HWND_TOP`).

**Okno nie ma systemowej ramki ani paska tytułowego** — canvas zaczyna się od samej
góry. Nad nim wiszą dwie małe **zakładki**: tytuł notatki na środku (tap, wpisz,
`Enter`; z lewej uchwyt do przesuwania okna) i przyciski **pełny ekran ⛶** /
minimalizuj / maksymalizuj / zamknij (= ukryj do tray) w prawym rogu — z takim samym
uchwytem ⠿ po lewej stronie przycisków. W pełnym ekranie przycisk pokazuje ⧉ (powrót
do okna), a zakładki chodzą razem z paskiem: podjedź rysikiem do krawędzi, a wracają
na ekran — inaczej ⛶ byłoby wejściem bez wyjścia dla kogoś, kto nie zna `F11`.
Uchwytów ⠿ tam nie ma, bo okna na pełnym ekranie nie ma po czym przesuwać.
**Pełny ekran jest trybem *nad* stanem okna, nie trzecim stanem obok zwykłego
i zmaksymalizowanego**: wejście zapamiętuje cały `WINDOWPLACEMENT`, więc wyjście
(⧉, `F11`, `Esc`) oddaje dokładnie to, co było — zmaksymalizowane okno wraca
zmaksymalizowane i pamięta, do czego się przywraca. Przycisk maksymalizacji
w pełnym ekranie najpierw z niego wychodzi, a potem przełącza okno, i pokazuje
stan, do którego wrócisz (❐ = wrócisz do zmaksymalizowanego). Do `config.txt`
zapisuje się położenie sprzed pełnego ekranu — prostokąt monitora nie jest niczyim
wyborem. Wejście i wyjście meldujemy powłoce przez `ITaskbarList2::MarkFullscreenWindow`,
więc pasek zadań ustępuje od razu, zamiast czekać na heurystykę powłoki (ta wymaga
pierwszego planu i zauważa rzecz po sekundzie czy dwóch).

**Start bez skoku rozmiaru** (issue #20): okno powstaje od razu w docelowym prostokącie
(dla zmaksymalizowanego — obszar roboczy monitora z zapisanego położenia), obszar klienta
to całe okno od pierwszego `WM_NCCALCSIZE`, więc renderer i pierwsza klatka mają właściwy
rozmiar; maksymalizacja idzie z miejsca, a pierwsze pokazanie pod maską DWM i bez animacji
(wracają po 500 ms). Widoczne okno nie zmienia prostokąta ani razu, treść jest na ekranie
~55 ms wcześniej niż przy trzech kolejnych rozmiarach. Przy okazji wyłączone IME
(`ImmDisableIME`: pierwsza aktywacja okna kosztowała ~30 ms na kontekst TSF; kto pisze
przez IME, ustawia `ime=1` w `config.txt`), a start do traya odtwarza maksymalizację
przy pierwszym pokazaniu. Aplikacja **otwiera notatkę z poprzedniej sesji** (`last_note`
w `config.txt`, issue #18); gdy jej nie ma — ostatnią własną.
Technicznie pełny ekran to **maksymalizacja do całego monitora**: okno (bez
`WS_CAPTION`, ale z `WS_THICKFRAME` — snap i Win+strzałki działają) odpowiada na
`WM_GETMINMAXINFO` obszarem roboczym, a w pełnym ekranie prostokątem monitora.
Zmaksymalizowane okno zajmuje więc dokładnie obszar roboczy od (0,0) — bez ramki
wysuniętej 9 px poza ekran — i przejście do pełnego ekranu to tylko dolna krawędź
rosnąca o pasek zadań; treść nie drga. Ze zwykłego okna wejście i wyjście grają
systemowe animacje maksymalizacji i przywracania. Dwie pułapki, obie zmierzone:
zapamiętane położenie (`SetWindowPlacement`) musi mieć `ptMaxPosition = (-1,-1)`,
bo zero jest dla systemu wprost zapisanym punktem i pierwsza maksymalizacja lądowała
w (-8,-8) — 8 px paska zadań zostawało odkryte u dołu i po prawej; a gdy pierwszy
plan ma sam pasek zadań (ostatnie kliknięcie w zegar czy tray — po bezczynności to
częste), powłoka ~50–100 ms po zmianie stanu okna wraca z paskiem **nad** nasze,
mimo „zawsze na wierzchu", i bywa, że drugi raz po ~0,8 s. Dlatego po wejściu
w pełny ekran `TIMER_TOPMOST` przez 1,6 s co 200 ms powtarza `HWND_TOPMOST`, a
w trakcie ochrony AMOLED raz na 5 s. Pasek narzędzi (i zakładki
w pełnym ekranie) nie znika w jednej klatce: wsuwa się w swoją krawędź i blednie
przez ~200 ms — także gdy chowa go ochrona AMOLED; podjazd rysika do krawędzi
przerywa to od razu. Trzecia pułapka to monitory: `WM_GETMINMAXINFO` liczy prostokąt
dla monitora, na którym okno **jest w chwili pytania**, a świeżo utworzone stoi na
głównym — odtwarzanie położenia na inny monitor (stacja dokująca) dawało rozmiar
głównego, czyli okno większe niż ekran z uciętym X. Dlatego `apply_placement` najpierw
przesuwa niepokazane okno w zapamiętane miejsce, a po pokazaniu (start, powrót z
traya) `fix_maximized_rect` porównuje prostokąt z obszarem roboczym bieżącego monitora
i poprawia go, jeśli się różni — tak samo po `WM_DISPLAYCHANGE`/`WM_DPICHANGED`
(przepięcie stacji dokującej) i w każdym tiku fal (`ensure_waves_fullscreen`,
z prostokątem **całego** monitora). Ta sama funkcja domyka drugą dziurę: wejście
w pełny ekran odpuszcza, gdy system nie poda prostokąta monitora (układ ekranów
zmienia się właśnie pod ręką), a wtedy fale płynęły po oknie zmaksymalizowanym,
z paskiem zadań nad notatką; teraz ochrona próbuje ponownie w każdym tiku i dopiero
udane wejście zapamiętuje jako **swoje** (do zdjęcia po ochronie). Nieudane wejście
zostawia linię w `partner.log`.
Na te próby jest **budżet trzech na jedno wejście ochrony** (`FS_FIX_TRIES`), bo
sterownik potrafi mieć w sprawie prostokąta własne zdanie: NVIDIA Surround spina dwa
monitory w jeden ekran, ale okna maksymalizuje do **jednego** z nich i oddaje nasz
prostokąt po każdej poprawce (zmierzone: 12 skoków 3840↔7680 na sekundę — ekran miga
raz jednym, raz drugim monitorem). Po wyczerpaniu budżetu odpuszczamy, z powodem
w `partner.log`: fale na mniejszym prostokącie są lepsze niż migotanie.
Gdy pasek narzędzi jest zadokowany u góry, **wchłania** oba pola — tytuł,
uchwyt i przyciski stają się jego elementami i znikają razem z nim; tytuł ma wtedy tę
samą szerokość i to samo miejsce co zakładka (środek okna), nie rozciąga się na wolne
miejsce paska — gdy elementy wchodzą na środek, zwęża się, a w ostateczności siada
w wolnym pasie. **Puste miejsce paska narzędzi (w każdym doku) też przesuwa okno** —
pasek zastępuje pasek tytułowy.
Dwuklik na uchwycie maksymalizuje, snap i `Win+strzałki` działają jak zwykle.

**Pasek narzędzi** wyjeżdża, gdy rysik zbliży się do krawędzi, przy której jest
zadokowany, i chowa się po 2,5 s bezczynności (Z7). **Jest dokowalny**: złap za
uchwyt (kropki ⠿ na **samym końcu** paska) i upuść przy dowolnej krawędzi — góra/dół
przełącza go w orientację poziomą. Kolejno: menu, pióro, gumka, zaznaczanie, kolory, grubość
(co 0,1 px), cofnij/ponów, kopiuj/wklej, lupa −/+, procent zoomu (tap = dopasuj
szerokość), **kłódka widoku**, dalej **kłódka komputera** (jak `Win+L`), a na samym
końcu uchwyt przesuwania paska. Uchwyt był kiedyś pierwszy, tuż przy menu, i łapał
się zamiast przycisków — na końcu, za kłódką, jest mniej pod ręką. W pasku u góry
koniec paska trzyma zapas od uchwytu przesuwania okna, żeby dwa takie same ⠿ nie
stały obok siebie.
Gdy pasek jest krótszy niż elementy, elementy kurczą się proporcjonalnie — nic nie
wypada poza okno. Dok i położenie okna zapisują się
w `%APPDATA%\SpectreNotes\config.txt` (jawny `klucz=wartość`).

**Zaznaczanie obrysem** (ikona przerywanej ramki na pasku albo `S`): rysik rysuje
**lasso**, a zaznaczeniem staje się to, co obrys otoczył **w całości** — kryterium
jest świadome: zaznacza się to, co się okrążyło, a nie to, co się musnęło. Wokół
zaznaczenia staje ramka z uchwytami: **wnętrze przesuwa**, **rogi skalują**
(proporcjonalnie, przeciwległy róg stoi w miejscu — grubość kreski rośnie razem
z treścią), **uchwyt nad ramką obraca** wokół środka. Przesuwa się tylko to, co się
złapie: **dotknięcie poza ramką i jej uchwytami odstawia zaznaczenie tam, gdzie jest**
(treść się nie rusza) i od razu zaczyna nowy obrys; `Esc`, zmiana narzędzia i dotknięcie
paska robią to samo.
W trakcie gestu dokument się nie zmienia: kreski znikają na ten czas z warstwy suchej
i są rysowane co klatkę z **gotowych obrysów** w transformacji zaznaczenia — 162
przeniesione kreski kosztowały 1,7 ms na klatkę (Arc, 25 %). Dopiero koniec gestu
zamienia je w dokumencie na przekształcone kopie — **jedna operacja w historii**
(`Ctrl+Z` cofa cały ruch), bo nagrobek CRDT jest nieodwracalny i „edycja" kreski to
zawsze wymazanie plus dodanie (ADR 0004).

**Kopiuj / wklej** (dwa przyciski na pasku za cofnij/ponów albo `Ctrl+C` / `Ctrl+V`):
kopiuje się zaznaczenie — kreski zapamiętane **względem środka swojej obwiedni**, więc
wklejenie nie zależy od tego, gdzie leżał oryginał. Schowek żyje w aplikacji, nie
w notatce, więc **kopiuje się w jednej notatce, a wkleja w drugiej** (schowek przeżywa
przełączenie notatki i space'u). Wklejona treść ląduje na środku widoku i jest od razu
**zaznaczona** — można ją chwycić i odstawić tam, gdzie ma być. Całe wklejenie to jedna
akcja historii (`Document::add_strokes`), więc jedno `Ctrl+Z` je zdejmuje.

**Widok zablokowany / odblokowany** (kłódka widoku na pasku, `view_lock` w config):
zablokowany — kolumna notatki zawsze wyśrodkowana (jej środek to bezwzględny środek
notatki), zoom wolno, przesuwanie w poziomie nie; zablokowanie wraca do dopasowanej
szerokości. Odblokowany — canvas nieskończony w osi X, przewijanie w bok bez granic
(dawniej granicą była kolumna poszerzona o treść poza nią, co pozwalało „odblokować"
scroll w bok rysując po oddaleniu — stąd ta kłódka).

**Menu** (☰ na pasku narzędzi albo `M`) to panel z trzema zakładkami. U góry stały
**nagłówek konta**: zdjęcie profilowe z GitHuba (albo inicjał, gdy niezalogowany),
nazwa i stan logowania, licznik od ostatniej synchronizacji tykający co sekundę
z dokładną godziną (`synced 4:37 ago · 12:04:11`; bez logowania `saved …`) i przycisk
„synchronizuj teraz". Zakładka *Konto* rozdziela trzy zegary: zapis na dysk, GitHub,
peer w LAN.
*Notatki*: **kafelki z miniaturą** notatki (dwie kolumny, od najnowszej), tytuł
i data na pasku u dołu miniatury — notatkę poznaje się po tym, co na niej
narysowano, bo większość nie ma nazwy. Miniatura to kadr jak strona: w poziomie
cała kolumna (ta sama skala w każdym kafelku), w pionie od góry treści, żeby
notatka zaczynająca się nisko nie dawała pustego kafelka. Każda powstaje raz,
do własnej bitmapy, i potem jest już tylko przepisywana; bieżąca odświeża się,
gdy notatka się zmieni. Rysuje je **osobny wątek** (`thumbs::Worker` z własnym
urządzeniem D3D/D2D — `ThumbRenderer`), od najnowszej notatki, a okno tylko wrzuca
gotowe piksele do bitmap (ułamek ms) i rysuje klatkę najwyżej co 16 ms; 300 notatek
na zimno to ~7,5 s pracy wątku, w trakcie których okno odpowiada w < 3 ms.
Narysowana miniatura trafia też na dysk
(`%APPDATA%\SpectreNotes\thumbs\<id>-<odcisk>-<w>x<h>.png`; odcisk = nazwy, rozmiary
i czasy plików z operacjami notatki), więc kolejny start tylko dekoduje PNG
(300 notatek: 150 ms w wątku, wszystkie kafelki ~170 ms po pierwszej klatce);
panel i okno wyboru używają tych samych plików.
Cudze notatki z LAN mają takie same kafelki — miniaturę
dostają po otwarciu, bo dopiero wtedy mamy ich treść.
Tap otwiera notatkę, „przenieś tutaj" przy nagłówku folderu przenosi bieżącą,
na dole nowa notatka (ląduje w folderze bieżącej) i nowy folder (wpisz nazwę,
`Enter`), pod spodem *This note on LAN* (udostępnij, hasło) i *Shared on LAN* (cudze
udostępnienia). Folder notatki jest jej metadaną w op-logu (jak tytuł), więc
zsynchronizuje się razem z nią; puste foldery leżą w `<space>/folders.txt`.
**Export this note to PDF…** zapisuje bieżącą notatkę wektorowo (te same obrysy
kresek co na ekranie, własny writer PDF i zlib — bez zależności): strona to kolumna
notatki w proporcji A4, cięta po wysokości; plik otwiera się po zapisie. *Ustawienia →
Export → PDF background*: białe (kolory z odwróconą jasnością, żeby jasna paleta AMOLED
była czytelna na papierze) albo czarne jak ekran.
*Ustawienia*: na samej górze zawsze **Send feedback** — okno nad canvasem z polem
tekstowym (Enter = nowy wiersz, Ctrl+Enter wysyła, Ctrl+V wkleja), *Attach a file…*
(systemowy wybór pliku) i *Attach app screenshot* (zrzut aplikacji bez okna i menu);
*Send* zakłada issue w publicznym `spectrenotes-releases` tokenem zalogowanego
użytkownika, a zrzuty/pliki lądują w publicznym repo `spectrenotes-feedback` na jego
koncie (Contents API — issue w cudzym repo nie przyjmie pliku; obrazek z surowego
adresu GitHub renderuje w treści). Pod tekstem doklejana jest wersja, Windows, GPU,
rozmiar okna i ostatni komunikat synca. W trakcie wysyłki pasek postępu odgrywa ~5–6 s
scenariusz komentarzy losowany za każdym razem z puli ~90 tekstów (easter egg — potrafi
się cofnąć, tempo też losowe; issue #19), prawdziwy błąd przerywa go od razu i szkic zostaje. Bez logowania przycisk otwiera formularz nowego issue w
przeglądarce z wpisaną treścią (bez załączników). Dalej grupy funkcjonalne: *Wyświetlanie* (vsync, tearing, pełny ekran,
HUD), *Pasek narzędzi* (krawędź dokowania, zawsze widoczny), *Nawigacja* (mnożnik
przewijania x1…x6 — kółko i przycisk boczny), *Ochrona AMOLED* (włącz/wyłącz; **tylko
na ekranie laptopa** — na zewnętrznym monitorze fale nie startują, panel wbudowany
rozpoznawany po typie złącza (a gdy jest **wygaszony** — zamknięta klapa, stacja,
NVIDIA Surround — chronionego ekranu po prostu nie ma i ochrona nie startuje nigdzie),
a praca na innym ekranie nie trzyma panelu rozświetlonego
(stacja dokująca — patrz *Ochrona AMOLED liczy bezczynność człowieka*); **tylko zmaksymalizowane lub pełny ekran** — domyślnie
włączone: w zwykłym oknie fale nie startują i Spectre nie dostaje próśb, bo skok okna na
cały ekran w trakcie pracy obok był uciążliwy; gdy ochrona czuwa, przed tytułem notatki
stoi tarcza 🛡 — potwierdzenie, że okno jest tam, gdzie ma chronić; czas bezczynności
10 s – 10 min; jasność notatki między pasami 0–100 %), *Sieć lokalna* (rysowanie na żywo), *System* (**autostart** — wpis
w kluczu `Run` użytkownika, aplikacja startuje z `--tray`, czyli schowana w trayu;
**wybór pamięta też `config.txt`**, a `autostart_sync` odtwarza wpis przy starcie
zainstalowanej kopii, gdy ktoś go skasował — antywirus zdejmujący „trwałość",
czyścik, przeniesiony profil — albo gdy został po starej ścieżce; sami go nigdy
nie wyłączamy, a naprawa zostawia linię w `partner.log`).
Trwałe wartości lądują w `config.txt`. *Konto*: autor, logowanie do GitHuba — przycisk **Sign in with GitHub** otwiera
przeglądarkę, po „Authorize" wraca ona sama do aplikacji (OAuth na loopbacku,
`http://127.0.0.1:<port>/callback`), token zapisuje się i repo zakłada samo; zapasowo
Device Flow (kod w schowku) i wklejony PAT. Aplikacja OAuth „SpectreNotes" jest
zarejestrowana na koncie AtmatiAdi: Client ID w `github.rs`, sekret **poza repo** —
`build.rs` wkleja go z `%USERPROFILE%\.spectrenotes-oauth-secret` przy budowaniu
(bez pliku binarka umie tylko Device Flow). Dalej: stan synchronizacji i budżetu ruchu, „Synchronizuj teraz".
Panel jest nieprzezroczysty; dotknięcie poza nim zamyka go. **Pióro na liście**: element
wybiera się przy **puszczeniu** rysika (nie przy kontakcie), a przeciągnięcie w pionie
przewija listę — bez tego ustawienia przełączały się „same" przy próbie przewinięcia
(issue #8). Okno startuje z **osobnym oknem wyboru notatki** (kafelki jak w panelu, tyle kolumn,
ile się mieści; dotknięcie kafelka otwiera, poza oknem — zostajesz w bieżącej; przy
pierwszym uruchomieniu zamiast niego panel Konto z logowaniem), pasek narzędzi jest
domyślnie przypięty (issues #2, #15). Okno wyboru pokazuje się przy **każdym** otwarciu okna —
także po X i powrocie z traya / ikony; ochrona AMOLED chowa je jak resztę UI i przywraca po
falach, bo notatka wciąż nie jest wybrana. W Ustawieniach reaguje tylko **wartość** po prawej
(przycisk), etykieta jest bierna.
Zaproszenia do space'ów widać też na górze zakładki Notatki (#16); notatki otwarte
z LAN nie trafiają do „moich" notatek — są tylko w *Shared on LAN* (#11).
Pióro (*Ustawienia → Pen*): **Ignore pressure below** (off, 2–20 %) — kontakt zgłoszony
przez sterownik z naciskiem pod progiem traktujemy jak rysik w powietrzu: kreska się
kończy, a następna próbka nad progiem zaczyna nową; nacisk nad progiem jest
przeskalowany, więc krzywa grubości zaczyna się od progu (#4). **Width at lightest
touch** (5–50 % grubości pióra, domyślnie 12 %) — grubość przy zerowym nacisku; to
parametr rysowania, więc zmiana przebudowuje wszystkie kreski, także stare (#5).
Kursor: pod piórem schowany (czubek rysika jest wskaźnikiem; *Ustawienia → Display →
Cursor under the pen* przywraca krzyżyk), pod myszą krzyżyk, w czasie fal AMOLED
zawsze schowany (#3, #13). **Nigdy nie zakrywa paska
narzędzi**: przy lewej krawędzi staje obok niego, u góry i u dołu zaczyna się za nim
(przy prawej pasek i tak nie jest po drodze). Dzięki temu ☰ zostaje pod palcem —
drugie dotknięcie zamyka menu — a ustawienia paska (krawędź doku, przypięcie) widać
od razu na żywym pasku. Otwarte menu trzyma pasek na ekranie, mimo Z7.

**Synchronizacja (Etap 5).** Space jest repozytorium git — `libgit2` siedzi w binarce,
niczego nie trzeba instalować (ADR 0003/0006). Aplikacja sama robi `commit` 10 s po
ostatniej zmianie, a pełny cykl `fetch → merge → push` przy starcie, ukryciu i pokazaniu
okna — w tle, rysowanie nigdy na to nie czeka. Po zalogowaniu do GitHuba (Konto)
aplikacja **sama wykrywa albo zakłada prywatne repozytorium `spectrenotes-<space>`**
na Twoim koncie; druga maszyna z tym samym space'em (domyślnie `default`) dostaje
notatki automatycznie, bieżąca notatka odświeża się po zakończeniu kreski. Konflikt
merge jest strukturalnie niemożliwy (każdy autor pisze do własnych plików). Bez
logowania: tylko lokalna historia. Ruchu do GitHuba pilnuje budżet (odstępy, limity
godzinowe i dobowe z predykcją); odmowa serwera (`429`, „too many requests") wstrzymuje
sync na 10 min – 2 h z ostrzeżeniem w Konto i HUD, commity idą dalej lokalnie.

**Space'y współdzielone (Etap 6¾).** Space `default` jest zawsze prywatny. Wspólne
notatki żyją w **osobnych space'ach** — każdy to własne repozytorium
`spectrenotes-<nazwa>` na koncie założyciela ze **współpracownikami GitHub**;
kto ma dostęp do repo, ten widzi space. *Konto → Friends*: lista loginów GitHub
(`friends.txt` w prywatnym space'ie, synchronizowana jak notatki), sprawdzanych
przy dodaniu. *Konto → Shared spaces*: nowy space, przy własnym przyciski
„invite <znajomy>" (zaproszenie do repo), lista współpracowników, „leave".
Zaproszenia od innych pojawiają się tam same („Join …") — przyjęcie zakłada
lokalny katalog `spaces\<nazwa>` i podpina cudze repo. Lista notatek scala
wszystkie space'y (kafelek „in <space>"); **folder jest metadaną notatki**, więc
notatka przeniesiona do wspólnego space'u (*Notes → This note in a shared space*)
zostaje w swoim folderze, a u współpracownika ten folder pojawia się z nią.
Przeniesienie = ruch katalogu `notes/<ULID>` między repozytoriami (w starym commit
z usunięciem, w nowym z dodaniem) — notatka zawsze żyje w jednym repo. Nowa
notatka powstaje w space'ie bieżącej. Rejestr space'ów tej maszyny:
`%APPDATA%\SpectreNotes\spaces.txt`. Warstwa live w LAN na razie widzi tylko
space domyślny.

**Rysowanie na żywo w sieci (Etap 6).** Urządzenia w tej samej sieci **znajdują się
same** (multicast, bez internetu) i łączą po TCP; przez Tailscale — adres `host:port`
wpisany w *Konto → Sieć lokalna*. **Sieć widzi tylko to, co udostępnisz** (ADR 0008):
w menu *Notatki → This note on LAN* włączasz udostępnienie bieżącej notatki i opcjonalnie
ustawiasz hasło; u innych pojawia się ona w sekcji *Shared on LAN* (z kłódką, gdy
chroniona) — dotknięcie otwiera (pyta o hasło), ponowne zamyka, kopia zostaje.
Otwarta notatka płynie w obie strony: kreska drugiej osoby pojawia się **w trakcie
rysowania**, jej rysik jako kropka, tytuł natychmiast — niezależnie od GitHuba.
Hasło nie idzie siecią (dowód SHA-256 z kluczem pochodnym i nonce'ami), ale treść
płynie jawnym TCP — poufność daje LAN albo Tailscale. Cudze operacje lądują w plikach
`via-*.ops` w katalogu autora, więc git dalej nie ma jak się skonfliktować (ADR 0007).
**Cudza notatka otwarta z LAN leży poza space'ami** — w `%APPDATA%\SpectreNotes\lan\`,
katalogu bez gita: nie idzie do Twojego repo i nie jest „Twoją" notatką (widać ją
tylko w *Shared on LAN*). **Space'y współdzielone a LAN**: wszystkie notatki space'u
współdzielonego są w LAN udostępniane automatycznie (bez hasła) z nazwą space'u;
osoba, która ma ten space (przyjęła zaproszenie), widzi je w *Shared on LAN* i sesja
live otwiera się u niej sama — kreska idzie od razu, a plik ląduje w jej katalogu tego
space'u (tam, gdzie git i tak by go przyniósł). Kto space'u nie ma, tej notatki nie
zobaczy. Protokół live v3 (starsze wersje aplikacji nie połączą się).
Stan w Konto (*Sieć lokalna*) i w HUD-zie (peerzy, opóźnienie mokrej kreski);
wyłączenie w *Ustawienia → Sieć lokalna*. Zerwane połączenie wykrywane w ~12 s
(heartbeat). **Zapora**: `--install` (także instalator) dodaje regułę Windows Firewall dla
zainstalowanej binarki (`netsh` przez UAC — jedno pytanie o uprawnienia; odmowa nic nie
psuje, zostaje pytanie zapory przy pierwszym uruchomieniu), `--uninstall` ją zdejmuje.
Bez reguły live działa tylko między instancjami na tej samej maszynie. Testy
jednostkowe live słuchają wyłącznie na pętli zwrotnej i bez multicastu — inaczej każdy
`cargo test` (nowa binarka `spectre_sync-<hash>.exe`) budził pytanie zapory. Test na jednej maszynie:
druga instancja z innym `COMPUTERNAME` i `APPDATA` oraz własnym katalogiem space'u.

| Sterowanie | Działanie |
|---|---|
| pióro | rysowanie |
| rysik przy krawędzi doku | pasek: uchwyt, menu, pióro, gumka, kolory, grubość, undo/redo, lupa, procent, kłódka widoku, kłódka komputera (notatki: menu, `PgUp/PgDn`, `Ctrl+N`) |
| `Ctrl+kółko`, `0` | zoom wokół kursora, powrót do „dopasuj szerokość" (kolumna 2880 jednostek) |
| **przycisk gumki (trzymany)** | gumka kresek — usuwa całe kreski, które dotknie |
| **przycisk boczny (trzymany)** | przewijanie (w poziomie tylko przy odblokowanym widoku) |
| kółko myszy, `Home` | przewijanie, powrót na górę |
| `1`–`6` | kolor (paleta pod AMOLED) |
| `E` | gumka z klawiatury |
| `[` `]` | grubość (co 0,1 px) |
| `Ctrl+Z` / `Ctrl+Y` | cofnij / ponów (cofnięcie wymazania odtwarza kreskę) |
| `M` | menu: notatki w folderach, ustawienia, konto |
| `PgUp` / `PgDn` | poprzednia / następna notatka |
| `Ctrl+N` | nowa notatka |
| `F11`, `H`, `V`, `T` | pełny ekran, HUD (domyślnie wyłączony), vsync, tryb przewijania |
| `W` | fale przyciemnienia AMOLED na stałe (podgląd/debug; ponowne `W` wyłącza). Normalnie startują same po czasie z ustawień |
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

### Ochrona AMOLED liczy bezczynność człowieka

Odliczanie do fal bierze bezczynność **całego systemu** (`GetLastInputInfo`),
nie tylko tego okna. Okno nieaktywne nie dostaje żadnych komunikatów wejścia,
więc bez tego ochrona wchodziła (razem z pełnym ekranem) w trakcie pisania
w innej aplikacji. `W` (wymuszony podgląd) omija sprawdzenie.

Który ekran jest panelem laptopa, liczy `display::is_internal_monitor` z trzech
przypadków, bo „nie ma aktywnego panelu wbudowanego" znaczy co innego na laptopie,
a co innego na stacjonarce: (1) panel świeci — porównanie po nazwie GDI;
(2) panel jest **wygaszony**, ale maszyna go ma (`QDC_ALL_PATHS` widzi też wyjścia
nieaktywne) — chronionego ekranu teraz nie ma, więc **żaden** monitor nim nie jest;
(3) maszyny bez panelu (komputer stacjonarny) — „głównym" jest monitor podstawowy.
Bez (2) przypadek (3) łapał laptopa z zamkniętą klapą i ochrona wchodziła na
monitorze zewnętrznym mimo *Laptop screen only* (zgłoszenie z 24 IX, NVIDIA Surround).

Z jednym wyjątkiem: przy **tylko na ekranie laptopa** chronimy jeden panel,
a praca na innym monitorze go nie dotyczy. Gdy kursor **i** okno pierwszego
planu są na innym ekranie (stacja dokująca), wejście nie liczy się do
odliczania (`display::input_elsewhere`) — notatka na panelu laptopa gaśnie tak,
jakby użytkownik odszedł od biurka, mimo że on cały czas pisze na monitorze
stacji. Warunek jest celowo ostrożny: wystarczy, że kursor albo pierwszy plan
wrócą na panel, i wejście znów się liczy (a rysik nad samą notatką idzie przez
`WM_POINTER`, więc budzi ją natychmiast, w jednej klatce). Ochrona wchodzi
wtedy **bez zabierania pierwszego planu** (`SWP_NOACTIVATE`) — aplikacja, w
której się pracuje, zostaje aktywna. Bez tego ustawienia chronimy ekran, na
którym użytkownik właśnie pracuje, i każde wejście odsuwa fale.

### DPI i piksele

Całe UI aplikacji (pasek, zakładki, menu) jest liczone w **pikselach logicznych**
(96 DPI) i przeliczane na fizyczne raz, przy układzie, przez skalę DPI monitora
(`window::dpi_scale`) — tak jak Windows liczy swój pasek zadań. Nic nie jest
skalowane w trakcie rysowania: każdy prostokąt, glif i czcionka DWrite ma
policzony docelowy rozmiar (ostrość i brak kosztu skalowania rastra). Po
przeniesieniu okna na monitor o innym DPI `WM_DPICHANGED` przelicza układ i
odtwarza czcionki. **Canvas jest osobny** — kreski żyją we własnych jednostkach,
zoom to sprawa kamery, nie DPI. **Zmiana rozmiaru okna zoomu nie rusza**: notatka
otwiera się dopasowana do szerokości okna, a potem mniejsze okno pokazuje mniej
canvasu, a większe więcej — kreska ma zawsze ten sam rozmiar fizyczny. Dopasowanie
jest na żądanie: procent na pasku albo `0`.

### Testy GUI bez ruszania myszy

Proces uruchomiony ze zmienną `SPECTRENOTES_TEST_INPUT=1` przyjmuje pióro jako
tekst przez `WM_COPYDATA` (UTF-8, jedna komenda na linię): `down X Y [barrel|eraser] [pN]`,
`move X Y [pN]`, `up`, `hover X Y` — współrzędne w pikselach okna, `pN` = nacisk 0..1;
`width W` i `zoom Z` ustawiają grubość pióra i zoom, `tool pen|eraser|select` wybiera
narzędzie, `esc` kończy zaznaczenie. `SPECTRENOTES_NO_LAN=1` wyłącza
warstwę LAN bez tworzenia gniazd (testy i benchmarki z nowych ścieżek binarki nie
pytają zapory). Próbki idą tą samą
drogą co z `WM_POINTER` (pasek, menu, canvas), tylko bez dekodera. Klawisze można
podać `PostMessage(WM_KEYDOWN)`, przesuwanie okna sprawdzić pytaniem `WM_NCHITTEST`
(2 = `HTCAPTION`). Skrypt testowy nie zabiera więc kursora ani fokusu osobie, która
w tym czasie pracuje. Bez tej zmiennej `WM_COPYDATA` jest ignorowane.
Do okna feedbacku (tylko z tą zmienną): `SPECTRENOTES_FEEDBACK_FAKE=ok|err` udaje
zalogowanie i kończy wysyłkę podanym wynikiem bez GitHuba (scenariusz paska, ekrany
końcowe, załączniki), a `SPECTRENOTES_FEEDBACK_DUMP=<plik.png>` zapisuje na dysk zrzut
z *Attach app screenshot*.
`SPECTRENOTES_PDF_OUT=<plik.pdf>` omija okno zapisu przy eksporcie PDF.

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
  spectre-sync       store + git (libgit2, ADR 0006) + budzet ruchu + live (TCP, ADR 0007; udostepnianie per notatka, ADR 0008)  [dziala]
  spectre-render     Direct2D na DXGI flip-model, przewijanie przyrostowe          [dziala]
  spectre-shell-win  okno Win32, WM_POINTER, DPI, feedback piora, pelny ekran, WinHTTP, instalacja  [dziala]
  spectre-update     wydania GitHub: wersje, pobieranie z weryfikacja SHA-256           [dziala]
  spectre-app        binarka `spectrenotes`; ikona `icon.ico` (generator `icon/make-icon.ps1`), zasoby Win32 przez `build.rs` [dziala]
tools/
  inkdemo            demo odczucia piora (Etap 0)                                 [dziala]
  setup              SpectreNotes-Setup.exe: pobiera i instaluje najnowsze wydanie [dziala]
release.ps1          wydanie: wersja -> testy -> build -> tag -> GitHub Release (3 ostatnie)
docs/                zalozenia, architektura, ADR-y
```

`spectre-proto`, `-core`, `-ink` i `-sync` nie mają żadnej zależności od Windows
(`-sync` używa `socket2` tylko do multicastu z `SO_REUSEADDR`).
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
| [`docs/07-BENCHMARK.md`](docs/07-BENCHMARK.md) | rozmiar, start, RAM per wersja (`tools/bench-startup.ps1`) |
| [`docs/adr/`](docs/adr/) | decyzje architektoniczne wraz z odrzuconymi wariantami |

## Wymagania budowania

- Rust stable (MSVC), Windows 10 1809+ / Windows 11
- VS Build Tools z workloadem C++ oraz Windows SDK 10 (`rc.exe` z SDK kompiluje zasoby: ikonę i blok wersji)

## Licencja

MIT
