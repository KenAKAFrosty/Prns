# Navigation
nav-ethos = Design
nav-crates = Crates
nav-api = API

# Fußzeile
footer-tagline = Bereitgestellt vom Personal-Team.

# Startseite
landing-kicker = Unaufhaltbare Mesh-Netzwerke, für die Menschen
landing-title = Eine produktionsreife Portierung von Reticulum (RNS) in Rust.
landing-subtitle = Ein deterministischer, no_std und allokator-freier Kern. Vollständige Abdeckung von RNS und LXMF. Native Bindings für Kotlin, Swift, Python, TypeScript und C#. WebAssembly für Browser und Edge-Runtimes. Gebaut für die Performance und Akkulaufzeit, die jeder Reticulum-Stack braucht — vom Fünf-Dollar-Mikrocontroller bis zum Cloud-Knoten. Enthält einen Drop-in-Ersatz für rnsd.
landing-cta-ethos = Crate aussuchen
landing-cta-crates = Wie wir es bauen

# Pull-Quote
landing-quote-label = Worauf wir hinarbeiten
landing-quote-body = Reticulum ist die grundlegende Kommunikationsinfrastruktur der hellen Zukunft, die wir haben können, wenn wir sie schaffen. Dieses Projekt ist unser Beitrag, sie in die Hände von mehr Entwicklerinnen und Entwicklern zu legen und dabei zu helfen, jene Zukunft zu verwirklichen.

# Unsere Standards
standards-section-label = Unsere Standards
standards-section-title = Worauf du dich verlassen kannst
standards-license-label = Lizenz
standards-license-headline = MIT / Apache 2.0
standards-license-body = Doppellizenziert und freizügig. Kein Copyleft, keine nicht-kommerziellen Einschränkungen.
standards-coverage-label = Abdeckung
standards-coverage-headline = RNS und LXMF vollständig
standards-coverage-body = Nicht nur RNS. LXMF nicht als Beiwerk. Beides, in voller Tiefe.
standards-core-label = Kern
standards-core-headline = no_std, allokator-frei
standards-core-body = Ein deterministischer Kern, der dort läuft, wo Allokatoren nicht können.
standards-verification-label = Verifikation
standards-verification-headline = Gegen RNS diff-getestet
standards-verification-body = Jede Änderung wird gegen die Referenz geprüft; formale Beweise dort, wo sie zählen.

# Wo fange ich an?
start-section-label = Einstiege
start-section-title = Wo fange ich an?
start-section-lead = Wähle den Pfad, der zu dem passt, was du baust. Jeder landet heute auf einem einzelnen Crate; Guides ziehen nach.

start-daemon-headline = Ich will einen Reticulum-Knoten laufen lassen
start-daemon-body = Fertig gebauter Daemon. Drop-in für rnsd. Lass ihn neben den Knoten laufen, die du schon hast.
start-daemon-code = apt install personal-rnsd
start-daemon-target = personal-rnsd

start-mobile-headline = Ich baue eine Mobil-App
start-mobile-body = Kotlin (.aar), Swift (.xcframework) oder Python (.whl) — dieselbe Engine wie dein Daemon, direkt in deine App eingebettet.
start-mobile-code = implementation("org.staypersonal:rns:0.1")
    pod 'PersonalRns', '~> 0.1'
start-mobile-target = personal-rns-ffi

start-game-headline = Ich baue es in ein Spiel ein
start-game-body = C# / .NET-Bindings für Unity, Godot und MonoGame. Multiplayer, ohne einen Server hochzuziehen.
start-game-code = dotnet add package Personal.Rns
start-game-target = personal-rns-ffi

