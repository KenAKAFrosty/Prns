# 导航
nav-contributing = 贡献
nav-crates = Crates
nav-api = API

# 页脚
footer-tagline = 由 Personal 团队带来。

# 首页
landing-kicker = 为人们而生的不可阻挡的 mesh 网络
landing-kicker-prefix = 为人们而生的不可阻挡的 mesh 网络
landing-title = 用安全 Rust 编写的生产级 Reticulum (RNS) 移植。
landing-subtitle = 一个确定性的 no_std、无分配器核心。为每个 Reticulum 节点所需的性能与稳定性而构建，从五美元的微控制器到云服务器都能覆盖。
landing-cta-ethos = 选择一个 crate
landing-cta-contributing = 贡献

# 引文
landing-quote-label = 我们正在构建的方向
landing-quote-body = Reticulum 是通向一个明亮未来的基础通信设施，只要我们所有人一起构建，那个未来就可以实现。这是 Personal 团队的努力：把 RNS 交到更多 builder 手中，帮助那个未来成真。

# 接口
interfaces-section-label = 接口
interfaces-section-title = Mesh 与现实世界相接的地方
interfaces-section-lead = Prns 保留 builder 已经熟悉的 RNS 兼容接口，并用面向新设备和网络的原生链路扩展这张地图。

interfaces-radio-label = 无线
interfaces-radio-headline = 面向设备和开发板的近距离链路
interfaces-radio-body = BLE Auto-interface、ESP-NOW 和 LoRa 将附近设备、开发板集群和长距离链路带入同一个 Reticulum mesh。

interfaces-lan-label = LAN
interfaces-lan-headline = 自动发现的本地链路 peers
interfaces-lan-body = Wi-Fi Auto-interface 使用 multicast、mDNS 和 gateway rendezvous 找到附近节点，并把本地网络折入 mesh。

interfaces-cable-label = 线缆 + 分组无线电
interfaces-cable-headline = 线缆、TNC 和无线电调制解调器
interfaces-cable-body = USB Auto-interface、串行 framing、KISS、AX.25 和 RNode 将小设备和分组无线电硬件接入同一个 mesh。

interfaces-host-label = 路由 IP
interfaces-host-headline = Internet、WAN 和 backbone 链路
interfaces-host-body = TCP client/server、UDP 和 Backbone 让远端 peers 也能通过 private WAN、VPN 和 public Internet relay 参与 mesh。

# 可以依靠的标准
standards-section-label = 我们的标准
standards-section-title = 你可以依靠什么
standards-license-label = 许可证
standards-license-headline = MIT / Apache 2.0
standards-license-body = 双许可证，宽松授权。没有 copyleft 或商业限制。
standards-safety-label = 安全性
standards-safety-headline = #![forbid(unsafe_code)]
standards-safety-body = personal-rns 引擎不包含任何 unsafe，并由编译器强制保证。依赖中的 unsafe 会在 Miri 下检查 UB，并用 cargo-geiger 审计。
standards-correctness-label = 正确性
standards-correctness-headline = 与 RNS 做差异测试
standards-correctness-body = 每一次改动都会与参考实现核对，然后经过属性测试、模糊测试和 mutation 测试，在关键之处还会加入 Kani 证明。
standards-benchmarked-label = 性能
standards-benchmarked-headline = 测量，而不只是宣称
standards-benchmarked-body = 性能以公开方式跟踪，由你可以自己运行的 harness 测量。
standards-benchmarked-cta = 查看 benchmarks →

# 从哪里开始？
start-section-label = 进入路径
start-section-title = 从哪里开始？
start-section-lead = 选择与你正在构建的东西相匹配的路径。现在每条路径都落到一个单独的 crate；更多指南会陆续补上。

start-daemon-headline = 我想运行一个 Reticulum 节点
start-daemon-body = 预构建 daemon。rnsd 的 drop-in。把它放在你已有的节点旁边运行。
start-daemon-code = apt install personal-rnsd
start-daemon-target = personal-rnsd

start-mobile-headline = 我在构建移动 app
start-mobile-body = Kotlin (.aar)、Swift (.xcframework) 或 Python (.whl) — 与 daemon 相同的引擎，直接嵌入你的 app。
start-mobile-code = implementation("org.staypersonal:rns:0.1")
    pod 'PersonalRns', '~> 0.1'
start-mobile-target = personal-rns-ffi

