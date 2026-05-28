# Navigation
nav-ethos = Design
nav-crates = Crates
nav-api = API

# Pied de page
footer-tagline = Apporté par l'équipe Personal.

# Page d'accueil
landing-kicker = Réseaux maillés inarrêtables, pour les gens
landing-title = Un portage de Reticulum (RNS) écrit en Rust, prêt pour la production.
landing-subtitle = Un cœur déterministe, no_std, sans allocateur. Couvre RNS et LXMF intégralement. Bindings natifs pour Kotlin, Swift, Python, TypeScript et C#. WebAssembly pour les navigateurs et les runtimes edge. Conçu pour la performance et l'autonomie dont a besoin n'importe quelle pile Reticulum, d'un microcontrôleur à cinq dollars jusqu'à un nœud cloud. Inclut un remplacement drop-in pour rnsd.
landing-cta-ethos = Choisir un crate
landing-cta-crates = Comment on le construit

# Citation
landing-quote-label = Vers quoi on bâtit
landing-quote-body = Reticulum est l'infrastructure de communication fondatrice de l'avenir lumineux que nous pouvons avoir, si nous le construisons. Cet effort vise à le mettre entre les mains de plus de développeurs et à aider à concrétiser cet avenir.

# Ce sur quoi vous pouvez compter
standards-section-label = Nos standards
standards-section-title = Ce sur quoi vous pouvez compter
standards-license-label = Licence
standards-license-headline = MIT / Apache 2.0
standards-license-body = Double licence et permissive. Pas de copyleft, pas de restrictions non commerciales.
standards-coverage-label = Couverture
standards-coverage-headline = RNS et LXMF complets
standards-coverage-body = Pas seulement RNS. Pas LXMF en accessoire. Les deux, entièrement.
standards-core-label = Cœur
standards-core-headline = no_std, sans allocateur
standards-core-body = Un cœur déterministe qui tourne là où les allocateurs ne peuvent pas.
standards-verification-label = Vérification
standards-verification-headline = Diff-testé contre RNS
standards-verification-body = Chaque changement est vérifié contre la référence ; des preuves formelles là où ça compte.

# Par où je commence ?
start-section-label = Voies d'entrée
start-section-title = Par où je commence ?
start-section-lead = Choisis le chemin qui correspond à ce que tu construis. Chacun pointe vers un seul crate aujourd'hui ; davantage de guides arriveront avec eux.

start-daemon-headline = Je veux un nœud Reticulum en marche
start-daemon-body = Daemon prêt à l'emploi. Drop-in pour rnsd. Mets-le à côté des nœuds que tu as déjà.
start-daemon-code = apt install personal-rnsd
start-daemon-target = personal-rnsd

start-mobile-headline = Je construis une application mobile
start-mobile-body = Kotlin (.aar), Swift (.xcframework) ou Python (.whl) — le même moteur que ton daemon, embarqué directement dans ton app.
start-mobile-code = implementation("org.staypersonal:rns:0.1")
    pod 'PersonalRns', '~> 0.1'
start-mobile-target = personal-rns-ffi

start-game-headline = J'intègre dans un jeu
start-game-body = Bindings C# / .NET pour Unity, Godot et MonoGame. Multijoueur sans monter de serveur.
start-game-code = dotnet add package Personal.Rns
start-game-target = personal-rns-ffi

start-embedded-headline = Je vise les microcontrôleurs
start-embedded-body = Le moteur plus un trait Host à trois méthodes. L'ESP32-C6 est la référence ; S3, nRF, RP2040 et STM32 suivent.
start-embedded-code = cargo add personal-rns --no-default-features
start-embedded-target = personal-rns + hosts/*

start-web-headline = Je construis pour le web ou l'edge
start-web-body = Une build WebAssembly qui tourne dans le navigateur et sur les runtimes edge comme Cloudflare Workers, Fastly et Spin.
start-web-code = npm install personal-rns
start-web-target = personal-rns (wasm32)

start-rust-headline = Je l'embarque dans une app Rust
start-rust-body = Un runtime RNS complet prêt à l'emploi, ou le cœur pur pour bâtir ton propre runtime autour.
start-rust-code = cargo add personal-rnsd   # complete RNS runtime
    cargo add personal-rns      # pure core only
start-rust-target = personal-rnsd or personal-rns

start-lxmf-headline = Je veux envoyer des messages sur le mesh
start-lxmf-body = LXMF au-dessus de Reticulum — identités, adresses, livraison. La couche sur laquelle reposent Sideband et Nomadnet.
start-lxmf-code = cargo add personal-lxmf
start-lxmf-target = personal-lxmf

# Pied (licence)
footer-license = Open source. MIT / Apache 2.0.

# Page philosophie
ethos-kicker = La discipline
ethos-title = Comment on le construit
ethos-lead = Une note d'ingénieur à ingénieur sur la discipline derrière ce projet — moteur pur, cœur sans allocateur, chaque changement vérifié contre la référence RNS. Parcours-la avant d'en dépendre ; on veut que tu saches dans quoi tu t'engages.

# Index des crates
crates-kicker = Les pièces
crates-title = Choisis ce qui correspond à ce que tu construis.
crates-lead = Chaque crate est conçu pour être utile seul, même si tu n'amènes pas le reste. Le moteur est le substrat ; tout le reste s'empile dessus, et d'autres pièces arrivent au fur et à mesure que la suite grandit.
crates-card-cta = Ce qu'il fait →
crates-back = Tous les crates
crates-not-found = Aucun crate de ce nom

# Cartes par crate
crate-rns-role = Le moteur
crate-rns-blurb = Mets Reticulum dans n'importe quel projet Rust. Déterministe, no_std, sans allocateur ; pas d'état global, pas d'E/S intégrées — apporte ton horloge et ton câble.
crate-rnsd-role = Le daemon
crate-rnsd-blurb = Un drop-in pour rnsd qui tourne là où Linux tourne. Même fil que la référence RNS ; utilise-le à côté ou à la place des nœuds que tu as déjà.
crate-lxmf-role = Messagerie
crate-lxmf-blurb = LXMF au-dessus de Reticulum — la couche sur laquelle reposent Sideband et Nomadnet. Identités, adresses, livraison de messages.
crate-ffi-role = Bindings mobiles et Python
crate-ffi-blurb = Une seule interface uniffi génère Kotlin (.aar), Swift (.xcframework) et Python (.whl). Utilise Reticulum depuis Android, iOS ou un notebook Jupyter — même forme, même moteur.
crate-rvt-role = Débogueur visuel
crate-rvt-blurb = Regarde les paquets se déplacer entre des nœuds simulés sur une horloge virtuelle. Déterministe — même scénario, même trace, à chaque fois.
crate-esp32c6-role = Firmware ESP32-C6
crate-esp32c6-blurb = Adaptateur host bare-metal pour l'ESP32-C6. Pas d'OS, pas d'allocateur — la preuve que le moteur tourne sur une puce RISC-V à cinq dollars avec radios intégrées.

# 404
not-found-title = Il n'y a encore rien ici.
not-found-cta = Retour à l'accueil
