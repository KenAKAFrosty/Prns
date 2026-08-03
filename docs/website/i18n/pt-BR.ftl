# Navegação
nav-benchmarks = Benchmarks
nav-contributing = Contribuir
site-early-english-note = O Prns está no começo: a documentação completa vive no GitHub e no código-fonte, e por enquanto está apenas em inglês.

# Rodapé
footer-tagline = Criado por KenAKAFrosty e pelo time Personal/Prns.
footer-flash = Flashear um Hopspot (somente em inglês)
footer-playground = Playground do navegador (somente em inglês)

# Landing
landing-kicker = Redes mesh que são suas
landing-kicker-prefix = Redes mesh que são
landing-title = Reticulum (RNS) de alta performance, feito para rodar em qualquer dispositivo.
landing-title-lead = Reticulum (RNS) de alta performance,
landing-title-accent = feito para rodar em qualquer dispositivo.
landing-subtitle = Feito para a performance, a estabilidade e a eficiência energética de que todo nó Reticulum precisa, de um microcontrolador de 5 dólares a um cluster de servidores na nuvem. Um só motor e uma só API, iguais em embarcados, desktop, mobile, jogos e web.
landing-cta-ethos = Encontre seu caminho no Prns
landing-cta-standards = Nossos padrões
# Citação
landing-quote-label = O que queremos construir
landing-quote-body = Reticulum é a infraestrutura de comunicação fundamental de um futuro brilhante que podemos ter, desde que todos nós o construamos. Este é o esforço do time Personal para colocar RNS nas mãos de mais builders e ajudar esse futuro a acontecer.

# Interfaces
interfaces-section-label = Interfaces
interfaces-section-title = Onde a mesh encontra o mundo
interfaces-section-lead = O Prns preserva as interfaces compatíveis com RNS que os builders já conhecem e expande o mapa com links nativos para novos dispositivos e redes.
interfaces-section-hot-note = As interfaces do Prns são hot-swappable: adicione, remova ou altere uma interface sem reiniciar o nó.

interfaces-radio-label = Rádios
interfaces-radio-headline = Links de proximidade para dispositivos e placas
interfaces-radio-body = Bluetooth LE Auto-interface, ESP-NOW e LoRa trazem dispositivos próximos, frotas de placas e links RF de longo alcance para uma mesma mesh Reticulum.

interfaces-lan-label = LAN
interfaces-lan-headline = Pares de link local descobertos automaticamente
interfaces-lan-body = Wi-Fi Auto-interface usa multicast, mDNS e rendezvous de gateway para encontrar nós próximos e trazer uma rede local para dentro da mesh.

interfaces-cable-label = Cabos + rádio pacote
interfaces-cable-headline = Cabos, TNCs e modems de rádio
interfaces-cable-body = USB Auto-interface, framing serial, KISS, AX.25 e RNode conectam dispositivos pequenos e hardware de rádio pacote à mesma mesh.

interfaces-host-label = IP roteado
interfaces-host-headline = Internet, WAN e links backbone
interfaces-host-body = TCP cliente/servidor, UDP, WebSocket e Backbone permitem que pares distantes participem da mesh por WANs privadas, VPNs, relays na internet pública e integrações no navegador.

# Com o que você pode contar
standards-section-label = Nossos padrões
standards-section-title = Com o que você pode contar
standards-license-label = Licença
standards-license-headline = MIT / Apache 2.0
standards-license-body = Dupla licença permissiva. Sem copyleft ou restrições comerciais.
standards-safety-label = Segurança
standards-safety-headline = Imposta, depois auditada
standards-safety-body = No motor, panics, unwraps e unsafe sem justificativa nunca compilam. O que não pode ser proibido é auditado: o unsafe das dependências com cargo-geiger, comportamento indefinido no Miri, avisos de segurança com cargo-deny.
standards-correctness-label = Correção
standards-correctness-headline = Diff-testado contra RNS
standards-correctness-body = Cada mudança é checada contra a referência e depois passa por testes unitários, de propriedades, fuzz e mutação, com provas Kani onde importam.
standards-benchmarked-label = Performance
standards-benchmarked-headline = Medida, não só afirmada
standards-benchmarked-body = A performance é acompanhada às claras, medida por um harness que você pode executar por conta própria.
standards-benchmarked-cta = Ver benchmarks →

# Por onde eu começo?
start-section-label = Caminhos de entrada
start-section-title = O que você veio fazer?
start-section-lead = Escolha o caminho que combina com o jeito que o Prns entra no seu trabalho: hardware que você flasheia, infraestrutura que você roda ou software que você constrói.

start-daemon-headline = Rode um daemon
start-daemon-body = Instale um daemon Reticulum rápido para desktops, apps LXMF, VPSs de backbone e mais.
start-daemon-code = Drop-in para apps padrão
    Lê ~/.reticulum
    Edição de interfaces a quente
    Métricas embutidas
start-daemon-target = Rodar o Prnsd

start-embedded-headline = Flasheie um Hopspot
start-embedded-body = Escolha uma placa suportada, flasheie direto do navegador e tenha um dispositivo mesh dedicado em minutos.
start-embedded-code = Matriz de placas
    Flasher web
    Flash local
start-embedded-target = Flashear um Hopspot (somente em inglês)

start-web-headline = Use o playground do nó no navegador
start-web-body = Experimente a API TypeScript com o motor Rust compartilhado em WebAssembly, conecte-se por Auto Wi-Fi ou USB Auto e acompanhe a atividade local do nó em tempo real.
start-web-code = Runtime WebAssembly
    Auto Wi-Fi + USB Auto
    Exemplo TypeScript
start-web-target = Abrir playground (somente em inglês)

start-rust-headline = Construa sobre o Reticulum
start-rust-body = Use o motor e os bindings para adicionar redes mesh a apps, ferramentas, serviços ou jogos.
start-rust-target = Ler o README
start-rust-target-source = Baixar o código-fonte

# Plataformas ("Runs on") — rótulo do marquee do hero + CTA e página dedicada
landing-platforms-label = Roda em
landing-platforms-cta = Ver tudo →
platforms-title = Onde o Prns roda
platforms-lead = Um motor, muitos lares. Esta visão rápida separa o suporte de plataformas de runtime do suporte a placas Hopspot específicas.
platforms-board-support-link = Ver suporte de placas Hopspot e bring-up →

# Página de Flashear um Hopspot
flash-back = Plataformas
flash-back-boards = Placas
flash-card-action = Flashear

# Página de benchmarks
benchmarks-kicker = Performance
benchmarks-title = Benchmarks às claras
benchmarks-lead = Cada número abaixo vem dos resultados publicados no repo, medidos em hardware real por um harness que você pode executar por conta própria. Daqui em diante, o conteúdo está disponível apenas em inglês por enquanto.

# Sinal de licença (rodapé)
footer-license = Código aberto. MIT / Apache 2.0.
footer-trademarks = Logos, marcas e imagens de produtos de terceiros pertencem aos seus respectivos proprietários. Eles são exibidos apenas para identificar plataformas, hardware e alvos de compatibilidade. Nenhum endosso é reivindicado ou implícito.

# 404
not-found-title = Ainda não há nada aqui.
not-found-cta = Voltar para o início
