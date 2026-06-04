# Navigazione
nav-ethos = Design
nav-crates = Crate
nav-api = API

# Footer
footer-tagline = Costruito dal team Personal.

# Landing
landing-kicker = Reti mesh inarrestabili — per le persone
landing-title = Un porting di Reticulum (RNS) pronto per la produzione, scritto in Rust.
landing-subtitle = Un core deterministico, senza std e senza allocatore. Copertura completa di RNS e LXMF. Pensato per le prestazioni e l'autonomia di cui ogni stack Reticulum ha bisogno — da un microcontrollore da cinque dollari fino a un nodo cloud.
landing-cta-ethos = Scegli un crate
landing-cta-crates = Come lo costruiamo

# Pull quote
landing-quote-label = Verso cosa stiamo costruendo
landing-quote-body = Reticulum è l'infrastruttura di comunicazione fondante del futuro luminoso che possiamo avere — se lo costruiamo. Questo è il nostro impegno per metterlo nelle mani di più sviluppatori e contribuire a realizzare quel futuro.

# Su cosa puoi contare
standards-section-label = I nostri standard
standards-section-title = Su cosa puoi contare
standards-license-label = Licenza
standards-license-headline = MIT / Apache 2.0
standards-license-body = Doppia licenza, permissiva. Niente copyleft e nessuna clausola non commerciale.
standards-coverage-label = Copertura
standards-coverage-headline = RNS e LXMF, completi
standards-coverage-body = Non solo RNS. E LXMF non come comparsa. Entrambi, per intero.
standards-core-label = Core
standards-core-headline = no_std, senza allocatore
standards-core-body = Un core deterministico che gira dove gli allocatori non possono.
standards-verification-label = Verifica
standards-verification-headline = Diff-test contro RNS
standards-verification-body = Ogni modifica viene confrontata con la reference, e dove conta davvero arrivano prove formali.

# Da dove comincio?
start-section-label = Vie d'ingresso
start-section-title = Da dove comincio?
start-section-lead = Scegli il percorso che corrisponde a ciò che stai costruendo. Oggi ciascuno punta a un singolo crate; le guide dedicate seguono allo stesso passo.

start-daemon-headline = Voglio un nodo Reticulum in esecuzione
start-daemon-body = Daemon già pronto. Drop-in per rnsd. Mettilo accanto ai nodi che hai già e falli girare insieme.
start-daemon-code = apt install personal-rnsd
start-daemon-target = personal-rnsd

start-mobile-headline = Sto costruendo un'app mobile
start-mobile-body = Kotlin (.aar), Swift (.xcframework) o Python (.whl) — lo stesso motore che gira nel tuo daemon, integrato dentro la tua app.
start-mobile-code = implementation("org.staypersonal:rns:0.1")
    pod 'PersonalRns', '~> 0.1'
start-mobile-target = personal-rns-ffi

start-game-headline = Lo sto integrando in un gioco
start-game-body = Binding C#/.NET per Unity, Godot e MonoGame. Multiplayer senza dover tirare su un server.
start-game-code = dotnet add package Personal.Rns
start-game-target = personal-rns-ffi

start-embedded-headline = Sto puntando a microcontrollori
start-embedded-body = Il motore più un trait Host di sole tre metodi. L'ESP32-C6 è il riferimento; subito dopo arrivano S3, nRF, RP2040 e STM32.
start-embedded-code = cargo add personal-rns --no-default-features
start-embedded-target = personal-rns + hosts/*

start-web-headline = Sto costruendo per il web o l'edge
start-web-body = Una build WebAssembly che gira nel browser e su runtime edge come Cloudflare Workers, Fastly e Spin.
start-web-code = npm install personal-rns
start-web-target = personal-rns (wasm32)

start-rust-headline = Lo sto incorporando in un'app Rust
start-rust-body = Un runtime RNS completo già pronto, oppure il core puro per costruirci il tuo runtime sopra. Scegli ciò che ti torna meglio.
start-rust-code = cargo add personal-rnsd   # complete RNS runtime
    cargo add personal-rns      # pure core only
start-rust-target = personal-rnsd or personal-rns

start-lxmf-headline = Voglio inviare messaggi sulla mesh
start-lxmf-body = LXMF sopra Reticulum — identità, indirizzi, recapito. Lo strato su cui poggiano Sideband e Nomadnet.
start-lxmf-code = cargo add personal-lxmf
start-lxmf-target = personal-lxmf

# Footer (licenza)
footer-license = Open source. MIT / Apache 2.0.

# Pagina ethos
ethos-kicker = La disciplina
ethos-title = Come lo costruiamo
ethos-lead = Una nota da ingegnere a ingegnere sulla disciplina dietro questo progetto — motore puro, core senza allocatore, ogni modifica verificata contro la reference RNS. Leggila prima di farne una dipendenza; vogliamo che tu sappia in che cosa ti stai impegnando.

# Indice dei crate
crates-kicker = I pezzi
crates-title = Scegli quello che combacia con quello che stai costruendo.
crates-lead = Ogni crate è pensato per essere utile da solo, anche se non porti dietro tutto il resto. Il motore è il substrato; il resto si impila sopra, e altri pezzi arrivano man mano che la suite cresce.
crates-card-cta = Cosa fa →
crates-back = Tutti i crate
crates-not-found = Nessun crate con questo nome

# Card dei singoli crate
crate-rns-role = Il motore
crate-rns-blurb = Infila Reticulum dentro qualsiasi progetto Rust. Deterministico, no_std, senza allocatore; niente stato globale, niente I/O integrato — porta tu il clock e il canale.
crate-rnsd-role = Il daemon
crate-rnsd-blurb = Un drop-in per rnsd che gira ovunque giri Linux. Stesso protocollo della reference RNS; usalo accanto o al posto dei nodi che hai già.
crate-lxmf-role = Messaggistica
crate-lxmf-blurb = LXMF sopra Reticulum — lo strato su cui poggiano Sideband e Nomadnet. Identità, indirizzi, recapito dei messaggi.
crate-ffi-role = Binding Mobile + Python
crate-ffi-blurb = Una sola interfaccia uniffi genera Kotlin (.aar), Swift (.xcframework) e Python (.whl). Usa Reticulum da Android, iOS o un notebook Jupyter — stessa forma, stesso motore.
crate-esp32c6-role = Firmware ESP32-C6
crate-esp32c6-blurb = Adattatore host bare-metal per l'ESP32-C6. Niente OS, niente allocatore — la prova che il motore gira su un chip RISC-V da cinque dollari con radio integrate.

# 404
not-found-title = Qui ancora non c'è niente.
not-found-cta = Torna alla home
