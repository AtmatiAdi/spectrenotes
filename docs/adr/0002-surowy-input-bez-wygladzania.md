# ADR 0002 — Surowy input pióra, bez wygładzania

**Status:** przyjęte · 2026-09-11

## Kontekst

Założenie Z6: notowanie ma być rysowaniem. Aplikacja nie ma "pomagać" —
nie prostuje linii, nie upiększa kreski, nie zgaduje intencji.

Problem w tym, że pod hasłem "wygładzania" kryją się cztery różne techniki
o zupełnie różnych skutkach, i mieszanie ich prowadzi albo do gumowatej kreski,
albo do schodkowanej.

## Decyzja

| Technika | Co robi z próbkami | Decyzja |
|---|---|---|
| Filtr wygładzający (moving average, one-euro) | **zmienia** pozycje próbek | **NIE** |
| Fitting do Béziera z tolerancją | **usuwa** próbki uznane za szum | **NIE** |
| Predykcja / ekstrapolacja | **dodaje** próbki, których nie było | **domyślnie NIE**, przełącznik |
| Interpolacja centripetal Catmull-Rom | nie rusza żadnej próbki, rysuje tor **między** nimi | **TAK** |

Rozróżnienie w ostatnim wierszu jest sednem tego ADR-a i bywa mylone.
Splajn centripetal **przechodzi dokładnie przez każdą zaraportowaną próbkę**.
Nie usuwa i nie przesuwa żadnej informacji — dodaje ciągłość tam, gdzie digitizer
po prostu nie zdążył zmierzyć. Bez tego szybka kreska jest wielokątem, i to nie
dlatego, że jesteśmy wierni danym, tylko dlatego, że rysujemy je leniwie.

Wybór wariantu **centripetal** (a nie uniform czy chordal) jest istotny: tylko on
gwarantuje brak pętelek i ostrych wybrzuszeń przy nierównomiernie rozłożonych
próbkach, a takie właśnie dostajemy przy zmiennej prędkości pisania.

Predykcja zostaje jako przełącznik, ponieważ jest to jedyna technika, która realnie
zmniejsza **odczuwalną** latencję, a jej koszt (przestrzelenie na zwrotach, tzw. haczyki)
jest subiektywny. Decyzja domyślna jest zgodna z Z6, ale werdykt należy do testu
na żywo w `tools/inkdemo`.

## Konsekwencje

- Stroke wiernie odwzorowuje drżenie ręki. To jest zamierzone.
- Zapisujemy każdą próbkę z historii, więc pliki są większe niż przy fittingu.
  Łagodzone kompresją delta+varint (patrz `03-FORMAT-I-SYNC.md`), nie redukcją danych.
- Jeżeli firmware ELAN wygładza po swojej stronie, nie możemy tego cofnąć.
  Test kontrolny opisany w `04-ENERGIA-I-AMOLED.md`.
