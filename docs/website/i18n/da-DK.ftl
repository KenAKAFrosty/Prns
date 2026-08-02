# Navigation
nav-benchmarks = Benchmarks
nav-contributing = Bidrag
nav-api = API-reference

# Footer
footer-tagline = Bragt til dig af KenAKAFrosty og Personal/Prns-teamet.
footer-flash = Flash en Hopspot (kun på engelsk)
footer-playground = Browser-legeplads (kun på engelsk)

# Landing
landing-kicker = Mesh-netværk, der er dit
landing-kicker-prefix = Mesh-netværk, der er
landing-title = Højtydende Reticulum (RNS), bygget til at køre på enhver enhed.
landing-title-lead = Højtydende Reticulum (RNS),
landing-title-accent = bygget til at køre på enhver enhed.
landing-subtitle = Bygget til den ydeevne, stabilitet og energieffektivitet, enhver Reticulum-node har brug for, fra en mikrocontroller til fem dollars til en cloud-serverklynge. Én motor og ét API, det samme på embedded, desktop, mobil, i spil og på nettet.
landing-cta-ethos = Find din vej i Prns
landing-cta-standards = Vores standarder
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
interfaces-host-body = TCP client/server, UDP, WebSocket og Backbone lader fjerne peers deltage i meshet over private WANs, VPNs, relays på det offentlige internet og browserintegrationer.

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
standards-correctness-body = Hver ændring tjekkes mod referencen og køres derefter gennem unit-, property-, fuzz- og mutationstests med Kani-beviser dér, hvor de betyder noget.
standards-benchmarked-label = Ydeevne
standards-benchmarked-headline = Målt, ikke bare påstået
standards-benchmarked-body = Ydeevnen følges åbent, målt af et harness du selv kan køre.
standards-benchmarked-cta = Se benchmarks →

# Where do I start? (use-case cards on landing)
start-section-label = Veje ind
start-section-title = Hvad er du her for at gøre?
start-section-lead = Vælg den vej, der matcher, hvordan Prns passer ind i dit arbejde: hardware du flasher, infrastruktur du kører, eller software du bygger.

start-daemon-headline = Kør en daemon
start-daemon-body = Installer en hurtig Reticulum-daemon til desktops, LXMF-apps, backbone-VPS'er og mere.
start-daemon-code = Drop-in for standardapps
    Læser ~/.reticulum
    Live interface-ændringer
    Indbyggede metrikker
start-daemon-target = Kør Prnsd

start-embedded-headline = Flash en Hopspot
start-embedded-body = Vælg et understøttet board, sammenlign radio- og batteriafvejninger, og flash så en dedikeret mesh-enhed.
start-embedded-code = Board-matrix
    Web-flasher
    Lokal flash
start-embedded-target = Flash en Hopspot (kun på engelsk)

start-web-headline = Brug browsernode-legepladsen
start-web-body = Prøv TypeScript-API'et med den fælles Rust-motor i WebAssembly, forbind via Auto Wi-Fi eller USB Auto, og følg lokal nodeaktivitet live.
start-web-code = WebAssembly-runtime
    Auto Wi-Fi + USB Auto
    TypeScript-eksempel
start-web-target = Åbn legepladsen (kun på engelsk)

start-rust-headline = Byg på Reticulum
start-rust-body = Brug motoren og bindingerne til at føje mesh-netværk til apps, værktøjer, tjenester eller spil.
start-rust-target = Læs README-filen
start-rust-target-source = Hent kildekoden

# Platforms ("Runs on") — hero marquee label + CTA, and the dedicated page
landing-platforms-label = Kører på
landing-platforms-cta = Se alle →
platforms-title = Hvor Prns kører
platforms-lead = Én motor, mange hjem. Dette hurtige overblik adskiller runtime-platformsupport fra support til specifikke Hopspot-boards.
platforms-board-support-link = Se board-support & bring-up →

# Flash en Hopspot-side
flash-back = Platforme
flash-back-boards = Boards
flash-card-action = Flash

# Benchmarks page
benchmarks-kicker = Ydeevne
benchmarks-title = Benchmarket i det åbne
benchmarks-lead = Hvert tal nedenfor kommer fra de offentliggjorte resultater i repoet, målt på rigtig hardware af et harness, du selv kan køre. Herfra er indholdet indtil videre kun på engelsk.

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
