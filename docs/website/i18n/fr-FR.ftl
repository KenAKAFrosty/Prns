# Navigation
nav-benchmarks = Benchmarks
nav-contributing = Contribuer
site-early-english-note = Prns en est à ses débuts : la documentation complète se trouve sur GitHub et dans le code source, et n'est disponible qu'en anglais pour l'instant.

# Pied de page
footer-tagline = Proposé par KenAKAFrosty et l'équipe Personal/Prns.
footer-flash = Flasher un Hopspot (en anglais uniquement)
footer-playground = Playground navigateur (en anglais uniquement)

# Accueil
landing-kicker = Des réseaux mesh qui vous appartiennent
landing-kicker-prefix = Des réseaux mesh qui vous
landing-title = Reticulum (RNS) haute performance, conçu pour tourner sur n'importe quel appareil.
landing-title-lead = Reticulum (RNS) haute performance,
landing-title-accent = conçu pour tourner sur n'importe quel appareil.
landing-subtitle = Conçu pour les performances, la stabilité et l'efficacité énergétique dont chaque nœud Reticulum a besoin, du microcontrôleur à 5 dollars au cluster de serveurs cloud. Un seul moteur et une seule API, identiques en embarqué, sur desktop, mobile, dans les jeux et sur le web.
landing-cta-ethos = Trouvez votre chemin dans Prns
landing-cta-standards = Nos standards
# Citation
landing-quote-label = Ce que nous voulons construire
landing-quote-body = Reticulum est l'infrastructure de communication fondatrice d'un avenir lumineux que nous pouvons avoir, tant que nous le construisons tous ensemble. C'est l'effort de l'équipe Personal pour mettre RNS entre les mains de plus de builders et aider cet avenir à prendre forme.

# Interfaces
interfaces-section-label = Interfaces
interfaces-section-title = Là où le mesh rencontre le monde
interfaces-section-lead = Prns conserve les interfaces compatibles RNS que les builders connaissent déjà, puis élargit la carte avec des liens natifs pour de nouveaux appareils et réseaux.
interfaces-section-hot-note = Les interfaces Prns sont hot-swappable : ajoutez, supprimez ou modifiez une interface sans redémarrer le nœud.

interfaces-radio-label = Radios
interfaces-radio-headline = Liens de proximité pour appareils et cartes
interfaces-radio-body = Bluetooth LE Auto-interface, ESP-NOW et LoRa font entrer les appareils proches, les flottes de cartes et les liens RF longue portée dans un même mesh Reticulum.

interfaces-lan-label = LAN
interfaces-lan-headline = Pairs de lien local découverts automatiquement
interfaces-lan-body = Wi-Fi Auto-interface utilise le multicast, mDNS et le rendez-vous passerelle pour trouver les nœuds proches et intégrer un réseau local au mesh.

interfaces-cable-label = Filaire + packet radio
interfaces-cable-headline = Câbles, TNC et modems radio
interfaces-cable-body = USB Auto-interface, le framing série, KISS, AX.25 et RNode relient les petits appareils et le matériel radio paquet au même mesh.

interfaces-host-label = IP routée
interfaces-host-headline = Internet, WAN et liens backbone
interfaces-host-body = TCP client/serveur, UDP, WebSocket et Backbone permettent aux pairs distants de participer au mesh via des WAN privés, des VPN, des relais Internet publics et des intégrations navigateur.

