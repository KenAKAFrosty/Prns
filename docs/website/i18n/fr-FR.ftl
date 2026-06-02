# Navigation
nav-ethos = Design
nav-crates = Crates
nav-api = API

# Pied de page
footer-tagline = Construit par l'équipe Personal.

# Page d'accueil
landing-kicker = Des réseaux maillés que rien n'arrête — pour les gens
landing-title = Un portage de Reticulum (RNS) prêt pour la production, écrit en Rust.
landing-subtitle = Un cœur déterministe, sans std et sans allocateur. Couverture complète de RNS et LXMF. Pensé pour la performance et l'autonomie dont n'importe quelle pile Reticulum a besoin — d'un microcontrôleur à cinq dollars jusqu'à un nœud cloud.
landing-cta-ethos = Choisir un crate
landing-cta-crates = Comment on le construit

# Citation
landing-quote-label = Ce vers quoi on bâtit
landing-quote-body = Reticulum est l'infrastructure de communication fondatrice de l'avenir lumineux qu'on peut avoir — si nous le construisons. C'est notre manière de le mettre entre les mains de plus de développeurs et de faire avancer cet avenir, ensemble.

# Ce sur quoi vous pouvez compter
standards-section-label = Nos standards
standards-section-title = Ce sur quoi vous pouvez compter
standards-license-label = Licence
standards-license-headline = MIT / Apache 2.0
standards-license-body = Double licence, permissive. Pas de copyleft, pas de clauses non commerciales.
standards-coverage-label = Couverture
standards-coverage-headline = RNS et LXMF complets
standards-coverage-body = Pas seulement RNS. Et LXMF n'est pas en accessoire. Les deux, entiers.
standards-core-label = Cœur
standards-core-headline = no_std, sans allocateur
standards-core-body = Un cœur déterministe qui tourne là où les allocateurs ne peuvent pas.
standards-verification-label = Vérification
standards-verification-headline = Diff-testé contre RNS
standards-verification-body = Chaque changement est confronté à la référence, et là où ça compte vraiment, des preuves formelles viennent avec.

# Par où je commence ?
start-section-label = Voies d'entrée
start-section-title = Par où je commence ?
start-section-lead = Choisis le chemin qui correspond à ce que tu construis. Aujourd'hui chacun mène à un seul crate ; les guides dédiés suivent au même rythme.

start-daemon-headline = Je veux un nœud Reticulum en route
start-daemon-body = Daemon prêt à l'emploi. Drop-in pour rnsd. Pose-le à côté des nœuds que tu as déjà et laisse-les tourner ensemble.
start-daemon-code = apt install personal-rnsd
start-daemon-target = personal-rnsd

start-mobile-headline = Je construis une application mobile
start-mobile-body = Kotlin (.aar), Swift (.xcframework) ou Python (.whl) — le même moteur que ton daemon, embarqué directement dans ton app.
start-mobile-code = implementation("org.staypersonal:rns:0.1")
    pod 'PersonalRns', '~> 0.1'
start-mobile-target = personal-rns-ffi

start-game-headline = Je l'intègre dans un jeu
start-game-body = Bindings C#/.NET pour Unity, Godot et MonoGame. Multijoueur sans monter de serveur.
start-game-code = dotnet add package Personal.Rns
start-game-target = personal-rns-ffi

start-embedded-headline = Je vise les microcontrôleurs
start-embedded-body = Le moteur, plus un trait Host à seulement trois méthodes. L'ESP32-C6 est la référence ; suivent le S3, le nRF, le RP2040 et le STM32.
start-embedded-code = cargo add personal-rns --no-default-features
start-embedded-target = personal-rns + hosts/*

start-web-headline = Je construis pour le web ou l'edge
start-web-body = Une build WebAssembly qui tourne dans le navigateur et sur les runtimes edge comme Cloudflare Workers, Fastly et Spin.
start-web-code = npm install personal-rns
start-web-target = personal-rns (wasm32)

start-rust-headline = Je l'embarque dans une app Rust
start-rust-body = Un runtime RNS complet livré tel quel, ou le cœur pur pour bâtir ton propre runtime autour. À toi de choisir.
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
crates-title = Choisis ce qui colle à ce que tu construis.
crates-lead = Chaque crate est pensé pour être utile seul, même si tu n'amènes pas le reste. Le moteur est le substrat ; tout le reste s'empile dessus, et d'autres pièces arrivent à mesure que la suite grandit.
crates-card-cta = Ce qu'il fait →
crates-back = Tous les crates
crates-not-found = Aucun crate de ce nom

# Cartes par crate
crate-rns-role = Le moteur
crate-rns-blurb = Glisse Reticulum dans n'importe quel projet Rust. Déterministe, no_std, sans allocateur ; pas d'état global, pas d'E/S intégrées — apporte ton horloge et ton câble.
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
