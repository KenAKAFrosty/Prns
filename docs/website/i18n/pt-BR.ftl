# Navegação
nav-contributing = Contribuir
nav-crates = Crates
nav-api = API

# Rodapé
footer-tagline = Criado pelo time Personal.

# Landing
landing-kicker = Redes mesh imparáveis para as pessoas
landing-kicker-prefix = Redes mesh imparáveis para as
landing-title = Um port de alto desempenho de Reticulum (RNS), escrito em Rust seguro.
landing-title-lead = A high-performance port of Reticulum (RNS).
landing-title-accent = Runs on any device.
landing-subtitle = Um núcleo determinístico, no_std e sem alocador. Construído para a performance e a estabilidade de que todo nó Reticulum precisa, de um microcontrolador de cinco dólares a um servidor na nuvem.
landing-cta-ethos = Escolha um crate
landing-cta-contributing = Contribuir

# Citação
landing-quote-label = O que estamos construindo
landing-quote-body = Reticulum é a infraestrutura de comunicação fundamental de um futuro brilhante que podemos ter, desde que todos nós o construamos. Este é o esforço do time Personal para colocar RNS nas mãos de mais builders e ajudar esse futuro a acontecer.

# Interfaces
interfaces-section-label = Interfaces
interfaces-section-title = Onde a malha encontra o mundo
interfaces-section-lead = Prns preserva as interfaces compatíveis com RNS que builders já conhecem e expande o mapa com links nativos para novos dispositivos e redes.
interfaces-section-hot-note = As interfaces do Prns são hot-swappable: adicione, remova ou altere uma interface sem reiniciar o nó.

interfaces-radio-label = Rádios
interfaces-radio-headline = Links de proximidade para dispositivos e placas
interfaces-radio-body = BLE Auto-interface, ESP-NOW e LoRa trazem dispositivos próximos, frotas de placas e links de longo alcance para uma mesma malha RNS.

interfaces-lan-label = LAN
interfaces-lan-headline = Pares de link local descobertos automaticamente
interfaces-lan-body = Wi-Fi Auto-interface usa multicast, mDNS e rendezvous de gateway para encontrar nós próximos e dobrar uma rede local para dentro da malha.

interfaces-cable-label = Cabos + rádio pacote
interfaces-cable-headline = Cabos, TNCs e modems de rádio
interfaces-cable-body = USB Auto-interface, framing serial, KISS, AX.25 e RNode conectam dispositivos pequenos e hardware de rádio pacote à mesma malha.

interfaces-host-label = IP roteado
interfaces-host-headline = Internet, WAN e links backbone
interfaces-host-body = TCP cliente/servidor, UDP e Backbone permitem que pares distantes participem da malha por WANs privadas, VPNs e relays na internet pública.

# Com o que você pode contar
standards-section-label = Nossos padrões
standards-section-title = Com o que você pode contar
standards-license-label = Licença
standards-license-headline = MIT / Apache 2.0
standards-license-body = Dupla licença permissiva. Sem copyleft ou restrições comerciais.
standards-safety-label = Segurança
standards-safety-headline = #![forbid(unsafe_code)]
standards-safety-body = O motor personal-rns contém zero unsafe, imposto pelo compilador. O unsafe dentro das dependências é verificado contra UB no Miri e auditado com cargo-geiger.
standards-correctness-label = Correção
standards-correctness-headline = Diff-testado contra RNS
standards-correctness-body = Cada mudança é checada contra a referência e depois passa por testes de propriedades, fuzz e mutação, com provas Kani onde importam.
standards-benchmarked-label = Performance
standards-benchmarked-headline = Medida, não só afirmada
standards-benchmarked-body = A performance é acompanhada em aberto, medida por um harness que você pode executar por conta própria.
standards-benchmarked-cta = Ver benchmarks →

# Por onde eu começo?
start-section-label = Caminhos de entrada
start-section-title = Por onde eu começo?
start-section-lead = Escolha o caminho que combina com o que você está construindo. Hoje cada um leva a um único crate; mais guias chegarão junto deles.

start-daemon-headline = Quero um nó Reticulum rodando
start-daemon-body = Daemon pré-compilado. Drop-in para rnsd. Rode junto dos nós que você já tem.
start-daemon-code = apt install prnsd
start-daemon-target = prnsd

start-mobile-headline = Estou construindo um app mobile
start-mobile-body = Kotlin (.aar), Swift (.xcframework) ou Python (.whl) — o mesmo motor que roda no seu daemon, embutido direto no app.
start-mobile-code = implementation("org.staypersonal:rns:0.1")
    pod 'PersonalRns', '~> 0.1'
start-mobile-target = prns-ffi

start-game-headline = Vou lançar dentro de um jogo
start-game-body = Bindings C# / .NET para Unity, Godot e MonoGame. Multiplayer sem subir um servidor.
start-game-code = dotnet add package Personal.Rns
start-game-target = prns-ffi

start-embedded-headline = Flash a Hopspot
start-embedded-body = Pick a supported board, compare radio and battery tradeoffs, then flash a dedicated mesh device.
start-embedded-code = Board matrix
    Web flasher
    Local flash
start-embedded-target = Flash a Hopspot

