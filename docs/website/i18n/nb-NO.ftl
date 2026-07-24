# Navigasjon
nav-contributing = Bidra
nav-crates = Crates
nav-api = API

# Bunntekst
footer-tagline = Levert av Personal-teamet.

# Landing
landing-kicker = Ustoppelige mesh-nettverk for folk
landing-kicker-prefix = Ustoppelige mesh-nettverk for
landing-title = En høyytelsesport av Reticulum (RNS) skrevet i sikker Rust.
landing-title-lead = A high-performance port of Reticulum (RNS).
landing-title-accent = Runs on any device.
landing-subtitle = En deterministisk, no_std, allokatorfri kjerne. Bygget for ytelsen og stabiliteten alle Reticulum-noder trenger, fra en femdollars mikrokontroller til en skyserver.
landing-cta-ethos = Velg en crate
landing-cta-contributing = Bidra

# Sitat
landing-quote-label = Det vi bygger mot
landing-quote-body = Reticulum er den grunnleggende kommunikasjonsinfrastrukturen for en lys fremtid vi kan få, så lenge vi alle bygger den. Dette er Personal-teamets innsats for å få RNS i hendene på flere byggere og hjelpe den fremtiden frem.

# Interfaces
interfaces-section-label = Interfaces
interfaces-section-title = Der meshet møter verden
interfaces-section-lead = Prns bevarer de RNS-kompatible interfacene byggere allerede kjenner, og utvider kartet med native lenker for nye enheter og nettverk.
interfaces-section-hot-note = Prns-interfaces er hot-swappable: legg til, fjern eller endre et interface uten node-omstart.

interfaces-radio-label = Radioer
interfaces-radio-headline = Nærhetslenker for enheter og kort
interfaces-radio-body = Bluetooth LE Auto-interface, ESP-NOW og LoRa bringer nære enheter, kortflåter og langtrekkende lenker inn i ett RNS-mesh.

interfaces-lan-label = LAN
interfaces-lan-headline = Automatisk oppdagede local-link-peers
interfaces-lan-body = Wi-Fi Auto-interface bruker multicast, mDNS og gateway-rendezvous til å finne nære noder og folde et lokalt nettverk inn i meshet.

interfaces-cable-label = Kabler + packet radio
interfaces-cable-headline = Kabler, TNC-er og radiomodemer
interfaces-cable-body = USB Auto-interface, seriell framing, KISS, AX.25 og RNode kobler små enheter og packet-radio-hardware inn i samme mesh.

interfaces-host-label = Rutet IP
interfaces-host-headline = Internet-, WAN- og backbone-lenker
interfaces-host-body = TCP-klient/server, UDP og Backbone lar fjerne peers delta i meshet over private WAN, VPN og releer på det åpne internettet.

# Det du kan stole på
standards-section-label = Våre standarder
standards-section-title = Det du kan stole på
standards-license-label = Lisens
standards-license-headline = MIT / Apache 2.0
standards-license-body = Dobbeltlisensiert og permissiv. Ingen copyleft eller kommersielle begrensninger.
standards-safety-label = Sikkerhet
standards-safety-headline = #![forbid(unsafe_code)]
standards-safety-body = personal-rns-motoren inneholder null unsafe, håndhevet av kompilatoren. Unsafe i avhengigheter sjekkes for UB under Miri og auditeres med cargo-geiger.
standards-correctness-label = Korrekthet
standards-correctness-headline = Diff-testet mot RNS
standards-correctness-body = Hver endring sjekkes mot referansen og kjøres deretter gjennom property-, fuzz- og mutasjonstester, med Kani-bevis der de betyr noe.
standards-benchmarked-label = Ytelse
standards-benchmarked-headline = Målt, ikke bare påstått
standards-benchmarked-body = Ytelse følges åpent, målt av et harness du kan kjøre selv.
standards-benchmarked-cta = Se benchmarkene →

# Hvor begynner jeg?
start-section-label = Veier inn
start-section-title = Hvor begynner jeg?
start-section-lead = Velg veien som passer det du bygger. Hver lander på én crate i dag; flere guider kommer ved siden av dem.

