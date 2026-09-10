# ADR 0003 — Git jako warstwa trwała; odrzucenie modelu "branch = notatka"

**Status:** przyjęte · 2026-09-11

## Kontekst

Pierwotny pomysł: space to repozytorium, a każda notatka to osobny branch.
Motywacja była trafna — git jest szybki, niezawodny, daje historię za darmo
i rozwiązuje problem uzależnienia od cudzej chmury.

Utrzymujemy git jako warstwę trwałą. Zmieniamy natomiast to, **czym jest notatka**
w strukturze repozytorium.

## Dlaczego nie branch na notatkę

Branch w gicie modeluje **rozbieżną linię historii tej samej treści**, a nie
**równoległy niezależny dokument**. Użycie go do drugiego celu łamie się w praktyce
w pięciu miejscach:

1. **Nie da się mieć dwóch notatek otwartych naraz.** Working tree jest jeden,
   a `checkout` przełącza całe drzewo. Obejście to `git worktree` per notatka,
   czyli N kopii katalogu roboczego na dysku.
2. **Otwarcie notatki staje się operacją I/O.** Zamiast wczytać jeden plik,
   przepisujemy drzewo robocze. To stoi wprost w sprzeczności z Z2 (<30 ms na otwarcie).
3. **Sync skaluje się liniowo z liczbą notatek.** 500 notatek to 500 refów
   do negocjacji przy każdym `fetch`. Model katalogowy to jeden ref niezależnie
   od liczby notatek.
4. **Merge traci sens semantyczny.** Scalanie brancha notatki A z branchem notatki B
   nie znaczy nic. Tymczasem merge jest dokładnie tym, czego potrzebujemy przy
   współpracy dwóch osób nad **tą samą** notatką — a tego branch-per-notatka nie rozwiązuje.
5. **Historia notatki miesza się z historią space'u.** `git log` na branchu notatki
   pokazuje też wszystko, co odziedziczył z punktu odgałęzienia.

## Decyzja

**Space = repozytorium. Notatka = katalog. Wszyscy pracują na `main`.**

Operacje trafiają do plików append-only rozbitych po autorze, gdzie autor to para
**(użytkownik, urządzenie)**:

```
notes/<ulid>/ops/<user>@<device>/<seq>.ops
```

Konsekwencje:

- dwóch autorów nigdy nie pisze do tego samego pliku, więc **konflikt merge
  jest strukturalnie niemożliwy**, a nie tylko rzadki;
- to samo działa dla jednej osoby na dwóch maszynach — `adi@spectre-x360`
  i `adi@desktop` to różni autorzy;
- historia pojedynczej notatki to `git log -- notes/<ulid>/`, czysta i lokalna;
- otwarcie notatki to odczyt plików, bez dotykania stanu repozytorium;
- zbieżność treści gwarantuje CRDT w `spectre-core`, a git dostarcza tylko
  transport i historię. Git nie musi rozumieć naszych danych.

## Co z branchami

Zostają do tego, do czego naprawdę służą: **wersja alternatywna notatki**.
Chcesz przerobić schemat, mogąc wrócić lub scalić — robisz branch świadomie,
jako jawną operację. To jest funkcja na później, nie mechanizm codziennej pracy.

## Konsekwencje negatywne

- Repozytorium rośnie w nieskończoność, bo `.ops` są append-only. Łagodzone
  snapshotami i przycinaniem HEAD (historia zostaje w commitach), ale realnie
  kiedyś trzeba będzie zaproponować archiwizację starych spaceów.
- Nazwa katalogu to ULID, a nie tytuł — repozytorium jest nieczytelne bez aplikacji.
  Świadomy koszt: zmiana tytułu nie generuje wtedy przenosin plików w historii.
- Wymagamy stabilnego identyfikatora urządzenia. Reinstalacja systemu tworzy
  nowego autora, co jest nieszkodliwe, ale zaśmieca katalog `ops/`.