start-embedded-headline = Ich ziele auf Mikrocontroller
start-embedded-body = Die Engine plus ein Host-Trait mit drei Methoden. ESP32-C6 ist die Referenz; S3, nRF, RP2040 und STM32 folgen.
start-embedded-code = cargo add personal-rns --no-default-features
start-embedded-target = personal-rns + hosts/*

start-web-headline = Ich baue fürs Web oder den Edge
start-web-body = Ein WebAssembly-Build, der im Browser läuft und auf Edge-Runtimes wie Cloudflare Workers, Fastly und Spin.
start-web-code = npm install personal-rns
start-web-target = personal-rns (wasm32)

start-rust-headline = Ich binde es in eine Rust-App ein
start-rust-body = Eine vollständige RNS-Runtime out of the box, oder den reinen Kern, um deine eigene Runtime drumherum zu bauen.
start-rust-code = cargo add personal-rnsd   # complete RNS runtime
    cargo add personal-rns      # pure core only
start-rust-target = personal-rnsd or personal-rns

start-lxmf-headline = Ich will Nachrichten über das Mesh schicken
start-lxmf-body = LXMF auf Reticulum — Identitäten, Adressen, Zustellung. Die Schicht, auf der Sideband und Nomadnet sitzen.
start-lxmf-code = cargo add personal-lxmf
start-lxmf-target = personal-lxmf

# Fußzeile (Lizenz)
footer-license = Open Source. MIT / Apache 2.0.

# Ethos-Seite
ethos-kicker = Die Disziplin
ethos-title = Wie wir es bauen
ethos-lead = Eine Notiz von Ingenieur zu Ingenieur über die Disziplin hinter dem Projekt — reine Engine, allokator-freier Kern, jede Änderung gegen die RNS-Referenz verifiziert. Überflieg sie, bevor du dich darauf verlässt; wir wollen, dass du weißt, worauf du dich einlässt.

# Crates-Übersicht
crates-kicker = Die Bausteine
crates-title = Wähle, was zu dem passt, was du baust.
crates-lead = Jedes Crate ist so gebaut, dass es allein nützlich ist — auch ohne den Rest. Die Engine ist das Substrat; alles andere stapelt sich darauf, und weitere Bausteine landen, wie die Suite wächst.
crates-card-cta = Was es macht →
crates-back = Alle Crates
crates-not-found = Kein Crate mit diesem Namen

# Crate-Karten
crate-rns-role = Die Engine
crate-rns-blurb = Reticulum in jedes Rust-Projekt einbauen. Deterministisch, no_std, allokator-frei; kein globaler Zustand, kein eingebauter I/O — bring deine eigene Uhr und Leitung mit.
crate-rnsd-role = Der Daemon
crate-rnsd-blurb = Ein Drop-in für rnsd, der überall läuft, wo Linux läuft. Gleicher Draht wie die RNS-Referenz; nutze ihn neben oder anstelle der Knoten, die du schon hast.
crate-lxmf-role = Messaging
crate-lxmf-blurb = LXMF auf Reticulum — die Schicht, auf der Sideband und Nomadnet sitzen. Identitäten, Adressen, Nachrichten-Zustellung.
crate-ffi-role = Mobile- und Python-Bindings
crate-ffi-blurb = Ein einziges uniffi-Interface erzeugt Kotlin (.aar), Swift (.xcframework) und Python (.whl). Nutze Reticulum aus Android, iOS oder einem Jupyter-Notebook — gleiche Form, gleiche Engine.
crate-rvt-role = Visueller Debugger
crate-rvt-blurb = Sieh Pakete zwischen simulierten Knoten auf einer virtuellen Uhr wandern. Deterministisch — gleiches Szenario, gleicher Trace, jedes Mal.
crate-esp32c6-role = ESP32-C6-Firmware
crate-esp32c6-blurb = Bare-Metal-Host-Adapter für den ESP32-C6. Kein OS, kein Allokator — Beweis, dass die Engine auf einem RISC-V-Chip für fünf Dollar mit eingebauten Radios läuft.

# 404
not-found-title = Hier ist noch nichts.
not-found-cta = Zurück zur Startseite