# Ce sur quoi vous pouvez compter
standards-section-label = Nos standards
standards-section-title = Ce sur quoi vous pouvez compter
standards-license-label = Licence
standards-license-headline = MIT / Apache 2.0
standards-license-body = Double licence permissive. Pas de copyleft ni de restrictions commerciales.
standards-safety-label = Sécurité
standards-safety-headline = Imposée, puis auditée
standards-safety-body = Dans le moteur, les panics, les unwraps et le unsafe injustifié ne compilent jamais. Ce qui ne peut pas être interdit est audité : le unsafe des dépendances avec cargo-geiger, le comportement indéfini sous Miri, les avis de sécurité avec cargo-deny.
standards-correctness-label = Correction
standards-correctness-headline = Diff-testé contre RNS
standards-correctness-body = Chaque changement est vérifié par rapport à la référence, puis passe par des tests unitaires, de propriétés, de fuzzing et de mutation, avec des preuves Kani là où elles comptent.
standards-benchmarked-label = Performance
standards-benchmarked-headline = Mesurée, pas seulement affirmée
standards-benchmarked-body = Les performances sont suivies au grand jour, mesurées par un harness que vous pouvez exécuter vous-même.
standards-benchmarked-cta = Voir les benchmarks →

# Par où commencer ?
start-section-label = Chemins d'entrée
start-section-title = Que venez-vous faire ici ?
start-section-lead = Choisissez le chemin qui correspond à la place de Prns dans votre travail : du matériel à flasher, de l'infrastructure à faire tourner, ou du logiciel à construire.

start-daemon-headline = Lancer un daemon
start-daemon-body = Installez un daemon Reticulum rapide pour desktops, apps LXMF, VPS backbone, etc.
start-daemon-code = Drop-in pour les apps standard
    Lit ~/.reticulum
    Interfaces modifiables à chaud
    Métriques intégrées
start-daemon-target = Lancer Prnsd

start-embedded-headline = Flasher un Hopspot
start-embedded-body = Choisissez une carte prise en charge, flashez-la directement depuis votre navigateur et obtenez un appareil mesh dédié en quelques minutes.
start-embedded-code = Matrice des cartes
    Flasher web
    Flash local
start-embedded-target = Flasher un Hopspot (en anglais uniquement)

start-web-headline = Utiliser le playground du nœud navigateur
start-web-body = Essayez l'API TypeScript avec le moteur Rust partagé en WebAssembly, connectez-vous via Auto Wi-Fi ou USB Auto et suivez en direct l'activité locale du nœud.
start-web-code = Runtime WebAssembly
    Auto Wi-Fi + USB Auto
    Exemple TypeScript
start-web-target = Ouvrir le playground (en anglais uniquement)

start-rust-headline = Construire sur Reticulum
start-rust-body = Utilisez le moteur et les bindings pour ajouter du réseau mesh à des apps, outils, services ou jeux.
start-rust-target = Lire le README
start-rust-target-source = Télécharger le code source

# Plateformes ("Runs on") — libellé du marquee hero + CTA et page dédiée
landing-platforms-label = Tourne sur
landing-platforms-cta = Tout voir →
platforms-title = Où tourne Prns
platforms-lead = Un moteur, de nombreux foyers. Cette vue rapide sépare la prise en charge des plateformes runtime de celle des cartes Hopspot spécifiques.
platforms-board-support-link = Voir la prise en charge des cartes Hopspot et le bring-up →

# Page Flasher un Hopspot
flash-back = Plateformes
flash-back-boards = Cartes
flash-card-action = Flasher

# Page benchmarks
benchmarks-kicker = Performance
benchmarks-title = Benchmarké au grand jour
benchmarks-lead = Chaque chiffre ci-dessous vient des résultats publiés dans le dépôt, mesurés sur du vrai matériel par un harness que vous pouvez exécuter vous-même. À partir d'ici, le contenu est pour l'instant disponible uniquement en anglais.

# Signal licence (pied de page)
footer-license = Open source. MIT / Apache 2.0.
footer-trademarks = Les logos, marques et images de produits de tiers appartiennent à leurs propriétaires respectifs. Ils sont affichés uniquement pour identifier des plateformes, du matériel et des cibles de compatibilité. Aucune approbation n'est revendiquée ni implicite.

# 404
not-found-title = Il n'y a encore rien ici.
not-found-cta = Retour à l'accueil
