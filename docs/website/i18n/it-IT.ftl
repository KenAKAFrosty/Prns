# Navigazione
nav-contributing = Contribuire
nav-crates = Crate
nav-api = API

# Footer
footer-tagline = Offerto dal team Personal.

# Landing
landing-kicker = Reti mesh inarrestabili per le persone
landing-kicker-prefix = Reti mesh inarrestabili per le
landing-title = Un port di Reticulum (RNS) pronto per la produzione, scritto in Rust sicuro.
landing-subtitle = Un core deterministico, no_std e senza allocatore. Costruito per le prestazioni e la stabilità di cui ogni nodo Reticulum ha bisogno, da un microcontrollore da cinque dollari a un server cloud.
landing-cta-ethos = Scegli una crate
landing-cta-contributing = Contribuire

# Citazione
landing-quote-label = Ciò verso cui stiamo costruendo
landing-quote-body = Reticulum è l'infrastruttura di comunicazione fondamentale di un futuro luminoso che possiamo avere, finché lo costruiamo tutti insieme. Questo è lo sforzo del team Personal per mettere RNS nelle mani di più builder e aiutare quel futuro a diventare reale.

# Interfacce
interfaces-section-label = Interfacce
interfaces-section-title = Dove la mesh incontra il mondo
interfaces-section-lead = Prns mantiene le interfacce compatibili con RNS che i builder conoscono già e amplia la mappa con link nativi per nuovi dispositivi e reti.

interfaces-radio-label = Radio
interfaces-radio-headline = Link di prossimità per dispositivi e schede
interfaces-radio-body = BLE Auto-interface, ESP-NOW e LoRa portano dispositivi vicini, flotte di schede e link a lungo raggio dentro uno stesso mesh RNS.

interfaces-lan-label = LAN
interfaces-lan-headline = Peer di link locale scoperti automaticamente
interfaces-lan-body = Wi-Fi Auto-interface usa multicast, mDNS e rendezvous gateway per trovare nodi vicini e integrare una rete locale nella mesh.

interfaces-cable-label = Cavi + packet radio
interfaces-cable-headline = Cavi, TNC e modem radio
interfaces-cable-body = USB Auto-interface, framing seriale, KISS, AX.25 e RNode collegano piccoli dispositivi e hardware packet radio alla stessa mesh.

interfaces-host-label = IP instradato
interfaces-host-headline = Internet, WAN e link backbone
interfaces-host-body = TCP client/server, UDP e Backbone permettono ai peer distanti di partecipare al mesh tramite WAN private, VPN e relay su Internet pubblico.

# Su cosa puoi contare
standards-section-label = I nostri standard
standards-section-title = Su cosa puoi contare
standards-license-label = Licenza
standards-license-headline = MIT / Apache 2.0
standards-license-body = Doppia licenza permissiva. Nessun copyleft o restrizione commerciale.
standards-safety-label = Sicurezza
standards-safety-headline = #![forbid(unsafe_code)]
standards-safety-body = Il motore personal-rns contiene zero unsafe, imposto dal compilatore. L'unsafe nelle dipendenze viene controllato per UB con Miri e auditato con cargo-geiger.
standards-correctness-label = Correttezza
standards-correctness-headline = Diff-testato contro RNS
standards-correctness-body = Ogni modifica viene controllata contro la reference, poi passa attraverso test di proprietà, fuzz e mutazione, con prove Kani dove contano.
standards-benchmarked-label = Prestazioni
standards-benchmarked-headline = Misurate, non solo dichiarate
standards-benchmarked-body = Le prestazioni sono tracciate apertamente, misurate da un harness che puoi eseguire tu stesso.
standards-benchmarked-cta = Guarda i benchmark →

# Da dove comincio?
start-section-label = Vie d'ingresso
start-section-title = Da dove comincio?
start-section-lead = Scegli il percorso che corrisponde a ciò che stai costruendo. Oggi ognuno arriva su una singola crate; altre guide arriveranno accanto a loro.

start-daemon-headline = Voglio un nodo Reticulum in esecuzione
start-daemon-body = Daemon precompilato. Drop-in per rnsd. Eseguilo accanto ai nodi che hai già.
start-daemon-code = apt install personal-rnsd
start-daemon-target = personal-rnsd

start-mobile-headline = Sto costruendo un'app mobile
start-mobile-body = Kotlin (.aar), Swift (.xcframework) o Python (.whl) — lo stesso motore del tuo daemon, incorporato direttamente nella tua app.
start-mobile-code = implementation("org.staypersonal:rns:0.1")
    pod 'PersonalRns', '~> 0.1'
start-mobile-target = personal-rns-ffi