start-daemon-headline = Jeg vil kjøre en Reticulum-node
start-daemon-body = Ferdigbygd daemon. Drop-in for rnsd. Kjør den ved siden av nodene du allerede har.
start-daemon-target = prnsd

start-mobile-headline = Jeg bygger en mobilapp
start-mobile-body = Kotlin (.aar), Swift (.xcframework) eller Python (.whl) — samme motor som daemonen din kjører, innebygd direkte i appen din.
start-mobile-code = implementation("org.staypersonal:rns:0.1")
    pod 'PersonalRns', '~> 0.1'
start-mobile-target = prns-ffi

start-game-headline = Jeg shipper i et spill
start-game-body = C# / .NET-bindings for Unity, Godot og MonoGame. Flerspiller uten å sette opp en server.
start-game-code = dotnet add package Personal.Rns
start-game-target = prns-ffi

start-embedded-headline = Flash a Hopspot
start-embedded-body = Pick a supported board, compare radio and battery tradeoffs, then flash a dedicated mesh device.
start-embedded-code = Board matrix
    Web flasher
    Local flash
start-embedded-target = Flash a Hopspot

start-web-headline = Bruk lekeplassen for nettlesernoder
start-web-body = Prøv TypeScript-API-et med den delte Rust-motoren i WebAssembly, koble til via Auto Wi-Fi eller USB Auto, og følg lokal nodeaktivitet direkte.
start-web-code = WebAssembly-kjøremiljø
    Auto Wi-Fi + USB Auto
    TypeScript-eksempel
start-web-target = Åpne lekeplassen

start-rust-headline = Jeg bygger det inn i en Rust-app
start-rust-body = En komplett RNS-runtime rett ut av boksen, eller den rene kjernen for å bygge din egen runtime rundt.
start-rust-target = prnsd or personal-rns


# Plattformer ("Runs on") — hero marquee label + CTA og egen side
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
flash-interfaces-label = Eligible interfaces
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

# Benchmark-side
benchmarks-kicker = Ytelse
benchmarks-title = Benchmarket i det åpne
benchmarks-lead = Vi behandler ytelse som et tall, ikke et adjektiv. Hver verdi her kommer fra et deterministisk harness i repoet, målt på ekte maskinvare og sjekket mot RNS-referansen der sammenligningen er rettferdig. Tallene lander etter hvert som suiten stabiliseres; nedenfor er metodikken de skal holde.

# Lisenssignal (bunntekst)
footer-license = Åpen kildekode. MIT / Apache 2.0.
footer-trademarks = Tredjepartslogoer og varemerker tilhører sine respektive eiere. De vises bare for å identifisere plattformer, maskinvare og kompatibilitetsmål. Ingen godkjenning hevdes eller antydes.

# Bidrag-side
contributing-kicker = Listen
contributing-title = Bidra
contributing-lead = Slik bidrar du — hva vi verdsetter, konvensjonene koden din følger, og standarden hver endring må klare. For både menneskelige og automatiserte bidragsytere.

# Crates-indeks
crates-kicker = Delene
crates-title = Velg det som passer det du bygger.
crates-lead = Hver crate er bygget for å være nyttig alene, selv om du ikke trekker inn resten. Motoren er substratet; alt annet stables oppå, og flere deler lander etter hvert som suiten vokser.
crates-card-cta = Hva den gjør →
crates-back = Alle crates
crates-not-found = Ingen crate med det navnet

# Kort per crate
crate-rns-role = Motoren
crate-rns-blurb = Slipp Reticulum inn i hvilket som helst Rust-prosjekt. Deterministisk, no_std, allokatorfri; ingen global tilstand, ingen innebygd I/O — ta med din egen klokke og wire.
crate-rnsd-role = Daemonen
crate-rnsd-blurb = En drop-in for rnsd på macOS, Linux og Windows. Samme wire som RNS-referansen; bruk den ved siden av eller i stedet for nodene du allerede har.
crate-ffi-role = Mobil- og Python-bindings
crate-ffi-blurb = Ett uniffi-grensesnitt genererer Kotlin (.aar), Swift (.xcframework) og Python (.whl). Bruk Reticulum fra Android, iOS eller en Jupyter-notebook — samme form, samme motor.

# 404
not-found-title = Her er det ingenting ennå.
not-found-cta = Tilbake til forsiden
