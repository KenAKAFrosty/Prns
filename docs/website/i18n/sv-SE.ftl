# Navigering
nav-contributing = Bidra
nav-crates = Crates
nav-api = API

# Sidfot
footer-tagline = Levererat av Personal-teamet.

# Landing
landing-kicker = Ostoppbara mesh-nätverk för människor
landing-kicker-prefix = Ostoppbara mesh-nätverk för
landing-title = En produktionsklar port av Reticulum (RNS) skriven i säker Rust.
landing-subtitle = En deterministisk, no_std, allokeringsfri kärna. Byggd för den prestanda och stabilitet varje Reticulum-nod behöver, från en femdollars mikrokontroller till en molnserver.
landing-cta-ethos = Välj en crate
landing-cta-contributing = Bidra

# Citat
landing-quote-label = Det vi bygger mot
landing-quote-body = Reticulum är den grundläggande kommunikationsinfrastrukturen för en ljus framtid vi kan få, så länge vi alla bygger den. Det här är Personal-teamets arbete för att lägga RNS i händerna på fler byggare och hjälpa den framtiden att bli verklig.

# Det du kan räkna med
standards-section-label = Våra standarder
standards-section-title = Det du kan räkna med
standards-license-label = Licens
standards-license-headline = MIT / Apache 2.0
standards-license-body = Dubbellicensierat och permissivt. Ingen copyleft eller kommersiella begränsningar.
standards-safety-label = Säkerhet
standards-safety-headline = #![forbid(unsafe_code)]
standards-safety-body = Våra crates innehåller noll unsafe, framtvingat av kompilatorn. Unsafe i beroenden kontrolleras för UB under Miri och granskas med cargo-geiger.
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

start-embedded-headline = Jag siktar på mikrokontroller
start-embedded-body = Motorn plus ett Host-trait med tre metoder. ESP32-C6 är referensen; S3, nRF, RP2040 och STM32 är näst på tur.
start-embedded-code = cargo add personal-rns --no-default-features
start-embedded-target = personal-rns + personal-hopspot

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
landing-platforms-label = Kör på
landing-platforms-cta = Se alla →
platforms-title = Där Prns kör
platforms-lead = En motor, många hem. Några av dessa levereras idag; resten finns på roadmapen — nordstjärnan vi bygger mot. Fyllda chip kör nu; streckade är nästa.
platforms-legend-shipping = Levereras idag
platforms-legend-roadmap = Roadmap

# Benchmarksida
benchmarks-kicker = Prestanda
benchmarks-title = Benchmarkat öppet
benchmarks-lead = Vi behandlar prestanda som ett tal, inte ett adjektiv. Varje siffra här kommer från ett deterministiskt harness i repot, mätt på riktig hårdvara och kontrollerad mot RNS-referensen där jämförelsen är rättvis. Siffrorna landar medan sviten stabiliseras; nedan finns metodiken de håller sig till.

# Licenssignal (sidfot)
footer-license = Öppen källkod. MIT / Apache 2.0.

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
