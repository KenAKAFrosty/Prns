# Navigation
nav-ethos = Design
nav-crates = Crates
nav-api = API

# Footer
footer-tagline = Lavet af Personal-teamet.

# Landing
landing-kicker = Ustoppelige meshnetværk — til folket
landing-title = En produktionsklar port af Reticulum (RNS) skrevet i Rust.
landing-subtitle = En deterministisk kerne uden std og uden allokator. Fuld dækning af RNS og LXMF. Bygget med den ydelse og batterilevetid for øje, som enhver Reticulum-stak har brug for — fra en mikrocontroller til fem dollar til en cloud-node.
landing-cta-ethos = Vælg et crate
landing-cta-crates = Sådan bygger vi det

# Pull quote
landing-quote-label = Det vi bygger hen imod
landing-quote-body = Reticulum er den fundamentale kommunikationsinfrastruktur i den lyse fremtid, vi kan få — hvis vi bygger den. Dette er vores indsats for at lægge den i hænderne på flere udviklere og hjælpe den fremtid på vej.

# What you can count on
standards-section-label = Vores standarder
standards-section-title = Det du kan regne med
standards-license-label = Licens
standards-license-headline = MIT / Apache 2.0
standards-license-body = Dobbeltlicenseret og tilladende. Ingen copyleft og ingen ikke-kommercielle begrænsninger.
standards-coverage-label = Dækning
standards-coverage-headline = Fuld RNS og LXMF
standards-coverage-body = Ikke kun RNS. Og LXMF er ikke en bifigur. Begge dele, helt igennem.
standards-core-label = Kerne
standards-core-headline = no_std, ingen allokator
standards-core-body = En deterministisk kerne, der kører, hvor allokatorer ikke kan.
standards-verification-label = Verifikation
standards-verification-headline = Diff-testet mod RNS
standards-verification-body = Hver ændring tjekkes mod referencen, og dér hvor det virkelig betyder noget, kommer formelle beviser med.

# Where do I start?
start-section-label = Veje ind
start-section-title = Hvor begynder jeg?
start-section-lead = Vælg den vej, der passer til det, du er ved at bygge. Hver enkelt lander på ét crate i dag, og dedikerede guider følger lige efter.

start-daemon-headline = Jeg vil have en Reticulum-node kørende
start-daemon-body = Færdigbygget daemon. Drop-in til rnsd. Sæt den ved siden af de noder, du allerede har, og lad dem køre sammen.
start-daemon-code = apt install personal-rnsd
start-daemon-target = personal-rnsd

start-mobile-headline = Jeg bygger en mobilapp
start-mobile-body = Kotlin (.aar), Swift (.xcframework) eller Python (.whl) — den samme motor, som din daemon kører, lagt direkte ind i din app.
start-mobile-code = implementation("org.staypersonal:rns:0.1")
    pod 'PersonalRns', '~> 0.1'
start-mobile-target = personal-rns-ffi

start-game-headline = Jeg leverer det i et spil
start-game-body = C#/.NET-bindinger til Unity, Godot og MonoGame. Multiplayer uden at skulle sætte en server op.
start-game-code = dotnet add package Personal.Rns
start-game-target = personal-rns-ffi

start-embedded-headline = Jeg sigter mod mikrocontrollere
start-embedded-body = Motoren plus en Host-trait med kun tre metoder. ESP32-C6 er referencen; S3, nRF, RP2040 og STM32 står næst på listen.
start-embedded-code = cargo add personal-rns --no-default-features
start-embedded-target = personal-rns + personal-hopspot

start-web-headline = Jeg bygger til web eller edge
start-web-body = En WebAssembly-build, der kører i browseren og på edge-runtimes som Cloudflare Workers, Fastly og Spin.
start-web-code = npm install personal-rns
start-web-target = personal-rns (wasm32)

start-rust-headline = Jeg lægger det ind i en Rust-app
start-rust-body = En komplet RNS-runtime ud af kassen, eller den rene kerne, så du kan bygge din egen runtime omkring den. Vælg det, der passer dig.
start-rust-code = cargo add personal-rnsd   # complete RNS runtime
    cargo add personal-rns      # pure core only
start-rust-target = personal-rnsd or personal-rns

start-lxmf-headline = Jeg vil sende beskeder over meshet
start-lxmf-body = LXMF oven på Reticulum — identiteter, adresser, levering. Det lag, Sideband og Nomadnet hviler på.
start-lxmf-code = cargo add personal-lxmf
start-lxmf-target = personal-lxmf

# License signal (footer)
footer-license = Open source. MIT / Apache 2.0.

# Ethos page
ethos-kicker = Disciplinen
ethos-title = Sådan bygger vi det
ethos-lead = En ingeniør-til-ingeniør-note om disciplinen bag projektet — ren motor, kerne uden allokator, hver ændring verificeret mod RNS-referencen. Læs den, før du gør dig afhængig af det; vi vil have, du ved, hvad du går ind til.

# Crates index
crates-kicker = Brikkerne
crates-title = Vælg det, der passer til det, du er ved at bygge.
crates-lead = Hvert crate er bygget til at være nyttigt for sig selv, selv hvis du ikke trækker resten med ind. Motoren er substratet; alt andet stables ovenpå, og flere brikker lander, efterhånden som suiten vokser.
crates-card-cta = Hvad det gør →
crates-back = Alle crates
crates-not-found = Intet crate med det navn

# Per-crate cards
crate-rns-role = Motoren
crate-rns-blurb = Læg Reticulum ind i et hvilket som helst Rust-projekt. Deterministisk, no_std, uden allokator; ingen global tilstand, ingen indbygget I/O — du har selv ur og ledning med.
crate-rnsd-role = Daemonen
crate-rnsd-blurb = En drop-in til rnsd, der kører, hvor Linux kører. Samme tråd som RNS-referencen; brug den ved siden af eller i stedet for de noder, du allerede har.
crate-lxmf-role = Beskeder
crate-lxmf-blurb = LXMF oven på Reticulum — det lag, Sideband og Nomadnet hviler på. Identiteter, adresser, beskedlevering.
crate-ffi-role = Mobil- + Python-bindinger
crate-ffi-blurb = Ét uniffi-interface genererer Kotlin (.aar), Swift (.xcframework) og Python (.whl). Brug Reticulum fra Android, iOS eller en Jupyter-notebook — samme form, samme motor.

# 404
not-found-title = Her er endnu ikke noget.
not-found-cta = Tilbage til forsiden
