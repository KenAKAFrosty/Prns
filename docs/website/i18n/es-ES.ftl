# Navegación
nav-contributing = Contribuir
nav-api = API

# Pie de página
footer-tagline = Construido por el equipo de Personal.

# Página de inicio
landing-kicker = Redes mesh imparables para la gente
landing-kicker-prefix = Redes mesh imparables para la
landing-title = Un port de alto rendimiento de Reticulum (RNS), escrito en Rust seguro.
landing-title-lead = A high-performance port of Reticulum (RNS).
landing-title-accent = Runs on any device.
landing-subtitle = Un núcleo determinista, no_std y sin asignador. Construido para el rendimiento y la estabilidad que todo nodo Reticulum necesita, desde un microcontrolador de cinco dólares hasta un servidor en la nube.
landing-cta-ethos = Elige un crate
# Cita
landing-quote-label = Hacia lo que estamos construyendo
landing-quote-body = Reticulum es la infraestructura de comunicación fundacional de un futuro luminoso que podemos tener, siempre que lo construyamos entre todos. Este es el esfuerzo del equipo de Personal por poner RNS en manos de más builders y ayudar a hacer realidad ese futuro.

# Interfaces
interfaces-section-label = Interfaces
interfaces-section-title = Donde la mesh se encuentra con el mundo
interfaces-section-lead = Prns conserva las interfaces compatibles con RNS que los builders ya conocen y amplía el mapa con enlaces nativos para nuevos dispositivos y redes.
interfaces-section-hot-note = Las interfaces de Prns son hot-swappable: añade, elimina o cambia una interfaz sin reiniciar el nodo.

interfaces-radio-label = Radios
interfaces-radio-headline = Enlaces de proximidad para dispositivos y placas
interfaces-radio-body = Bluetooth LE Auto-interface, ESP-NOW y LoRa llevan dispositivos cercanos, flotas de placas y enlaces de largo alcance a una misma mesh RNS.

interfaces-lan-label = LAN
interfaces-lan-headline = Pares de enlace local descubiertos automáticamente
interfaces-lan-body = Wi-Fi Auto-interface usa multicast, mDNS y rendezvous de gateway para encontrar nodos cercanos e integrar una red local en la mesh.

interfaces-cable-label = Cables + radio por paquetes
interfaces-cable-headline = Cables, TNC y módems de radio
interfaces-cable-body = USB Auto-interface, framing serie, KISS, AX.25 y RNode conectan dispositivos pequeños y hardware de radio por paquetes a la misma mesh.

interfaces-host-label = IP enrutada
interfaces-host-headline = Internet, WAN y enlaces backbone
interfaces-host-body = TCP cliente/servidor, UDP y Backbone permiten que peers distantes participen en la mesh a través de WAN privadas, VPN y relays en Internet público.

# Con lo que puedes contar
standards-section-label = Nuestros estándares
standards-section-title = Con lo que puedes contar
standards-license-label = Licencia
standards-license-headline = MIT / Apache 2.0
standards-license-body = Doble licencia y permisiva. Sin copyleft ni restricciones comerciales.
standards-safety-label = Seguridad
standards-safety-headline = Impuesto, luego auditado
standards-safety-body = En el motor, los panics, los unwraps y el unsafe sin justificar nunca compilan. Lo que no se puede prohibir se audita: el unsafe de las dependencias con cargo-geiger, el comportamiento indefinido con Miri, los avisos de seguridad con cargo-deny.
standards-correctness-label = Corrección
standards-correctness-headline = Diff-testado contra RNS
standards-correctness-body = Cada cambio se contrasta con la referencia y luego pasa por pruebas de propiedades, fuzzing y mutación, con pruebas Kani donde importan.
standards-benchmarked-label = Rendimiento
standards-benchmarked-headline = Medido, no solo afirmado
standards-benchmarked-body = El rendimiento se sigue en abierto, medido por un harness que puedes ejecutar tú mismo.
standards-benchmarked-cta = Ver benchmarks →

# ¿Por dónde empiezo?
start-section-label = Caminos de entrada
start-section-title = ¿Por dónde empiezo?
start-section-lead = Elige el camino que coincida con lo que estás construyendo. Hoy cada uno aterriza en un único crate; llegarán más guías junto a ellos.

start-daemon-headline = Quiero un nodo Reticulum corriendo
start-daemon-body = Daemon ya construido. Drop-in para rnsd. Ejecútalo junto a los nodos que ya tienes.
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

start-web-headline = Usa el playground del nodo en el navegador
start-web-body = Prueba la API de TypeScript con el motor Rust compartido en WebAssembly, conéctate mediante Auto Wi-Fi o USB Auto y observa la actividad local del nodo en tiempo real.
start-web-code = Runtime WebAssembly
    Auto Wi-Fi + USB Auto
    Ejemplo TypeScript
start-web-target = Abrir playground

start-rust-headline = Construye sobre Reticulum
start-rust-body = Usa el motor y los bindings para añadir redes mesh a apps, herramientas, servicios o juegos.
start-rust-target = Leer el README
start-rust-target-source = Descargar el código fuente

# Plataformas ("Runs on") — etiqueta del marquee del hero + CTA y página dedicada
landing-platforms-label = Runs on
landing-platforms-cta = See all →
platforms-title = Where Prns runs
platforms-lead = One engine, many homes. This quick view separates runtime platform support from specific Hopspot board support.
platforms-board-support-link = View board support & bring-up →

# Flash a Hopspot page
flash-back = Platforms
flash-back-boards = Boards
flash-card-action = Flash

# Página de benchmarks
benchmarks-kicker = Rendimiento
benchmarks-title = Benchmarks en abierto
benchmarks-lead = Tratamos el rendimiento como un número, no como un adjetivo. Cada cifra aquí viene de un harness determinista en el repo, medida en hardware real y comprobada contra la referencia de RNS cuando la comparación es justa. Los números van llegando a medida que la suite se estabiliza; abajo está la metodología que deben cumplir.

# Pie (licencia)
footer-license = Código abierto. MIT / Apache 2.0.
footer-trademarks = Los logotipos y marcas de terceros pertenecen a sus respectivos propietarios. Se muestran solo para identificar plataformas, hardware y objetivos de compatibilidad. No se afirma ni se implica ningún respaldo.

# Página de contribución
contributing-kicker = El estándar
contributing-title = Contribuir
contributing-lead = Cómo contribuir — qué valoramos, las convenciones que sigue tu código y el estándar que supera cada cambio. Para contribuidores humanos y automatizados por igual.

# 404
not-found-title = Aquí todavía no hay nada.
not-found-cta = Volver al inicio
