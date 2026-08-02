# Navigation
nav-benchmarks = Benchmarks
nav-contributing = Mitwirken
site-early-english-note = Prns ist noch jung: die vollständige Dokumentation liegt auf GitHub und im Quellcode und ist vorerst nur auf Englisch verfügbar.

# Footer
footer-tagline = Präsentiert von KenAKAFrosty und dem Personal/Prns-Team.
footer-flash = Hopspot flashen (nur Englisch)
footer-playground = Browser-Playground (nur Englisch)

# Landing
landing-kicker = Mesh-Netzwerke, die dir gehören
landing-kicker-prefix = Mesh-Netzwerke, die dir
landing-title = Hochperformantes Reticulum (RNS), gebaut für jedes Gerät.
landing-title-lead = Hochperformantes Reticulum (RNS),
landing-title-accent = gebaut für jedes Gerät.
landing-subtitle = Gebaut für die Performance, Stabilität und Energieeffizienz, die jeder Reticulum-Knoten braucht, vom 5-Dollar-Mikrocontroller bis zum Cloud-Server-Cluster. Eine Engine und eine API, identisch auf Embedded, Desktop, Mobile, in Spielen und im Web.
landing-cta-ethos = Finde deinen Weg in Prns
landing-cta-standards = Unsere Standards
# Pull quote
landing-quote-label = Worauf wir hinarbeiten
landing-quote-body = Reticulum ist die grundlegende Kommunikationsinfrastruktur einer strahlenden Zukunft, die wir haben können, solange wir sie alle mitbauen. Dies ist der Beitrag des Personal-Teams, RNS in die Hände von mehr Buildern zu legen und diese Zukunft möglich zu machen.

# Interfaces
interfaces-section-label = Interfaces
interfaces-section-title = Wo das Mesh auf die Welt trifft
interfaces-section-lead = Prns behält die RNS-kompatiblen Interfaces bei, die Builder schon kennen, und erweitert die Karte mit nativen Links für neue Geräte und Netzwerke.
interfaces-section-hot-note = Prns-Interfaces sind hot-swappable: Füge ein Interface hinzu, entferne es oder ändere es ohne Node-Neustart.

interfaces-radio-label = Funk
interfaces-radio-headline = Nahbereichslinks für Geräte und Boards
interfaces-radio-body = Bluetooth LE Auto-interface, ESP-NOW und LoRa bringen nahe Geräte, Board-Flotten und Langstrecken-RF-Links in ein gemeinsames Reticulum-Mesh.

interfaces-lan-label = LAN
interfaces-lan-headline = Automatisch entdeckte Local-Link-Peers
interfaces-lan-body = Wi-Fi Auto-interface nutzt Multicast, mDNS und Gateway-Rendezvous, um nahe Nodes zu finden und ein lokales Netzwerk ins Mesh einzubinden.

interfaces-cable-label = Kabel + Packet Radio
interfaces-cable-headline = Kabel, TNCs und Funkmodems
interfaces-cable-body = USB Auto-interface, serielles Framing, KISS, AX.25 und RNode bringen kleine Geräte und Packet-Radio-Hardware in dasselbe Mesh.

interfaces-host-label = Geroutetes IP
interfaces-host-headline = Internet-, WAN- und Backbone-Links
interfaces-host-body = TCP Client/Server, UDP, WebSocket und Backbone lassen entfernte Peers über private WANs, VPNs, öffentliche Internet-Relays und Browser-Integrationen am Mesh teilnehmen.

# What you can count on (standards callout)
standards-section-label = Unsere Standards
standards-section-title = Worauf du dich verlassen kannst
standards-license-label = Lizenz
standards-license-headline = MIT / Apache 2.0
standards-license-body = Doppelt lizenziert und permissiv. Kein Copyleft und keine kommerziellen Einschränkungen.
standards-safety-label = Sicherheit
standards-safety-headline = Erzwungen, dann auditiert
standards-safety-body = In der Engine kompilieren Panics, Unwraps und unbegründetes unsafe nie. Was sich nicht verbieten lässt, wird auditiert: unsafe in Abhängigkeiten mit cargo-geiger, Undefined Behavior unter Miri, Advisories mit cargo-deny.
standards-correctness-label = Korrektheit
standards-correctness-headline = Gegen RNS diff-getestet
standards-correctness-body = Jede Änderung wird gegen die Referenz geprüft und dann durch Unit-, Property-, Fuzz- und Mutationstests geschickt, mit Kani-Beweisen dort, wo sie zählen.
standards-benchmarked-label = Performance
standards-benchmarked-headline = Gemessen, nicht nur behauptet
standards-benchmarked-body = Performance wird offen verfolgt, gemessen mit einem Harness, den du selbst ausführen kannst.
standards-benchmarked-cta = Benchmarks ansehen →

