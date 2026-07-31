# Anteckningar - ASC

Anteckningar - ASC är en plattformsoberoende skrivbordsapp för att övervaka skärmar eller fönster, spara skärmklipp och analysera visuella eller textbaserade förändringar. Gränssnittet är byggt direkt i Rust med `eframe`/`egui`; Tauri, Node.js och webbfrontend behövs inte längre.

## Funktioner

- Liveövervakning av skärm eller enskilt fönster
- Visuell områdesväljare för skärmklipp, flera OCR-områden och flera pixel-/färgområden
- Befintliga områden kan flyttas och storleksändras med hörnhandtag
- Resultatvy visar exakt det färdiga skärmklippet och erbjuder "Tillbaka och ändra"
- Förhandsgranskning av beskärningsområdet före analysstart
- Förändringsanalys med justerbar pixeltröskel
- Känsligt lokalt läge som kan upptäcka färgväxlingar i indikatorer på bara några pixlar
- Inbyggd OCR på macOS och Windows
- Koppling mellan ett OCR-sökord och en grön/röd indikator, med valfri fördröjning och separat statistik/export
- Efterhandsanalys av PNG- och JPEG-bilder i en mapp
- Efterhandsanalysen kan stoppas mellan filer/OCR-steg utan att redan framräknade resultat försvinner
- Justerbara paneler för inställningar, händelselogg och förhandsgranskning
- Skärmklippsvisning som passar hela bilden, med zoom och panorering
- Virtualiserad händelselogg som förblir snabb även vid långa analyser
- Klickbara loggrader för att växla mellan sparade skärmklipp
- Loggen beskriver ändrade pixlar och ändringsområdets koordinater/storlek; valt klipp får en röd markeringsram över området
- Icke-blockerande mappväljare som låter appen fortsätta svara
- Separat flik för mjuka, slumpmässiga musrörelser med intervall och pauser
- Valfria titelradsklick som begränsas till särskilt markerade fönster
- Valfri textinmatning från en egen ordlista i särskilt markerade fönster
- Ett separat val anger exakt vilket fönster som får ta emot slumpmässigt inskriven text
- Egen RPA-flik med ordnade steg för fönsterbyte, väntan, OCR-klick, bildmatchningsklick, kortkommandon och textinmatning
- Bildmål markeras direkt i ett nytt klipp av valt målfönster och söks bara inom ±10 % av ursprungsplatsen
- RPA-steget "Vänta tills sidan är klar" kombinerar en minimitid med visuell stabilitetskontroll och timeout
- OCR-klick kan hitta både enstaka ord och fraser, exempelvis `Ladda ner`
- Målfönstret flyttas längst fram före automatiserade klick; flödet stoppas om det inte kan aktiveras
- Torrkörning är förvald och verifierar mål utan klick, kortkommandon eller textinmatning
- Manuella bekräftelsepunkter kan stoppa flödet före exempelvis att ett Outlook-utkast skickas
- Färdiga startmallar för Outlook → ChatGPT → Outlook-utkast och ett flöde med exakt tio verifierade klick
- Windows 10/11: global makroinspelning av vänster-/högerklick och Ctrl+C/V/A, Enter och Escape
- Inspelade klick lagras relativt respektive fönster och importeras med sina verkliga väntetider till den redigerbara steglistan
- Oberoende start/stopp så analys och musautomatisering kan köras samtidigt
- Analysområden, OCR och trösklar kan ändras och tillämpas under en pågående liveövervakning
- Automatisk export till `asc-analysis.csv` och `asc-analysis.json`
- Automatisk export av ord-/färgmätning till `asc-keyword-colors.csv` och `asc-keyword-colors.json`
- Teams-statusflik som loggar statusbyten och visualiserar tid per status
- Teams kan följas via svensk OCR och/eller en liten markerad grön/röd/gul/grå statusboll
- Teams-historik exporteras lokalt till `teams-status.csv` och `teams-status.json`

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

På macOS måste appen få behörighet för skärminspelning i Systeminställningar. OCR-hjälpprogrammet i `resources/` bäddas in i appbinären och prioriterar svenska (`sv-SE`) med engelska/systemspråk som reserv. Windows-hjälpprogrammet publiceras som en självförsörjande enkelfils-EXE i GitHub Actions före Rust-bygget, så någon separat `OcrHelper.dll` eller .NET-installation ska inte behövas.

Den första bilden används som referens. Från och med bild två markeras en förändring när RGB-skillnaden når pixeltröskeln eller när OCR-texten ändras. Analysfilerna uppdateras automatiskt i den valda mappen.

Musautomatiseringen har ett separat start/stopp och kan därför köras samtidigt med analysen. Fönsterklick och textinmatning är avstängda som standard och begränsas till de fönster som användaren uttryckligen markerar. Textinmatning ändrar innehållet i målprogrammet och ska därför bara aktiveras med en kontrollerad ordlista.

RPA-flödet börjar i ett valt standardfönster och kan byta mellan flera uttryckligen valda fönster under samma sekvens. Ett OCR-steg söker efter ordets eller frasens aktuella bildposition. Ett bildsteg söker efter en tätt beskuren PNG- eller JPEG-referens i samma skala. Kortkommandon omfattar kopiera, klistra in, markera allt, Enter och Escape. Använd explicita väntesteg efter klick som laddar en ny sida eller startar en nedladdning. Windows OCR-hjälpare körs dolt utan blinkande konsolfönster.

Ett Outlook–ChatGPT-flöde kan därför byggas med bild-/OCR-klick, Kopiera, Byt målfönster, Klistra in och Vänta tills sidan är klar. Avsluta med en manuell bekräftelsepunkt efter att svaret klistrats in som Outlook-utkast. Den globala inspelaren är inledningsvis Windows-specifik; alla importerade steg kan därefter redigeras i RPA-fliken.

På Windows kan ett makro även skapas med **Spela in klick och kortkommandon**. Inspelningen ignorerar vanlig tangenttext, lösenord, urklippsinnehåll och musrörelser. Stoppa från valfritt fönster med `Ctrl+Alt+Esc`; händelserna importeras automatiskt och torrkörning aktiveras. Fönstertitel och fokus verifieras före uppspelning, och flödet stoppas om ett annat fönster ligger framför målet.

Flytta pekaren till huvudskärmens övre vänstra hörn för nödstopp. På macOS behöver Anteckningar - ASC även behörighet under Integritet och säkerhet > Hjälpmedel för att styra muspekaren.

## Release

Workflowen i `.github/workflows/release.yml` bygger fristående binärer för macOS och Windows när en tagg som börjar med `v` pushas. Den kan även startas manuellt från GitHub Actions.
