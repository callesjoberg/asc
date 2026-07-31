# ASC – Skärmklipp & Analys

ASC är en plattformsoberoende skrivbordsapp för att övervaka skärmar eller fönster, spara skärmklipp och analysera visuella eller textbaserade förändringar. Gränssnittet är byggt direkt i Rust med `eframe`/`egui`; Tauri, Node.js och webbfrontend behövs inte längre.

## Funktioner

- Liveövervakning av skärm eller enskilt fönster
- Visuell områdesväljare för skärmklipp, flera OCR-områden och flera pixel-/färgområden
- Förhandsgranskning av beskärningsområdet före analysstart
- Förändringsanalys med justerbar pixeltröskel
- Känsligt lokalt läge som kan upptäcka färgväxlingar i indikatorer på bara några pixlar
- Inbyggd OCR på macOS och Windows
- Koppling mellan ett OCR-sökord och en grön/röd indikator, med valfri fördröjning och separat statistik/export
- Efterhandsanalys av PNG- och JPEG-bilder i en mapp
- Justerbara paneler för inställningar, händelselogg och förhandsgranskning
- Skärmklippsvisning som passar hela bilden, med zoom och panorering
- Virtualiserad händelselogg som förblir snabb även vid långa analyser
- Klickbara loggrader för att växla mellan sparade skärmklipp
- Icke-blockerande mappväljare som låter appen fortsätta svara
- Separat flik för mjuka, slumpmässiga musrörelser med intervall och pauser
- Valfria titelradsklick som begränsas till särskilt markerade fönster
- Valfri textinmatning från en egen ordlista i särskilt markerade fönster
- Ett separat val anger exakt vilket fönster som får ta emot slumpmässigt inskriven text
- Egen RPA-flik med ordnade steg för väntan, OCR-klick, bildmatchningsklick och textinmatning
- OCR-klick kan hitta både enstaka ord och fraser, exempelvis `Ladda ner`
- Målfönstret flyttas längst fram före automatiserade klick; flödet stoppas om det inte kan aktiveras
- Oberoende start/stopp så analys och musautomatisering kan köras samtidigt
- Analysområden, OCR och trösklar kan ändras och tillämpas under en pågående liveövervakning
- Automatisk export till `asc-analysis.csv` och `asc-analysis.json`
- Automatisk export av ord-/färgmätning till `asc-keyword-colors.csv` och `asc-keyword-colors.json`

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

På macOS måste appen få behörighet för skärminspelning i Systeminställningar. OCR-hjälpprogrammet i `resources/` bäddas in i appbinären. Windows-hjälpprogrammet publiceras som en självförsörjande enkelfils-EXE i GitHub Actions före Rust-bygget, så någon separat `OcrHelper.dll` eller .NET-installation ska inte behövas.

Den första bilden används som referens. Från och med bild två markeras en förändring när RGB-skillnaden når pixeltröskeln eller när OCR-texten ändras. Analysfilerna uppdateras automatiskt i den valda mappen.

Musautomatiseringen har ett separat start/stopp och kan därför köras samtidigt med analysen. Fönsterklick och textinmatning är avstängda som standard och begränsas till de fönster som användaren uttryckligen markerar. Textinmatning ändrar innehållet i målprogrammet och ska därför bara aktiveras med en kontrollerad ordlista.

RPA-flödet arbetar mot ett enda valt fönster. Ett OCR-steg söker efter ordets eller frasens aktuella bildposition. Ett bildsteg söker efter en tätt beskuren PNG- eller JPEG-referens i samma skala. Använd explicita väntesteg efter klick som laddar en ny sida eller startar en nedladdning. Windows OCR-hjälpare körs dolt utan blinkande konsolfönster.

Flytta pekaren till huvudskärmens övre vänstra hörn för nödstopp. På macOS behöver ASC även behörighet under Integritet och säkerhet > Hjälpmedel för att styra muspekaren.

## Release

Workflowen i `.github/workflows/release.yml` bygger fristående binärer för macOS och Windows när en tagg som börjar med `v` pushas. Den kan även startas manuellt från GitHub Actions.
