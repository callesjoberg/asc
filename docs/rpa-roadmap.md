# RPA-roadmap för Anteckningar - ASC

## Mål

RPA-fliken ska kunna bygga och spela upp redigerbara makron mellan flera uttryckligen valda fönster. Två prioriterade användningsfall är:

1. Öppna senaste synliga mejlet i Outlook, kopiera brödtexten, skapa en redigerbar ChatGPT-prompt, hämta svaret och lägga det som ett Outlook-utkast.
2. Utföra ett verifierat webbflöde med ett bestämt antal klick, intervall och väntan på sidstabilitet.

## Stegkatalog

- Aktivera målfönster och verifiera app/titel.
- Vänta en bestämd tid.
- Vänta minst X sekunder och därefter tills fönstret varit visuellt stabilt.
- Klicka på ett OCR-ord eller en fras.
- Klicka på en visuellt markerad bild nära dess ursprungliga position.
- Klicka på en fönsterrelativ koordinat som uttryckligen godkänts som reserv.
- Enkelklick, dubbelklick, drag och scroll.
- Kortkommando: kopiera, klistra in, markera allt, Enter och Escape.
- Skriv redigerbar text.
- Upprepa ett avgränsat block exakt N gånger med minsta intervall.
- Verifiera förväntad OCR-text, bild eller nedladdad fil.
- Pausa vid en synlig bekräftelsepunkt.

## Makroinspelning

- Windows använder low-level mouse/keyboard hooks; macOS använder Quartz event taps.
- Råa klick sparas med tid och fönsterrelativa koordinater, inte enbart skärmkoordinater.
- Onödiga musrörelser slås ihop.
- Tangenttext spelas inte in som standard för att undvika lösenord och personuppgifter.
- Inspelade klick måste få ett OCR-/bildankare eller uttryckligen godkännas som koordinatreserv före uppspelning.
- Sjvgenererade uppspelningshändelser filtreras så att de inte spelas in på nytt.

## Outlook → ChatGPT → Outlook

1. Aktivera valt Outlook-fönster.
2. Hitta och öppna senaste mejlet samt verifiera avsändare/ämne.
3. Kopiera brödtexten utan att spara den i körningsloggen.
4. Visa en redigerbar prompt och varning innan mejlinnehåll förs till ChatGPT.
5. Aktivera valt ChatGPT-fönster, klistra in prompten och vänta på stabilt svar.
6. Kopiera svaret.
7. Aktivera Outlook, öppna Svara och klistra in svaret som utkast.
8. Visa mottagare, ämne och svar för manuell granskning.
9. Kräv en separat aktiv bekräftelse precis före Skicka. Upprepade automatiska skick tillåts inte.

## Säkerhet och acceptanskriterier

- Torrkörning söker och markerar mål men klickar, skriver, klistrar eller laddar inte ned något.
- Före varje inmatning aktiveras och verifieras rätt fönster på nytt.
- Flera OCR-/bildträffar, låg bildlikhet, fel fönster eller timeout stoppar flödet utan att gissa.
- Nödstopp och vanlig stoppknapp kontrolleras omedelbart före varje klick eller tangentåtgärd.
- Nedladdningsflödet stoppar efter exakt valt antal verifierade klick och vid första oväntade dialogen.
- Mejlinnehåll, urklipp och skärmbilder sparas inte i loggen som standard.
- Windows v1 begränsar koordinatreserv till huvudskärmen; OCR-/bildankare ska användas på andra skärmar tills virtuella skärmkoordinater stöds säkert.
