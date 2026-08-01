# Navigation
nav-contributing = Contribuer
nav-api = API

# Pied de page
footer-tagline = Proposé par l'équipe Personal.

# Accueil
landing-kicker = Des réseaux mesh inarrêtables pour tous
landing-kicker-prefix = Des réseaux mesh inarrêtables pour
landing-title = Un port haute performance de Reticulum (RNS), écrit en Rust sûr.
landing-title-lead = A high-performance port of Reticulum (RNS).
landing-title-accent = Runs on any device.
landing-subtitle = Un cœur déterministe, no_std et sans allocateur. Conçu pour les performances et la stabilité dont chaque nœud Reticulum a besoin, du microcontrôleur à cinq dollars au serveur cloud.
landing-cta-ethos = Choisir une crate
# Citation
landing-quote-label = Ce vers quoi nous construisons
landing-quote-body = Reticulum est l'infrastructure de communication fondatrice d'un avenir lumineux que nous pouvons avoir, tant que nous le construisons tous ensemble. C'est l'effort de l'équipe Personal pour mettre RNS entre les mains de plus de builders et aider cet avenir à prendre forme.

# Interfaces
interfaces-section-label = Interfaces
interfaces-section-title = Là où le mesh rencontre le monde
interfaces-section-lead = Prns conserve les interfaces compatibles RNS que les builders connaissent déjà, puis élargit la carte avec des liens natifs pour de nouveaux appareils et réseaux.
interfaces-section-hot-note = Les interfaces Prns sont hot-swappable : ajoutez, supprimez ou modifiez une interface sans redémarrer le nœud.

interfaces-radio-label = Radios
interfaces-radio-headline = Liens de proximité pour appareils et cartes
interfaces-radio-body = Bluetooth LE Auto-interface, ESP-NOW et LoRa font entrer les appareils proches, les flottes de cartes et les liens longue portée dans un même mesh RNS.

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
standards-safety-headline = Imposé, puis audité
standards-safety-body = Dans le moteur, les panics, les unwraps et le unsafe injustifié ne compilent jamais. Ce qui ne peut pas être interdit est audité : le unsafe des dépendances avec cargo-geiger, le comportement indéfini sous Miri, les avis de sécurité avec cargo-deny.
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
start-daemon-code = Drop-in for stock apps
    Reads ~/.reticulum
    Live interface edits
    Built-in metrics
start-daemon-target = prnsd

start-embedded-headline = Flash a Hopspot
start-embedded-body = Pick a supported board, compare radio and battery tradeoffs, then flash a dedicated mesh device.
start-embedded-code = Board matrix
    Web flasher
    Local flash
start-embedded-target = Flash a Hopspot

start-web-headline = Utiliser le playground du nœud navigateur
start-web-body = Essayez l’API TypeScript avec le moteur Rust partagé en WebAssembly, connectez-vous via Auto Wi-Fi ou USB Auto et suivez en direct l’activité locale du nœud.
start-web-code = Runtime WebAssembly
    Auto Wi-Fi + USB Auto
    Exemple TypeScript
start-web-target = Ouvrir le playground

start-rust-headline = Construisez sur Reticulum
start-rust-body = Utilisez le moteur et les bindings pour ajouter du réseau mesh à des apps, outils, services ou jeux.
start-rust-target = Lire le README
start-rust-target-source = Télécharger le code source

# Plateformes ("Runs on") — libellé du marquee hero + CTA et page dédiée
landing-platforms-label = Runs on
landing-platforms-cta = See all →
platforms-title = Where Prns runs
platforms-lead = One engine, many homes. This quick view separates runtime platform support from specific Hopspot board support.
platforms-board-support-link = View board support & bring-up →

# Flash a Hopspot page
flash-back = Platforms
flash-back-boards = Boards
flash-card-action = Flash

# Page benchmarks
benchmarks-kicker = Performance
benchmarks-title = Benchmarké au grand jour
benchmarks-lead = Nous traitons la performance comme un nombre, pas comme un adjectif. Chaque chiffre ici vient d'un harness déterministe dans le dépôt, mesuré sur du vrai matériel et vérifié contre la référence RNS lorsque la comparaison est juste. Les chiffres arrivent à mesure que la suite se stabilise ; ci-dessous, la méthodologie qu'ils doivent respecter.

# Signal licence (pied de page)
footer-license = Open source. MIT / Apache 2.0.
footer-trademarks = Les logos et marques de tiers appartiennent à leurs propriétaires respectifs. Ils sont affichés uniquement pour identifier des plateformes, du matériel et des cibles de compatibilité. Aucune approbation n'est revendiquée ni implicite.

# Page contribution
contributing-kicker = Le niveau d'exigence
contributing-title = Contribuer
contributing-lead = Comment contribuer — ce que nous valorisons, les conventions que votre code suit, et le standard que chaque changement franchit. Pour les contributeurs humains comme automatisés.

# 404
not-found-title = Il n'y a encore rien ici.
not-found-cta = Retour à l'accueil
