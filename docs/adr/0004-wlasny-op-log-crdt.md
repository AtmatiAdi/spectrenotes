# ADR 0004 — Własny op-log CRDT zamiast Yjs/Automerge

**Status:** przyjęte · 2026-09-11

## Kontekst

Założenie Z4 wymaga, żeby dwie osoby rysowały na jednym canvasie jednocześnie,
bez serwera-arbitra i bez blokad. To jest podręcznikowe zastosowanie CRDT.
Naturalnym odruchem jest sięgnięcie po gotową bibliotekę — w Ruście `yrs`
(port Yjs) albo `automerge`.

## Obserwacja

Te biblioteki rozwiązują problem, którego my nie mamy.

Ich trudną częścią jest **utrzymanie porządku w sekwencji podlegającej edycji**:
gdy dwie osoby jednocześnie wstawiają znak w to samo miejsce tekstu, trzeba
deterministycznie rozstrzygnąć, który znak jest pierwszy, i to tak, żeby wszyscy
doszli do tej samej odpowiedzi. Stąd biorą się identyfikatory pozycji, drzewa
i metadane doklejone do każdego elementu.

Atrament nie ma tego problemu. Rysunek to **zbiór rosnący z nagrobkami**:

- dodanie dwóch stroke'ów w dowolnej kolejności daje ten sam rysunek — operacje
  są przemienne z definicji;
- wymazanie jest idempotentne — wymazanie dwa razy to to samo co raz;
- kolejność warstw wynika z `(lamport, author)` i jest totalna, więc każdy peer
  liczy ją niezależnie i wychodzi mu to samo.

Zbieżność jest tu własnością samych danych, a nie czymś, co trzeba wywalczyć algorytmem.

## Decyzja

Własny op-log CRDT w `spectre-core`, kilkaset linii.

| | Biblioteka ogólna | Nasz op-log |
|---|---|---|
| Narzut metadanych | na element sekwencji | brak — próbka to 3–4 bajty |
| Rozmiar zapisu | rośnie z metadanymi CRDT | rośnie z liczbą próbek |
| Zależności | duża biblioteka + jej model danych | brak |
| Trudność | ukryta w bibliotece | jawna, ale mała |

Przy 240 Hz próbkowania różnica w narzucie na próbkę przekłada się wprost na
rozmiar repozytorium i na czas wczytania notatki, więc nie jest kosmetyczna.

## Konsekwencje

- Poprawność jest **naszą** odpowiedzialnością. Dlatego przemienność i idempotencja
  operacji muszą być pokryte testami własności (proptest), a nie tylko przykładami.
  Jest to warunek zamknięcia Etapu 1.
- Gdyby doszły notatki tekstowe z prawdziwą edycją, ta decyzja ich nie obejmuje —
  wtedy `automerge` wraca do rozważenia dla **tego** typu treści, obok op-logu
  dla atramentu.
- `StrokeTransform` (przesunięcie/skalowanie istniejącego stroke'a) jest jedyną
  operacją, która nie jest naturalnie przemienna. Rozstrzygamy ją przez LWW po
  `(lamport, author)`. To świadoma utrata: przy jednoczesnym przesunięciu tego
  samego stroke'a przez dwie osoby jedno przesunięcie przepada. Alternatywy
  (składanie transformacji) dają wyniki jeszcze mniej przewidywalne dla człowieka.
