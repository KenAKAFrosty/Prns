# Navigering
nav-benchmarks = Benchmarks
nav-contributing = Bidra
site-early-english-note = Prns är fortfarande i ett tidigt skede: den fullständiga dokumentationen finns på GitHub och i källkoden och är tills vidare endast på engelska.

# Sidfot
footer-tagline = Levererat av KenAKAFrosty och Personal/Prns-teamet.
footer-flash = Flasha en Hopspot (endast på engelska)
footer-playground = Webbläsarlekplats (endast på engelska)

# Landing
landing-kicker = Mesh-nätverk som är ditt
landing-kicker-prefix = Mesh-nätverk som är
landing-title = Högpresterande Reticulum (RNS), byggt för att köras på vilken enhet som helst.
landing-title-lead = Högpresterande Reticulum (RNS),
landing-title-accent = byggt för att köras på vilken enhet som helst.
landing-subtitle = Byggt för den prestanda, stabilitet och energieffektivitet varje Reticulum-nod behöver, från en 5-dollars mikrokontroller till ett molnserverkluster. En motor och ett API, samma på embedded, desktop, mobil, i spel och på webben.
landing-cta-ethos = Hitta din väg i Prns
landing-cta-standards = Våra standarder
# Citat
landing-quote-label = Det vi bygger mot
landing-quote-body = Reticulum är den grundläggande kommunikationsinfrastrukturen för en ljus framtid vi kan få, så länge vi alla bygger den. Det här är Personal-teamets arbete för att lägga RNS i händerna på fler byggare och hjälpa den framtiden att bli verklig.

# Interfaces
interfaces-section-label = Interfaces
interfaces-section-title = Där meshet möter världen
interfaces-section-lead = Prns behåller de RNS-kompatibla interfaces som byggare redan känner till och utökar kartan med native-länkar för nya enheter och nätverk.
interfaces-section-hot-note = Prns-interfaces är hot-swappable: lägg till, ta bort eller ändra ett interface utan nodomstart.

interfaces-radio-label = Radio
interfaces-radio-headline = Närhetslänkar för enheter och kort
interfaces-radio-body = Bluetooth LE Auto-interface, ESP-NOW och LoRa för in nära enheter, kortflottor och långräckviddiga RF-länkar i ett Reticulum-mesh.

interfaces-lan-label = LAN
interfaces-lan-headline = Automatiskt upptäckta local-link-peers
interfaces-lan-body = Wi-Fi Auto-interface använder multicast, mDNS och gateway-rendezvous för att hitta nära noder och väva in ett lokalt nätverk i meshet.

interfaces-cable-label = Kablar + packet radio
interfaces-cable-headline = Kablar, TNC:er och radiomodem
interfaces-cable-body = USB Auto-interface, seriell framing, KISS, AX.25 och RNode kopplar små enheter och packet-radio-hårdvara till samma mesh.

interfaces-host-label = Routad IP
interfaces-host-headline = Internet-, WAN- och backbone-länkar
interfaces-host-body = TCP-klient/server, UDP, WebSocket och Backbone låter avlägsna peers delta i meshet över privata WAN, VPN, reläer på det öppna internet och webbläsarintegrationer.

# Det du kan räkna med
standards-section-label = Våra standarder
standards-section-title = Det du kan räkna med
standards-license-label = Licens
standards-license-headline = MIT / Apache 2.0
standards-license-body = Dubbellicensierat och permissivt. Ingen copyleft eller kommersiella begränsningar.
standards-safety-label = Säkerhet
standards-safety-headline = Framtvingat, sedan granskat
standards-safety-body = I motorn kompilerar panics, unwraps och ogrundad unsafe aldrig. Det som inte kan förbjudas granskas: unsafe i beroenden med cargo-geiger, odefinierat beteende under Miri, säkerhetsvarningar med cargo-deny.
standards-correctness-label = Korrekthet
standards-correctness-headline = Diff-testat mot RNS
standards-correctness-body = Varje ändring kontrolleras mot referensen och körs sedan genom unit-, property-, fuzz- och mutationstester, med Kani-bevis där de spelar roll.
standards-benchmarked-label = Prestanda
standards-benchmarked-headline = Mätt, inte bara påstådd
standards-benchmarked-body = Prestanda följs öppet, mätt av ett harness som du kan köra själv.
standards-benchmarked-cta = Se benchmarks →

# Var börjar jag?
start-section-label = Vägar in
start-section-title = Vad är du här för att göra?
start-section-lead = Välj den väg som matchar hur Prns passar in i ditt arbete: hårdvara du flashar, infrastruktur du kör eller mjukvara du bygger.

start-daemon-headline = Kör en daemon
start-daemon-body = Installera en snabb Reticulum-daemon för desktops, LXMF-appar, backbone-VPS:er med mera.
start-daemon-code = Drop-in för standardappar
    Läser ~/.reticulum
    Live-redigering av interfaces
    Inbyggda mätvärden
start-daemon-target = Kör Prnsd

start-embedded-headline = Flasha en Hopspot
start-embedded-body = Välj ett kort som stöds, flasha det direkt från webbläsaren och du har en dedikerad mesh-enhet på några minuter.
start-embedded-code = Kortmatris
    Webbflashare
    Lokal flash
start-embedded-target = Flasha en Hopspot (endast på engelska)

start-web-headline = Använd lekplatsen för webbläsarnoder
start-web-body = Prova TypeScript-API:t med den delade Rust-motorn i WebAssembly, anslut via Auto Wi-Fi eller USB Auto och följ lokal nodaktivitet live.
start-web-code = WebAssembly-runtime
    Auto Wi-Fi + USB Auto
    TypeScript-exempel
start-web-target = Öppna lekplatsen (endast på engelska)

start-rust-headline = Bygg på Reticulum
start-rust-body = Använd motorn och bindningarna för att lägga till mesh-nätverk i appar, verktyg, tjänster eller spel.
start-rust-target = Läs README:n
start-rust-target-source = Ladda ner källkoden

# Plattformar ("Runs on") — hero marquee label + CTA och dedikerad sida
landing-platforms-label = Körs på
landing-platforms-cta = Se alla →
platforms-title = Där Prns körs
platforms-lead = En motor, många hem. Den här snabbvyn skiljer runtime-plattformsstöd från stöd för specifika Hopspot-kort.
platforms-board-support-link = Se Hopspot-kortstöd & bring-up →

# Flasha en Hopspot-sida
flash-back = Plattformar
flash-back-boards = Kort
flash-card-action = Flasha

# Benchmarksida
benchmarks-kicker = Prestanda
benchmarks-title = Benchmarkat öppet
benchmarks-lead = Varje siffra nedan kommer från de publicerade resultaten i repot, mätt på riktig hårdvara av ett harness som du kan köra själv. Härifrån är innehållet tills vidare endast på engelska.

# Licenssignal (sidfot)
footer-license = Öppen källkod. MIT / Apache 2.0.
footer-trademarks = Tredjepartslogotyper, varumärken och produktbilder tillhör sina respektive ägare. De visas endast för att identifiera plattformar, hårdvara och kompatibilitetsmål. Inget godkännande hävdas eller antyds.

# 404
not-found-title = Här finns inget än.
not-found-cta = Tillbaka till startsidan
