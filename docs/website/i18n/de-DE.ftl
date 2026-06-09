# Navigation
nav-ethos = Design
nav-crates = Crates
nav-api = API

# Fußzeile
footer-tagline = Gebaut vom Personal-Team.

# Startseite
landing-kicker = Unaufhaltsame Mesh-Netzwerke — für die Menschen
landing-title = Eine produktionsreife Portierung von Reticulum (RNS) in Rust.
landing-subtitle = Ein deterministischer Kern ohne std und ohne Allokator. Volle Abdeckung von RNS und LXMF. Gebaut mit Blick auf die Leistung und Akkulaufzeit, die jeder Reticulum-Stack braucht — vom Fünf-Dollar-Mikrocontroller bis zum Cloud-Knoten.
landing-cta-ethos = Crate aussuchen
landing-cta-crates = Wie wir es bauen

# Pull-Quote
landing-quote-label = Worauf wir hinarbeiten
landing-quote-body = Reticulum ist die grundlegende Kommunikationsinfrastruktur der hellen Zukunft, die wir haben können — wenn wir sie bauen. Das hier ist unser Beitrag, sie in die Hände von mehr Entwicklerinnen und Entwicklern zu legen und an dieser Zukunft mitzuwirken.

# Unsere Standards
standards-section-label = Unsere Standards
standards-section-title = Worauf du dich verlassen kannst
standards-license-label = Lizenz
standards-license-headline = MIT / Apache 2.0
standards-license-body = Doppellizenziert und freizügig. Kein Copyleft und keine nicht-kommerziellen Einschränkungen.
standards-coverage-label = Abdeckung
standards-coverage-headline = RNS und LXMF in voller Tiefe
standards-coverage-body = Nicht nur RNS. Und LXMF nicht als Beiwerk. Beides, ganz.
standards-core-label = Kern
standards-core-headline = no_std, ohne Allokator
standards-core-body = Ein deterministischer Kern, der dort läuft, wo Allokatoren es nicht tun.
standards-verification-label = Verifikation
standards-verification-headline = Diff-getestet gegen RNS
standards-verification-body = Jede Änderung wird gegen die Referenz geprüft, und an den Stellen, an denen es wirklich zählt, kommen formale Beweise dazu.

# Wo fange ich an?
start-section-label = Einstiege
start-section-title = Wo fange ich an?
start-section-lead = Wähle den Weg, der zu dem passt, was du gerade baust. Jeder führt heute zu einer einzelnen Crate — die passenden Guides ziehen parallel nach.

start-daemon-headline = Ich will einen Reticulum-Knoten am Laufen haben
start-daemon-body = Fertig gebauter Daemon. Drop-in für rnsd. Stell ihn neben die Knoten, die du schon hast, und lass sie zusammen laufen.
start-daemon-code = apt install personal-rnsd
start-daemon-target = personal-rnsd

start-mobile-headline = Ich baue eine Mobil-App
start-mobile-body = Kotlin (.aar), Swift (.xcframework) oder Python (.whl) — dieselbe Engine, die dein Daemon fährt, direkt in deine App eingebettet.
start-mobile-code = implementation("org.staypersonal:rns:0.1")
    pod 'PersonalRns', '~> 0.1'
start-mobile-target = personal-rns-ffi

start-game-headline = Ich baue es in ein Spiel ein
start-game-body = C#/.NET-Bindings für Unity, Godot und MonoGame. Multiplayer, ohne einen Server hochzuziehen.
start-game-code = dotnet add package Personal.Rns
start-game-target = personal-rns-ffi

start-embedded-headline = Ich ziele auf Mikrocontroller
start-embedded-body = Die Engine plus ein Host-Trait mit nur drei Methoden. ESP32-C6 ist die Referenz; S3, nRF, RP2040 und STM32 stehen als Nächstes an.
start-embedded-code = cargo add personal-rns --no-default-features
start-embedded-target = personal-rns + personal-hopspot

start-web-headline = Ich baue fürs Web oder den Edge
start-web-body = Ein WebAssembly-Build, der im Browser läuft und auf Edge-Runtimes wie Cloudflare Workers, Fastly und Spin.
start-web-code = npm install personal-rns
start-web-target = personal-rns (wasm32)

start-rust-headline = Ich binde es in eine Rust-App ein
start-rust-body = Eine vollständige RNS-Runtime ab Werk, oder der reine Kern, um deine eigene Runtime drumherum zu bauen. Such dir aus, was passt.
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
ethos-lead = Eine Notiz von Ingenieur zu Ingenieur über die Disziplin hinter dem Projekt — reine Engine, allokatorfreier Kern, jede Änderung gegen die RNS-Referenz verifiziert. Überflieg sie, bevor du dich darauf verlässt; wir wollen, dass du weißt, worauf du dich einlässt.

# Crates-Übersicht
crates-kicker = Die Bausteine
crates-title = Nimm das, was zu dem passt, was du baust.
crates-lead = Jede Crate ist so gebaut, dass sie auch alleine etwas taugt — selbst ohne den Rest. Die Engine ist das Substrat; alles andere stapelt sich darauf, und mit der Suite kommen weitere Bausteine dazu.
crates-card-cta = Was sie macht →
crates-back = Alle Crates
crates-not-found = Keine Crate mit diesem Namen

# Crate-Karten
crate-rns-role = Die Engine
crate-rns-blurb = Reticulum in jedes Rust-Projekt einsetzen. Deterministisch, no_std, ohne Allokator; kein globaler Zustand, kein eingebautes I/O — bring deine eigene Uhr und Leitung mit.
crate-rnsd-role = Der Daemon
crate-rnsd-blurb = Ein Drop-in für rnsd, der dort läuft, wo Linux läuft. Gleicher Draht wie die RNS-Referenz; nutze ihn neben oder anstelle der Knoten, die du schon hast.
crate-lxmf-role = Messaging
crate-lxmf-blurb = LXMF auf Reticulum — die Schicht, auf der Sideband und Nomadnet sitzen. Identitäten, Adressen, Nachrichtenzustellung.
crate-ffi-role = Mobile- und Python-Bindings
crate-ffi-blurb = Ein einziges uniffi-Interface erzeugt Kotlin (.aar), Swift (.xcframework) und Python (.whl). Nutze Reticulum aus Android, iOS oder einem Jupyter-Notebook — gleiche Form, gleiche Engine.

# 404
not-found-title = Hier ist noch nichts.
not-found-cta = Zurück zur Startseite
