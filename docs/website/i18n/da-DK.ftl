# Navigation
nav-contributing = Bidrag
nav-crates = Crates
nav-api = API

# Footer
footer-tagline = Bragt til dig af Personal-teamet.

# Landing
landing-kicker = Ustoppelige mesh-netværk for mennesker
landing-kicker-prefix = Ustoppelige mesh-netværk for
landing-title = En produktionsklar port af Reticulum (RNS) skrevet i sikker Rust.
landing-title-lead = A production-grade port of Reticulum (RNS).
landing-title-accent = Runs on any device.
landing-subtitle = En deterministisk, no_std, allokatorfri kerne. Bygget til den ydeevne og stabilitet, som enhver Reticulum-node har brug for, fra en mikrocontroller til fem dollars til en cloud-server.
landing-cta-ethos = Vælg en crate
landing-cta-contributing = Bidrag

# Pull quote
landing-quote-label = Det, vi bygger hen imod
landing-quote-body = Reticulum er den grundlæggende kommunikationsinfrastruktur for en lys fremtid, vi kan få, så længe vi alle bygger den. Dette er Personal-teamets indsats for at få RNS i hænderne på flere byggere og hjælpe den fremtid på vej.

# Interfaces
interfaces-section-label = Interfaces
interfaces-section-title = Hvor meshet møder verden
interfaces-section-lead = Prns bevarer de RNS-kompatible interfaces, buildere allerede kender, og udvider kortet med native links til nye enheder og netværk.

interfaces-radio-label = Radioer
interfaces-radio-headline = Nærhedslinks til enheder og boards
interfaces-radio-body = BLE Auto-interface, ESP-NOW og LoRa bringer nære enheder, board-flåder og langtrækkende links ind i ét RNS-mesh.

interfaces-lan-label = LAN
interfaces-lan-headline = Automatisk fundne local-link-peers
interfaces-lan-body = Wi-Fi Auto-interface bruger multicast, mDNS og gateway-rendezvous til at finde nære noder og folde et lokalt netværk ind i meshet.

interfaces-cable-label = Kabler + packet radio
interfaces-cable-headline = Kabler, TNC'er og radiomodems
interfaces-cable-body = USB Auto-interface, seriel framing, KISS, AX.25 og RNode forbinder små enheder og packet-radio-hardware til det samme mesh.

interfaces-host-label = Routet IP
interfaces-host-headline = Internet-, WAN- og backbone-links
interfaces-host-body = TCP client/server, UDP og Backbone lader fjerne peers deltage i meshet over private WANs, VPNs og relays på det offentlige internet.

# What you can count on (standards callout)
standards-section-label = Vores standarder
standards-section-title = Det kan du regne med
standards-license-label = Licens
standards-license-headline = MIT / Apache 2.0
standards-license-body = Dobbeltlicenseret og permissiv. Ingen copyleft eller kommercielle begrænsninger.
standards-safety-label = Sikkerhed
standards-safety-headline = #![forbid(unsafe_code)]
standards-safety-body = personal-rns-motoren indeholder nul unsafe, håndhævet af compileren. Unsafe i afhængigheder tjekkes for UB under Miri og auditeres med cargo-geiger.
standards-correctness-label = Korrekthed
standards-correctness-headline = Diff-testet mod RNS
standards-correctness-body = Hver ændring tjekkes mod referencen og køres derefter gennem property-, fuzz- og mutationstests med Kani-beviser dér, hvor de betyder noget.
standards-benchmarked-label = Ydeevne
standards-benchmarked-headline = Målt, ikke bare påstået
standards-benchmarked-body = Ydeevnen følges åbent, målt af et harness du selv kan køre.
standards-benchmarked-cta = Se benchmarks →

# Where do I start? (use-case cards on landing)
start-section-label = Veje ind
start-section-title = Hvor starter jeg?
start-section-lead = Vælg den vej, der matcher det, du bygger. Hver vej lander på én crate i dag; flere guides kommer ved siden af dem.

start-daemon-headline = Jeg vil have en Reticulum-node kørende
start-daemon-body = Færdigbygget daemon. Drop-in for rnsd. Kør den ved siden af de noder, du allerede har.
start-daemon-code = apt install personal-rnsd
start-daemon-target = personal-rnsd

start-mobile-headline = Jeg bygger en mobilapp
start-mobile-body = Kotlin (.aar), Swift (.xcframework) eller Python (.whl) — samme engine som din daemon bruger, indlejret direkte i din app.
start-mobile-code = implementation("org.staypersonal:rns:0.1")
    pod 'PersonalRns', '~> 0.1'
start-mobile-target = personal-rns-ffi