start-game-headline = Lo sto distribuendo in un gioco
start-game-body = Binding C# / .NET per Unity, Godot e MonoGame. Multiplayer senza mettere in piedi un server.
start-game-code = dotnet add package Personal.Rns
start-game-target = personal-rns-ffi

start-embedded-headline = Punto ai microcontrollori
start-embedded-body = Il motore più un trait Host di tre metodi. ESP32-C6 è la reference; S3, nRF, RP2040 e STM32 sono i prossimi.
start-embedded-code = cargo add personal-rns --no-default-features
start-embedded-target = personal-rns + personal-hopspot

start-web-headline = Costruisco per web o edge
start-web-body = Una build WebAssembly che gira nel browser e su runtime edge come Cloudflare Workers, Fastly e Spin.
start-web-code = npm install personal-rns
start-web-target = personal-rns (wasm32)

start-rust-headline = Lo incorporo in un'app Rust
start-rust-body = Un runtime RNS completo pronto all'uso, oppure il core puro per costruirci intorno il tuo runtime.
start-rust-code = cargo add personal-rnsd   # complete RNS runtime
    cargo add personal-rns      # pure core only
start-rust-target = personal-rnsd or personal-rns

start-lxmf-headline = Voglio inviare messaggi sulla mesh
start-lxmf-body = LXMF sopra Reticulum — identità, indirizzi, consegna. Il livello su cui poggiano Sideband e Nomadnet.
start-lxmf-code = cargo add personal-lxmf
start-lxmf-target = personal-lxmf

# Piattaforme ("Runs on") — etichetta marquee dell'hero + CTA e pagina dedicata
landing-platforms-label = Gira su
landing-platforms-cta = Vedi tutto →
platforms-title = Dove gira Prns
platforms-lead = Un motore, molte case. Alcune sono disponibili oggi; il resto è nella roadmap — la stella polare verso cui costruiamo. I chip pieni girano ora; quelli tratteggiati sono i prossimi.
platforms-legend-shipping = Disponibile oggi
platforms-legend-roadmap = Roadmap

# Pagina benchmark
benchmarks-kicker = Prestazioni
benchmarks-title = Benchmark in pubblico
benchmarks-lead = Trattiamo le prestazioni come un numero, non come un aggettivo. Ogni cifra qui viene da un harness deterministico nel repo, misurata su hardware reale e controllata contro la reference RNS quando il confronto è corretto. I numeri arrivano mentre la suite si stabilizza; sotto c'è la metodologia a cui devono reggere.

# Segnale licenza (footer)
footer-license = Open source. MIT / Apache 2.0.
footer-trademarks = Loghi e marchi di terze parti appartengono ai rispettivi proprietari. Sono mostrati solo per identificare piattaforme, hardware e obiettivi di compatibilità; non implicano alcuna approvazione.

# Pagina contributi
contributing-kicker = L'asticella
contributing-title = Contribuire
contributing-lead = Come contribuire — ciò che apprezziamo, le convenzioni che il tuo codice segue e lo standard che ogni modifica supera. Per contributor umani e automatizzati allo stesso modo.

# Indice crate
crates-kicker = I pezzi
crates-title = Scegli ciò che corrisponde a quello che stai costruendo.
crates-lead = Ogni crate è costruita per essere utile da sola, anche se non porti con te il resto. Il motore è il substrato; tutto il resto si impila sopra, e altri pezzi arrivano mentre la suite cresce.
crates-card-cta = Cosa fa →
crates-back = Tutte le crate
crates-not-found = Nessuna crate con quel nome

# Schede per crate
crate-rns-role = Il motore
crate-rns-blurb = Porta Reticulum in qualsiasi progetto Rust. Deterministico, no_std, senza allocatore; niente stato globale, niente I/O integrato — porta il tuo clock e il tuo filo.
crate-rnsd-role = Il daemon
crate-rnsd-blurb = Un drop-in per rnsd che gira ovunque giri Linux. Stesso wire della reference RNS; usalo accanto o al posto dei nodi che hai già.
crate-lxmf-role = Messaggistica
crate-lxmf-blurb = LXMF sopra Reticulum — il livello su cui poggiano Sideband e Nomadnet. Identità, indirizzi, consegna dei messaggi.
crate-ffi-role = Binding mobile + Python
crate-ffi-blurb = Una sola interfaccia uniffi genera Kotlin (.aar), Swift (.xcframework) e Python (.whl). Usa Reticulum da Android, iOS o un notebook Jupyter — stessa forma, stesso motore.

# 404
not-found-title = Qui non c'è ancora niente.
not-found-cta = Torna alla home
