# Navigasjon
nav-contributing = Bidra
nav-api = API-referanse

# Bunntekst
footer-tagline = Levert av KenAKAFrosty og Personal/Prns-teamet.
footer-flash = Flash en Hopspot (kun på engelsk)
footer-playground = Nettleser-lekeplass (kun på engelsk)

# Landing
landing-kicker = Mesh-nettverk som er ditt
landing-kicker-prefix = Mesh-nettverk som er
landing-title = Høyytelses Reticulum (RNS), bygget for å kjøre på enhver enhet.
landing-title-lead = Høyytelses Reticulum (RNS),
landing-title-accent = bygget for å kjøre på enhver enhet.
landing-subtitle = Bygget for ytelsen, stabiliteten og energieffektiviteten alle Reticulum-noder trenger, fra en femdollars mikrokontroller til en skyserverklynge. Én motor og ett API, det samme på embedded, desktop, mobil, i spill og på nettet.
landing-cta-ethos = Finn din vei i Prns
landing-cta-standards = Våre standarder
# Sitat
landing-quote-label = Det vi bygger mot
landing-quote-body = Reticulum er den grunnleggende kommunikasjonsinfrastrukturen for en lys fremtid vi kan få, så lenge vi alle bygger den. Dette er Personal-teamets innsats for å få RNS i hendene på flere byggere og hjelpe den fremtiden frem.

# Interfaces
interfaces-section-label = Interfaces
interfaces-section-title = Der meshet møter verden
interfaces-section-lead = Prns bevarer de RNS-kompatible interfacene byggere allerede kjenner, og utvider kartet med native lenker for nye enheter og nettverk.
interfaces-section-hot-note = Prns-interfaces er hot-swappable: legg til, fjern eller endre et interface uten node-omstart.

interfaces-radio-label = Radioer
interfaces-radio-headline = Nærhetslenker for enheter og kort
interfaces-radio-body = Bluetooth LE Auto-interface, ESP-NOW og LoRa bringer nære enheter, kortflåter og langtrekkende lenker inn i ett RNS-mesh.

interfaces-lan-label = LAN
interfaces-lan-headline = Automatisk oppdagede local-link-peers
interfaces-lan-body = Wi-Fi Auto-interface bruker multicast, mDNS og gateway-rendezvous til å finne nære noder og folde et lokalt nettverk inn i meshet.

interfaces-cable-label = Kabler + packet radio
interfaces-cable-headline = Kabler, TNC-er og radiomodemer
interfaces-cable-body = USB Auto-interface, seriell framing, KISS, AX.25 og RNode kobler små enheter og packet-radio-hardware inn i samme mesh.

interfaces-host-label = Rutet IP
interfaces-host-headline = Internet-, WAN- og backbone-lenker
interfaces-host-body = TCP-klient/server, UDP, WebSocket og Backbone lar fjerne peers delta i meshet over private WAN, VPN, releer på det åpne internettet og nettleserintegrasjoner.

# Det du kan stole på
standards-section-label = Våre standarder
standards-section-title = Det du kan stole på
standards-license-label = Lisens
standards-license-headline = MIT / Apache 2.0
standards-license-body = Dobbeltlisensiert og permissiv. Ingen copyleft eller kommersielle begrensninger.
standards-safety-label = Sikkerhet
standards-safety-headline = Håndhevet, deretter auditert
standards-safety-body = I motoren kompilerer panics, unwraps og ubegrunnet unsafe aldri. Det som ikke kan forbys, auditeres: unsafe i avhengigheter med cargo-geiger, udefinert atferd under Miri, sikkerhetsvarsler med cargo-deny.
standards-correctness-label = Korrekthet
standards-correctness-headline = Diff-testet mot RNS
standards-correctness-body = Hver endring sjekkes mot referansen og kjøres deretter gjennom unit-, property-, fuzz- og mutasjonstester, med Kani-bevis der de betyr noe.
standards-benchmarked-label = Ytelse
standards-benchmarked-headline = Målt, ikke bare påstått
standards-benchmarked-body = Ytelse følges åpent, målt av et harness du kan kjøre selv.
standards-benchmarked-cta = Se benchmarkene →

# Hvor begynner jeg?
start-section-label = Veier inn
start-section-title = Hva er du her for å gjøre?
start-section-lead = Velg veien som matcher hvordan Prns passer inn i arbeidet ditt: maskinvare du flasher, infrastruktur du kjører, eller programvare du bygger.

start-daemon-headline = Kjør en daemon
start-daemon-body = Installer en rask Reticulum-daemon for desktoper, LXMF-apper, backbone-VPS-er og mer.
start-daemon-code = Drop-in for standardapper
    Leser ~/.reticulum
    Live interface-endringer
    Innebygde metrikker
start-daemon-target = Kjør Prnsd

start-embedded-headline = Flash en Hopspot
start-embedded-body = Velg et støttet kort, sammenlign radio- og batteriavveininger, og flash så en dedikert mesh-enhet.
start-embedded-code = Kortmatrise
    Web-flasher
    Lokal flash
start-embedded-target = Flash en Hopspot (kun på engelsk)

start-web-headline = Bruk lekeplassen for nettlesernoder
start-web-body = Prøv TypeScript-API-et med den delte Rust-motoren i WebAssembly, koble til via Auto Wi-Fi eller USB Auto, og følg lokal nodeaktivitet direkte.
start-web-code = WebAssembly-kjøremiljø
    Auto Wi-Fi + USB Auto
    TypeScript-eksempel
start-web-target = Åpne lekeplassen (kun på engelsk)

start-rust-headline = Bygg på Reticulum
start-rust-body = Bruk motoren og bindingene til å legge til mesh-nettverk i apper, verktøy, tjenester eller spill.
start-rust-target = Les README-en
start-rust-target-source = Last ned kildekoden

# Plattformer ("Runs on") — hero marquee label + CTA og egen side
landing-platforms-label = Kjører på
landing-platforms-cta = Se alle →
platforms-title = Hvor Prns kjører
platforms-lead = Én motor, mange hjem. Denne hurtigvisningen skiller runtime-plattformstøtte fra støtte for spesifikke Hopspot-kort.
platforms-board-support-link = Se kortstøtte og bring-up →

# Flash en Hopspot-side
flash-back = Plattformer
flash-back-boards = Kort
flash-card-action = Flash

# Benchmark-side
benchmarks-kicker = Ytelse
benchmarks-title = Benchmarket i det åpne
benchmarks-lead = Vi behandler ytelse som et tall, ikke et adjektiv. Hver verdi her kommer fra et deterministisk harness i repoet, målt på ekte maskinvare og sjekket mot RNS-referansen der sammenligningen er rettferdig. Tallene lander etter hvert som suiten stabiliseres; nedenfor er metodikken de skal holde.

# Lisenssignal (bunntekst)
footer-license = Åpen kildekode. MIT / Apache 2.0.
footer-trademarks = Tredjepartslogoer og varemerker tilhører sine respektive eiere. De vises bare for å identifisere plattformer, maskinvare og kompatibilitetsmål. Ingen godkjenning hevdes eller antydes.

# Bidrag-side
contributing-kicker = Listen
contributing-title = Bidra
contributing-lead = Slik bidrar du — hva vi verdsetter, konvensjonene koden din følger, og standarden hver endring må klare. For både menneskelige og automatiserte bidragsytere.

# 404
not-found-title = Her er det ingenting ennå.
not-found-cta = Tilbake til forsiden
