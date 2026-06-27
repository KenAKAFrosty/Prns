# Navigation
nav-contributing = Contribuer
nav-crates = Crates
nav-api = API

# Pied de page
footer-tagline = Proposé par l'équipe Personal.

# Accueil
landing-kicker = Des réseaux mesh inarrêtables pour tous
landing-kicker-prefix = Des réseaux mesh inarrêtables pour
landing-title = Un port de Reticulum (RNS) prêt pour la production, écrit en Rust sûr.
landing-title-lead = A production-grade port of Reticulum (RNS).
landing-title-accent = Runs on any device.
landing-subtitle = Un cœur déterministe, no_std et sans allocateur. Conçu pour les performances et la stabilité dont chaque nœud Reticulum a besoin, du microcontrôleur à cinq dollars au serveur cloud.
landing-cta-ethos = Choisir une crate
landing-cta-contributing = Contribuer

# Citation
landing-quote-label = Ce vers quoi nous construisons
landing-quote-body = Reticulum est l'infrastructure de communication fondatrice d'un avenir lumineux que nous pouvons avoir, tant que nous le construisons tous ensemble. C'est l'effort de l'équipe Personal pour mettre RNS entre les mains de plus de builders et aider cet avenir à prendre forme.

# Interfaces
interfaces-section-label = Interfaces
interfaces-section-title = Là où le mesh rencontre le monde
interfaces-section-lead = Prns conserve les interfaces compatibles RNS que les builders connaissent déjà, puis élargit la carte avec des liens natifs pour de nouveaux appareils et réseaux.

interfaces-radio-label = Radios
interfaces-radio-headline = Liens de proximité pour appareils et cartes
interfaces-radio-body = BLE Auto-interface, ESP-NOW et LoRa font entrer les appareils proches, les flottes de cartes et les liens longue portée dans un même mesh RNS.

interfaces-lan-label = LAN
interfaces-lan-headline = Pairs de lien local découverts automatiquement
interfaces-lan-body = Wi-Fi Auto-interface utilise le multicast, mDNS et le rendez-vous passerelle pour trouver les nœuds proches et intégrer un réseau local au mesh.

interfaces-cable-label = Fils + radio paquet
interfaces-cable-headline = Câbles, TNC et modems radio
interfaces-cable-body = USB Auto-interface, le framing série, KISS, AX.25 et RNode relient les petits appareils et le matériel radio paquet au même mesh.

interfaces-host-label = IP routée
interfaces-host-headline = Internet, WAN et liens backbone
interfaces-host-body = TCP client/serveur, UDP et Backbone permettent aux pairs distants de participer au mesh via des WAN privés, des VPN et des relais Internet publics.

# Ce sur quoi vous pouvez compter
standards-section-label = Nos standards
standards-section-title = Ce sur quoi vous pouvez compter
standards-license-label = Licence
standards-license-headline = MIT / Apache 2.0
standards-license-body = Double licence permissive. Pas de copyleft ni de restrictions commerciales.
standards-safety-label = Sécurité
standards-safety-headline = #![forbid(unsafe_code)]
standards-safety-body = Le moteur personal-rns ne contient aucun unsafe, garanti par le compilateur. Le unsafe des dépendances est vérifié contre l'UB sous Miri et audité avec cargo-geiger.
standards-correctness-label = Correction
standards-correctness-headline = Diff-testé contre RNS
standards-correctness-body = Chaque changement est vérifié contre la référence, puis passe par des tests de propriétés, de fuzzing et de mutation, avec des preuves Kani là où elles comptent.
standards-benchmarked-label = Performance
standards-benchmarked-headline = Mesurée, pas seulement affirmée
standards-benchmarked-body = Les performances sont suivies au grand jour, mesurées par un harness que vous pouvez exécuter vous-même.
standards-benchmarked-cta = Voir les benchmarks →

# Par où commencer ?
start-section-label = Chemins d'entrée
start-section-title = Par où commencer ?
start-section-lead = Choisissez le chemin qui correspond à ce que vous construisez. Chacun mène aujourd'hui à une seule crate ; d'autres guides arriveront à leurs côtés.

start-daemon-headline = Je veux lancer un nœud Reticulum
start-daemon-body = Daemon précompilé. Drop-in pour rnsd. Faites-le tourner à côté des nœuds que vous avez déjà.
start-daemon-code = apt install personal-rnsd
start-daemon-target = personal-rnsd

