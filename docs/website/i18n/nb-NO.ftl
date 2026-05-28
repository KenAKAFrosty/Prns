# Navigasjon
nav-ethos = Design
nav-crates = Crates
nav-api = API

# Bunntekst
footer-tagline = Levert til deg av Personal-teamet.

# Landing
landing-kicker = Ustoppelige mesh-nettverk, for folket
landing-title = En produksjonsklar port av Reticulum (RNS) skrevet i Rust.
landing-subtitle = En deterministisk, no_std, allokatorfri kjerne. Dekker RNS og LXMF fullt ut. Native bindinger for Kotlin, Swift, Python, TypeScript og C#. WebAssembly for nettlesere og edge-runtimer. Bygd for ytelsen og batteritiden enhver Reticulum-stakk trenger, fra en mikrokontroller til fem dollar til en skynode. Inkluderer en drop-in-erstatning for rnsd.
landing-cta-ethos = Velg en crate
landing-cta-crates = Slik bygger vi det

# Pull quote
landing-quote-label = Hva vi bygger mot
landing-quote-body = Reticulum er den grunnleggende kommunikasjonsinfrastrukturen i den lyse fremtiden vi kan ha, hvis vi skaper den. Dette er vår innsats for å legge den i hendene på flere utviklere og bidra til å realisere den fremtiden.

# What you can count on
standards-section-label = Våre standarder
standards-section-title = Hva du kan stole på
standards-license-label = Lisens
standards-license-headline = MIT / Apache 2.0
standards-license-body = Dobbeltlisensiert og tillatende. Ingen copyleft, ingen ikke-kommersielle restriksjoner.
standards-coverage-label = Dekning
standards-coverage-headline = Fullt RNS og LXMF
standards-coverage-body = Ikke bare RNS. Ikke LXMF ved siden av. Begge, fullt ut.
standards-core-label = Kjerne
standards-core-headline = no_std, allokatorfri
standards-core-body = En deterministisk kjerne som kjører der allokatorer ikke kan.
standards-verification-label = Verifisering
standards-verification-headline = Diff-testet mot RNS
standards-verification-body = Hver endring sjekkes mot referansen; formelle bevis der det betyr noe.

# Where do I start?
start-section-label = Veier inn
start-section-title = Hvor begynner jeg?
start-section-lead = Velg veien som passer det du bygger. Hver enkelt lander på én crate i dag; flere guider kommer ved siden av.

start-daemon-headline = Jeg vil ha en Reticulum-node i gang
start-daemon-body = Ferdigbygd daemon. Drop-in for rnsd. Kjør den ved siden av nodene du allerede har.
start-daemon-code = apt install personal-rnsd
start-daemon-target = personal-rnsd

start-mobile-headline = Jeg bygger en mobil-app
start-mobile-body = Kotlin (.aar), Swift (.xcframework) eller Python (.whl) — samme motor som daemonen kjører, innebygd direkte i appen.
start-mobile-code = implementation("org.staypersonal:rns:0.1")
    pod 'PersonalRns', '~> 0.1'
start-mobile-target = personal-rns-ffi

start-game-headline = Jeg leverer det i et spill
start-game-body = C# / .NET-bindinger for Unity, Godot og MonoGame. Multiplayer uten å sette opp en server.
start-game-code = dotnet add package Personal.Rns
start-game-target = personal-rns-ffi

start-embedded-headline = Jeg sikter mot mikrokontrollere
start-embedded-body = Motoren pluss en Host-trait med tre metoder. ESP32-C6 er referansen; S3, nRF, RP2040 og STM32 står for tur.
start-embedded-code = cargo add personal-rns --no-default-features
start-embedded-target = personal-rns + hosts/*

start-web-headline = Jeg bygger for web eller edge
start-web-body = En WebAssembly-build som kjører i nettleseren og på edge-runtimer som Cloudflare Workers, Fastly og Spin.
start-web-code = npm install personal-rns
start-web-target = personal-rns (wasm32)

start-rust-headline = Jeg bygger det inn i en Rust-app
start-rust-body = En komplett RNS-runtime rett ut av boksen, eller den rene kjernen til å bygge din egen runtime rundt.
start-rust-code = cargo add personal-rnsd   # complete RNS runtime
    cargo add personal-rns      # pure core only
start-rust-target = personal-rnsd or personal-rns

start-lxmf-headline = Jeg vil sende meldinger over meshet
start-lxmf-body = LXMF oppå Reticulum — identiteter, adresser, levering. Laget som Sideband og Nomadnet hviler på.
start-lxmf-code = cargo add personal-lxmf
start-lxmf-target = personal-lxmf

# License signal (footer)
footer-license = Open source. MIT / Apache 2.0.

# Ethos page
ethos-kicker = Disiplinen
ethos-title = Slik bygger vi det
ethos-lead = En ingeniør-til-ingeniør-merknad om disiplinen bak prosjektet — ren motor, allokatorfri kjerne, hver endring verifisert mot RNS-referansen. Bla gjennom før du gjør deg avhengig; vi vil at du skal vite hva du går inn i.

# Crates index
crates-kicker = Delene
crates-title = Velg det som passer det du bygger.
crates-lead = Hver crate er bygd til å være nyttig alene, selv om du ikke trekker inn resten. Motoren er substratet; alt annet stables oppå, og flere deler kommer etter hvert som suiten vokser.
crates-card-cta = Hva den gjør →
crates-back = Alle crates
crates-not-found = Ingen crate med det navnet

# Per-crate cards
crate-rns-role = Motoren
crate-rns-blurb = Legg Reticulum inn i et hvilket som helst Rust-prosjekt. Deterministisk, no_std, allokatorfri; ingen global state, ingen innebygd I/O — ta med din egen klokke og ledning.
crate-rnsd-role = Daemonen
crate-rnsd-blurb = En drop-in for rnsd som kjører hvor som helst Linux kjører. Samme protokoll som RNS-referansen; bruk den ved siden av eller i stedet for nodene du allerede har.
crate-lxmf-role = Meldinger
crate-lxmf-blurb = LXMF oppå Reticulum — laget som Sideband og Nomadnet hviler på. Identiteter, adresser, melding-levering.
crate-ffi-role = Mobil- + Python-bindinger
crate-ffi-blurb = Ett uniffi-grensesnitt genererer Kotlin (.aar), Swift (.xcframework) og Python (.whl). Bruk Reticulum fra Android, iOS eller en Jupyter-notebook — samme form, samme motor.
crate-rvt-role = Visuell debugger
crate-rvt-blurb = Se pakker bevege seg mellom simulerte noder på en virtuell klokke. Deterministisk — samme scenario, samme spor, hver gang.
crate-esp32c6-role = ESP32-C6-firmware
crate-esp32c6-blurb = Bare-metal-host-adapter for ESP32-C6. Ingen OS, ingen allokator — bevis på at motoren kjører på en RISC-V-brikke til fem dollar med innebygde radioer.

# 404
not-found-title = Her er det ikke noe ennå.
not-found-cta = Tilbake til forsiden
