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
niczego potrzebne). Wymaga czystego drzewa na `main`, zalogowanego `gh` i pliku
`%USERPROFILE%\.spectrenotes-oauth-secret` (sekret OAuth App do logowania
przeglądarką — `build.rs` wkleja go do binarki; bez niego wydanie umie tylko
Device Flow).
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

## Fałszywy alarm antywirusa (issue #21)

22 IX 2026 Defender zablokował 0.3.7 jako `Trojan:Win32/Barefoos.A!ml`.
Sufiks `!ml` znaczy „werdykt modelu, nie sygnatury": świeża, **niepodpisana**
binarka bez reputacji, która przy pierwszym uruchomieniu robi dokładnie to, co
robi złośliwe oprogramowanie — dopisuje się do `HKCU\...\Run`, chowa się do
traya, pobiera plik `.exe` z sieci i podmienia nim samą siebie. Każda z tych
rzeczy jest tu funkcją, ale razem wyglądają jak wzorzec. To wyjaśnienie, nie
usprawiedliwienie: skutek dla użytkownika jest taki, że aplikacja znika z dysku.

Co z tym robimy:

- **`release.ps1` skanuje artefakty Defenderem przed publikacją**
  (`MpCmdRun.exe -Scan -ScanType 3 -File`) i przerywa wydanie, gdy plik jest
  zgłoszony. Lepiej, żeby dowiedział się o tym wydający niż użytkownik.
- Zgłoszenie próbki jako fałszywego alarmu:
  <https://www.microsoft.com/en-us/wdsi/filesubmission> (kategoria *Software
  developer*, „Incorrectly detected as malware"). Werdykt bywa w kilka godzin
  i wraca aktualizacją sygnatur do wszystkich.
- Plik z kwarantanny odzyskuje się w *Ochrona przed wirusami → Historia
  ochrony → Przywróć*; sumy z `SHA256SUMS.txt` pozwalają sprawdzić, że to ten
  plik z wydania.
- Prawdziwe rozwiązanie to podpis kodu (patrz tabela wyżej) — ta sama decyzja
  i ten sam koszt co przy SmartScreenie.

Czego **nie** robimy: nie dodajemy wykluczeń Defendera z instalatora ani nie
prosimy o nie użytkownika. Katalog, do którego updater wrzuca nowe pliki, ma
być skanowany.

Autostart przeżywa taką remediację: antywirus, czyszcząc „trwałość", kasuje
wpis w `HKCU\...\Run` razem z plikiem, więc aplikacja po prostu przestawała
startować z systemem (a w Ustawieniach stało *Off*, bez śladu). Wybór
użytkownika pamięta teraz `config.txt` (`autostart=1`), a `App::autostart_sync`
przy każdym starcie **zainstalowanej** kopii odtwarza wpis, jeśli zniknął albo
wskazuje na starą ścieżkę; naprawa zostawia linię w `partner.log`.

## Kompilacja u odbiorcy

Alternatywa dla podpisu: odbiorca buduje ze źródeł (`.\dev.cmd -Install`,
`cargo build --release`). Wymaga Rusta i VS Build Tools, więc realnie działa
tylko dla osób technicznych — ale omija SmartScreen w całości. Tak zbudowana
binarka też aktualizuje się z wydań.

## Defender i pierwszy start nowej binarki

Każdy **nowy plik exe** Defender skanuje przy pierwszym uruchomieniu — zanim
proces dojdzie do `main`. Zmierzone (`tools/bench-startup.ps1`, ta sama binarka):
świeża kopia 214–236 ms do okna, każdy kolejny start 108–125 ms. Wynik skanu
zostaje przy pliku (także po `rename`), więc skan jest jednorazowy — ale przy
częstych wydaniach to właśnie „start po aktualizacji", który użytkownik zapamiętuje.

Nie da się skanu wyłączyć z poziomu aplikacji (i nie chcemy: wyłączenie
Defendera dla katalogu, do którego updater wrzuca nowe pliki, to dziura), podpis
kodu też go nie omija (podpis to SmartScreen/reputacja, nie skanowanie). Da się
go **przesunąć w czas, gdy nikt nie czeka**: updater po pobraniu i weryfikacji
sumy uruchamia `spectrenotes-<wersja>.exe --warmup` (proces wychodzi natychmiast,
`install::warmup`, limit 10 s), a instalator robi to samo ze skopiowaną binarką
przed jej uruchomieniem. Restart po podmianie startuje wtedy jak każdy inny:
zmierzone 119–137 ms zamiast 214–236.
