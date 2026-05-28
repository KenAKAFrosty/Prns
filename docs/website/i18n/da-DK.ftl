# Navigation
nav-ethos = Design
nav-crates = Crates
nav-api = API

# Footer
footer-tagline = Bragt til dig af Personal-teamet.

# Landing
landing-kicker = Ustoppelige meshnetværk, til folket
landing-title = En produktionsklar port af Reticulum (RNS) skrevet i Rust.
landing-subtitle = En deterministisk, no_std, alloc-fri kerne. Dækker RNS og LXMF fuldt ud. Native bindinger til Kotlin, Swift, Python, TypeScript og C#. WebAssembly til browsere og edge-runtimes. Bygget til den ydelse og batterilevetid enhver Reticulum-stak har brug for, fra en mikrocontroller til fem dollars til en cloud-node. Inkluderer en drop-in-erstatning for rnsd.
landing-cta-ethos = Vælg et crate
landing-cta-crates = Sådan bygger vi det

# Pull quote
landing-quote-label = Hvad vi bygger hen imod
landing-quote-body = Reticulum er den fundamentale kommunikationsinfrastruktur i den lyse fremtid vi kan få, hvis vi skaber den. Dette er vores indsats for at lægge den i hænderne på flere udviklere og hjælpe med at virkeliggøre den fremtid.

# What you can count on
standards-section-label = Vores standarder
standards-section-title = Hvad du kan regne med
standards-license-label = Licens
standards-license-headline = MIT / Apache 2.0
standards-license-body = Dobbeltlicenseret og tilladende. Ingen copyleft, ingen ikke-kommercielle begrænsninger.
standards-coverage-label = Dækning
standards-coverage-headline = Fuld RNS og LXMF
standards-coverage-body = Ikke kun RNS. Ikke LXMF som sidemand. Begge, fuldt ud.
standards-core-label = Kerne
standards-core-headline = no_std, alloc-fri
standards-core-body = En deterministisk kerne der kører hvor allokatorer ikke kan.
standards-verification-label = Verifikation
standards-verification-headline = Diff-testet mod RNS
standards-verification-body = Hver ændring tjekkes mod referencen; formelle beviser hvor det betyder noget.

# Where do I start?
start-section-label = Veje ind
start-section-title = Hvor begynder jeg?
start-section-lead = Vælg den vej der matcher det du bygger. Hver enkelt lander på ét crate i dag; flere guider lander sammen med dem.

start-daemon-headline = Jeg vil have en Reticulum-node kørende
start-daemon-body = Færdigbygget daemon. Drop-in til rnsd. Kør den ved siden af de noder du allerede har.
start-daemon-code = apt install personal-rnsd
start-daemon-target = personal-rnsd

start-mobile-headline = Jeg bygger en mobilapp
start-mobile-body = Kotlin (.aar), Swift (.xcframework) eller Python (.whl) — den samme motor din daemon kører, indlejret direkte i din app.
start-mobile-code = implementation("org.staypersonal:rns:0.1")
    pod 'PersonalRns', '~> 0.1'
start-mobile-target = personal-rns-ffi

start-game-headline = Jeg leverer det i et spil
start-game-body = C# / .NET-bindinger til Unity, Godot og MonoGame. Multiplayer uden at sætte en server op.
start-game-code = dotnet add package Personal.Rns
start-game-target = personal-rns-ffi

start-embedded-headline = Jeg arbejder med mikrocontrollere
start-embedded-body = Motoren plus en Host-trait med tre metoder. ESP32-C6 er referencen; S3, nRF, RP2040 og STM32 er de næste.
start-embedded-code = cargo add personal-rns --no-default-features
start-embedded-target = personal-rns + hosts/*

start-web-headline = Jeg bygger til web eller edge
start-web-body = En WebAssembly-build der kører i browseren og på edge-runtimes som Cloudflare Workers, Fastly og Spin.
start-web-code = npm install personal-rns
start-web-target = personal-rns (wasm32)

start-rust-headline = Jeg indlejrer det i en Rust-app
start-rust-body = En komplet RNS-runtime ud af boksen, eller den rene kerne til at bygge din egen runtime omkring.
start-rust-code = cargo add personal-rnsd   # complete RNS runtime
    cargo add personal-rns      # pure core only
start-rust-target = personal-rnsd or personal-rns

start-lxmf-headline = Jeg vil sende beskeder over meshet
start-lxmf-body = LXMF oven på Reticulum — identiteter, adresser, levering. Det lag som Sideband og Nomadnet hviler på.
start-lxmf-code = cargo add personal-lxmf
start-lxmf-target = personal-lxmf

# License signal (footer)
footer-license = Open source. MIT / Apache 2.0.

# Ethos page
ethos-kicker = Disciplinen
ethos-title = Sådan bygger vi det
ethos-lead = En ingeniør-til-ingeniør-note om disciplinen bag dette projekt — ren motor, alloc-fri kerne, hver ændring verificeret mod RNS-referencen. Skim den før du gør dig afhængig af det; vi vil have du ved hvad du går ind til.

# Crates index
crates-kicker = Brikkerne
crates-title = Vælg det der matcher det du bygger.
crates-lead = Hvert crate er bygget til at være nyttigt alene, selv hvis du ikke trækker resten ind. Motoren er substratet; alt andet stables ovenpå, og flere brikker kommer til efterhånden som suiten vokser.
crates-card-cta = Hvad det gør →
crates-back = Alle crates
crates-not-found = Intet crate med det navn

# Per-crate cards
crate-rns-role = Motoren
crate-rns-blurb = Læg Reticulum ind i ethvert Rust-projekt. Deterministisk, no_std, alloc-fri; ingen global tilstand, ingen indbygget I/O — medbring dit eget ur og din egen ledning.
crate-rnsd-role = Daemonen
crate-rnsd-blurb = En drop-in til rnsd der kører hvor som helst Linux kører. Samme tråd som RNS-referencen; brug den ved siden af eller i stedet for de noder du allerede har.
crate-lxmf-role = Beskeder
crate-lxmf-blurb = LXMF oven på Reticulum — det lag som Sideband og Nomadnet hviler på. Identiteter, adresser, beskedlevering.
crate-ffi-role = Mobil- + Python-bindinger
crate-ffi-blurb = Ét uniffi-interface genererer Kotlin (.aar), Swift (.xcframework) og Python (.whl). Brug Reticulum fra Android, iOS eller en Jupyter-notebook — samme form, samme motor.
crate-rvt-role = Visuel debugger
crate-rvt-blurb = Se pakker bevæge sig på tværs af simulerede noder på et virtuelt ur. Deterministisk — samme scenarie, samme spor, hver gang.
crate-esp32c6-role = ESP32-C6-firmware
crate-esp32c6-blurb = Bare-metal-host-adapter til ESP32-C6. Intet OS, ingen allokator — bevis for at motoren kører på en RISC-V-chip til fem dollars med indbyggede radioer.

# 404
not-found-title = Her er endnu ikke noget.
not-found-cta = Tilbage til forsiden