start-mobile-headline = Je construis une app mobile
start-mobile-body = Kotlin (.aar), Swift (.xcframework) ou Python (.whl) — le même moteur que votre daemon, intégré directement dans votre app.
start-mobile-code = implementation("org.staypersonal:rns:0.1")
    pod 'PersonalRns', '~> 0.1'
start-mobile-target = personal-rns-ffi

start-game-headline = Je livre dans un jeu
start-game-body = Bindings C# / .NET pour Unity, Godot et MonoGame. Du multijoueur sans monter de serveur.
start-game-code = dotnet add package Personal.Rns
start-game-target = personal-rns-ffi

start-embedded-headline = Flash a Hopspot
start-embedded-body = Pick a supported board, compare radio and battery tradeoffs, then flash a dedicated mesh device.
start-embedded-code = Board matrix
    Web flasher
    Local flash
start-embedded-target = Flash a Hopspot

start-web-headline = Je construis pour le web ou l'edge
start-web-body = Un build WebAssembly qui tourne dans le navigateur et sur des runtimes edge comme Cloudflare Workers, Fastly et Spin.
start-web-code = npm install personal-rns
start-web-target = personal-rns (wasm32)

start-rust-headline = Je l'intègre dans une app Rust
start-rust-body = Une runtime RNS complète prête à l'emploi, ou le cœur pur pour construire votre propre runtime autour.
start-rust-code = cargo add personal-rnsd   # complete RNS runtime
    cargo add personal-rns      # pure core only
start-rust-target = personal-rnsd or personal-rns

start-lxmf-headline = Je veux envoyer des messages sur le mesh
start-lxmf-body = LXMF au-dessus de Reticulum — identités, adresses, livraison. La couche sur laquelle reposent Sideband et Nomadnet.
start-lxmf-code = cargo add personal-lxmf
start-lxmf-target = personal-lxmf

# Plateformes ("Runs on") — libellé du marquee hero + CTA et page dédiée
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

# Page benchmarks
benchmarks-kicker = Performance
benchmarks-title = Benchmarké au grand jour
benchmarks-lead = Nous traitons la performance comme un nombre, pas comme un adjectif. Chaque chiffre ici vient d'un harness déterministe dans le dépôt, mesuré sur du vrai matériel et vérifié contre la référence RNS lorsque la comparaison est juste. Les chiffres arrivent à mesure que la suite se stabilise ; ci-dessous, la méthodologie qu'ils doivent respecter.

# Signal licence (pied de page)
footer-license = Open source. MIT / Apache 2.0.
footer-trademarks = Les logos et marques de tiers appartiennent à leurs propriétaires respectifs. Ils sont affichés uniquement pour identifier des plateformes, du matériel et des cibles de compatibilité ; aucune approbation n'est implicite.

# Page contribution
contributing-kicker = Le niveau d'exigence
contributing-title = Contribuer
contributing-lead = Comment contribuer — ce que nous valorisons, les conventions que votre code suit, et le standard que chaque changement franchit. Pour les contributeurs humains comme automatisés.

# Index des crates
crates-kicker = Les pièces
crates-title = Choisissez ce qui correspond à ce que vous construisez.
crates-lead = Chaque crate est conçue pour être utile seule, même sans tirer le reste. Le moteur est le substrat ; tout le reste s'empile dessus, et d'autres pièces arrivent à mesure que la suite grandit.
crates-card-cta = Ce qu'elle fait →
crates-back = Toutes les crates
crates-not-found = Aucune crate avec ce nom

# Cartes par crate
crate-rns-role = Le moteur
crate-rns-blurb = Intégrez Reticulum dans n'importe quel projet Rust. Déterministe, no_std, sans allocateur ; pas d'état global, pas d'E/S intégrée — apportez votre horloge et votre fil.
crate-rnsd-role = Le daemon
crate-rnsd-blurb = Un drop-in pour rnsd qui tourne partout où Linux tourne. Même fil que la référence RNS ; utilisez-le à côté ou à la place des nœuds que vous avez déjà.
crate-lxmf-role = Messagerie
crate-lxmf-blurb = LXMF au-dessus de Reticulum — la couche sur laquelle reposent Sideband et Nomadnet. Identités, adresses, livraison des messages.
crate-ffi-role = Bindings mobile + Python
crate-ffi-blurb = Une seule interface uniffi génère Kotlin (.aar), Swift (.xcframework) et Python (.whl). Utilisez Reticulum depuis Android, iOS ou un notebook Jupyter — même forme, même moteur.

# 404
not-found-title = Il n'y a encore rien ici.
not-found-cta = Retour à l'accueil