start-web-headline = Estou construindo para web ou edge
start-web-body = Um build WebAssembly que roda no navegador e em runtimes edge como Cloudflare Workers, Fastly e Spin.
start-web-code = npm install personal-rns
start-web-target = personal-rns (wasm32)

start-rust-headline = Vou embutir em um app Rust
start-rust-body = Um runtime RNS completo pronto para uso, ou o núcleo puro para construir seu próprio runtime em volta.
start-rust-code = cargo add prnsd   # complete RNS runtime
    cargo add personal-rns      # pure core only
start-rust-target = prnsd or personal-rns

start-lxmf-headline = Quero enviar mensagens pela mesh
start-lxmf-body = LXMF sobre Reticulum — identidades, endereços, entrega. A camada em que Sideband e Nomadnet se apoiam.
start-lxmf-code = cargo add personal-lxmf
start-lxmf-target = personal-lxmf

# Plataformas ("Runs on") — rótulo do marquee do hero + CTA e página dedicada
landing-platforms-label = Runs on
landing-platforms-cta = See all →
platforms-title = Where Prns runs
platforms-lead = One engine, many homes. This quick view separates runtime platform support from specific Hopspot board support.
platforms-legend-runtime = Runtime platform
platforms-legend-bringup = Active bring-up
platforms-legend-roadmap = Roadmap
platforms-runtime-title = Runtime support quick view
platforms-runtime-lead = Microcontrollers list silicon and radio families here; exact boards, flashing readiness, and interfaces live in the board catalog.
platforms-board-support-link = Specific board support →

# Flash a Hopspot page
flash-back = Platforms
flash-back-boards = Boards
flash-kicker = Supported boards
flash-title = Flash a Hopspot
flash-lead = Pick a specific board, compare radio and battery tradeoffs, then flash or build the dedicated Hopspot firmware path.
flash-note = Hosted builds can download firmware artifacts directly. When this same docs site is served from a Hopspot, artifact actions should stay disabled and point back to the online flasher or local build path.
flash-board-title = Select a board
flash-board-lead = Choose a flashable target to load its board-specific flasher. Bring-up and roadmap boards stay visible here, but cannot be selected yet.
flash-picker-change-title = Change board
flash-interfaces-label = Eligible interfaces
flash-interfaces-pending = Interfaces pending board bring-up
flash-card-action = Flash
flash-card-selected = Selected
flash-ready-kicker = Ready target
flash-ready-title = Web flashing
flash-ready-action = Connect and flash
flash-ready-action-pending = Firmware artifacts are not wired into this build yet.
flash-local-title = Local build
flash-local-body = Fully offline? Build this repo locally and flash the board-specific Hopspot target from a developer machine.
flash-unavailable-title = Not flashable yet
flash-unavailable-body = This target is listed for bring-up or roadmap tracking, but it does not have a public web-flash artifact yet.
flash-missing-title = Board not found
flash-missing-body = Pick a supported board from the catalog.

# Página de benchmarks
benchmarks-kicker = Performance
benchmarks-title = Benchmarks em aberto
benchmarks-lead = Tratamos performance como número, não como adjetivo. Cada valor aqui vem de um harness determinístico no repo, medido em hardware real e checado contra a referência RNS quando a comparação é justa. Os números chegam conforme a suite se estabiliza; abaixo está a metodologia que eles seguem.

# Sinal de licença (rodapé)
footer-license = Código aberto. MIT / Apache 2.0.
footer-trademarks = Logos e marcas de terceiros pertencem aos seus respectivos proprietários. Eles são exibidos apenas para identificar plataformas, hardware e alvos de compatibilidade. Nenhum endosso é reivindicado ou implícito.

# Página de contribuição
contributing-kicker = O padrão
contributing-title = Contribuir
contributing-lead = Como contribuir — o que valorizamos, as convenções que seu código segue e o padrão que cada mudança precisa cumprir. Para contribuidores humanos e automatizados do mesmo jeito.

# Índice de crates
crates-kicker = As peças
crates-title = Escolha o que combina com o que você está construindo.
crates-lead = Cada crate foi feito para ser útil sozinho, mesmo que você não traga o resto junto. O motor é o substrato; todo o resto se empilha em cima, e mais peças chegam conforme a suite cresce.
crates-card-cta = O que faz →
crates-back = Todos os crates
crates-not-found = Nenhum crate com esse nome

# Cards por crate
crate-rns-role = O motor
crate-rns-blurb = Coloque Reticulum em qualquer projeto Rust. Determinístico, no_std, sem alocador; sem estado global, sem I/O embutido — traga seu próprio clock e wire.
crate-rnsd-role = O daemon
crate-rnsd-blurb = Um drop-in para rnsd no macOS, Linux e Windows. Mesmo wire da referência RNS; use junto ou no lugar dos nós que você já tem.
crate-lxmf-role = Mensageria
crate-lxmf-blurb = LXMF sobre Reticulum — a camada em que Sideband e Nomadnet se apoiam. Identidades, endereços, entrega de mensagens.
crate-ffi-role = Bindings mobile + Python
crate-ffi-blurb = Uma interface uniffi gera Kotlin (.aar), Swift (.xcframework) e Python (.whl). Use Reticulum do Android, iOS ou de um notebook Jupyter — mesmo formato, mesmo motor.

# 404
not-found-title = Ainda não há nada aqui.
not-found-cta = Voltar para o início
