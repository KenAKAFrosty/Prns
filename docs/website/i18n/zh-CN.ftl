# 导航
nav-benchmarks = 基准测试
nav-contributing = 贡献
site-early-english-note = Prns 尚处早期：完整文档在 GitHub 和源代码中，目前仅提供英文。

# 页脚
footer-tagline = 由 KenAKAFrosty 与 Personal/Prns 团队打造。
footer-flash = 烧录 Hopspot（仅英文）
footer-playground = 浏览器演示平台（仅英文）

# 首页
landing-kicker = 属于你的 mesh 网络
landing-kicker-prefix = 属于你的 mesh 网络
landing-title = 高性能 Reticulum (RNS)，为在任何设备上运行而构建。
landing-title-lead = 高性能 Reticulum (RNS)，
landing-title-accent = 为在任何设备上运行而构建。
landing-subtitle = 从 5 美元的微控制器到云服务器集群，为每个 Reticulum 节点所需的性能、稳定性与能效而构建。同一个引擎、同一套 API，在嵌入式、桌面、移动、游戏和 Web 上完全一致。
landing-cta-ethos = 在 Prns 中找到你的路
landing-cta-standards = 我们的标准
# 引文
landing-quote-label = 我们正在构建的方向
landing-quote-body = Reticulum 是通向一个光明未来的基础通信设施，只要我们所有人一起构建，那个未来就可以实现。这是 Personal 团队的努力：把 RNS 交到更多 builder 手中，帮助那个未来成真。

# 接口
interfaces-section-label = 接口
interfaces-section-title = Mesh 与现实世界相接的地方
interfaces-section-lead = Prns 保留 builder 已经熟悉的 RNS 兼容接口，并用面向新设备和网络的原生链路扩展这张地图。
interfaces-section-hot-note = Prns 接口支持热插拔：无需重启节点即可添加、移除或更改接口。

interfaces-radio-label = 无线
interfaces-radio-headline = 面向设备和开发板的近距离链路
interfaces-radio-body = Bluetooth LE Auto-interface、ESP-NOW 和 LoRa 将附近设备、开发板群和长距离射频链路带入同一个 Reticulum mesh。

interfaces-lan-label = LAN
interfaces-lan-headline = 自动发现的本地链路 peer
interfaces-lan-body = Wi-Fi Auto-interface 使用 multicast、mDNS 和 gateway rendezvous 找到附近节点，并把本地网络并入 mesh。

interfaces-cable-label = 线缆 + 分组无线电
interfaces-cable-headline = 线缆、TNC 和无线电调制解调器
interfaces-cable-body = USB Auto-interface、串行 framing、KISS、AX.25 和 RNode 将小设备和分组无线电硬件接入同一个 mesh。

interfaces-host-label = 路由 IP
interfaces-host-headline = Internet、WAN 和 backbone 链路
interfaces-host-body = TCP client/server、UDP、WebSocket 和 Backbone 让远端 peer 也能通过 private WAN、VPN、public Internet relay 和浏览器集成参与 mesh。

# 可以依靠的标准
standards-section-label = 我们的标准
standards-section-title = 你可以依靠什么
standards-license-label = 许可证
standards-license-headline = MIT / Apache 2.0
standards-license-body = 双许可证，宽松授权。没有 copyleft 或商业限制。
standards-safety-label = 安全性
standards-safety-headline = 先强制，后审计
standards-safety-body = 在引擎中，panic、unwrap 与未经论证的 unsafe 永远无法编译。无法禁止的，就加以审计：依赖中的 unsafe 用 cargo-geiger，未定义行为用 Miri，安全公告用 cargo-deny。
standards-correctness-label = 正确性
standards-correctness-headline = 与 RNS 做差异测试
standards-correctness-body = 每一次改动都会与参考实现核对，然后经过单元测试、属性测试、模糊测试和变异测试，在关键之处还会加入 Kani 证明。
standards-benchmarked-label = 性能
standards-benchmarked-headline = 测量，而不只是宣称
standards-benchmarked-body = 性能以公开方式跟踪，由你可以自己运行的 harness 测量。
standards-benchmarked-cta = 查看基准测试 →

# 从哪里开始？
start-section-label = 进入路径
start-section-title = 你来这里想做什么？
start-section-lead = 按 Prns 融入你工作的方式选择路径：要烧录的硬件、要运行的基础设施，或要构建的软件。

start-daemon-headline = 运行一个 daemon
start-daemon-body = 为桌面、LXMF 应用、backbone VPS 等安装一个快速的 Reticulum daemon。
start-daemon-code = 对现有应用即插即用
    读取 ~/.reticulum
    实时编辑接口
    内置指标
start-daemon-target = 运行 Prnsd

start-embedded-headline = 烧录一个 Hopspot
start-embedded-body = 选择一块受支持的开发板，比较无线与电池的取舍，然后烧录一台专用 mesh 设备。
start-embedded-code = 开发板矩阵
    Web 烧录器
    本地烧录
start-embedded-target = 烧录 Hopspot（仅英文）

start-web-headline = 使用浏览器节点演示平台
start-web-body = 体验通过 WebAssembly 运行共享 Rust 引擎的 TypeScript API，使用 Auto Wi-Fi 或 USB Auto 连接，并实时查看本地节点活动。
start-web-code = WebAssembly 运行时
    Auto Wi-Fi + USB Auto
    TypeScript 示例
start-web-target = 打开演示平台（仅英文）

start-rust-headline = 在 Reticulum 上构建
start-rust-body = 用引擎和绑定，为应用、工具、服务或游戏加入 mesh 网络。
start-rust-target = 阅读 README
start-rust-target-source = 下载源码

# 平台（"Runs on"）— hero marquee 标签 + CTA，以及专门页面
landing-platforms-label = 运行于
landing-platforms-cta = 查看全部 →
platforms-title = Prns 运行在哪里
platforms-lead = 一个引擎，四处安家。这份速览把运行时平台支持与具体 Hopspot 开发板支持分开呈现。
platforms-board-support-link = 查看开发板支持与 bring-up →

# 烧录 Hopspot 页面
flash-back = 平台
flash-back-boards = 开发板
flash-card-action = 烧录

# Benchmarks 页面
benchmarks-kicker = 性能
benchmarks-title = 公开基准测试
benchmarks-lead = 下面的每个数字都来自 repo 中公开发布的结果，在真实硬件上由你可以自己运行的 harness 测得。从这里开始的内容目前仅提供英文。

# 许可证信号（页脚）
footer-license = 开源。MIT / Apache 2.0。
footer-trademarks = 第三方标志、商标和产品图片归各自所有者所有。它们仅用于标识平台、硬件和兼容性目标。不主张也不暗示任何认可或背书。

# 贡献页面
contributing-kicker = 门槛
contributing-title = 贡献
contributing-lead = 如何贡献——我们重视什么、你的代码遵循哪些约定，以及每个改动需要达到的标准。人类贡献者和自动化贡献者一视同仁。

# 404
not-found-title = 这里还什么都没有。
not-found-cta = 回到首页
