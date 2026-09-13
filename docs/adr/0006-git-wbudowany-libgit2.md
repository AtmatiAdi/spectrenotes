# ADR 0006 — Git wbudowany w binarkę (`libgit2`), GitHub bez zewnętrznych narzędzi

**Status:** przyjęte · 2026-09-13 (zastępuje wariant z procesem `git` z 2026-09-12)

## Kontekst

ADR 0003 ustala git jako warstwę trwałą; ADR 0001 zakładał `gix`. Przy wejściu
w Etap 5 okazało się, że w gitoxide **push jest na poziomie plumbing**, a uwierzytelnianie
do GitHuba trzeba by pisać samemu. Pierwsza wersja Etapu 5 wołała proces `git`
z Git Credential Managerem — działała, ale uzależniała aplikację od tego, co użytkownik
ma zainstalowane, i wymagała ręcznego wskazywania repozytorium. Wymaganie produktowe:
**aplikacja ma wszystko w sobie, a repozytorium wykrywa i zakłada sama.**

## Decyzja

1. **`libgit2` statycznie w binarce** (crate `git2`, `vendored-libgit2`, HTTPS przez
   systemowy WinHTTP — TLS i proxy z Windows, bez OpenSSL). Fetch, merge, push kompletne.
   Binarka: ~2 MB. Zero zależności od `git.exe`.
2. **Merge bez wspólnego przodka** (dwa niezależne `init` na dwóch maszynach) jest
   normalną ścieżką: libgit2 scala z pustym przodkiem. Jedyny plik pisany przez wielu
   (`folders.txt`) ma `FileFavor::Union`. Merge liczy się w pamięci; drzewo robocze
   dostaje dopiero gotowy wynik, więc błąd w środku nie zostawia repo w połowie scalenia.
3. **Repozytorium automatyczne**: `spectrenotes-<nazwa katalogu space'u>` na koncie
   zalogowanego użytkownika. Aplikacja pyta GitHub REST API, czy istnieje; jeśli nie —
   zakłada prywatne; ustawia jako `origin`. Użytkownik nie widzi adresów ani przycisków
   „utwórz repo". Dwie maszyny współdzielą space, gdy katalog nazywa się tak samo
   (domyślnie `default`).
4. **Logowanie GitHub Device Flow** (OAuth): aplikacja pokazuje kod, otwiera
   `github.com/login/device`, odpytuje do zatwierdzenia. Token trafia do
   `%APPDATA%\SpectreNotes\github.token` zaszyfrowany DPAPI (tylko to konto Windows
   na tej maszynie go odczyta). Wymaga `client_id` aplikacji OAuth zarejestrowanej
   na GitHubie z włączonym Device Flow (`github.rs::CLIENT_ID`, nadpisywalne
   `github_client_id=` w `config.txt`). Zapasowo: wklejenie tokenu (PAT, zakres `repo`).
   **Logowania samą nazwą i hasłem nie ma** — GitHub wyłączył je dla API i git po
   HTTPS w 2021; Device Flow (przeglądarka) jest jego bezpiecznym odpowiednikiem.
   Nagłówek menu Konto pokazuje **zdjęcie profilowe** zalogowanego (jedno pobranie
   z GitHuba, dekodowane przez systemowy WIC do BGRA i rysowane pędzlem bitmapowym
   D2D; cache w `avatar.img`, po restarcie bez ruchu sieci), nazwę i wiek synchronizacji.
5. **Budżet ruchu** (`spectre-sync::budget`): księga połączeń z 24 h (plik `traffic.txt`),
   minimalny odstęp między cyklami 30 s, twarde progi 120 połączeń/h, 1200/dobę,
   500 MB/dobę, **predykcja** — tempo z ostatniej godziny rzutowane na dobę rozciąga
   odstęp, gdy przekroczyłoby budżet. Odpowiedź `429`/`403` z tekstem o limicie
   → **odczekanie** 10 min, potem ×2 do 2 h; commity idą lokalnie, menu Konto i HUD
   pokazują „WSTRZYMANA do HH:MM — GitHub zgłosił limit ruchu (za dużo prób)".
6. HTTP do GitHub API własnym klientem na WinHTTP (`spectre-shell-win::http`), JSON
   parsowany ręcznie (kilka płaskich pól) — bez serde, bez reqwest.

## Odrzucone

- **Proces `git` + GCM** (poprzednia wersja tego ADR): zależność od instalacji użytkownika,
  konieczność wskazywania repo; ~7 s testów vs 0,5 s przez bibliotekę.
- **`gix`** — brak push i uwierzytelniania. Wracamy, gdy dojrzeje; interfejs `Git` jest wąski.
- **serde/reqwest** — trzy odpowiedzi JSON nie uzasadniają największych crate'ów w binarce.

## Konsekwencje

- Zależność od kompilatora C przy budowaniu (libgit2) — VS Build Tools są już wymagane.
- Bez `client_id` użytkownik loguje się tokenem PAT — do zarejestrowania aplikacji OAuth
  przez właściciela projektu (jednorazowo, darmowe).
- Commit może objąć plik `.ops` z niedokończonym rekordem na końcu (autor dopisuje w tej
  samej chwili). Czytnik obcina uszkodzony ogon, następny commit dopisze całość — ta sama
  własność, która daje odporność na utratę zasilania.
- Zweryfikowane end-to-end na GitHubie: dwie „maszyny" (różny `COMPUTERNAME`) z tym samym
  space'em — pierwsza założyła repo i wypchnęła, druga pobrała notatki, scaliła niezależną
  historię i wypchnęła; pierwsza odebrała fast-forwardem. Księga ruchu: 2 połączenia
  i 0,6–8,6 kB na cykl.
