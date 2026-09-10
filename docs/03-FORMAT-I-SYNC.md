# Format danych i synchronizacja

## Idea nadrzędna

Git jest warstwą **trwałą i historyczną**, nie realtime'ową. Commit + push to
100 ms – 2 s; rysowanie potrzebuje 1–5 ms. Dlatego są dwie warstwy nad **jednym
formatem operacji**:

```
       rysowanie                        co ~10 s / na idle
  ┌──────────────┐   QUIC 1-5 ms   ┌──────────────┐   git   ┌────────────┐
  │  peer A      │ ◄─────────────► │  peer B      │ ◄─────► │ GitHub     │
  │  op-log      │   Tailscale/LAN │  op-log      │  push   │ (private)  │
  └──────────────┘                 └──────────────┘         └────────────┘
        │                                                          ▲
        └────────────────────── git push ──────────────────────────┘
```

Te same bajty operacji lecą po QUIC i lądują w pliku. Jeden format: `spectre-proto`.

## Układ repozytorium (space)

```
space-repo/
  space.json                     # nazwa, uczestnicy, wersja formatu
  notes/
    01J8F../                     # ULID notatki - nazwa katalogu nigdy się nie zmienia
      note.json                  # tytuł, tagi, tło, rozmiar strony
      ops/
        adi@spectre-x360/
          000001.ops             # append-only, cap ~2 MB, potem nowy plik
          000002.ops
        adi@desktop/
          000001.ops
        kuba@surface/
          000001.ops
      snapshots/
        000000123456-8f3a.snap   # stan do lamporta N; nazwa zawiera hash
  assets/
    b3-4f2a91....png             # content-addressed (BLAKE3)
```

Tytuł notatki jest w `note.json`, nie w nazwie katalogu. Zmiana tytułu nie jest
wtedy przeniesieniem pliku w gicie i nie generuje szumu w historii.

## Dlaczego konflikt merge jest tu strukturalnie niemożliwy

Trzy własności, działające razem:

1. **Pliki `.ops` są append-only.** Nic nie jest nadpisywane ani kasowane.
2. **Autor to para (użytkownik, urządzenie).** `adi@spectre-x360` i `adi@desktop`
   to różni autorzy, czyli różne pliki. Nawet ja sam, pracując na dwóch maszynach
   jednocześnie, nie mogę wejść sobie w drogę.
3. **Snapshoty mają hash w nazwie**, więc każdy jest nowym plikiem, nigdy modyfikacją.

Skoro dwóch autorów nigdy nie dotyka tego samego pliku, `git merge` sprowadza się
zawsze do połączenia zbiorów plików: bez strategii merge, bez sterownika, bez konfliktu.
Poprawność semantyczną (że suma operacji daje u wszystkich ten sam rysunek)
gwarantuje CRDT, a nie git.

Jedynym miejscem, które realnie może się rozjechać, jest `note.json` (tytuł, tagi).
Tam stosujemy LWW po `(lamport, author)`, rozstrzygane przy wczytaniu — nie przez gita.

## Format `.ops`

```
nagłówek pliku:  MAGIC "SPCTOPS1" | u16 wersja | AuthorId | u64 lamport_bazowy
rekord:          u32 długość | u8 typ | payload (bincode) | u32 crc32
```

CRC na rekord, ponieważ plik append-only przerwany utratą zasilania musi dać się
odczytać do ostatniego **całego** rekordu. Uszkodzony ogon obcinamy przy starcie —
to realizuje kryterium "utrata maksymalnie 1 s pracy".

Zapis: bufor w pamięci, `write` na każdą operację, `fsync` po 300 ms bezczynności
oraz na końcu każdego stroke'a. Nie fsyncujemy na próbkę — to zabiłoby wydajność i dysk.

## Snapshoty i przycinanie

Gdy łączny rozmiar `ops/` notatki przekroczy próg (np. 8 MB), sync zapisuje
`snapshots/<lamport>-<hash>.snap` ze zbitym stanem wszystkich żywych stroke'ów.
Starsze `.ops` można wtedy usunąć z HEAD — **historia zostaje w commitach**,
`git log` nadal ją ma. Wczytanie notatki = ostatni snapshot + operacje po nim.

Kompresja próbek: pozycje jako delty w stałym punkcie (1/32 px), zigzag + varint;
nacisk 10 bitów, tilt po 8 bitów na oś. Realnie 3–4 bajty na próbkę zamiast 24.
Przy 240 Hz to około 1 MB na godzinę nieprzerwanego rysowania.

## Warstwa live

- **Transport**: QUIC (`quinn`). Strumień niezawodny dla operacji, datagramy
  zawodne dla obecności (kursor, hover pióra) — obecność się nie starzeje
  w sensie trwałym, więc zgubienie datagramu jest bez znaczenia.
- **Zdalnie**: adres Tailscale peera. Tailscale daje szyfrowanie i tożsamość,
  więc nie budujemy własnego PKI.
- **LAN**: rozgłoszenie mDNS `_spectrenotes._udp`, działa bez internetu.
- **Wysyłka**: operacje lecą batchami co ~8 ms **w trakcie** stroke'a, żeby druga
  osoba widziała kreskę na bieżąco, a nie dopiero po jej zakończeniu.
- **Dołączenie w trakcie**: peer prosi o stan od lamporta N i dostaje snapshot
  plus ogon operacji. To dokładnie ta sama ścieżka co wczytanie z dysku.

## Pętla synchronizacji

```
operacja lokalna
   ├─► pamięć (render widzi natychmiast)
   ├─► bufor .ops        → fsync na idle
   └─► QUIC → peers      → batch co 8 ms

co 10 s bez rysowania / przy chowaniu okna / przy zamknięciu:
   git add -A, commit, push        (wątek sync, nigdy nie blokuje renderu)

start / powrót online:
   git fetch, git merge            (zawsze bezkonfliktowy)
   → odtworzenie operacji, których nie mamy
```

## Bezpieczeństwo

Prywatne repo GitHub plus Tailscale to granica zaufania v1. Szyfrowanie treści
at-rest jest poza v1, ale format ma zarezerwowany typ rekordu na zaszyfrowany
payload, żeby dało się je dołożyć bez migracji formatu.

## Otwarte pytania

- Duże assety (zdjęcia, PDF): Git LFS czy własny store poza repo? Do decyzji przy v2.
- Czy klient ma sam odpalać `git gc`, czy zostawiamy to użytkownikowi?
