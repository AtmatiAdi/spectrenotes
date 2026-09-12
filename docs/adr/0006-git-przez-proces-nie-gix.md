# ADR 0006 — Warstwa git przez proces `git`, nie przez `gix`

**Status:** przyjęte · 2026-09-12

## Kontekst

ADR 0003 ustala git jako warstwę trwałą; ADR 0001 i `01-ARCHITEKTURA.md` zakładały
implementację przez `gix` (gitoxide, czysty Rust). Przy wejściu w Etap 5 sprawdziliśmy
stan gitoxide: fetch i merge są, **push jest na poziomie plumbing** (bez gotowego
przepływu do zdalnego repozytorium), a uwierzytelnianie do GitHuba trzeba by zbudować
samodzielnie (OAuth device flow, przechowywanie tokenu). To dwie rzeczy, które właśnie
są całą treścią Etapu 5.

Na maszynie docelowej i na każdej maszynie deweloperskiej jest Git for Windows, a razem
z nim Git Credential Manager (GCM) i zwykle `gh`.

## Decyzja

`spectre-sync::git` opakowuje polecenia **`git`** uruchamiane jako proces potomny:
bez okna konsoli (`CREATE_NO_WINDOW`), bez pytań na terminalu (`GIT_TERMINAL_PROMPT=0`),
zawsze z osobnego wątku (`spectre-app::sync`), nigdy z wątku wejścia ani renderu.

- **Uwierzytelnianie = GCM.** „Zaloguj przez GitHub" to `git credential fill` dla
  `github.com`: GCM otwiera przeglądarkę, robi OAuth i zapisuje token w Menedżerze
  poświadczeń Windows. Stan logowania odczytujemy tym samym poleceniem
  z `GCM_INTERACTIVE=never`. Wylogowanie to `git credential reject`.
- **Zdalne repo** ustawia użytkownik adresem (`Ctrl+V` w polu) albo jednym przyciskiem
  przez `gh repo create --private` (gdy `gh` jest w PATH).
- **Cykl**: `add -A` → `commit` → `fetch` → `merge --allow-unrelated-histories` → `push`.
  Merge, który się nie powiedzie, jest natychmiast przerywany (`merge --abort`) —
  repo nigdy nie zostaje w połowie scalenia.
- **Atrybuty**: `*.ops binary`, `folders.txt merge=union` (jedyny plik pisany przez
  wielu autorów — suma linii). `.cache/` w `.gitignore`.
- **Tożsamość commitów** z globalnej konfiguracji gita; gdy jej brak — lokalnie
  `user@device` z adresem `@spectrenotes.invalid`.

Interfejs `Git` jest wąski (init, commit_all, fetch, merge, push, sync, log_note,
credential_*), żeby podmiana na `gix` była lokalna.

## Odrzucone

- **`gix`** — brak push i uwierzytelniania (patrz kontekst). Wracamy, gdy dojrzeje.
- **`git2` (libgit2)** — ma push i credentials, ale wymaga własnej obsługi OAuth
  do GitHuba i kompilacji C; nie daje nic ponad proces `git` z GCM, a kosztuje czas
  budowania i drugą implementację merge.

## Konsekwencje

- **Wymóg: Git for Windows w PATH.** Bez niego aplikacja działa lokalnie (menu Konto
  mówi wprost, czego brakuje). Przy rozdawaniu aplikacji (Etap 7) rozstrzygniemy,
  czy dociągać MinGit (~40 MB) automatycznie.
- Koszt jednego cyklu to kilka procesów `git` (setki ms lokalnie, sekundy przy pushu) —
  nieistotny, bo dzieje się w tle 10 s po odłożeniu rysika, przy ukryciu i pokazaniu okna.
- Commit może objąć plik `.ops` z niedokończonym rekordem na końcu (autor dopisuje w tej
  samej chwili). Czytnik obcina uszkodzony ogon, a następny commit dopisze całość —
  to ta sama własność, która daje odporność na utratę zasilania.
