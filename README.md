# ASC – Skärmklipp & Analys

ASC är en plattformsoberoende skrivbordsapp för att övervaka skärmar eller fönster, spara skärmklipp och analysera visuella eller textbaserade förändringar. Gränssnittet är byggt direkt i Rust med `eframe`/`egui`; Tauri, Node.js och webbfrontend behövs inte längre.

## Funktioner

- Liveövervakning av skärm eller enskilt fönster
- Valfri beskärning av skärmklipp och OCR-område
- Förhandsgranskning av beskärningsområdet före analysstart
- Förändringsanalys med justerbar pixeltröskel
- Inbyggd OCR på macOS och Windows
- Efterhandsanalys av PNG- och JPEG-bilder i en mapp
- Justerbara paneler för inställningar, händelselogg och förhandsgranskning
- Skärmklippsvisning som passar hela bilden, med zoom och panorering
- Virtualiserad händelselogg som förblir snabb även vid långa analyser
- Klickbara loggrader för att växla mellan sparade skärmklipp
- Automatisk export till `asc-analysis.csv` och `asc-analysis.json`

## Utveckling

Installera en stabil Rust-verktygskedja och kör:

```sh
cargo run
```

Kontroller som ska passera före en release:

```sh
cargo fmt -- --check
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release
```

På macOS måste appen få behörighet för skärminspelning i Systeminställningar. OCR-hjälpprogrammet i `resources/` bäddas in i appbinären. Windows-hjälpprogrammet byggs automatiskt i GitHub Actions före Rust-bygget.

Den första bilden används som referens. Från och med bild två markeras en förändring när RGB-skillnaden når pixeltröskeln eller när OCR-texten ändras. Analysfilerna uppdateras automatiskt i den valda mappen.

## Release

Workflowen i `.github/workflows/release.yml` bygger fristående binärer för macOS och Windows när en tagg som börjar med `v` pushas. Den kan även startas manuellt från GitHub Actions.
