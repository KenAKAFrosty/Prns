# Navigazione
nav-benchmarks = Benchmark
nav-contributing = Contribuire
nav-api = Riferimento API

# Footer
footer-tagline = Offerto da KenAKAFrosty e dal team Personal/Prns.
footer-flash = Flasha un Hopspot (solo in inglese)
footer-playground = Playground del browser (solo in inglese)

# Landing
landing-kicker = Reti mesh che sono tue
landing-kicker-prefix = Reti mesh che sono
landing-title = Reticulum (RNS) ad alte prestazioni, costruito per girare su qualsiasi dispositivo.
landing-title-lead = Reticulum (RNS) ad alte prestazioni,
landing-title-accent = costruito per girare su qualsiasi dispositivo.
landing-subtitle = Costruito per le prestazioni, la stabilità e l'efficienza energetica di cui ogni nodo Reticulum ha bisogno, da un microcontrollore da cinque dollari a un cluster di server cloud. Un solo motore e una sola API, identici su embedded, desktop, mobile, giochi e web.
landing-cta-ethos = Trova la tua strada in Prns
landing-cta-standards = I nostri standard
# Citazione
landing-quote-label = Ciò verso cui stiamo costruendo
landing-quote-body = Reticulum è l'infrastruttura di comunicazione fondamentale di un futuro luminoso che possiamo avere, finché lo costruiamo tutti insieme. Questo è lo sforzo del team Personal per mettere RNS nelle mani di più builder e aiutare quel futuro a diventare reale.

# Interfacce
interfaces-section-label = Interfacce
interfaces-section-title = Dove la mesh incontra il mondo
interfaces-section-lead = Prns mantiene le interfacce compatibili con RNS che i builder conoscono già e amplia la mappa con link nativi per nuovi dispositivi e reti.
interfaces-section-hot-note = Le interfacce Prns sono hot-swappable: aggiungi, rimuovi o cambia un'interfaccia senza riavviare il nodo.

interfaces-radio-label = Radio
interfaces-radio-headline = Link di prossimità per dispositivi e schede
interfaces-radio-body = Bluetooth LE Auto-interface, ESP-NOW e LoRa portano dispositivi vicini, flotte di schede e link a lungo raggio dentro uno stesso mesh RNS.

interfaces-lan-label = LAN
interfaces-lan-headline = Peer di link locale scoperti automaticamente
interfaces-lan-body = Wi-Fi Auto-interface usa multicast, mDNS e rendezvous gateway per trovare nodi vicini e integrare una rete locale nella mesh.

interfaces-cable-label = Cavi + packet radio
interfaces-cable-headline = Cavi, TNC e modem radio
interfaces-cable-body = USB Auto-interface, framing seriale, KISS, AX.25 e RNode collegano piccoli dispositivi e hardware packet radio alla stessa mesh.

interfaces-host-label = IP instradato
interfaces-host-headline = Internet, WAN e link backbone
interfaces-host-body = TCP client/server, UDP, WebSocket e Backbone permettono ai peer distanti di partecipare al mesh tramite WAN private, VPN, relay su Internet pubblico e integrazioni nel browser.

# Su cosa puoi contare
standards-section-label = I nostri standard
standards-section-title = Su cosa puoi contare
standards-license-label = Licenza
standards-license-headline = MIT / Apache 2.0
standards-license-body = Doppia licenza permissiva. Nessun copyleft o restrizione commerciale.
standards-safety-label = Sicurezza
standards-safety-headline = Imposto, poi auditato
standards-safety-body = Nel motore, i panic, gli unwrap e l'unsafe ingiustificato non compilano mai. Ciò che non si può vietare viene auditato: l'unsafe nelle dipendenze con cargo-geiger, il comportamento indefinito sotto Miri, gli avvisi di sicurezza con cargo-deny.
standards-correctness-label = Correttezza
standards-correctness-headline = Diff-testato contro RNS
standards-correctness-body = Ogni modifica viene controllata contro la reference, poi passa attraverso test unitari, di proprietà, fuzz e mutazione, con prove Kani dove contano.
standards-benchmarked-label = Prestazioni
standards-benchmarked-headline = Misurate, non solo dichiarate
standards-benchmarked-body = Le prestazioni sono tracciate apertamente, misurate da un harness che puoi eseguire tu stesso.
standards-benchmarked-cta = Guarda i benchmark →

# Da dove comincio?
start-section-label = Vie d'ingresso
start-section-title = Cosa sei qui per fare?
start-section-lead = Scegli il percorso che corrisponde a come Prns entra nel tuo lavoro: hardware da flashare, infrastruttura da far girare o software da costruire.

start-daemon-headline = Esegui un daemon
start-daemon-body = Installa un daemon Reticulum veloce per desktop, app LXMF, VPS backbone e altro.
start-daemon-code = Drop-in per le app standard
    Legge ~/.reticulum
    Modifiche alle interfacce dal vivo
    Metriche integrate
start-daemon-target = Avvia Prnsd

start-embedded-headline = Flasha un Hopspot
start-embedded-body = Scegli una scheda supportata, confronta i compromessi tra radio e batteria, poi flasha un dispositivo mesh dedicato.
start-embedded-code = Matrice delle schede
    Flasher web
    Flash locale
start-embedded-target = Flasha un Hopspot (solo in inglese)

start-web-headline = Usa il playground del nodo nel browser
start-web-body = Prova l’API TypeScript con il motore Rust condiviso in WebAssembly, connettiti tramite Auto Wi-Fi o USB Auto e osserva in tempo reale l’attività del nodo locale.
start-web-code = Runtime WebAssembly
    Auto Wi-Fi + USB Auto
    Esempio TypeScript
start-web-target = Apri il playground (solo in inglese)

start-rust-headline = Costruisci su Reticulum
start-rust-body = Usa il motore e i binding per aggiungere reti mesh ad app, strumenti, servizi o giochi.
start-rust-target = Leggi il README
start-rust-target-source = Scarica il sorgente

# Piattaforme ("Runs on") — etichetta marquee dell'hero + CTA e pagina dedicata
landing-platforms-label = Gira su
landing-platforms-cta = Vedi tutto →
platforms-title = Dove gira Prns
platforms-lead = Un motore, tante case. Questa vista rapida separa il supporto delle piattaforme runtime dal supporto delle specifiche schede Hopspot.
platforms-board-support-link = Vedi supporto schede e bring-up →

# Pagina Flasha un Hopspot
flash-back = Piattaforme
flash-back-boards = Schede
flash-card-action = Flash

# Pagina benchmark
benchmarks-kicker = Prestazioni
benchmarks-title = Benchmark in pubblico
benchmarks-lead = Ogni numero qui sotto viene dai risultati pubblicati nel repo, misurati su hardware reale da un harness che puoi eseguire tu stesso. Da qui in poi i contenuti sono per ora disponibili solo in inglese.

# Segnale licenza (footer)
footer-license = Open source. MIT / Apache 2.0.
footer-trademarks = Loghi e marchi di terze parti appartengono ai rispettivi proprietari. Sono mostrati solo per identificare piattaforme, hardware e obiettivi di compatibilità. Nessuna approvazione è dichiarata o implicita.

# Pagina contributi
contributing-kicker = L'asticella
contributing-title = Contribuire
contributing-lead = Come contribuire — ciò che apprezziamo, le convenzioni che il tuo codice segue e lo standard che ogni modifica supera. Per contributor umani e automatizzati allo stesso modo.

# 404
not-found-title = Qui non c'è ancora niente.
not-found-cta = Torna alla home