# Where do I start? (use-case cards on landing)
start-section-label = Wege hinein
start-section-title = Was willst du hier tun?
start-section-lead = Wähle den Weg, der dazu passt, wie Prns in deine Arbeit kommt: Hardware, die du flashst, Infrastruktur, die du betreibst, oder Software, die du baust.

start-daemon-headline = Einen Daemon betreiben
start-daemon-body = Installiere einen schnellen Reticulum-Daemon für Desktops, LXMF-Apps, Backbone-VPS und mehr.
start-daemon-code = Drop-in für Standard-Apps
    Liest ~/.reticulum
    Interface-Änderungen live
    Metriken eingebaut
start-daemon-target = Prnsd starten

start-embedded-headline = Einen Hopspot flashen
start-embedded-body = Wähle ein unterstütztes Board, vergleiche Funk- und Akku-Kompromisse und flashe dann ein dediziertes Mesh-Gerät.
start-embedded-code = Board-Matrix
    Web-Flasher
    Lokales Flashen
start-embedded-target = Hopspot flashen (nur Englisch)

start-web-headline = Browser-Node-Playground verwenden
start-web-body = Teste die TypeScript-API mit der gemeinsamen Rust-Engine in WebAssembly, verbinde dich über Auto Wi-Fi oder USB Auto und beobachte die lokale Node-Aktivität live.
start-web-code = WebAssembly-Runtime
    Auto Wi-Fi + USB Auto
    TypeScript-Beispiel
start-web-target = Playground öffnen (nur Englisch)

start-rust-headline = Auf Reticulum bauen
start-rust-body = Nutze Engine und Bindings, um Mesh-Netzwerke in Apps, Tools, Dienste oder Spiele einzubauen.
start-rust-target = README lesen
start-rust-target-source = Quellcode herunterladen

# Platforms ("Runs on") - hero marquee label + CTA, and the dedicated page
landing-platforms-label = Läuft auf
landing-platforms-cta = Alle ansehen →
platforms-title = Wo Prns läuft
platforms-lead = Eine Engine, überall zuhause. Diese Schnellübersicht trennt die Runtime-Plattformunterstützung von der Unterstützung konkreter Hopspot-Boards.
platforms-board-support-link = Board-Unterstützung & Bring-up ansehen →

# Flash a Hopspot page
flash-back = Plattformen
flash-back-boards = Boards
flash-card-action = Flashen

# Benchmarks page
benchmarks-kicker = Performance
benchmarks-title = Offen gebenchmarkt
benchmarks-lead = Jede Zahl unten stammt aus den veröffentlichten Ergebnissen im Repo, gemessen auf echter Hardware mit einem Harness, den du selbst ausführen kannst. Ab hier sind die Inhalte vorerst nur auf Englisch verfügbar.

# License signal (footer)
footer-license = Open Source. MIT / Apache 2.0.
footer-trademarks = Logos, Marken und Produktbilder Dritter gehören ihren jeweiligen Inhabern. Sie werden nur gezeigt, um Plattformen, Hardware und Kompatibilitätsziele zu identifizieren. Eine Billigung wird weder beansprucht noch impliziert.

# Contributing page
contributing-kicker = Die Messlatte
contributing-title = Mitwirken
contributing-lead = Wie du beiträgst — was wir wertschätzen, welchen Konventionen dein Code folgt und welchen Standard jede Änderung erfüllt. Für menschliche und automatisierte Beitragende gleichermaßen.

# 404
not-found-title = Hier ist noch nichts.
not-found-cta = Zurück zur Startseite
