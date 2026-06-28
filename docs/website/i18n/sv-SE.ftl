# Navigering
nav-contributing = Bidra
nav-crates = Crates
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
landing-cta-contributing = Bidra

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
interfaces-radio-body = BLE Auto-interface, ESP-NOW och LoRa för in nära enheter, kortflottor och långräckviddiga länkar i ett RNS-mesh.

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
standards-safety-headline = #![forbid(unsafe_code)]
standards-safety-body = personal-rns-motorn innehåller noll unsafe, framtvingat av kompilatorn. Unsafe i beroenden kontrolleras för UB under Miri och granskas med cargo-geiger.
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
start-daemon-code = apt install personal-rnsd
start-daemon-target = personal-rnsd

start-mobile-headline = Jag bygger en mobilapp
start-mobile-body = Kotlin (.aar), Swift (.xcframework) eller Python (.whl) — samma motor som din daemon kör, inbäddad direkt i din app.
start-mobile-code = implementation("org.staypersonal:rns:0.1")
    pod 'PersonalRns', '~> 0.1'
start-mobile-target = personal-rns-ffi

start-game-headline = Jag shippar i ett spel
start-game-body = C# / .NET-bindningar för Unity, Godot och MonoGame. Multiplayer utan att sätta upp en server.
start-game-code = dotnet add package Personal.Rns
start-game-target = personal-rns-ffi

start-embedded-headline = Flash a Hopspot
start-embedded-body = Pick a supported board, compare radio and battery tradeoffs, then flash a dedicated mesh device.
start-embedded-code = Board matrix
    Web flasher
    Local flash
start-embedded-target = Flash a Hopspot

start-web-headline = Jag bygger för webben eller edge
start-web-body = En WebAssembly-build som kör i webbläsaren och på edge-runtimes som Cloudflare Workers, Fastly och Spin.
start-web-code = npm install personal-rns
start-web-target = personal-rns (wasm32)

start-rust-headline = Jag bäddar in i en Rust-app
start-rust-body = En komplett RNS-runtime direkt ur lådan, eller den rena kärnan för att bygga din egen runtime runt.
start-rust-code = cargo add personal-rnsd   # complete RNS runtime
    cargo add personal-rns      # pure core only
start-rust-target = personal-rnsd or personal-rns

start-lxmf-headline = Jag vill skicka meddelanden över meshet
start-lxmf-body = LXMF ovanpå Reticulum — identiteter, adresser, leverans. Lagret som Sideband och Nomadnet vilar på.
start-lxmf-code = cargo add personal-lxmf
start-lxmf-target = personal-lxmf

# Plattformar ("Runs on") — hero marquee label + CTA och dedikerad sida
landing-platforms-label = Runs on
landing-platforms-cta = See all →
platforms-title = Where Prns runs
platforms-lead = One engine, many homes. This quick view separates runtime platform support from specific Hopspot board support.
platforms-legend-runtime = Runtime platform
platforms-legend-bringup = Active bring-up
platforms-legend-roadmap = Roadmap
platforms-runtime-title = Runtime support quick view
platforms-runtime-lead = Microcontrollers list silicon and radio families here; exact boards, flashing readiness, and interfaces live in the board catalog.
platforms-board-support-link = Specific board support →

# Flash a Hopspot page
flash-back = Platforms
flash-back-boards = Boards
flash-kicker = Supported boards
flash-title = Flash a Hopspot
flash-lead = Pick a specific board, compare radio and battery tradeoffs, then flash or build the dedicated Hopspot firmware path.
flash-note = Hosted builds can download firmware artifacts directly. When this same docs site is served from a Hopspot, artifact actions should stay disabled and point back to the online flasher or local build path.
flash-board-title = Select a board
flash-board-lead = Choose a flashable target to load its board-specific flasher. Bring-up and roadmap boards stay visible here, but cannot be selected yet.
flash-picker-change-title = Change board
flash-interfaces-label = Interfaces
flash-interfaces-pending = Interfaces pending board bring-up
flash-card-action = Flash
flash-card-selected = Selected
flash-ready-kicker = Ready target
flash-ready-title = Web flashing
flash-ready-action = Connect and flash
flash-ready-action-pending = Firmware artifacts are not wired into this build yet.
flash-local-title = Local build
flash-local-body = Fully offline? Build this repo locally and flash the board-specific Hopspot target from a developer machine.
flash-unavailable-title = Not flashable yet
flash-unavailable-body = This target is listed for bring-up or roadmap tracking, but it does not have a public web-flash artifact yet.
flash-missing-title = Board not found
flash-missing-body = Pick a supported board from the catalog.

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

# Crates-index
crates-kicker = Delarna
crates-title = Välj det som matchar det du bygger.
crates-lead = Varje crate är byggd för att vara användbar på egen hand, även om du inte drar in resten. Motorn är substratet; allt annat staplas ovanpå, och fler delar landar när sviten växer.
crates-card-cta = Vad den gör →
crates-back = Alla crates
crates-not-found = Ingen crate med det namnet

# Kort per crate
crate-rns-role = Motorn
crate-rns-blurb = Släpp in Reticulum i vilket Rust-projekt som helst. Deterministisk, no_std, allokeringsfri; inget globalt tillstånd, ingen inbyggd I/O — ta med din egen klocka och wire.
crate-rnsd-role = Daemonen
crate-rnsd-blurb = En drop-in för rnsd som kör överallt där Linux kör. Samma wire som RNS-referensen; använd den bredvid eller istället för noderna du redan har.
crate-lxmf-role = Meddelanden
crate-lxmf-blurb = LXMF ovanpå Reticulum — lagret som Sideband och Nomadnet vilar på. Identiteter, adresser, meddelandeleverans.
crate-ffi-role = Mobil- och Python-bindningar
crate-ffi-blurb = Ett uniffi-gränssnitt genererar Kotlin (.aar), Swift (.xcframework) och Python (.whl). Använd Reticulum från Android, iOS eller en Jupyter-notebook — samma form, samma motor.

# 404
not-found-title = Här finns inget än.
not-found-cta = Tillbaka till startsidan
