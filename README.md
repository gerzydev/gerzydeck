# Gerzy's deck

Vlastní software pro **Fifine AmpliGame D6** (15 tlačítek s displejem) na Windows. Nahrazuje aplikaci od Fifine:

- obrázky a popisky na tlačítkách,
- akce tlačítek: klávesa nebo zkratka, makro, spuštění programu nebo webu, média a hlasitost, zamknutí nebo uspání PC, přepnutí profilu,
- profily, které se samy přepnou podle programu v popředí (např. `wow.exe`),
- šablony pro Discord a Windows.

Program běží v oznamovací oblasti (tray). Editor se otevře kliknutím na ikonu. Zavřením okna se program neukončí.

## Instalace

1. Zavři aplikaci *fifine Control Deck*, jinak se budou s deckem přetahovat.
2. Spusť `Gerzys Deck_<verze>_x64-setup.exe` (z `src-tauri/target/release/bundle/nsis/`).
3. V editoru v **Nastavení** zapni „Spouštět s Windows“.

Nastavení a obrázky jsou uložené v `%APPDATA%\GerzysDeck` (`config.json`, `images\`, `log.txt`).

## Vývoj

Potřebuješ Rust (rustup, MSVC), Microsoft C++ Build Tools a Node.js (jen kvůli Tauri CLI).

```powershell
npm install
npm run dev        # spustí aplikaci v režimu vývoje
npm run build      # vyrobí instalátor
cd src-tauri
cargo test --lib   # testy
```

### Diagnostika decku

```powershell
cd src-tauri
cargo run --bin diag                # čísla 1–15 na tlačítkách + výpis stisků
cargo run --bin diag -- --frame104  # měřicí rámečky (displej tlačítka má 104×104 px)
cargo run --bin diag -- --type      # test kláves (klikni do Poznámkového bloku)
```

Diagnostiku spouštěj, jen když Gerzy's deck neběží.

## Poznámky

- Klávesy se posílají jako scan kódy (podle pozice na klávesnici), takže je hry berou jako opravdovou klávesnici.
- Do oken spuštěných jako správce Windows klávesy nepustí. Editor na to upozorní.
- Ve WoW je víc herních akcí na jedno zmáčknutí proti pravidlům Blizzardu, s makry opatrně.
- Protokol decku vychází z [fifine-ampligame-d6](https://github.com/micah1crandell/fifine-ampligame-d6) (MIT) a byl ověřen na hardwaru. Displej tlačítka má 104×104 px.