start-game-headline = Jeg sender i et spil
start-game-body = C# / .NET-bindings til Unity, Godot og MonoGame. Multiplayer uden at rejse en server.
start-game-code = dotnet add package Personal.Rns
start-game-target = personal-rns-ffi

start-embedded-headline = Flash a Hopspot
start-embedded-body = Pick a supported board, compare radio and battery tradeoffs, then flash a dedicated mesh device.
start-embedded-code = Board matrix
    Web flasher
    Local flash
start-embedded-target = Flash a Hopspot

start-web-headline = Jeg bygger til web eller edge
start-web-body = En WebAssembly-build, der kører i browseren og på edge-runtimes som Cloudflare Workers, Fastly og Spin.
start-web-code = npm install personal-rns
start-web-target = personal-rns (wasm32)

start-rust-headline = Jeg indlejrer i en Rust-app
start-rust-body = En komplet RNS-runtime ud af boksen, eller den rene kerne til at bygge din egen runtime omkring.
start-rust-code = cargo add personal-rnsd   # complete RNS runtime
    cargo add personal-rns      # pure core only
start-rust-target = personal-rnsd or personal-rns

start-lxmf-headline = Jeg vil sende beskeder over meshet
start-lxmf-body = LXMF oven på Reticulum — identiteter, adresser, levering. Laget som Sideband og Nomadnet ligger på.
start-lxmf-code = cargo add personal-lxmf
start-lxmf-target = personal-lxmf

# Platforms ("Runs on") — hero marquee label + CTA, and the dedicated page
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
flash-ready-body = This shared flasher surface follows the selected Hopspot board. Hosted builds will load that board's firmware artifact here; embedded-served docs should keep artifact flashing disabled and link back online.
flash-ready-action = Connect and flash
flash-ready-action-pending = Firmware artifacts are not wired into this build yet.
flash-local-title = Local build
flash-local-body = Fully offline? Build this repo locally and flash the board-specific Hopspot target from a developer machine.
flash-unavailable-title = Not flashable yet
flash-unavailable-body = This target is listed for bring-up or roadmap tracking, but it does not have a public web-flash artifact yet.
flash-missing-title = Board not found
flash-missing-body = Pick a supported board from the catalog.

# Benchmarks page
benchmarks-kicker = Ydeevne
benchmarks-title = Benchmarket i det åbne
benchmarks-lead = Vi behandler ydeevne som et tal, ikke et adjektiv. Hver figur her kommer fra et deterministisk harness i repoet, målt på rigtig hardware og tjekket mod RNS-referencen, hvor sammenligningen er fair. Tallene lander, efterhånden som suiten stabiliseres; nedenfor er metoden, de skal leve op til.

# License signal (footer)
footer-license = Open source. MIT / Apache 2.0.
footer-trademarks = Tredjepartslogoer og varemærker tilhører deres respektive ejere. De vises kun for at identificere platforme, hardware og kompatibilitetsmål; ingen godkendelse er underforstået.

# Contributing page
contributing-kicker = Standarden
contributing-title = Bidrag
contributing-lead = Sådan bidrager du — hvad vi værdsætter, de konventioner din kode følger, og den standard hver ændring skal klare. For både menneskelige og automatiserede bidragydere.

# Crates index
crates-kicker = Delene
crates-title = Vælg det, der matcher det, du bygger.
crates-lead = Hver crate er bygget til at være nyttig alene, selv hvis du ikke tager resten med. Enginen er substratet; alt andet stables ovenpå, og flere dele lander efterhånden som suiten vokser.
crates-card-cta = Hvad den gør →
crates-back = Alle crates
crates-not-found = Ingen crate med det navn

# Per-crate cards (consumer-framed)
crate-rns-role = Enginen
crate-rns-blurb = Slip Reticulum ind i ethvert Rust-projekt. Deterministisk, no_std, allokatorfri; ingen global tilstand, ingen indbygget I/O — tag dit eget ur og wire med.
crate-rnsd-role = Daemonen
crate-rnsd-blurb = En drop-in for rnsd, der kører overalt hvor Linux kører. Samme wire som RNS-referencen; brug den ved siden af eller i stedet for de noder, du allerede har.
crate-lxmf-role = Beskeder
crate-lxmf-blurb = LXMF oven på Reticulum — laget som Sideband og Nomadnet ligger på. Identiteter, adresser, levering af beskeder.
crate-ffi-role = Mobil- og Python-bindings
crate-ffi-blurb = Én uniffi-grænseflade genererer Kotlin (.aar), Swift (.xcframework) og Python (.whl). Brug Reticulum fra Android, iOS eller en Jupyter-notebook — samme form, samme engine.

# 404
not-found-title = Her er der ikke noget endnu.
not-found-cta = Tilbage til forsiden
