# Navigation
nav-contributing = Mitwirken
nav-crates = Crates
nav-api = API

# Footer
footer-tagline = Präsentiert vom Personal-Team.

# Landing
landing-kicker = Unaufhaltsame Mesh-Netzwerke für die Menschen
landing-kicker-prefix = Unaufhaltsame Mesh-Netzwerke für die
landing-title = Eine produktionsreife Portierung von Reticulum (RNS), geschrieben in sicherem Rust.
landing-subtitle = Ein deterministischer no_std-Kern ohne Allocator. Gebaut für die Performance und Stabilität, die jeder Reticulum-Knoten braucht, vom Fünf-Dollar-Mikrocontroller bis zum Cloud-Server.
landing-cta-ethos = Wähle eine Crate
landing-cta-contributing = Mitwirken

# Pull quote
landing-quote-label = Worauf wir hinarbeiten
landing-quote-body = Reticulum ist die grundlegende Kommunikationsinfrastruktur einer hellen Zukunft, die wir haben können, solange wir sie alle mitbauen. Dies ist der Beitrag des Personal-Teams, RNS in die Hände von mehr Buildern zu legen und diese Zukunft möglich zu machen.

# What you can count on (standards callout)
standards-section-label = Unsere Standards
standards-section-title = Worauf du dich verlassen kannst
standards-license-label = Lizenz
standards-license-headline = MIT / Apache 2.0
standards-license-body = Doppelt lizenziert und permissiv. Kein Copyleft und keine kommerziellen Einschränkungen.
standards-safety-label = Sicherheit
standards-safety-headline = #![forbid(unsafe_code)]
standards-safety-body = Unsere Crates enthalten null unsafe, vom Compiler erzwungen. Unsafe in Abhängigkeiten wird unter Miri auf UB geprüft und mit cargo-geiger auditiert.
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
start-daemon-code = apt install personal-rnsd
start-daemon-target = personal-rnsd

start-mobile-headline = Ich baue eine Mobile-App
start-mobile-body = Kotlin (.aar), Swift (.xcframework) oder Python (.whl) — dieselbe Engine, die dein Daemon nutzt, direkt in deine App eingebettet.
start-mobile-code = implementation("org.staypersonal:rns:0.1")
    pod 'PersonalRns', '~> 0.1'
start-mobile-target = personal-rns-ffi

start-game-headline = Ich liefere in einem Spiel aus
start-game-body = C# / .NET-Bindings für Unity, Godot und MonoGame. Multiplayer, ohne einen Server aufzusetzen.
start-game-code = dotnet add package Personal.Rns
start-game-target = personal-rns-ffi

start-embedded-headline = Ich ziele auf Mikrocontroller
start-embedded-body = Die Engine plus ein Host-Trait mit drei Methoden. ESP32-C6 ist die Referenz; S3, nRF, RP2040 und STM32 sind als Nächstes dran.
start-embedded-code = cargo add personal-rns --no-default-features
start-embedded-target = personal-rns + personal-hopspot

start-web-headline = Ich baue fürs Web oder Edge
start-web-body = Ein WebAssembly-Build, der im Browser und auf Edge-Runtimes wie Cloudflare Workers, Fastly und Spin läuft.
start-web-code = npm install personal-rns
start-web-target = personal-rns (wasm32)

start-rust-headline = Ich bette es in eine Rust-App ein
start-rust-body = Eine vollständige RNS-Runtime sofort einsatzbereit, oder der reine Kern, um deine eigene Runtime darum herum zu bauen.
start-rust-code = cargo add personal-rnsd   # complete RNS runtime
    cargo add personal-rns      # pure core only
start-rust-target = personal-rnsd or personal-rns

start-lxmf-headline = Ich will Nachrichten über das Mesh senden
start-lxmf-body = LXMF auf Reticulum — Identitäten, Adressen, Zustellung. Die Schicht, auf der Sideband und Nomadnet sitzen.
start-lxmf-code = cargo add personal-lxmf
start-lxmf-target = personal-lxmf

# Platforms ("Runs on") — hero marquee label + CTA, and the dedicated page
landing-platforms-label = Läuft auf
landing-platforms-cta = Alle ansehen →
platforms-title = Wo Prns läuft
platforms-lead = Eine Engine, viele Zuhause. Einige davon werden heute ausgeliefert; der Rest steht auf der Roadmap — der Nordstern, auf den wir hinarbeiten. Durchgezogene Chips laufen jetzt; gestrichelte kommen als Nächstes.
platforms-legend-shipping = Heute verfügbar
platforms-legend-roadmap = Roadmap

# Benchmarks page
benchmarks-kicker = Performance
benchmarks-title = Offen benchmarked
benchmarks-lead = Wir behandeln Performance als Zahl, nicht als Adjektiv. Jede Kennzahl hier kommt aus einem deterministischen Harness im Repo, gemessen auf echter Hardware und gegen die RNS-Referenz geprüft, wo der Vergleich fair ist. Die Zahlen landen, während sich die Suite stabilisiert; unten steht die Methodik, der sie standhalten.

# License signal (footer)
footer-license = Open Source. MIT / Apache 2.0.

# Contributing page
contributing-kicker = Die Messlatte
contributing-title = Mitwirken
contributing-lead = Wie du beiträgst — was wir wertschätzen, welchen Konventionen dein Code folgt und welchen Standard jede Änderung erfüllt. Für menschliche und automatisierte Beitragende gleichermaßen.

# Crates index
crates-kicker = Die Bausteine
crates-title = Wähle, was zu dem passt, was du baust.
crates-lead = Jede Crate ist so gebaut, dass sie für sich allein nützlich ist, auch wenn du den Rest nicht mitziehst. Die Engine ist das Substrat; alles andere stapelt sich darauf, und weitere Teile landen, während die Suite wächst.
crates-card-cta = Was sie tut →
crates-back = Alle Crates
crates-not-found = Keine Crate mit diesem Namen

# Per-crate cards (consumer-framed)
crate-rns-role = Die Engine
crate-rns-blurb = Bring Reticulum in jedes Rust-Projekt. Deterministisch, no_std, ohne Allocator; kein globaler Zustand, kein eingebautes I/O — bring deine eigene Uhr und Leitung mit.
crate-rnsd-role = Der Daemon
crate-rnsd-blurb = Ein Drop-in für rnsd, das überall läuft, wo Linux läuft. Derselbe Wire wie die RNS-Referenz; nutze ihn neben oder statt der Knoten, die du schon hast.
crate-lxmf-role = Messaging
crate-lxmf-blurb = LXMF auf Reticulum — die Schicht, auf der Sideband und Nomadnet sitzen. Identitäten, Adressen, Nachrichtenzustellung.
crate-ffi-role = Mobile- und Python-Bindings
crate-ffi-blurb = Eine einzige uniffi-Schnittstelle erzeugt Kotlin (.aar), Swift (.xcframework) und Python (.whl). Nutze Reticulum von Android, iOS oder einem Jupyter-Notebook aus — dieselbe Form, dieselbe Engine.

# 404
not-found-title = Hier ist noch nichts.
not-found-cta = Zurück zur Startseite
