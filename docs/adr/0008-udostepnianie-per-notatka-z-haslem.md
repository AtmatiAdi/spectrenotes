# ADR 0008 — Sieć widzi tylko to, co udostępnione: notatka po notatce, z hasłem lub bez

**Status:** przyjęte · 2026-09-14

## Kontekst

Warstwa live z ADR 0007 replikowała **cały space** do każdego peera w LAN,
który miał space o tej samej nazwie. Działało to dla jednego użytkownika na
dwóch maszynach, ale nie odpowiada temu, jak ma wyglądać praca z innymi ludźmi:
domyślna nazwa space'u to u każdego `default`, więc dwie obce osoby w jednej
sieci wymieniłyby się **wszystkimi** notatkami bez pytania. Granicą zaufania
była sieć, a nie decyzja użytkownika.

Wymaganie (właściciel projektu): urządzenia w LAN (i przez Tailscale) mają się
**wykrywać same**, na liście mają się pojawiać **cudze notatki udostępnione**,
a własną notatkę udostępnia się **wybierając ją** i włączając „udostępniona
w sieci", z hasłem albo bez. GUI na razie minimalne — docelowy układ ustali
Etap 6½.

## Decyzja

1. **Jednostką udostępniania jest notatka, nie space.** Połączenie TCP między
   dwoma instancjami samo w sobie nic nie replikuje. Po `Hello` każda strona
   wysyła `Shared` — listę tego, co udostępnia (id, tytuł, czy chronione).
   Replikacja (`Summary`/`Ops`), mokra kreska i kursor płyną **wyłącznie** dla
   notatek **otwartych na tym połączeniu**: udostępnionych przez jedną stronę
   i otwartych przez drugą (`Open` → `Opened`). Nazwa space'u przestaje mieć
   znaczenie — łączą się dowolne instancje, id notatek (ULID) są globalne.
2. **Udostępnienie to decyzja urządzenia, nie notatki.** Lista udostępnień,
   klucze i otwarte cudze notatki leżą w `lan-<space>.txt` w danych aplikacji,
   nie w space — nie idą do gita i nie przenoszą się na drugą maszynę
   użytkownika. Otwarta cudza notatka staje się zwykłą notatką w moim space
   (pliki `via-*` z ADR 0007), więc po zamknięciu sesji kopia zostaje, a git
   zsynchronizuje ją do mojego repo jak każdą inną.
3. **Hasło nigdy nie idzie siecią ani na dysk.** Z hasła powstaje klucz
   (`SHA-256(domena ‖ hasło)`), zapisywany po obu stronach. Przy otwieraniu
   otwierający wysyła **dowód** `SHA-256(domena ‖ klucz ‖ id notatki ‖ nonce
   udostępniającego ‖ nonce otwierającego)`; nonce'y są losowane na każde
   połączenie i wymieniane w `Hello`, więc podsłuchany dowód nie nadaje się do
   powtórzenia. Zła odpowiedź = `Opened(ok=false)`, aplikacja zapomina klucz
   i prosi o hasło ponownie (bez automatycznego ponawiania — inaczej byłoby
   to zgadywanie hasła co połączenie).
4. **Treść nadal płynie jawnym TCP.** Hasło chroni przed *przypadkowym* albo
   niechcianym otwarciem notatki przez kogoś w tej samej sieci, nie przed
   podsłuchem. Granica zaufania dla poufności zostaje jak w ADR 0007: LAN
   albo tunel Tailscale (WireGuard). Szyfrowanie warstwy własnej — gdy pojawi
   się potrzeba pracy przez sieć, której nie ufamy, bez Tailscale.
5. **Heartbeat i stałe adresy.** Pętla węzła budzi się co 2 s: po 5 s ciszy
   `Ping`, brak odpowiedzi do 12 s = zerwane (TCP sam zauważyłby po minutach).
   Peer bez multicastu (Tailscale) to adres `host:port` wpisany w *Konto* —
   łączymy się sami i ponawiamy co 10 s, deduplikacja po id instancji, gdy obie
   strony mają się nawzajem wpisane.
6. **Protokół w wersji 2** (`SPCTLV2`, `PROTO_VERSION = 2`). Wersja 1 nie
   rozmawia z 2 — nie ma jeszcze użytkowników poza właścicielem.

## Konsekwencje

- Dwie własne maszyny tego samego użytkownika **nie replikują już całego
  space'u** przez LAN — to robi git; live przenosi tylko notatki, które
  użytkownik udostępnił i otworzył na drugiej maszynie. Świadoma zmiana: prostszy
  model („sieć widzi tylko to, co pokazałem") jest wart utraty tej wygody.
- Trzeci węzeł dostaje operacje drugiego przez pierwszy tylko wtedy, gdy ma tę
  notatkę otwartą z pierwszym (przekazywanie ograniczone do sesji).
- `sha2` w zależnościach `spectre-sync` (RustCrypto, bez dalszych zależności).
- Zweryfikowane: dwie instancje na jednej maszynie — udostępnienie z hasłem,
  odmowa przy złym haśle, otwarcie przy dobrym, kreska A widoczna u B; test
  jednostkowy z „milczącym" klientem potwierdza `Ping` i zrzucenie po ~12 s.