start-game-headline = 我要在游戏里发布
start-game-body = 面向 Unity、Godot 和 MonoGame 的 C# / .NET bindings。不用架服务器也能做多人。
start-game-code = dotnet add package Personal.Rns
start-game-target = personal-rns-ffi

start-embedded-headline = 我面向微控制器
start-embedded-body = 引擎加上只有三个方法的 Host trait。ESP32-C6 是参考平台；S3、nRF、RP2040 和 STM32 接下来会跟上。
start-embedded-code = cargo add personal-rns --no-default-features
start-embedded-target = personal-rns + personal-hopspot

start-web-headline = 我为 web 或 edge 构建
start-web-body = 一个 WebAssembly build，可在浏览器以及 Cloudflare Workers、Fastly、Spin 等 edge runtime 上运行。
start-web-code = npm install personal-rns
start-web-target = personal-rns (wasm32)

start-rust-headline = 我要嵌入 Rust app
start-rust-body = 开箱即用的完整 RNS runtime，或用于围绕它构建你自己的 runtime 的纯核心。
start-rust-code = cargo add personal-rnsd   # complete RNS runtime
    cargo add personal-rns      # pure core only
start-rust-target = personal-rnsd or personal-rns

start-lxmf-headline = 我想在 mesh 上发送消息
start-lxmf-body = Reticulum 之上的 LXMF — identities、addresses、delivery。Sideband 和 Nomadnet 所处的那一层。
start-lxmf-code = cargo add personal-lxmf
start-lxmf-target = personal-lxmf

# 平台（"Runs on"）— hero marquee 标签 + CTA，以及专门页面
landing-platforms-label = 可运行于
landing-platforms-cta = 查看全部 →
platforms-title = Prns 可运行的地方
platforms-lead = 一个引擎，许多归宿。有些今天已经可用；其余在 roadmap 上 — 这是我们构建时追向的北极星。实心芯片现在可运行；虚线芯片随后到来。
platforms-legend-shipping = 今日可用
platforms-legend-roadmap = Roadmap

# Benchmarks 页面
benchmarks-kicker = 性能
benchmarks-title = 公开 benchmark
benchmarks-lead = 我们把性能当作数字，而不是形容词。这里的每个数字都来自 repo 中的确定性 harness，在真实硬件上测得，并在比较公平时与 RNS 参考实现核对。随着 suite 稳定，数字会陆续补齐；下面是它们遵循的方法论。

# 许可证信号（页脚）
footer-license = 开源。MIT / Apache 2.0。
footer-trademarks = 第三方标志和商标归各自所有者所有。它们仅用于标识平台、硬件和兼容性目标；不表示任何认可或背书。

# 贡献页面
contributing-kicker = 标准
contributing-title = 贡献
contributing-lead = 如何贡献 — 我们重视什么、你的代码遵循哪些约定，以及每个改动需要达到的标准。人类贡献者和自动化贡献者一视同仁。

# Crates 索引
crates-kicker = 组件
crates-title = 选择与你正在构建的东西相匹配的部分。
crates-lead = 每个 crate 都被设计成可以独立发挥作用，即使你不引入其余部分。引擎是底座；其他东西都叠在其上，随着 suite 成长，会有更多组件落地。
crates-card-cta = 它做什么 →
crates-back = 所有 crates
crates-not-found = 没有这个名字的 crate

# 每个 crate 的卡片
crate-rns-role = 引擎
crate-rns-blurb = 把 Reticulum 放进任何 Rust 项目。确定性、no_std、无分配器；没有全局状态，没有内置 I/O — 你自带时钟和 wire。
crate-rnsd-role = Daemon
crate-rnsd-blurb = rnsd 的 drop-in，可在任何运行 Linux 的地方运行。与 RNS 参考实现使用同样的 wire；可以和你已有的节点并排使用，也可以替代它们。
crate-lxmf-role = 消息
crate-lxmf-blurb = Reticulum 之上的 LXMF — Sideband 和 Nomadnet 所处的那一层。Identities、addresses、message delivery。
crate-ffi-role = 移动端 + Python bindings
crate-ffi-blurb = 一个 uniffi interface 生成 Kotlin (.aar)、Swift (.xcframework) 和 Python (.whl)。从 Android、iOS 或 Jupyter notebook 使用 Reticulum — 相同形状，相同引擎。

# 404
not-found-title = 这里还什么都没有。
not-found-cta = 回到首页
