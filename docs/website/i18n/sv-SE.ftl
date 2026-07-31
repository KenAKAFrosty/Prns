# Navigering
nav-contributing = Bidra
nav-api = API

# Sidfot
footer-tagline = Levererat av Personal-teamet.

# Landing
landing-kicker = Ostoppbara mesh-nätverk för människor
landing-kicker-prefix = Ostoppbara mesh-nätverk för
landing-title = En högpresterande port av Reticulum (RNS) skriven i säker Rust.
landing-title-lead = A high-performance port of Reticulum (RNS).
landing-title-accent = Runs on any device.
landing-subtitle = En deterministisk, no_std, allokeringsfri kärna. Byggd för den prestanda och stabilitet varje Reticulum-nod behöver, från en femdollars mikrokontroller till en molnserver.
landing-cta-ethos = Välj en crate
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
interfaces-radio-body = Bluetooth LE Auto-interface, ESP-NOW och LoRa för in nära enheter, kortflottor och långräckviddiga länkar i ett RNS-mesh.

interfaces-lan-label = LAN
interfaces-lan-headline = Automatiskt upptäckta local-link-peers
interfaces-lan-body = Wi-Fi Auto-interface använder multicast, mDNS och gateway-rendezvous för att hitta nära noder och vika in ett lokalt nätverk i meshet.

interfaces-cable-label = Kablar + packet radio
interfaces-cable-headline = Kablar, TNC:er och radiomodem
interfaces-cable-body = USB Auto-interface, seriell framing, KISS, AX.25 och RNode kopplar små enheter och packet-radio-hårdvara till samma mesh.

interfaces-host-label = Routad IP
interfaces-host-headline = Internet-, WAN- och backbone-länkar
interfaces-host-body = TCP-klient/server, UDP och Backbone låter avlägsna peers delta i meshet över privata WAN, VPN och reläer på det öppna internet.

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
standards-correctness-body = Varje ändring kontrolleras mot referensen och körs sedan genom property-, fuzz- och mutationstester, med Kani-bevis där de spelar roll.
standards-benchmarked-label = Prestanda
standards-benchmarked-headline = Mätt, inte bara påstådd
standards-benchmarked-body = Prestanda följs öppet, mätt av ett harness som du kan köra själv.
standards-benchmarked-cta = Se benchmarks →

# Var börjar jag?
start-section-label = Vägar in
start-section-title = Var börjar jag?
start-section-lead = Välj den väg som matchar det du bygger. Varje väg landar på en enda crate idag; fler guider kommer bredvid dem.

start-daemon-headline = Jag vill köra en Reticulum-nod
start-daemon-body = Färdigbyggd daemon. Drop-in för rnsd. Kör den bredvid noderna du redan har.
start-daemon-target = prnsd

start-embedded-headline = Flash a Hopspot
start-embedded-body = Pick a supported board, compare radio and battery tradeoffs, then flash a dedicated mesh device.
start-embedded-code = Board matrix
    Web flasher
    Local flash
start-embedded-target = Flash a Hopspot

start-web-headline = Använd lekplatsen för webbläsarnoder
start-web-body = Prova TypeScript-API:t med den delade Rust-motorn i WebAssembly, anslut via Auto Wi-Fi eller USB Auto och följ lokal nodaktivitet live.
start-web-code = WebAssembly-körning
    Auto Wi-Fi + USB Auto
    TypeScript-exempel
start-web-target = Öppna lekplatsen

start-rust-headline = Jag bäddar in i en Rust-app
start-rust-body = En komplett RNS-runtime direkt ur lådan, eller den rena kärnan för att bygga din egen runtime runt.
start-rust-target = prnsd or personal-rns

# Plattformar ("Runs on") — hero marquee label + CTA och dedikerad sida
landing-platforms-label = Runs on
landing-platforms-cta = See all →
platforms-title = Where Prns runs
platforms-lead = One engine, many homes. This quick view separates runtime platform support from specific Hopspot board support.
platforms-board-support-link = View board support & bring-up →

# Flash a Hopspot page
flash-back = Platforms
flash-back-boards = Boards
flash-card-action = Flash

# Benchmarksida
benchmarks-kicker = Prestanda
benchmarks-title = Benchmarkat öppet
benchmarks-lead = Vi behandlar prestanda som ett tal, inte ett adjektiv. Varje siffra här kommer från ett deterministiskt harness i repot, mätt på riktig hårdvara och kontrollerad mot RNS-referensen där jämförelsen är rättvis. Siffrorna landar medan sviten stabiliseras; nedan finns metodiken de håller sig till.

# Licenssignal (sidfot)
footer-license = Öppen källkod. MIT / Apache 2.0.
footer-trademarks = Tredjepartslogotyper och varumärken tillhör sina respektive ägare. De visas endast för att identifiera plattformar, hårdvara och kompatibilitetsmål. Inget godkännande hävdas eller antyds.

# Bidragssida
contributing-kicker = Ribban
contributing-title = Bidra
contributing-lead = Så här bidrar du — vad vi värdesätter, konventionerna din kod följer och standarden varje ändring klarar. För både mänskliga och automatiserade bidragsgivare.

# 404
not-found-title = Här finns inget än.
not-found-cta = Tillbaka till startsidan
