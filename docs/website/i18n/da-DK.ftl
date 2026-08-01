# Navigation
nav-contributing = Bidrag
nav-api = API

# Footer
footer-tagline = Bragt til dig af Personal-teamet.

# Landing
landing-kicker = Ustoppelige mesh-netværk for mennesker
landing-kicker-prefix = Ustoppelige mesh-netværk for
landing-title = En højtydende port af Reticulum (RNS) skrevet i sikker Rust.
landing-title-lead = A high-performance port of Reticulum (RNS).
landing-title-accent = Runs on any device.
landing-subtitle = En deterministisk, no_std, allokatorfri kerne. Bygget til den ydeevne og stabilitet, som enhver Reticulum-node har brug for, fra en mikrocontroller til fem dollars til en cloud-server.
landing-cta-ethos = Vælg en crate
# Pull quote
landing-quote-label = Det, vi bygger hen imod
landing-quote-body = Reticulum er den grundlæggende kommunikationsinfrastruktur for en lys fremtid, vi kan få, så længe vi alle bygger den. Dette er Personal-teamets indsats for at få RNS i hænderne på flere byggere og hjælpe den fremtid på vej.

# Interfaces
interfaces-section-label = Interfaces
interfaces-section-title = Hvor meshet møder verden
interfaces-section-lead = Prns bevarer de RNS-kompatible interfaces, buildere allerede kender, og udvider kortet med native links til nye enheder og netværk.
interfaces-section-hot-note = Prns-interfaces kan skiftes hot: tilføj, fjern eller ændr en interface uden en node-genstart.

interfaces-radio-label = Radioer
interfaces-radio-headline = Nærhedslinks til enheder og boards
interfaces-radio-body = Bluetooth LE Auto-interface, ESP-NOW og LoRa bringer nære enheder, board-flåder og langtrækkende links ind i ét RNS-mesh.

interfaces-lan-label = LAN
interfaces-lan-headline = Automatisk fundne local-link-peers
interfaces-lan-body = Wi-Fi Auto-interface bruger multicast, mDNS og gateway-rendezvous til at finde nære noder og folde et lokalt netværk ind i meshet.

interfaces-cable-label = Kabler + packet radio
interfaces-cable-headline = Kabler, TNC'er og radiomodems
interfaces-cable-body = USB Auto-interface, seriel framing, KISS, AX.25 og RNode forbinder små enheder og packet-radio-hardware til det samme mesh.

interfaces-host-label = Routet IP
interfaces-host-headline = Internet-, WAN- og backbone-links
interfaces-host-body = TCP client/server, UDP og Backbone lader fjerne peers deltage i meshet over private WANs, VPNs og relays på det offentlige internet.

# What you can count on (standards callout)
standards-section-label = Vores standarder
standards-section-title = Det kan du regne med
standards-license-label = Licens
standards-license-headline = MIT / Apache 2.0
standards-license-body = Dobbeltlicenseret og permissiv. Ingen copyleft eller kommercielle begrænsninger.
standards-safety-label = Sikkerhed
standards-safety-headline = Håndhævet, derefter auditeret
standards-safety-body = I motoren kompilerer panics, unwraps og ubegrundet unsafe aldrig. Hvad der ikke kan forbydes, auditeres: unsafe i afhængigheder med cargo-geiger, udefineret adfærd under Miri, sikkerhedsadvarsler med cargo-deny.
standards-correctness-label = Korrekthed
standards-correctness-headline = Diff-testet mod RNS
standards-correctness-body = Hver ændring tjekkes mod referencen og køres derefter gennem property-, fuzz- og mutationstests med Kani-beviser dér, hvor de betyder noget.
standards-benchmarked-label = Ydeevne
standards-benchmarked-headline = Målt, ikke bare påstået
standards-benchmarked-body = Ydeevnen følges åbent, målt af et harness du selv kan køre.
standards-benchmarked-cta = Se benchmarks →

# Where do I start? (use-case cards on landing)
start-section-label = Veje ind
start-section-title = Hvor starter jeg?
start-section-lead = Vælg den vej, der matcher det, du bygger. Hver vej lander på én crate i dag; flere guides kommer ved siden af dem.

start-daemon-headline = Jeg vil have en Reticulum-node kørende
start-daemon-body = Færdigbygget daemon. Drop-in for rnsd. Kør den ved siden af de noder, du allerede har.
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

start-web-headline = Brug browsernode-legepladsen
start-web-body = Prøv TypeScript-API'et med den fælles Rust-motor i WebAssembly, forbind via Auto Wi-Fi eller USB Auto, og følg lokal nodeaktivitet live.
start-web-code = WebAssembly-runtime
    Auto Wi-Fi + USB Auto
    TypeScript-eksempel
start-web-target = Åbn legepladsen

start-rust-headline = Byg på Reticulum
start-rust-body = Brug motoren og bindingerne til at føje mesh-netværk til apps, værktøjer, tjenester eller spil.
start-rust-target = Læs README-filen
start-rust-target-source = Hent kildekoden

# Platforms ("Runs on") — hero marquee label + CTA, and the dedicated page
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
benchmarks-kicker = Ydeevne
benchmarks-title = Benchmarket i det åbne
benchmarks-lead = Vi behandler ydeevne som et tal, ikke et adjektiv. Hver figur her kommer fra et deterministisk harness i repoet, målt på rigtig hardware og tjekket mod RNS-referencen, hvor sammenligningen er fair. Tallene lander, efterhånden som suiten stabiliseres; nedenfor er metoden, de skal leve op til.

# License signal (footer)
footer-license = Open source. MIT / Apache 2.0.
footer-trademarks = Tredjepartslogoer og varemærker tilhører deres respektive ejere. De vises kun for at identificere platforme, hardware og kompatibilitetsmål. Ingen godkendelse hævdes eller antydes.

# Contributing page
contributing-kicker = Standarden
contributing-title = Bidrag
contributing-lead = Sådan bidrager du — hvad vi værdsætter, de konventioner din kode følger, og den standard hver ændring skal klare. For både menneskelige og automatiserede bidragydere.

# 404
not-found-title = Her er der ikke noget endnu.
not-found-cta = Tilbage til forsiden
