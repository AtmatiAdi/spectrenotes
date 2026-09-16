# Dystrybucja i aktualizacje

## Postać dystrybucyjna

Jeden `.exe` (`spectrenotes.exe`, ok. 2,5 MB): statyczna binarka Rusta,
bez runtime'u. Działa przenośnie (uruchomiona skądkolwiek), a instalacja to
ten sam plik skopiowany do katalogu użytkownika — bez uprawnień administratora:

| co | gdzie |
|---|---|
| binarka | `%LOCALAPPDATA%\SpectreNotes\app\spectrenotes.exe` |
| skrót | menu Start bieżącego użytkownika (`SpectreNotes.lnk`) |
| wpis „Zainstalowane aplikacje" | `HKCU\...\Uninstall\SpectreNotes` (odinstalowanie: `spectrenotes.exe --uninstall`) |
| autostart (opcja w Settings) | `HKCU\...\Run` → `spectrenotes.exe --tray` |
| **dane i konfiguracja** | `%APPDATA%\SpectreNotes\` — **instalacja i odinstalowanie ich nie dotykają** |

`spectrenotes.exe --install` robi całą instalację sam (zamyka działającą
instancję, kopiuje się, tworzy skrót i wpis, uruchamia zainstalowaną kopię).
Kod: `spectre-shell-win::install`.

## Wydania (GitHub Releases)

Wydania żyją w **osobnym, publicznym repozytorium** `AtmatiAdi/spectrenotes-releases`
(stała `RELEASES_REPO` w `spectre-update` — jedno źródło prawdy, z niego bierze
adres i aplikacja, i instalator, i `release.ps1`); źródła zostają prywatne. Wydanie
to tag `vX.Y.Z` z trzema zasobami o **stałych nazwach**, żeby `releases/latest/download/<nazwa>` był trwałym adresem:

| zasób | co to |
|---|---|
| `spectrenotes.exe` | aplikacja |
| `SpectreNotes-Setup.exe` | instalator (patrz niżej) |
| `SHA256SUMS.txt` | sumy obu plików w formacie `sha256sum` |

```
.\release.ps1 -Bump patch            # 0.2.0 -> 0.2.1
.\release.ps1 -Version 0.3.0 -Notes "..."
.\release.ps1 -Bump minor -DryRun    # wszystko lokalnie, bez pushu i release'u
```

Skrypt: podnosi `version` w `[workspace.package]`, `cargo test --workspace`,
`cargo build --release`, liczy sumy, commituje „Wydanie vX.Y.Z", taguje, pushuje,
`gh release create`, a na końcu **kasuje wydania starsze niż 3 ostatnie** (tagi
zostają — to tylko referencje do commitów, a zasoby starych wydań nie są do
niczego potrzebne). Wymaga czystego drzewa na `main` i zalogowanego `gh`.
Opis wydania domyślnie to lista commitów od poprzedniego tagu `v*`.

Wersja binarki = `CARGO_PKG_VERSION` z workspace'u; aplikacja porównuje ją
z tagiem najnowszego wydania (`spectre-update::Version`, semver bez sufiksów).

## Instalator

`SpectreNotes-Setup.exe` (`tools/setup`) sam **nic nie zawiera**: pyta GitHub
o najnowsze wydanie, pobiera `spectrenotes.exe` z paskiem postępu, sprawdza
SHA-256 z `SHA256SUMS.txt` tego samego wydania i uruchamia pobrany plik
z `--install`. Dzięki temu instalator się nie starzeje — ten sam plik pobrany
pół roku temu zainstaluje bieżącą wersję. `--repo owner/repo` wskazuje inne
repozytorium, `--token` (albo `GITHUB_TOKEN`) daje dostęp do prywatnego.

## Aktualizacje w aplikacji

```
start + 5 s, potem co 10 min (albo przycisk w menu)
   → GET /repos/<repo>/releases/latest   (spectre-update, WinHTTP, bez zależności)
   → nowsza wersja?  → Settings → Application: "version X available (2.3 MB)  [Download]"
   → Download        → pasek postępu, anulowanie; plik w %LOCALAPPDATA%\SpectreNotes\updates\
   → weryfikacja SHA-256 (bez sumy nie instalujemy)
   → [Install and restart]
        spectrenotes.exe -> spectrenotes.old.exe    (rename działa na uruchomionym pliku)
        nowy plik -> spectrenotes.exe
        zapis stanu, zamknięcie, start nowej binarki z tym samym space'em
   → nowa instancja na starcie kasuje spectrenotes.old.exe
