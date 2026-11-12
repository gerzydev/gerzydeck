# gerzyDECK (Gerzy's deck)

Neoficiální software pro **Fifine AmpliGame D6** (15 tlačítek s displejem) na Windows. Nahrazuje aplikaci od Fifine.

## Co umí

- **Tlačítka:** vlastní obrázky (katalog ikon, vlastní ikony, balíky ikon), popisky a barva pozadí, přetahování tlačítek myší.
- **Akce:** klávesa nebo zkratka, makro, spuštění programu nebo webu, média a hlasitost, zamknutí nebo uspání PC, přepnutí profilu.
- **Profily:** každý má 15 tlačítek a může se sám přepnout podle programu v popředí (např. `wow.exe`).
- **Pluginy pro Stream Deck:** instalace přímo v aplikaci z katalogů OpenAction a archivu Elgato (OBS, Spotify, Discord, PC metriky a další).
- **Využití AI:** tlačítka s procenty limitů Claude a ChatGPT (Codex) a časem do resetu.
- **Ikony:** katalog Iconify (přes 200 000 open-source ikon), knihovna vlastních ikon a import balíků `.streamDeckIconPack`.
- **Boot logo:** vlastní obrázek 800 × 480, který deck ukáže po zapojení.

Program běží v oznamovací oblasti (tray). Editor se otevře kliknutím na ikonu a zavřením okna se program neukončí.

## Instalace

1. Ukonči aplikaci **fifine Control Deck** (i v trayi) a vypni jí spouštění s Windows. Jinak se budou o deck přetahovat.
2. Spusť `Gerzys Deck_<verze>_x64-setup.exe` a odsouhlas licenční podmínky.
3. V editoru v **Nastavení** zapni „Spouštět s Windows“.

Nastavení, obrázky, ikony a pluginy jsou uložené v `%APPDATA%\GerzysDeck`.

## Důležité upozornění

Software je neoficiální a používáš ho **na vlastní nebezpečí**. Celé podmínky jsou v souboru `LICENSE.txt`, který se zobrazí při instalaci. Hlavně:

- **Boot logo** se zapisuje do trvalé paměti decku. Formát byl převzat z oficiální aplikace a ověřen, ale chyba při zápisu (třeba odpojení decku nebo jiný program, který do decku zrovna píše) může deck v krajním případě poškodit.
- **Využití AI** čte neveřejná rozhraní Anthropic a OpenAI s přihlášením z Claude Code a Codexu a při vypršení ho obnovuje. Rozhraní se může kdykoli změnit nebo přestat fungovat.
- **Pluginy** jsou programy třetích stran a běží na tvém počítači. Instaluj jen ty, kterým věříš.
- **Klávesy a makra:** některé hry (anticheat) simulované klávesy zakazují. Ve WoW je víc herních akcí na jedno zmáčknutí proti pravidlům Blizzardu.

## Vývoj

Potřebuješ Rust (rustup, MSVC), Microsoft C++ Build Tools a Node.js (kvůli Tauri CLI).

```powershell
npm install
npm run dev        # spustí aplikaci v režimu vývoje
npm run build      # vyrobí instalátor (src-tauri/target/release/bundle/nsis/)
cd src-tauri
cargo test --lib   # testy
```

### Diagnostika decku

Spouštěj, jen když gerzyDECK ani aplikace Fifine neběží.

```powershell
cd src-tauri
cargo run --bin diag                # čísla 1–15 na tlačítkách + výpis stisků
cargo run --bin diag -- --frame104  # měřicí rámečky (displej tlačítka má 104×104 px)
cargo run --bin diag -- --type      # test kláves (klikni do Poznámkového bloku)
cargo run --bin boot_logo_replay -- obrazek.png   # nahraje boot logo
```

## Poznámky

- Klávesy se posílají jako scan kódy (podle pozice na klávesnici), takže je hry berou jako opravdovou klávesnici.
- Do oken spuštěných jako správce Windows klávesy nepustí. Editor na to upozorní.
- Protokol decku vychází z [fifine-ampligame-d6](https://github.com/micah1crandell/fifine-ampligame-d6) (MIT). Boot logo bylo zachyceno z oficiální aplikace. Displej tlačítka má 104×104 px, boot logo 800×480 (mřížka 5×3 po 160 px).
- Ikony v katalogu pocházejí z [Iconify](https://iconify.design). Každá sada má vlastní licenci.
