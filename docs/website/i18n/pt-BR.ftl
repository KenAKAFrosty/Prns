# Navegação
nav-ethos = Filosofia
nav-crates = Crates
nav-api = API

# Rodapé
footer-tagline = Porte o contrato, não a implementação.

# Página inicial
landing-kicker = Reticulum, portado com fidelidade
landing-title = Um motor puro. Cada plataforma traz um host enxuto.
landing-subtitle = Um porte em Rust feito do zero do contrato de rede mesh do Reticulum — voltado primeiro para embarcados, determinístico, compatível com no_std, com uma pequena costura Host que permite ao mesmo motor rodar em um daemon, um microcontrolador e um celular.
landing-cta-ethos = Leia a filosofia
landing-cta-crates = Conheça os crates
landing-triumvirate-label = O triunvirato
landing-quote-label = Diretriz de construção
landing-quote-body = Porte o contrato, não a implementação. Construa um único motor puro e deixe cada plataforma trazer seu host enxuto.

# Cartões do triunvirato
triumvirate-rns-role = O motor puro
triumvirate-rns-blurb = Contrato de fio, roteamento, anúncios, enlaces — tick/ingest puros, sem I/O, sem std obrigatório.
triumvirate-rnsd-role = O host daemon
triumvirate-rnsd-blurb = Um adaptador Host enxuto baseado em std; o exemplo canônico de como uma plataforma dá vida ao motor.
triumvirate-lxmf-role = A camada de mensagens
triumvirate-lxmf-blurb = Camada de aplicação LXMF acima do motor — endereçamento, entrega e identidade para quem constrói apps.

# Página de filosofia
ethos-kicker = Como isso é construído
ethos-title = A diretriz de construção
ethos-lead = Esta é a filosofia de engenharia por trás de cada decisão da suíte. Leia uma vez para entender por que a arquitetura é assim — motor puro, host enxuto, contrato acima de implementação.

# Índice de crates
crates-kicker = A suíte
crates-title = Crates em um relance
crates-lead = Seis crates compõem a suíte. O motor é o substrato; todo o resto é um host enxuto ou um consumidor.
crates-card-cta = Leia mais →
crates-back = Voltar aos crates
crates-not-found = Não existe crate com esse nome

# Cartões por crate
crate-rns-role = O motor Reticulum puro
crate-rns-blurb = Contrato de fio e roteamento como uma máquina de estados pura. no_std + alloc, determinístico, embarcados primeiro.
crate-rnsd-role = O host daemon de referência
crate-rnsd-blurb = Adaptador Host baseado em std e binário daemon Linux. O exemplo canônico de como colocar o motor online.
crate-lxmf-role = A camada de aplicação LXMF
crate-lxmf-blurb = Endereçamento, entrega e identidade para quem constrói apps. Fica sobre personal-rns; consumido por Personal e outros.
crate-ffi-role = Bindings Kotlin / Swift / Python
crate-ffi-blurb = Um UDL, três linguagens via uniffi. A porta do SDK para quem está fora do Rust.
crate-rvt-role = Simulação multinó e ferramentas
crate-rvt-blurb = Reticulum Visual Toolkit. Hoje simulação multinó com relógio virtual; em breve depurador ao vivo. Em Dioxus e portável para a web.
crate-esp32c6-role = Adaptador Host para ESP32-C6
crate-esp32c6-blurb = Host no_std/no_main em bare metal. Prova de que o motor cabe em um microcontrolador de verdade.

# 404
not-found-title = Ainda não há nada aqui.
not-found-cta = Voltar ao início
