# Navegação
nav-contributing = Contribuir
nav-crates = Crates
nav-api = API

# Rodapé
footer-tagline = Criado pelo time Personal.

# Landing
landing-kicker = Redes mesh imparáveis para as pessoas
landing-kicker-prefix = Redes mesh imparáveis para as
landing-title = Um port de Reticulum (RNS) pronto para produção, escrito em Rust seguro.
landing-subtitle = Um núcleo determinístico, no_std e sem alocador. Construído para a performance e a estabilidade de que todo nó Reticulum precisa, de um microcontrolador de cinco dólares a um servidor na nuvem.
landing-cta-ethos = Escolha um crate
landing-cta-contributing = Contribuir

# Citação
landing-quote-label = O que estamos construindo
landing-quote-body = Reticulum é a infraestrutura de comunicação fundamental de um futuro brilhante que podemos ter, desde que todos nós o construamos. Este é o esforço do time Personal para colocar RNS nas mãos de mais builders e ajudar esse futuro a acontecer.

# Com o que você pode contar
standards-section-label = Nossos padrões
standards-section-title = Com o que você pode contar
standards-license-label = Licença
standards-license-headline = MIT / Apache 2.0
standards-license-body = Dupla licença permissiva. Sem copyleft ou restrições comerciais.
standards-safety-label = Segurança
standards-safety-headline = #![forbid(unsafe_code)]
standards-safety-body = Nossos crates contêm zero unsafe, imposto pelo compilador. O unsafe dentro das dependências é verificado contra UB no Miri e auditado com cargo-geiger.
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
start-daemon-code = apt install personal-rnsd
start-daemon-target = personal-rnsd

start-mobile-headline = Estou construindo um app mobile
start-mobile-body = Kotlin (.aar), Swift (.xcframework) ou Python (.whl) — o mesmo motor que roda no seu daemon, embutido direto no app.
start-mobile-code = implementation("org.staypersonal:rns:0.1")
    pod 'PersonalRns', '~> 0.1'
start-mobile-target = personal-rns-ffi

start-game-headline = Vou lançar dentro de um jogo
start-game-body = Bindings C# / .NET para Unity, Godot e MonoGame. Multiplayer sem subir um servidor.
start-game-code = dotnet add package Personal.Rns
start-game-target = personal-rns-ffi

start-embedded-headline = Estou mirando microcontroladores
start-embedded-body = O motor mais um trait Host de três métodos. ESP32-C6 é a referência; S3, nRF, RP2040 e STM32 vêm depois.
start-embedded-code = cargo add personal-rns --no-default-features
start-embedded-target = personal-rns + personal-hopspot

start-web-headline = Estou construindo para web ou edge
start-web-body = Um build WebAssembly que roda no navegador e em runtimes edge como Cloudflare Workers, Fastly e Spin.
start-web-code = npm install personal-rns
start-web-target = personal-rns (wasm32)

start-rust-headline = Vou embutir em um app Rust
start-rust-body = Um runtime RNS completo pronto para uso, ou o núcleo puro para construir seu próprio runtime em volta.
start-rust-code = cargo add personal-rnsd   # complete RNS runtime
    cargo add personal-rns      # pure core only
start-rust-target = personal-rnsd or personal-rns

start-lxmf-headline = Quero enviar mensagens pela mesh
start-lxmf-body = LXMF sobre Reticulum — identidades, endereços, entrega. A camada em que Sideband e Nomadnet se apoiam.
start-lxmf-code = cargo add personal-lxmf
start-lxmf-target = personal-lxmf

# Plataformas ("Runs on") — rótulo do marquee do hero + CTA e página dedicada
landing-platforms-label = Roda em
landing-platforms-cta = Ver tudo →
platforms-title = Onde o Prns roda
platforms-lead = Um motor, muitos lares. Alguns já são entregues hoje; o resto está no roadmap — a estrela-guia para onde estamos construindo. Chips sólidos rodam agora; os tracejados vêm depois.
platforms-legend-shipping = Disponível hoje
platforms-legend-roadmap = Roadmap

# Página de benchmarks
benchmarks-kicker = Performance
benchmarks-title = Benchmarks em aberto
benchmarks-lead = Tratamos performance como número, não como adjetivo. Cada valor aqui vem de um harness determinístico no repo, medido em hardware real e checado contra a referência RNS quando a comparação é justa. Os números chegam conforme a suite se estabiliza; abaixo está a metodologia que eles seguem.

# Sinal de licença (rodapé)
footer-license = Código aberto. MIT / Apache 2.0.

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
crate-rnsd-blurb = Um drop-in para rnsd que roda onde Linux roda. Mesmo wire da referência RNS; use junto ou no lugar dos nós que você já tem.
crate-lxmf-role = Mensageria
crate-lxmf-blurb = LXMF sobre Reticulum — a camada em que Sideband e Nomadnet se apoiam. Identidades, endereços, entrega de mensagens.
crate-ffi-role = Bindings mobile + Python
crate-ffi-blurb = Uma interface uniffi gera Kotlin (.aar), Swift (.xcframework) e Python (.whl). Use Reticulum do Android, iOS ou de um notebook Jupyter — mesmo formato, mesmo motor.

# 404
not-found-title = Ainda não há nada aqui.
not-found-cta = Voltar para o início
