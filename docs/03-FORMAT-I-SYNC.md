# Format danych i synchronizacja

## Idea nadrzędna

Git jest warstwą **trwałą i historyczną**, nie realtime'ową. Commit + push to
100 ms – 2 s; rysowanie potrzebuje 1–5 ms. Dlatego są dwie warstwy nad **jednym
formatem operacji**:

```
       rysowanie                        co ~10 s / na idle
  ┌──────────────┐   TCP <1 ms     ┌──────────────┐   git   ┌────────────┐
  │  peer A      │ ◄─────────────► │  peer B      │ ◄─────► │ GitHub     │
  │  op-log      │   LAN/Tailscale │  op-log      │  push   │ (private)  │
  └──────────────┘                 └──────────────┘         └────────────┘
        │                                                          ▲
        └────────────────────── git push ──────────────────────────┘
```

Te same bajty operacji lecą po TCP do peera i lądują w pliku. Jeden format: `spectre-proto`.

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
          via-adi@spectre-x360-000001.ops   # operacje kuby odebrane na zywo przez adi@spectre-x360
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
   jednocześnie, nie mogę wejść sobie w drogę. Operacje odebrane na żywo od innych
   odbiorca odkłada do **własnego** pliku `via-<odbiorca>-*.ops` w katalogu autora —
   nadal jeden pisarz na plik (ADR 0007).
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

## Warstwa live (Etap 6, ADR 0007)

- **Transport**: TCP z `TCP_NODELAY` (`std::net`), ramki = te same rekordy, co
  w pliku `.ops`. QUIC odłożony: w LAN TCP daje ~0,1 ms, a przez Tailscale jedzie
  w tunelu WireGuard; `quinn` to tokio + rustls w binarce bez powodu.
- **LAN**: własny beacon multicast `239.255.94.94:47941` co 2 s (`SPCTLV2`: autor,
  port TCP, id instancji); łączy instancja o mniejszym id. Działa bez
  internetu. Rozgłaszanie tylko przy widocznym oknie (Z2).
- **Zdalnie**: adres `host:port` peera wpisany w *Konto* (Tailscale) — węzeł łączy
  się sam i ponawia co 10 s; Tailscale daje szyfrowanie i tożsamość, więc nie
  budujemy własnego PKI.
- **Co płynie (ADR 0008)**: samo połączenie nic nie replikuje. Po `Hello` każda
  strona wysyła `Shared` (co udostępnia: id, tytuł, czy z hasłem). Notatka płynie
  na połączeniu dopiero po `Open` → `Opened(ok)`; dowód hasła to SHA-256 z klucza
  pochodnego, id notatki i dwóch nonce'ów z `Hello` — hasło nie idzie siecią.
  `Summary`, `Ops`, `Wet`, `Cursor` dotyczą wyłącznie notatek otwartych na tym
  połączeniu. Udostępnienia i otwarte notatki tej maszyny: `lan-<space>.txt`
  w danych aplikacji (nie w space, nie w gicie).
- **Heartbeat**: po 5 s ciszy `Ping`, brak `Pong` do 12 s = zerwane połączenie.
- **Mokra kreska**: paczka próbek na każdy komunikat pióra (~4 ms), z kolorem
  i grubością; odbiorca rysuje ją tym samym `StrokeBuilder`, co własną. Obecność
  (rysik peera) tym samym strumieniem.
- **Dołączenie w trakcie**: po otwarciu notatki wymiana `Summary` tej notatki —
  (autor) → ostatni lamport — i dosłanie różnicy. Ta sama ścieżka co po merge'u gita.
- **Zdalne operacje na dysku**: `ops/<autor>/via-<ja>-000001.ops` — plik, który
  pisze tylko ta maszyna; niezmiennik „jeden plik = jeden pisarz" zostaje.
  Odczyt deduplikuje po `(autor, lamport)`.

## Pętla synchronizacji

```
operacja lokalna
   ├─► pamięć (render widzi natychmiast)
   ├─► bufor .ops        → fsync na idle
   └─► TCP → peers       → co komunikat piora (mokra kreska), operacja na koniec

10 s po ostatniej zmianie:
   git add -A, commit              (wątek sync, nigdy nie blokuje renderu)

start / pokazanie okna / ukrycie okna / "Synchronizuj teraz":
   commit, git fetch, git merge, git push   (merge zawsze bezkonfliktowy)
   → lista notatek i cache metadanych odświeżone z plików, które merge zmienił,
     bieżąca notatka przeładowana po zakończeniu kreski

Realizacja: `libgit2` w binarce, GitHub REST (Device Flow, repo automatyczne), budżet
ruchu z predykcją i odczekaniem po odmowie — ADR 0006. Nie `gix`, nie proces `git`.
```

## Bezpieczeństwo

Prywatne repo GitHub plus Tailscale to granica zaufania v1. Szyfrowanie treści
at-rest jest poza v1, ale format ma zarezerwowany typ rekordu na zaszyfrowany
payload, żeby dało się je dołożyć bez migracji formatu.

## Otwarte pytania

- Duże assety (zdjęcia, PDF): Git LFS czy własny store poza repo? Do decyzji przy v2.
- Czy klient ma sam odpalać `git gc`, czy zostawiamy to użytkownikowi?
