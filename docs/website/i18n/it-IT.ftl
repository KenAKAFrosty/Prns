# Navigazione
nav-ethos = Design
nav-crates = Crate
nav-api = API

# Footer
footer-tagline = Portato a te dal team Personal.

# Landing
landing-kicker = Reti mesh inarrestabili, per la gente
landing-title = Una porta production-grade di Reticulum (RNS) scritta in Rust.
landing-subtitle = Un core deterministico, no_std, senza allocatore. Copertura completa di RNS e LXMF. Binding nativi per Kotlin, Swift, Python, TypeScript e C#. WebAssembly per browser e runtime edge. Pensato per le prestazioni e l'autonomia che ogni stack Reticulum richiede, da un microcontrollore da cinque dollari a un nodo cloud. Include un sostituto drop-in per rnsd.
landing-cta-ethos = Scegli un crate
landing-cta-crates = Come lo costruiamo

# Pull quote
landing-quote-label = Verso cosa stiamo costruendo
landing-quote-body = Reticulum è l'infrastruttura di comunicazione fondante del futuro luminoso che possiamo avere, se lo costruiamo. Questo è il nostro impegno per metterlo nelle mani di più sviluppatori e contribuire a realizzare quel futuro.

# What you can count on
standards-section-label = I nostri standard
standards-section-title = Su cosa puoi contare
standards-license-label = Licenza
standards-license-headline = MIT / Apache 2.0
standards-license-body = Doppia licenza, permissiva. Niente copyleft, nessuna restrizione non-commerciale.
standards-coverage-label = Copertura
standards-coverage-headline = RNS e LXMF completi
standards-coverage-body = Non solo RNS. Non LXMF di contorno. Entrambi, per intero.
standards-core-label = Core
standards-core-headline = no_std, senza allocatore
standards-core-body = Un core deterministico che gira dove gli allocatori non possono.
standards-verification-label = Verifica
standards-verification-headline = Diff-test contro RNS
standards-verification-body = Ogni cambiamento controllato contro la reference; prove formali dove servono.

# Where do I start?
start-section-label = Vie d'ingresso
start-section-title = Da dove comincio?
start-section-lead = Scegli il percorso che corrisponde a ciò che stai costruendo. Oggi ciascuno punta a un singolo crate; presto arriveranno guide dedicate.

start-daemon-headline = Voglio un nodo Reticulum in esecuzione
start-daemon-body = Daemon pronto all'uso. Drop-in per rnsd. Eseguilo accanto ai nodi che hai già.
start-daemon-code = apt install personal-rnsd
start-daemon-target = personal-rnsd

start-mobile-headline = Sto costruendo un'app mobile
start-mobile-body = Kotlin (.aar), Swift (.xcframework) o Python (.whl) — lo stesso motore del tuo daemon, integrato direttamente nell'app.
start-mobile-code = implementation("org.staypersonal:rns:0.1")
    pod 'PersonalRns', '~> 0.1'
start-mobile-target = personal-rns-ffi

start-game-headline = Lo sto integrando in un gioco
start-game-body = Binding C# / .NET per Unity, Godot e MonoGame. Multiplayer senza tirare su un server.
start-game-code = dotnet add package Personal.Rns
start-game-target = personal-rns-ffi

start-embedded-headline = Sto puntando a microcontrollori
start-embedded-body = Il motore più un trait Host di tre metodi. ESP32-C6 è il riferimento; S3, nRF, RP2040 e STM32 sono i prossimi.
start-embedded-code = cargo add personal-rns --no-default-features
start-embedded-target = personal-rns + hosts/*

start-web-headline = Sto costruendo per il web o l'edge
start-web-body = Una build WebAssembly che gira nel browser e su runtime edge come Cloudflare Workers, Fastly e Spin.
start-web-code = npm install personal-rns
start-web-target = personal-rns (wasm32)

start-rust-headline = Lo sto incorporando in un'app Rust
start-rust-body = Un runtime RNS completo pronto all'uso, oppure il core puro per costruirci il tuo runtime sopra.
start-rust-code = cargo add personal-rnsd   # complete RNS runtime
    cargo add personal-rns      # pure core only
start-rust-target = personal-rnsd or personal-rns

start-lxmf-headline = Voglio inviare messaggi sulla mesh
start-lxmf-body = LXMF sopra Reticulum — identità, indirizzi, recapito. Lo strato su cui poggiano Sideband e Nomadnet.
start-lxmf-code = cargo add personal-lxmf
start-lxmf-target = personal-lxmf

# License signal (footer)
footer-license = Open source. MIT / Apache 2.0.

# Ethos page
ethos-kicker = La disciplina
ethos-title = Come lo costruiamo
ethos-lead = Una nota da ingegnere a ingegnere sulla disciplina dietro questo progetto — motore puro, core senza allocatore, ogni cambiamento verificato contro la reference RNS. Leggila prima di dipenderne; vogliamo che tu sappia in cosa ti stai impegnando.

# Crates index
crates-kicker = I pezzi
crates-title = Scegli ciò che combacia con quello che stai costruendo.
crates-lead = Ogni crate è pensato per essere utile da solo, anche senza il resto. Il motore è il substrato; tutto il resto si impila sopra, e altri pezzi arrivano man mano che la suite cresce.
crates-card-cta = Cosa fa →
crates-back = Tutti i crate
crates-not-found = Nessun crate con questo nome

# Per-crate cards
crate-rns-role = Il motore
crate-rns-blurb = Inserisci Reticulum in qualsiasi progetto Rust. Deterministico, no_std, senza allocatore; niente stato globale, niente I/O integrato — porta il tuo clock e il tuo canale.
crate-rnsd-role = Il daemon
crate-rnsd-blurb = Un drop-in per rnsd che gira ovunque giri Linux. Stesso protocollo della reference RNS; usalo accanto o al posto dei nodi che hai già.
crate-lxmf-role = Messaggistica
crate-lxmf-blurb = LXMF sopra Reticulum — lo strato su cui poggiano Sideband e Nomadnet. Identità, indirizzi, recapito dei messaggi.
crate-ffi-role = Binding Mobile + Python
crate-ffi-blurb = Un'unica interfaccia uniffi genera Kotlin (.aar), Swift (.xcframework) e Python (.whl). Usa Reticulum da Android, iOS o un notebook Jupyter — stessa forma, stesso motore.
crate-rvt-role = Debugger visivo
crate-rvt-blurb = Osserva i pacchetti muoversi tra nodi simulati su un clock virtuale. Deterministico — stesso scenario, stessa traccia, ogni volta.
crate-esp32c6-role = Firmware ESP32-C6
crate-esp32c6-blurb = Adattatore host bare-metal per l'ESP32-C6. Niente OS, niente allocatore — la prova che il motore gira su un chip RISC-V da cinque dollari con radio integrate.

# 404
not-found-title = Qui ancora non c'è niente.
not-found-cta = Torna alla home
