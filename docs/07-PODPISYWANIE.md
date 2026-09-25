# Podpisywanie kodu przez SignPath Foundation

Cel: podpisane wydania, żeby zniknęło ostrzeżenie SmartScreen przy instalatorze
i żeby heurystyka Defendera przestała zjadać świeże binarki (issue #21).
Droga: **SignPath Foundation** — darmowe podpisywanie dla projektów open source,
certyfikatem OV, który ma już zbudowaną reputację.

Warunki, które SignPath stawia, i jak je spełniamy:

| warunek | stan |
|---|---|
| licencja zatwierdzona przez OSI, bez dual-licensingu i części własnościowych | do zrobienia (Faza 1) |
| publicznie dostępny kod | repo źródeł jest prywatne — do otwarcia (Faza 3) |
| projekt utrzymywany i **już wydany** w podpisywanej postaci | jest: wydania 0.1–0.3.7 |
| binarki budowane ze źródeł **w sposób weryfikowalny** | do zrobienia: wydanie z GitHub Actions (Faza 2) |
| MFA u wszystkich w zespole (SignPath i repo) | do sprawdzenia (Faza 3) |
| instalator ma też odinstalowanie | jest: `--uninstall` + wpis w „Zainstalowane aplikacje" |
| nie narzędzie ofensywne, nie narusza prywatności | jest |
| każde wydanie zatwierdzane ręcznie przed podpisaniem | tak działa polityka „release" |

Wydawcą widocznym w Windows będzie **SignPath Foundation**, nie nasza nazwa —
taka jest cena darmowego certyfikatu i to właśnie ten certyfikat ma reputację.

## Układ repozytoriów po zmianie

Bez zmian dla użytkowników: aplikacja dalej sprawdza aktualizacje w publicznym
repo wydań (`RELEASES_REPO`), więc **adres aktualizacji się nie zmienia** i nikt
nie musi nic przeinstalowywać.

```
AtmatiAdi/spectrenotes            (źródła)  prywatne -> PUBLICZNE, tu działa CI
AtmatiAdi/spectrenotes-releases   (wydania) publiczne, bez zmian - tu lądują pliki
```

## Fazy

### Faza 1 — przygotowanie repo (robi Claude, lokalnie, odwracalne)

- `LICENSE` (wybór licencji — patrz *Decyzje*),
- w `README.md` sekcja po angielsku: co to jest, jak zbudować ze źródeł
  (recenzent SignPath czyta opis projektu, a strona pobierania ma opisywać
  funkcjonalność),
- to samo streszczenie po angielsku w README repo wydań,
- przegląd historii pod kątem sekretów — **zrobiony 25 IX**: brak tokenów,
  kluczy prywatnych i danych notatek; sekret OAuth nigdy nie był w repo
  (`build.rs` + `option_env!` z `%USERPROFILE%\.spectrenotes-oauth-secret`),
  w historii nie ma też binariów (60 MB packa to kolejne wersje `app.rs`),
- ten dokument.

### Faza 2 — wydanie z CI (robi Claude)

`.github/workflows/release.yml`, wyzwalane tagiem `v*`:

1. `windows-latest`, toolchain z `rust-toolchain.toml`,
2. `cargo test --workspace` (z `SPECTRENOTES_NO_LAN=1`),
3. `cargo build --release -p spectre-app -p spectrenotes-setup`, sekret OAuth
   z `secrets.SPECTRENOTES_OAUTH_SECRET`,
4. skan Defendera na artefaktach (to, co dziś robi `release.ps1`),
5. wysłanie do podpisu (Faza 5) i pobranie podpisanych plików,
6. `SHA256SUMS.txt` liczone **po podpisaniu** (podpis zmienia bajty!),
7. `gh release create` w repo wydań (token z `secrets.RELEASES_TOKEN`),
8. zostawienie trzech ostatnich wydań.

`release.ps1` zostaje do buildów lokalnych i prób, ale **oficjalne wydanie idzie
z CI** — inaczej nie ma mowy o „weryfikowalnym buildzie ze źródeł".

### Faza 3 — otwarcie repo (jedno słowo od Ciebie, komendy odpala Claude)

1. `git push` sześciu commitów, które leżą lokalnie,
2. `gh repo edit AtmatiAdi/spectrenotes --visibility public --accept-visibility-change-consequences`,
3. sekrety repozytorium:
   - `SPECTRENOTES_OAUTH_SECRET` — z pliku w `%USERPROFILE%`, wstawi Claude,
   - `RELEASES_TOKEN` — **musisz wygenerować sam** (patrz *Co robisz ręcznie*),
4. sprawdzenie, że masz włączone 2FA na GitHubie.

### Faza 4 — wniosek do SignPath (tylko Ty, ~10 min)

<https://signpath.org/apply> → *Apply for a free SignPath.io subscription*.
Wniosek jest wiązany z tożsamością maintainera, więc nie da się go wysłać
„przez kogoś". Gotowy tekst do wklejenia przygotuje Claude (Faza 1).
Czas rozpatrzenia: od kilku dni do kilku tygodni.

### Faza 5 — konfiguracja SignPath po akceptacji (klikasz Ty, wartości daje Claude)

W panelu SignPath:

1. *Trusted Build Systems* → dodaj predefiniowany **GitHub.com** i podłącz do
   organizacji,
2. *Project* `spectrenotes` → podłącz do niego ten trusted build system,
3. *Artifact Configuration* — zip z dwoma plikami (`spectrenotes.exe`,
   `SpectreNotes-Setup.exe`), oba podpisywane Authenticode,
4. *Signing Policy* `release` z **ręcznym zatwierdzaniem**,
5. *API token* z uprawnieniem submittera → do sekretu `SIGNPATH_API_TOKEN`.

Potrzebne do workflow (podasz je Claude'owi, nie są tajne):
`organization-id`, `project-slug`, `signing-policy-slug`,
`artifact-configuration-slug`.

Krok w workflow:

```yaml
- uses: signpath/github-action-submit-signing-request@v3
  with:
    api-token: '${{ secrets.SIGNPATH_API_TOKEN }}'
    organization-id: '<organization-id>'
    project-slug: 'spectrenotes'
    signing-policy-slug: 'release'
    artifact-configuration-slug: '<artifact-config>'
    github-artifact-id: '${{ steps.upload-unsigned.outputs.artifact-id }}'
    wait-for-completion: true
    output-artifact-directory: 'dist-signed'
```

### Faza 6 — pierwsze podpisane wydanie

Claude odpala tag, CI buduje i wysyła do podpisu, **Ty klikasz „Approve"**
w panelu SignPath (tak działa polityka release), CI pobiera podpisane pliki,
liczy sumy i publikuje wydanie.

Weryfikacja (Claude, z pomiarem):

- `signtool verify /pa /v` na obu plikach,
- `Get-AuthenticodeSignature` → `Valid`, wydawca `SignPath Foundation`,
- skan Defendera,
- pobranie instalatora przeglądarką na czystym koncie i sprawdzenie, czy
  SmartScreen jeszcze straszy (to jedyny test, który mówi prawdę o reputacji).

## Co robisz ręcznie (minimum)

1. **Decyzje** — licencja i e-mail w historii (patrz niżej). 30 sekund.
2. **Zgoda na push i upublicznienie repo.** Komendy odpala Claude.
3. **PAT do repo wydań** (~2 min), bo token musi powstać na Twoim koncie:
   GitHub → *Settings* → *Developer settings* → *Personal access tokens* →
   *Fine-grained tokens* → *Generate new token*:
   - *Resource owner*: AtmatiAdi, *Repository access*: tylko
     `spectrenotes-releases`,
   - *Permissions* → *Repository permissions* → **Contents: Read and write**,
   - ważność 1 rok, *Generate*, skopiuj token,
   - w terminalu (token nie przechodzi przez czat):
     `gh secret set RELEASES_TOKEN --repo AtmatiAdi/spectrenotes` i wklej.
4. **Wniosek do SignPath** (~10 min, gotowy tekst dostaniesz).
5. **Po akceptacji: konfiguracja w panelu SignPath** (~15 min, wartości
   dostaniesz) + `gh secret set SIGNPATH_API_TOKEN --repo AtmatiAdi/spectrenotes`.
6. **Przy każdym wydaniu: jedno kliknięcie „Approve"** w SignPath.

Wszystko inne — licencja, README, CI, integracja, wydania, weryfikacja,
dokumentacja — robi Claude.

## Decyzje do podjęcia

**Licencja.** Musi być z listy OSI, bez komercyjnego dual-licensingu.
- `GPL-3.0` — kto forkuje i wydaje, musi zostawić to otwarte (rekomendacja),
- `MIT` — każdy może wszystko, łącznie z zamknięciem forka.

**E-mail w historii.** 108 commitów jest podpisanych adresem
`atmati.adi@gmail.com`; po otwarciu repo staje się on publiczny. W repo wydań
używasz adresu `32931612+AtmatiAdi@users.noreply.github.com`, więc dziś nigdzie
publicznie go nie ma. Historię da się przepisać na adres noreply **zanim** repo
stanie się publiczne (potem już nie ma sensu). Rewrite zmienia wszystkie sumy
commitów — jeśli masz klon na drugiej maszynie, trzeba go sklonować od nowa.

## Czego to **nie** zmienia

- adres i format wydań — aktualizacje w zainstalowanych kopiach działają dalej,
- weryfikacja sumą SHA-256 przy aktualizacji zostaje (podpis jej nie zastępuje),
- sekret OAuth dalej nie leży w repo; jest wstrzykiwany przy buildzie. Uwaga
  praktyczna: w gotowej binarce i tak da się go znaleźć — tak jest dziś i tak
  będzie po otwarciu źródeł, bo aplikacja desktopowa jest „klientem publicznym".
