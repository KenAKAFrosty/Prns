# Navigering
nav-ethos = Design
nav-crates = Crates
nav-api = API

# Sidfot
footer-tagline = Skapad av Personal-teamet.

# Landing
landing-kicker = Ostoppbara mesh-nätverk — för folket
landing-title = En produktionsklar port av Reticulum (RNS) skriven i Rust.
landing-subtitle = En deterministisk kärna utan std och utan allokerare. Fullständig täckning av RNS och LXMF. Byggd med den prestanda och batteritid i åtanke som varje Reticulum-stack behöver — från en mikrokontroller för fem dollar ända till en molnnod.
landing-cta-ethos = Välj en crate
landing-cta-crates = Så bygger vi det

# Pull quote
landing-quote-label = Det vi bygger mot
landing-quote-body = Reticulum är den grundläggande kommunikationsinfrastrukturen i den ljusa framtid vi kan ha — om vi bygger den. Detta är vår insats för att lägga den i händerna på fler utvecklare och hjälpa den framtiden på väg.

# What you can count on
standards-section-label = Våra standarder
standards-section-title = Det du kan räkna med
standards-license-label = Licens
standards-license-headline = MIT / Apache 2.0
standards-license-body = Dubbellicensierad och tillåtande. Ingen copyleft och inga icke-kommersiella villkor.
standards-coverage-label = Täckning
standards-coverage-headline = Fullt RNS och LXMF
standards-coverage-body = Inte bara RNS. Och LXMF är ingen bifigur. Båda, hela vägen.
standards-core-label = Kärna
standards-core-headline = no_std, ingen allokerare
standards-core-body = En deterministisk kärna som kör där allokerare inte kan.
standards-verification-label = Verifiering
standards-verification-headline = Diff-testad mot RNS
standards-verification-body = Varje ändring stäms av mot referensen, och där det verkligen spelar roll följer formella bevis med.

# Where do I start?
start-section-label = Vägar in
start-section-title = Var börjar jag?
start-section-lead = Välj vägen som matchar det du bygger. Var och en landar i en enda crate idag, och dedikerade guider följer i samma takt.

start-daemon-headline = Jag vill ha en Reticulum-nod igång
start-daemon-body = Färdigbyggd daemon. Drop-in för rnsd. Ställ den bredvid noderna du redan har och låt dem köra tillsammans.
start-daemon-code = apt install personal-rnsd
start-daemon-target = personal-rnsd

start-mobile-headline = Jag bygger en mobilapp
start-mobile-body = Kotlin (.aar), Swift (.xcframework) eller Python (.whl) — samma motor som din daemon kör, inbäddad direkt i din app.
start-mobile-code = implementation("org.staypersonal:rns:0.1")
    pod 'PersonalRns', '~> 0.1'
start-mobile-target = personal-rns-ffi

start-game-headline = Jag levererar det i ett spel
start-game-body = C#/.NET-bindningar för Unity, Godot och MonoGame. Multiplayer utan att behöva sätta upp en server.
start-game-code = dotnet add package Personal.Rns
start-game-target = personal-rns-ffi

start-embedded-headline = Jag siktar på mikrokontroller
start-embedded-body = Motorn plus en Host-trait med bara tre metoder. ESP32-C6 är referensen; S3, nRF, RP2040 och STM32 står näst på tur.
start-embedded-code = cargo add personal-rns --no-default-features
start-embedded-target = personal-rns + hosts/*

start-web-headline = Jag bygger för webb eller edge
start-web-body = En WebAssembly-build som kör i webbläsaren och på edge-runtimes som Cloudflare Workers, Fastly och Spin.
start-web-code = npm install personal-rns
start-web-target = personal-rns (wasm32)

start-rust-headline = Jag bäddar in det i en Rust-app
start-rust-body = En komplett RNS-runtime direkt ur lådan, eller den rena kärnan att bygga din egen runtime omkring. Välj det som passar dig.
start-rust-code = cargo add personal-rnsd   # complete RNS runtime
    cargo add personal-rns      # pure core only
start-rust-target = personal-rnsd or personal-rns

start-lxmf-headline = Jag vill skicka meddelanden över meshet
start-lxmf-body = LXMF ovanpå Reticulum — identiteter, adresser, leverans. Det lager som Sideband och Nomadnet vilar på.
start-lxmf-code = cargo add personal-lxmf
start-lxmf-target = personal-lxmf

# License signal (footer)
footer-license = Open source. MIT / Apache 2.0.

# Ethos page
ethos-kicker = Disciplinen
ethos-title = Så bygger vi det
ethos-lead = En anteckning från ingenjör till ingenjör om disciplinen bakom projektet — ren motor, kärna utan allokerare, varje ändring verifierad mot RNS-referensen. Bläddra igenom innan du gör dig beroende av det; vi vill att du vet vad du ger dig in på.

# Crates index
crates-kicker = Delarna
crates-title = Välj den som passar det du bygger.
crates-lead = Varje crate är byggd för att vara användbar på egen hand, även om du inte drar in resten. Motorn är substratet; allt annat staplas ovanpå, och fler delar landar i takt med att sviten växer.
crates-card-cta = Vad den gör →
crates-back = Alla crates
crates-not-found = Ingen crate med det namnet

# Per-crate cards
crate-rns-role = Motorn
crate-rns-blurb = Lägg in Reticulum i vilket Rust-projekt som helst. Deterministisk, no_std, utan allokerare; ingen global state, ingen inbyggd I/O — du tar med dig klocka och kabel.
crate-rnsd-role = Daemonen
crate-rnsd-blurb = En drop-in för rnsd som kör var som helst där Linux kör. Samma protokoll som RNS-referensen; använd den vid sidan av eller istället för noderna du redan har.
crate-lxmf-role = Meddelanden
crate-lxmf-blurb = LXMF ovanpå Reticulum — det lager som Sideband och Nomadnet vilar på. Identiteter, adresser, leverans av meddelanden.
crate-ffi-role = Mobil- + Python-bindningar
crate-ffi-blurb = Ett uniffi-gränssnitt genererar Kotlin (.aar), Swift (.xcframework) och Python (.whl). Använd Reticulum från Android, iOS eller en Jupyter-notebook — samma form, samma motor.
crate-esp32c6-role = ESP32-C6-firmware
crate-esp32c6-blurb = Bare-metal-host-adapter för ESP32-C6. Inget OS, ingen allokerare — bevis för att motorn kör på ett RISC-V-chip för fem dollar med inbyggda radios.

# 404
not-found-title = Här finns ingenting än.
not-found-cta = Tillbaka till startsidan
