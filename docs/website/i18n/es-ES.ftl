# Navegación
nav-benchmarks = Benchmarks
nav-contributing = Contribuir
site-early-english-note = Prns está en una etapa temprana: la documentación completa vive en GitHub y en el código fuente, y por ahora solo está en inglés.

# Pie de página
footer-tagline = Hecho por KenAKAFrosty y el equipo de Personal/Prns.
footer-flash = Flashear un Hopspot (solo en inglés)
footer-playground = Playground del navegador (solo en inglés)

# Página de inicio
landing-kicker = Redes mesh que son tuyas
landing-kicker-prefix = Redes mesh que son
landing-title = Reticulum (RNS) de alto rendimiento, construido para funcionar en cualquier dispositivo.
landing-title-lead = Reticulum (RNS) de alto rendimiento,
landing-title-accent = construido para funcionar en cualquier dispositivo.
landing-subtitle = Construido para el rendimiento, la estabilidad y la eficiencia energética que todo nodo Reticulum necesita, desde un microcontrolador de 5 dólares hasta un clúster de servidores en la nube. Un solo motor y una sola API, iguales en embebido, escritorio, móvil, juegos y web.
landing-cta-ethos = Encuentra tu camino en Prns
landing-cta-standards = Nuestros estándares
# Cita
landing-quote-label = Lo que queremos construir
landing-quote-body = Reticulum es la infraestructura de comunicación fundacional de un futuro luminoso que podemos tener, siempre que lo construyamos entre todos. Este es el esfuerzo del equipo de Personal por poner RNS en manos de más builders y ayudar a hacer realidad ese futuro.

# Interfaces
interfaces-section-label = Interfaces
interfaces-section-title = Donde la mesh se encuentra con el mundo
interfaces-section-lead = Prns conserva las interfaces compatibles con RNS que los builders ya conocen y amplía el mapa con enlaces nativos para nuevos dispositivos y redes.
interfaces-section-hot-note = Las interfaces de Prns son hot-swappable: añade, elimina o cambia una interfaz sin reiniciar el nodo.

interfaces-radio-label = Radios
interfaces-radio-headline = Enlaces de proximidad para dispositivos y placas
interfaces-radio-body = Bluetooth LE Auto-interface, ESP-NOW y LoRa llevan dispositivos cercanos, flotas de placas y enlaces RF de largo alcance a una misma mesh Reticulum.

interfaces-lan-label = LAN
interfaces-lan-headline = Pares de enlace local descubiertos automáticamente
interfaces-lan-body = Wi-Fi Auto-interface usa multicast, mDNS y rendezvous de gateway para encontrar nodos cercanos e integrar una red local en la mesh.

interfaces-cable-label = Cables + radio por paquetes
interfaces-cable-headline = Cables, TNC y módems de radio
interfaces-cable-body = USB Auto-interface, framing serie, KISS, AX.25 y RNode conectan dispositivos pequeños y hardware de radio por paquetes a la misma mesh.

interfaces-host-label = IP enrutada
interfaces-host-headline = Internet, WAN y enlaces backbone
interfaces-host-body = TCP cliente/servidor, UDP, WebSocket y Backbone permiten que peers distantes participen en la mesh a través de WAN privadas, VPN, relays en Internet público e integraciones en el navegador.

# Con lo que puedes contar
standards-section-label = Nuestros estándares
standards-section-title = Con lo que puedes contar
standards-license-label = Licencia
standards-license-headline = MIT / Apache 2.0
standards-license-body = Doble licencia permisiva. Sin copyleft ni restricciones comerciales.
standards-safety-label = Seguridad
standards-safety-headline = Impuesta, luego auditada
standards-safety-body = En el motor, los panics, los unwraps y el unsafe sin justificar nunca compilan. Lo que no se puede prohibir se audita: el unsafe de las dependencias con cargo-geiger, el comportamiento indefinido con Miri, los avisos de seguridad con cargo-deny.
standards-correctness-label = Corrección
standards-correctness-headline = Diff-testado contra RNS
standards-correctness-body = Cada cambio se contrasta con la referencia y luego pasa por pruebas unitarias, de propiedades, fuzzing y mutación, con pruebas Kani donde importan.
standards-benchmarked-label = Rendimiento
standards-benchmarked-headline = Medido, no solo afirmado
standards-benchmarked-body = El rendimiento se sigue en abierto, medido por un harness que puedes ejecutar tú mismo.
standards-benchmarked-cta = Ver benchmarks →

# ¿Por dónde empiezo?
start-section-label = Caminos de entrada
start-section-title = ¿Qué vienes a hacer?
start-section-lead = Elige el camino que encaje con cómo Prns entra en tu trabajo: hardware que flasheas, infraestructura que operas o software que construyes.

start-daemon-headline = Ejecuta un daemon
start-daemon-body = Instala un daemon Reticulum rápido para escritorios, apps LXMF, VPS de backbone y más.
start-daemon-code = Drop-in para apps estándar
    Lee ~/.reticulum
    Edición de interfaces en caliente
    Métricas integradas
start-daemon-target = Ejecutar Prnsd

start-embedded-headline = Flashea un Hopspot
start-embedded-body = Elige una placa compatible, compara los compromisos de radio y batería y flashea un dispositivo mesh dedicado.
start-embedded-code = Matriz de placas
    Flasher web
    Flasheo local
start-embedded-target = Flashear un Hopspot (solo en inglés)

start-web-headline = Usa el playground del nodo en el navegador
start-web-body = Prueba la API de TypeScript con el motor Rust compartido en WebAssembly, conéctate mediante Auto Wi-Fi o USB Auto y observa la actividad local del nodo en tiempo real.
start-web-code = Runtime WebAssembly
    Auto Wi-Fi + USB Auto
    Ejemplo TypeScript
start-web-target = Abrir playground (solo en inglés)

start-rust-headline = Construye sobre Reticulum
start-rust-body = Usa el motor y los bindings para añadir redes mesh a apps, herramientas, servicios o juegos.
start-rust-target = Leer el README
start-rust-target-source = Descargar el código fuente

# Plataformas ("Runs on") — etiqueta del marquee del hero + CTA y página dedicada
landing-platforms-label = Funciona en
landing-platforms-cta = Ver todo →
platforms-title = Dónde funciona Prns
platforms-lead = Un motor, muchos hogares. Esta vista rápida separa el soporte de plataformas de runtime del soporte de placas Hopspot concretas.
platforms-board-support-link = Ver soporte de placas y bring-up →

# Página de Flashear un Hopspot
flash-back = Plataformas
flash-back-boards = Placas
flash-card-action = Flashear

# Página de benchmarks
benchmarks-kicker = Rendimiento
benchmarks-title = Benchmarks en abierto
benchmarks-lead = Cada número de abajo viene de los resultados publicados en el repo, medidos en hardware real por un harness que puedes ejecutar tú mismo. A partir de aquí, el contenido está disponible solo en inglés por ahora.

# Pie (licencia)
footer-license = Código abierto. MIT / Apache 2.0.
footer-trademarks = Los logotipos, marcas e imágenes de productos de terceros pertenecen a sus respectivos propietarios. Se muestran solo para identificar plataformas, hardware y objetivos de compatibilidad. No se afirma ni se implica ningún respaldo.

# Página de contribución
contributing-kicker = El listón
contributing-title = Contribuir
contributing-lead = Cómo contribuir — qué valoramos, las convenciones que sigue tu código y el estándar que cada cambio supera. Para contribuidores humanos y automatizados por igual.

# 404
not-found-title = Aquí todavía no hay nada.
not-found-cta = Volver al inicio
