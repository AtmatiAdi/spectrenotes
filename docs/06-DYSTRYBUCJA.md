# Dystrybucja i aktualizacje

## Postać dystrybucyjna

Jeden przenośny `.exe`. Bez instalatora, bez runtime'u, bez wpisów w rejestrze.
Konfiguracja i dane w `%APPDATA%\SpectreNotes\`, więc usunięcie aplikacji to
skasowanie jednego pliku.

Statyczna binarka Rusta w profilu release z `lto = "thin"`, `codegen-units = 1`
i `strip = true` powinna zmieścić się w kilkunastu megabajtach.

## Aktualizacje przez git

Aplikacja sprawdza tagi w repozytorium wydań (nie w repozytorium notatek — to są
dwie różne rzeczy) i pobiera artefakt przypięty do tagu.

```
start aplikacji (albo raz na dobę, w tle)
   → sprawdzenie najnowszego tagu
   → pobranie artefaktu + weryfikacja sumy BLAKE3
   → zapis obok biezacej binarki jako <nazwa>.new
   → przy nastepnym starcie: atomowa podmiana i usuniecie starej
```

Podmiana przy starcie, nie w trakcie działania — proces rezydentny (Z2) nie może
sobie podmienić własnego pliku pod nogami. Aktualizacja nigdy nie przerywa pracy;
w najgorszym razie czeka do następnego uruchomienia.

Kanał `stable` i `edge` jako dwa różne wzorce tagów, żeby dało się komuś dać
wersję do testów bez ruszania wersji, na której się realnie notuje.

## Rozdawanie aplikacji innym

Dołączenie kogoś do space'u to dwie rzeczy:

1. dostęp do prywatnego repo GitHub (zaproszenie),
2. jego węzeł w tej samej sieci Tailscale (dla warstwy live).

Bez punktu 2 współpraca nadal działa, tylko przez gita — czyli z opóźnieniem
rzędu sekund zamiast milisekund. To sensowny tryb degradacji, a nie awaria.

## Otwarty problem: SmartScreen

Niepodpisana binarka pobrana z internetu dostaje pełnoekranowe ostrzeżenie
„Windows protected your PC". Dla kogoś, komu dajesz aplikację, wygląda to jak
malware, niezależnie od tego, czym jest.

Warianty:

| | Koszt | Efekt |
|---|---|---|
| Brak podpisu | 0 | ostrzeżenie u każdego, zawsze |
| Certyfikat self-signed | 0 | ostrzeżenie nadal; pomaga tylko przy ręcznym zaufaniu certyfikatowi |
| Certyfikat OV | kilkaset zł/rok | ostrzeżenie znika dopiero po zbudowaniu reputacji przez SmartScreen |
| Certyfikat EV | drożej, wymaga tokena sprzętowego | reputacja od pierwszego uruchomienia |
| Microsoft Store | opłata jednorazowa + proces certyfikacji | brak ostrzeżeń, ale kłóci się z „update przez gita" |

Decyzja odłożona do Etapu 7 — nie blokuje niczego wcześniej, a przy rozdawaniu
w gronie kilku znajomych da się na razie żyć z ręcznym „Więcej informacji → Uruchom mimo to".

## Kompilacja u odbiorcy

Alternatywa dla podpisu: odbiorca buduje ze źródeł. Wymaga u niego Rusta i
VS Build Tools, więc realnie działa tylko dla osób technicznych — ale jest
całkowicie darmowa i omija SmartScreen w całości.
