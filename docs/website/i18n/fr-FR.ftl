# Navigation
nav-ethos = Philosophie
nav-crates = Crates
nav-api = API

# Pied de page
footer-tagline = Porter le contrat, pas l'implémentation.

# Page d'accueil
landing-kicker = Reticulum, porté avec fidélité
landing-title = Un moteur pur. Chaque plateforme apporte un host léger.
landing-subtitle = Un portage Rust à partir de zéro du contrat de réseau maillé Reticulum — pensé d'abord pour l'embarqué, déterministe, compatible no_std, avec une petite couture Host qui permet au même moteur de tourner sur un démon, un microcontrôleur et un téléphone.
landing-cta-ethos = Lire la philosophie
landing-cta-crates = Parcourir les crates
landing-triumvirate-label = Le triumvirat
landing-quote-label = Directive de conception
landing-quote-body = Porter le contrat, pas l'implémentation. Construire un seul moteur pur, et laisser chaque plateforme apporter son host léger.

# Cartes du triumvirat
triumvirate-rns-role = Le moteur pur
triumvirate-rns-blurb = Contrat fil, routage, annonces, liens — tick/ingest purs, pas d'E/S, std non requis.
triumvirate-rnsd-role = Le host démon
triumvirate-rnsd-blurb = Un adaptateur Host léger basé sur std ; l'exemple canonique de la façon dont une plateforme donne vie au moteur.
triumvirate-lxmf-role = La couche de messages
triumvirate-lxmf-blurb = Couche applicative LXMF au-dessus du moteur — adressage, livraison et identité pour les développeurs d'apps.

# Page philosophie
ethos-kicker = Comment c'est construit
ethos-title = La directive de conception
ethos-lead = Voici la philosophie d'ingénierie derrière chaque décision de la suite. Lisez-la une fois pour comprendre pourquoi l'architecture est ce qu'elle est : moteur pur, host léger, contrat sur implémentation.

# Index des crates
crates-kicker = La suite
crates-title = Les crates en un coup d'œil
crates-lead = Six crates composent la suite. Le moteur est le substrat ; tout le reste est un host léger ou un consommateur.
crates-card-cta = Lire la suite →
crates-back = Retour aux crates
crates-not-found = Aucun crate de ce nom

# Cartes par crate
crate-rns-role = Le moteur Reticulum pur
crate-rns-blurb = Contrat fil et routage comme machine d'état pure. no_std + alloc, déterministe, pensé pour l'embarqué.
crate-rnsd-role = Le host démon de référence
crate-rnsd-blurb = Adaptateur Host basé sur std et binaire démon Linux. L'exemple canonique pour amener le moteur en ligne.
crate-lxmf-role = La couche applicative LXMF
crate-lxmf-blurb = Adressage, livraison et identité pour les développeurs d'apps. Au-dessus de personal-rns ; consommé par Personal et d'autres.
crate-ffi-role = Bindings Kotlin / Swift / Python
crate-ffi-blurb = Un seul UDL, trois langages via uniffi. La porte d'entrée du SDK pour les consommateurs non-Rust.
crate-rvt-role = Simulation multi-nœuds et outillage
crate-rvt-blurb = Reticulum Visual Toolkit. Aujourd'hui simulation multi-nœuds à horloge virtuelle ; bientôt débogueur en direct. En Dioxus et portable au web.
crate-esp32c6-role = Adaptateur Host pour ESP32-C6
crate-esp32c6-blurb = Host no_std/no_main sur bare metal. La preuve que le moteur tient sur un vrai microcontrôleur.

# 404
not-found-title = Il n'y a encore rien ici.
not-found-cta = Retour à l'accueil
