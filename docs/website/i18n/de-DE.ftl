# Navigation
nav-contributing = Mitwirken
nav-api = API

# Footer
footer-tagline = Präsentiert vom Personal-Team.

# Landing
landing-kicker = Unaufhaltsame Mesh-Netzwerke für die Menschen
landing-kicker-prefix = Unaufhaltsame Mesh-Netzwerke für die
landing-title = Eine leistungsstarke Portierung von Reticulum (RNS), geschrieben in sicherem Rust.
landing-title-lead = A high-performance port of Reticulum (RNS).
landing-title-accent = Runs on any device.
landing-subtitle = Ein deterministischer no_std-Kern ohne Allocator. Gebaut für die Performance und Stabilität, die jeder Reticulum-Knoten braucht, vom Fünf-Dollar-Mikrocontroller bis zum Cloud-Server.
landing-cta-ethos = Wähle eine Crate
# Pull quote
landing-quote-label = Worauf wir hinarbeiten
landing-quote-body = Reticulum ist die grundlegende Kommunikationsinfrastruktur einer hellen Zukunft, die wir haben können, solange wir sie alle mitbauen. Dies ist der Beitrag des Personal-Teams, RNS in die Hände von mehr Buildern zu legen und diese Zukunft möglich zu machen.

# Interfaces
interfaces-section-label = Interfaces
interfaces-section-title = Wo das Mesh die Welt berührt
interfaces-section-lead = Prns behält die RNS-kompatiblen Interfaces bei, die Builder schon kennen, und erweitert die Karte mit nativen Links für neue Geräte und Netzwerke.
interfaces-section-hot-note = Prns-Interfaces sind hot-swappable: Füge ein Interface hinzu, entferne es oder ändere es ohne Node-Neustart.

interfaces-radio-label = Funk
interfaces-radio-headline = Nahbereichslinks für Geräte und Boards
interfaces-radio-body = Bluetooth LE Auto-interface, ESP-NOW und LoRa bringen nahe Geräte, Board-Flotten und Langstreckenlinks in ein gemeinsames Reticulum-Mesh.

interfaces-lan-label = LAN
interfaces-lan-headline = Automatisch entdeckte Local-Link-Peers
interfaces-lan-body = Wi-Fi Auto-interface nutzt Multicast, mDNS und Gateway-Rendezvous, um nahe Nodes zu finden und ein lokales Netzwerk ins Mesh zu falten.

interfaces-cable-label = Kabel + Packet Radio
interfaces-cable-headline = Kabel, TNCs und Funkmodems
interfaces-cable-body = USB Auto-interface, serielles Framing, KISS, AX.25 und RNode bringen kleine Geräte und Packet-Radio-Hardware in dasselbe Mesh.

interfaces-host-label = Geroutetes IP
interfaces-host-headline = Internet-, WAN- und Backbone-Links
interfaces-host-body = TCP Client/Server, UDP und Backbone lassen entfernte Peers über private WANs, VPNs und öffentliche Internet-Relays am Mesh teilnehmen.

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
standards-correctness-body = Jede Änderung wird gegen die Referenz geprüft und dann durch Property-, Fuzz- und Mutationstests geschickt, mit Kani-Beweisen dort, wo sie zählen.
standards-benchmarked-label = Performance
standards-benchmarked-headline = Gemessen, nicht nur behauptet
standards-benchmarked-body = Performance wird offen verfolgt, gemessen mit einem Harness, den du selbst ausführen kannst.
standards-benchmarked-cta = Benchmarks ansehen →

# Where do I start? (use-case cards on landing)
start-section-label = Wege hinein
start-section-title = Wo fange ich an?
start-section-lead = Wähle den Weg, der zu dem passt, was du baust. Jeder landet heute bei einer einzelnen Crate; weitere Guides kommen daneben hinzu.

start-daemon-headline = Ich will einen Reticulum-Knoten betreiben
start-daemon-body = Vorgefertigter Daemon. Drop-in für rnsd. Lass ihn neben den Knoten laufen, die du schon hast.
start-daemon-target = prnsd

start-embedded-headline = Flash a Hopspot
start-embedded-body = Pick a supported board, compare radio and battery tradeoffs, then flash a dedicated mesh device.
start-embedded-code = Board matrix
    Web flasher
    Local flash
start-embedded-target = Flash a Hopspot

start-web-headline = Browser-Node-Playground verwenden
start-web-body = Teste die TypeScript-API mit der gemeinsamen Rust-Engine in WebAssembly, verbinde dich über Auto Wi-Fi oder USB Auto und beobachte die lokale Node-Aktivität live.
start-web-code = WebAssembly-Runtime
    Auto Wi-Fi + USB Auto
    TypeScript-Beispiel
start-web-target = Playground öffnen

start-rust-headline = Ich bette es in eine Rust-App ein
start-rust-body = Eine vollständige RNS-Runtime sofort einsatzbereit, oder der reine Kern, um deine eigene Runtime darum herum zu bauen.
start-rust-target = prnsd or personal-rns

# Platforms ("Runs on") - hero marquee label + CTA, and the dedicated page
landing-platforms-label = Runs on
landing-platforms-cta = See all →
platforms-title = Where Prns runs
platforms-lead = One engine, many homes. This quick view separates runtime platform support from specific Hopspot board support.
platforms-board-support-link = View board support & bring-up →

# Flash a Hopspot page
flash-back = Platforms
flash-back-boards = Boards
flash-card-action = Flash

# Benchmarks page
benchmarks-kicker = Performance
benchmarks-title = Offen benchmarked
benchmarks-lead = Wir behandeln Performance als Zahl, nicht als Adjektiv. Jede Kennzahl hier kommt aus einem deterministischen Harness im Repo, gemessen auf echter Hardware und gegen die RNS-Referenz geprüft, wo der Vergleich fair ist. Die Zahlen landen, während sich die Suite stabilisiert; unten steht die Methodik, der sie standhalten.

# License signal (footer)
footer-license = Open Source. MIT / Apache 2.0.
footer-trademarks = Logos und Marken Dritter gehören ihren jeweiligen Inhabern. Sie werden nur gezeigt, um Plattformen, Hardware und Kompatibilitätsziele zu identifizieren. Eine Billigung wird weder beansprucht noch impliziert.

# Contributing page
contributing-kicker = Die Messlatte
contributing-title = Mitwirken
contributing-lead = Wie du beiträgst — was wir wertschätzen, welchen Konventionen dein Code folgt und welchen Standard jede Änderung erfüllt. Für menschliche und automatisierte Beitragende gleichermaßen.

# 404
not-found-title = Hier ist noch nichts.
not-found-cta = Zurück zur Startseite
