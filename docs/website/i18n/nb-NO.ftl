# Navigasjon
nav-ethos = Design
nav-crates = Crates
nav-api = API

# Bunntekst
footer-tagline = Laget av Personal-teamet.

# Landing
landing-kicker = Ustoppelige mesh-nettverk — for folket
landing-title = En produksjonsklar port av Reticulum (RNS) skrevet i Rust.
landing-subtitle = En deterministisk kjerne uten std og uten allokator. Full dekning av RNS og LXMF. Native bindinger for Kotlin, Swift, Python, TypeScript og C#. WebAssembly for nettlesere og edge-runtimer som Cloudflare Workers, Fastly og Spin. Bygd med ytelsen og batteritiden i tankene som enhver Reticulum-stakk trenger — fra en mikrokontroller til fem dollar til en skynode. En drop-in-erstatning for rnsd følger med i pakken.
landing-cta-ethos = Velg en crate
landing-cta-crates = Slik bygger vi det

# Pull quote
landing-quote-label = Det vi bygger mot
landing-quote-body = Reticulum er den grunnleggende kommunikasjonsinfrastrukturen i den lyse framtiden vi kan ha — hvis vi velger å skape den. Dette er vår innsats for å legge den i hendene på flere utviklere og hjelpe den framtiden på vei.

# What you can count on
standards-section-label = Våre standarder
standards-section-title = Det du kan stole på
standards-license-label = Lisens
standards-license-headline = MIT / Apache 2.0
standards-license-body = Dobbeltlisensiert og tillatende. Ingen copyleft og ingen ikke-kommersielle vilkår.
standards-coverage-label = Dekning
standards-coverage-headline = Fullt RNS og LXMF
standards-coverage-body = Ikke bare RNS. Og LXMF er ingen bifigur. Begge, hele veien.
standards-core-label = Kjerne
standards-core-headline = no_std, ingen allokator
standards-core-body = En deterministisk kjerne som kjører der allokatorer ikke kan.
standards-verification-label = Verifisering
standards-verification-headline = Diff-testet mot RNS
standards-verification-body = Hver endring sjekkes mot referansen, og der det virkelig betyr noe, følger formelle bevis med.

# Where do I start?
start-section-label = Veier inn
start-section-title = Hvor begynner jeg?
start-section-lead = Velg veien som passer det du bygger. Hver enkelt lander på én crate i dag, og dedikerte guider følger i samme tempo.

start-daemon-headline = Jeg vil ha en Reticulum-node i gang
start-daemon-body = Ferdigbygd daemon. Drop-in for rnsd. Sett den ved siden av nodene du allerede har, og la dem kjøre sammen.
start-daemon-code = apt install personal-rnsd
start-daemon-target = personal-rnsd

start-mobile-headline = Jeg bygger en mobil-app
start-mobile-body = Kotlin (.aar), Swift (.xcframework) eller Python (.whl) — samme motor som daemonen kjører, lagt rett inn i appen din.
start-mobile-code = implementation("org.staypersonal:rns:0.1")
    pod 'PersonalRns', '~> 0.1'
start-mobile-target = personal-rns-ffi

start-game-headline = Jeg leverer det i et spill
start-game-body = C#/.NET-bindinger for Unity, Godot og MonoGame. Multiplayer uten å sette opp en server.
start-game-code = dotnet add package Personal.Rns
start-game-target = personal-rns-ffi

start-embedded-headline = Jeg sikter mot mikrokontrollere
start-embedded-body = Motoren pluss en Host-trait med bare tre metoder. ESP32-C6 er referansen; S3, nRF, RP2040 og STM32 står for tur.
start-embedded-code = cargo add personal-rns --no-default-features
start-embedded-target = personal-rns + hosts/*

start-web-headline = Jeg bygger for web eller edge
start-web-body = En WebAssembly-build som kjører i nettleseren og på edge-runtimer som Cloudflare Workers, Fastly og Spin.
start-web-code = npm install personal-rns
start-web-target = personal-rns (wasm32)

start-rust-headline = Jeg legger det inn i en Rust-app
start-rust-body = En komplett RNS-runtime rett ut av boksen, eller den rene kjernen til å bygge din egen runtime rundt. Velg det som passer deg.
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
ethos-lead = En ingeniør-til-ingeniør-merknad om disiplinen bak prosjektet — ren motor, kjerne uten allokator, hver endring verifisert mot RNS-referansen. Bla gjennom før du gjør deg avhengig; vi vil at du skal vite hva du går inn i.

# Crates index
crates-kicker = Delene
crates-title = Velg den som passer det du bygger.
crates-lead = Hver crate er bygd til å være nyttig alene, selv om du ikke trekker inn resten. Motoren er substratet; alt annet stables oppå, og flere deler kommer etter hvert som suiten vokser.
crates-card-cta = Hva den gjør →
crates-back = Alle crates
crates-not-found = Ingen crate med det navnet

# Per-crate cards
crate-rns-role = Motoren
crate-rns-blurb = Legg Reticulum inn i et hvilket som helst Rust-prosjekt. Deterministisk, no_std, uten allokator; ingen global state, ingen innebygd I/O — du tar med deg klokke og ledning.
crate-rnsd-role = Daemonen
crate-rnsd-blurb = En drop-in for rnsd som kjører hvor som helst Linux kjører. Samme protokoll som RNS-referansen; bruk den ved siden av eller i stedet for nodene du allerede har.
crate-lxmf-role = Meldinger
crate-lxmf-blurb = LXMF oppå Reticulum — laget som Sideband og Nomadnet hviler på. Identiteter, adresser, melding-levering.
crate-ffi-role = Mobil- + Python-bindinger
crate-ffi-blurb = Ett uniffi-grensesnitt genererer Kotlin (.aar), Swift (.xcframework) og Python (.whl). Bruk Reticulum fra Android, iOS eller en Jupyter-notebook — samme form, samme motor.
crate-rvt-role = Visuell debugger
crate-rvt-blurb = Følg pakker bevege seg mellom simulerte noder på en virtuell klokke. Deterministisk — samme scenario, samme spor, hver gang.
crate-esp32c6-role = ESP32-C6-firmware
crate-esp32c6-blurb = Bare-metal-host-adapter for ESP32-C6. Ingen OS, ingen allokator — bevis på at motoren kjører på en RISC-V-brikke til fem dollar med innebygde radioer.

# 404
not-found-title = Her er det ingenting ennå.
not-found-cta = Tilbake til forsiden
