# Navegação
nav-ethos = Design
nav-crates = Crates
nav-api = API

# Rodapé
footer-tagline = Construído pelo time Personal.

# Página inicial
landing-kicker = Redes mesh imparáveis — para as pessoas
landing-title = Uma portação de produção do Reticulum (RNS) escrita em Rust.
landing-subtitle = Um núcleo determinístico, sem std e sem alocador. Cobertura completa de RNS e LXMF. Bindings nativos para Kotlin, Swift, Python, TypeScript e C#. WebAssembly para navegadores e runtimes edge como Cloudflare Workers, Fastly e Spin. Pensado para o desempenho e a autonomia que qualquer stack Reticulum exige — de um microcontrolador de cinco dólares a um nó na nuvem. E inclui um substituto drop-in para o rnsd, já no pacote.
landing-cta-ethos = Escolha um crate
landing-cta-crates = Como construímos isto

# Citação
landing-quote-label = Para onde estamos indo
landing-quote-body = Reticulum é a infraestrutura de comunicação fundadora do futuro luminoso que podemos ter — se a gente quiser construir. Este é o nosso esforço para colocá-la nas mãos de mais desenvolvedores e ajudar a fazer esse futuro acontecer.

# Em que você pode contar
standards-section-label = Nossos padrões
standards-section-title = Em que você pode contar
standards-license-label = Licença
standards-license-headline = MIT / Apache 2.0
standards-license-body = Licença dupla e permissiva. Sem copyleft e sem restrições não comerciais.
standards-coverage-label = Cobertura
standards-coverage-headline = RNS e LXMF, completos
standards-coverage-body = Não só RNS. E LXMF não é coadjuvante. Os dois, por inteiro.
standards-core-label = Núcleo
standards-core-headline = no_std, sem alocador
standards-core-body = Um núcleo determinístico que roda onde alocadores não conseguem.
standards-verification-label = Verificação
standards-verification-headline = Diff-testado contra o RNS
standards-verification-body = Toda mudança é conferida contra a referência, e onde faz diferença mesmo, vêm junto provas formais.

# Por onde começo?
start-section-label = Caminhos de entrada
start-section-title = Por onde começo?
start-section-lead = Escolha o caminho que combina com o que você está construindo. Hoje cada um aterrissa em um único crate; os guias dedicados vêm logo atrás.

start-daemon-headline = Quero um nó Reticulum rodando
start-daemon-body = Daemon pronto pra usar. Drop-in para o rnsd. Coloque do lado dos nós que você já tem e deixe rodar junto.
start-daemon-code = apt install personal-rnsd
start-daemon-target = personal-rnsd

start-mobile-headline = Estou construindo um app mobile
start-mobile-body = Kotlin (.aar), Swift (.xcframework) ou Python (.whl) — o mesmo motor que roda no seu daemon, embarcado direto dentro do app.
start-mobile-code = implementation("org.staypersonal:rns:0.1")
    pod 'PersonalRns', '~> 0.1'
start-mobile-target = personal-rns-ffi

start-game-headline = Estou levando para dentro de um jogo
start-game-body = Bindings C#/.NET para Unity, Godot e MonoGame. Multiplayer sem precisar subir um servidor.
start-game-code = dotnet add package Personal.Rns
start-game-target = personal-rns-ffi

start-embedded-headline = Estou mirando microcontroladores
start-embedded-body = O motor mais uma trait Host com só três métodos. O ESP32-C6 é a referência; em seguida vêm S3, nRF, RP2040 e STM32.
start-embedded-code = cargo add personal-rns --no-default-features
start-embedded-target = personal-rns + hosts/*

start-web-headline = Construo para web ou edge
start-web-body = Um build WebAssembly que roda no navegador e em runtimes edge como Cloudflare Workers, Fastly e Spin.
start-web-code = npm install personal-rns
start-web-target = personal-rns (wasm32)

start-rust-headline = Embuto em um app Rust
start-rust-body = Um runtime RNS completo direto da caixa, ou o núcleo puro para você montar seu próprio runtime em volta. Escolha o que cabe.
start-rust-code = cargo add personal-rnsd   # complete RNS runtime
    cargo add personal-rns      # pure core only
start-rust-target = personal-rnsd or personal-rns

start-lxmf-headline = Quero enviar mensagens pela mesh
start-lxmf-body = LXMF em cima do Reticulum — identidades, endereços, entrega. A camada onde Sideband e Nomadnet se apoiam.
start-lxmf-code = cargo add personal-lxmf
start-lxmf-target = personal-lxmf

# Rodapé (licença)
footer-license = Código aberto. MIT / Apache 2.0.

# Página de filosofia
ethos-kicker = A disciplina
ethos-title = Como construímos isto
ethos-lead = Uma nota de engenheiro pra engenheiro sobre a disciplina por trás deste projeto — motor puro, núcleo sem alocador, cada mudança verificada contra a referência RNS. Dá uma passada antes de depender; queremos que você saiba no que está se metendo.

# Índice de crates
crates-kicker = As peças
crates-title = Pegue o que combina com o que você está construindo.
crates-lead = Cada crate é feito pra ser útil sozinho, mesmo se você não puxar o resto. O motor é o substrato; o resto empilha em cima, e mais peças vão chegando conforme a suíte cresce.
crates-card-cta = O que ele faz →
crates-back = Todos os crates
crates-not-found = Nenhum crate com esse nome

# Cards por crate
crate-rns-role = O motor
crate-rns-blurb = Encaixa o Reticulum em qualquer projeto Rust. Determinístico, no_std, sem alocador; sem estado global, sem I/O embutido — traga seu relógio e seu fio.
crate-rnsd-role = O daemon
crate-rnsd-blurb = Um drop-in para o rnsd que roda em qualquer lugar onde o Linux roda. Mesmo fio da referência RNS; use junto ou no lugar dos nós que você já tem.
crate-lxmf-role = Mensageria
crate-lxmf-blurb = LXMF em cima do Reticulum — a camada onde Sideband e Nomadnet se apoiam. Identidades, endereços, entrega de mensagens.
crate-ffi-role = Bindings mobile + Python
crate-ffi-blurb = Uma única interface uniffi gera Kotlin (.aar), Swift (.xcframework) e Python (.whl). Use o Reticulum do Android, iOS ou de um notebook Jupyter — mesma forma, mesmo motor.
crate-rvt-role = Depurador visual
crate-rvt-blurb = Veja pacotes se moverem entre nós simulados sobre um relógio virtual. Determinístico — mesmo cenário, mesmo trace, toda vez.
crate-esp32c6-role = Firmware do ESP32-C6
crate-esp32c6-blurb = Adaptador host bare-metal para o ESP32-C6. Sem SO, sem alocador — prova de que o motor roda num chip RISC-V de cinco dólares com rádios embutidos.

# 404
not-found-title = Aqui ainda não tem nada.
not-found-cta = Voltar para o início
