# Navegación
nav-ethos = Diseño
nav-crates = Crates
nav-api = API

# Pie de página
footer-tagline = Hecho por el equipo de Personal.

# Página de inicio
landing-kicker = Redes mesh imparables, para la gente
landing-title = Una portación de Reticulum (RNS) escrita en Rust, lista para producción.
landing-subtitle = Un núcleo determinista, no_std y sin asignador. Cubre RNS y LXMF por completo. Bindings nativos para Kotlin, Swift, Python, TypeScript y C#. WebAssembly para navegadores y runtimes edge. Pensado para el rendimiento y la autonomía que cualquier stack de Reticulum necesita, desde un microcontrolador de cinco dólares hasta un nodo en la nube. Incluye un reemplazo drop-in para rnsd.
landing-cta-ethos = Elige un crate
landing-cta-crates = Cómo lo construimos

# Cita
landing-quote-label = Hacia lo que construimos
landing-quote-body = Reticulum es la infraestructura de comunicación fundacional del futuro luminoso que podemos tener, si lo hacemos realidad. Este es nuestro esfuerzo por ponerlo en manos de más desarrolladores y ayudar a hacer ese futuro posible.

# Con lo que puedes contar
standards-section-label = Nuestros estándares
standards-section-title = Con lo que puedes contar
standards-license-label = Licencia
standards-license-headline = MIT / Apache 2.0
standards-license-body = Doble licencia y permisiva. Sin copyleft, sin restricciones no comerciales.
standards-coverage-label = Cobertura
standards-coverage-headline = RNS y LXMF completos
standards-coverage-body = No solo RNS. No LXMF como añadido. Ambos, por completo.
standards-core-label = Núcleo
standards-core-headline = no_std, sin asignador
standards-core-body = Un núcleo determinista que corre donde los asignadores no llegan.
standards-verification-label = Verificación
standards-verification-headline = Comparado contra RNS
standards-verification-body = Cada cambio se contrasta con la referencia; pruebas formales donde importan.

# ¿Por dónde empiezo?
start-section-label = Caminos de entrada
start-section-title = ¿Por dónde empiezo?
start-section-lead = Elige el camino que coincide con lo que estás construyendo. Hoy cada uno apunta a un único crate; pronto irán llegando guías junto a ellos.

start-daemon-headline = Quiero un nodo Reticulum corriendo
start-daemon-body = Daemon listo para usar. Drop-in para rnsd. Ponlo junto a los nodos que ya tienes.
start-daemon-code = apt install personal-rnsd
start-daemon-target = personal-rnsd

start-mobile-headline = Estoy construyendo una app móvil
start-mobile-body = Kotlin (.aar), Swift (.xcframework) o Python (.whl) — el mismo motor que corre tu daemon, embebido directamente dentro de tu app.
start-mobile-code = implementation("org.staypersonal:rns:0.1")
    pod 'PersonalRns', '~> 0.1'
start-mobile-target = personal-rns-ffi

start-game-headline = Lo estoy integrando en un juego
start-game-body = Bindings C# / .NET para Unity, Godot y MonoGame. Multijugador sin levantar un servidor.
start-game-code = dotnet add package Personal.Rns
start-game-target = personal-rns-ffi

start-embedded-headline = Apunto a microcontroladores
start-embedded-body = El motor más un trait Host de tres métodos. El ESP32-C6 es la referencia; S3, nRF, RP2040 y STM32 vienen después.
start-embedded-code = cargo add personal-rns --no-default-features
start-embedded-target = personal-rns + hosts/*

start-web-headline = Construyo para web o edge
start-web-body = Una build WebAssembly que corre en el navegador y en runtimes edge como Cloudflare Workers, Fastly y Spin.
start-web-code = npm install personal-rns
start-web-target = personal-rns (wasm32)

start-rust-headline = Lo embebo en una app Rust
start-rust-body = Un runtime RNS completo listo para usar, o el núcleo puro para montar tu propio runtime alrededor.
start-rust-code = cargo add personal-rnsd   # complete RNS runtime
    cargo add personal-rns      # pure core only
start-rust-target = personal-rnsd or personal-rns

start-lxmf-headline = Quiero enviar mensajes por la mesh
start-lxmf-body = LXMF sobre Reticulum — identidades, direcciones, entrega. La capa sobre la que se apoyan Sideband y Nomadnet.
start-lxmf-code = cargo add personal-lxmf
start-lxmf-target = personal-lxmf

# Pie (licencia)
footer-license = Código abierto. MIT / Apache 2.0.

# Página de filosofía
ethos-kicker = La disciplina
ethos-title = Cómo lo construimos
ethos-lead = Una nota de ingeniero a ingeniero sobre la disciplina detrás de este proyecto — motor puro, núcleo sin asignador, cada cambio verificado contra la referencia de RNS. Échale un vistazo antes de depender de él; queremos que sepas en qué te estás metiendo.

# Índice de crates
crates-kicker = Las piezas
crates-title = Elige lo que coincide con lo que estás construyendo.
crates-lead = Cada crate está pensado para ser útil por sí mismo, aunque no traigas el resto. El motor es el sustrato; todo lo demás se apila encima, y más piezas irán llegando a medida que la suite crezca.
crates-card-cta = Qué hace →
crates-back = Todos los crates
crates-not-found = No existe un crate con ese nombre

# Tarjetas por crate
crate-rns-role = El motor
crate-rns-blurb = Mete Reticulum en cualquier proyecto Rust. Determinista, no_std, sin asignador; sin estado global, sin E/S incorporada — trae tu propio reloj y tu propio cable.
crate-rnsd-role = El daemon
crate-rnsd-blurb = Un drop-in para rnsd que corre donde corra Linux. Mismo hilo que la referencia de RNS; úsalo junto a o en lugar de los nodos que ya tienes.
crate-lxmf-role = Mensajería
crate-lxmf-blurb = LXMF sobre Reticulum — la capa sobre la que se apoyan Sideband y Nomadnet. Identidades, direcciones, entrega de mensajes.
crate-ffi-role = Bindings móviles y de Python
crate-ffi-blurb = Una sola interfaz uniffi genera Kotlin (.aar), Swift (.xcframework) y Python (.whl). Usa Reticulum desde Android, iOS o un notebook Jupyter — misma forma, mismo motor.
crate-rvt-role = Depurador visual
crate-rvt-blurb = Mira a los paquetes moverse entre nodos simulados sobre un reloj virtual. Determinista — mismo escenario, mismo trazo, cada vez.
crate-esp32c6-role = Firmware para ESP32-C6
crate-esp32c6-blurb = Adaptador host bare-metal para el ESP32-C6. Sin sistema operativo, sin asignador — la prueba de que el motor corre en un chip RISC-V de cinco dólares con radios integradas.

# 404
not-found-title = Aquí todavía no hay nada.
not-found-cta = Volver al inicio