```

Wszystko poza podmianą dzieje się w osobnym wątku (`spectre-app::update`,
jak `sync`); wątek renderu nigdy nie czeka na sieć. Podmiana jest natychmiastowa,
nie „przy następnym starcie" — proces rezydentny (Z2) mógłby nie być
restartowany tygodniami, a użytkownik, który kliknął „Install", chce widzieć
efekt. Aktualizacja **nigdy nie zaczyna się sama**: bez kliknięcia „Download"
nic nie jest pobierane, a bez „Install" nic nie jest podmieniane.

Wyłączenie automatu: `Check for updates automatically` w Settings
(`update_check=0` w `config.txt`). Inne repozytorium wydań: `update_repo=owner/repo`.

Jeśli aplikacja uruchomiona jest spoza katalogu instalacji (np. `target\release`),
aktualizacja podmienia **ten** plik — updater nie przenosi aplikacji.

## Dlaczego osobne publiczne repo wydań

Zasoby wydań w **prywatnym** repozytorium wymagają tokenu — anonimowe zapytanie
dostaje 404, a token logowania w aplikacji to PAT ograniczony do repo notatek
(16 IX 2026: właściciel dostawał 404 na własne wydania). Publiczne repo tylko
na binarki rozwiązuje to dla wszystkich bez żadnego tokenu, a źródła zostają
prywatne. Prywatne repo wydań nadal działa (`update_repo=owner/repo` +
token logowania z dostępem, albo `GITHUB_TOKEN`; instalator: `--repo`, `--token`). Dołączenie kogoś do
**space'u** to osobna sprawa (03-FORMAT-I-SYNC): dostęp do jego prywatnego repo
notatek i węzeł w tej samej sieci Tailscale dla warstwy live.

## Otwarty problem: SmartScreen

Niepodpisana binarka pobrana z internetu dostaje pełnoekranowe ostrzeżenie
„Windows protected your PC". Dotyczy `SpectreNotes-Setup.exe` pobranego
przeglądarką; `spectrenotes.exe` pobrany **przez instalator lub updater**
(WinHTTP) nie ma znacznika „z internetu" (Mark of the Web), więc nie wywołuje
ostrzeżenia — a SHA-256 z tego samego wydania pilnuje, że to ten plik.

| | Koszt | Efekt |
|---|---|---|
| Brak podpisu | 0 | ostrzeżenie przy instalatorze, zawsze |
| Certyfikat self-signed | 0 | ostrzeżenie nadal; pomaga tylko przy ręcznym zaufaniu certyfikatowi |
| Certyfikat OV | kilkaset zł/rok | ostrzeżenie znika dopiero po zbudowaniu reputacji przez SmartScreen |
| Certyfikat EV | drożej, wymaga tokena sprzętowego | reputacja od pierwszego uruchomienia |
| Microsoft Store | opłata jednorazowa + proces certyfikacji | brak ostrzeżeń, ale kłóci się z wydaniami przez GitHub |

Decyzja odłożona — w gronie kilku znajomych da się żyć z ręcznym
„Więcej informacji → Uruchom mimo to" raz, przy instalatorze.

## Kompilacja u odbiorcy

Alternatywa dla podpisu: odbiorca buduje ze źródeł (`.\dev.cmd -Install`,
`cargo build --release`). Wymaga Rusta i VS Build Tools, więc realnie działa
tylko dla osób technicznych — ale omija SmartScreen w całości. Tak zbudowana
binarka też aktualizuje się z wydań.
