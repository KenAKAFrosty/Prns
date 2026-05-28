# 导航
nav-ethos = 设计
nav-crates = Crates
nav-api = API

# 页脚
footer-tagline = 由 Personal 团队打造。

# 着陆页
landing-kicker = 属于每个人的、势不可挡的网状网络
landing-title = 用 Rust 重写的 Reticulum (RNS) 生产级移植。
landing-subtitle = 确定性、no_std、无分配器的内核。完整覆盖 RNS 与 LXMF。Kotlin、Swift、Python、TypeScript、C# 的原生绑定一应俱全。可在浏览器与 Cloudflare Workers、Fastly、Spin 等边缘运行时上以 WebAssembly 运行。每一个 Reticulum 栈所需的性能与续航都被认真照顾——从五美元的单片机一路到云端节点。landing-cta-ethos = 选一个 crate
landing-cta-crates = 我们如何构建

# 引用
landing-quote-label = 我们正朝向的远方
landing-quote-body = Reticulum 是那个值得共同构筑的光明未来的基础通信设施——只要我们把它建出来。这正是我们的努力：把它送到更多开发者手中，与大家一同把那未来变成真实。

# 你可以放心依赖的事情
standards-section-label = 我们的标准
standards-section-title = 你可以放心依赖的事情
standards-license-label = 许可证
standards-license-headline = MIT / Apache 2.0
standards-license-body = 双许可证、宽松授权。没有 copyleft，没有非商业限制。
standards-coverage-label = 覆盖范围
standards-coverage-headline = RNS 与 LXMF 全覆盖
standards-coverage-body = 不只是 RNS，也不是把 LXMF 当配角。两者，皆完整。
standards-core-label = 内核
standards-core-headline = no_std、无分配器
standards-core-body = 一颗在分配器都跑不动的地方依然能继续工作的确定性内核。
standards-verification-label = 验证
standards-verification-headline = 与 RNS 对照差分测试
standards-verification-body = 每一处改动都与参考实现对照核验；要紧之处还备有形式化证明。

# 我该从哪里开始？
start-section-label = 入口
start-section-title = 我该从哪里开始？
start-section-lead = 挑一条与你正在构建的事物相称的路。今天每一条路都通向一个具体的 crate；专门的指南会随之跟上。

start-daemon-headline = 我想跑一个 Reticulum 节点
start-daemon-body = 预先构建好的守护进程。rnsd 即插即用替代。把它摆到你已有的节点旁，一起跑就行。
start-daemon-code = apt install personal-rnsd
start-daemon-target = personal-rnsd

start-mobile-headline = 我在做移动 App
start-mobile-body = Kotlin (.aar)、Swift (.xcframework) 或 Python (.whl)——和你守护进程跑的是同一台引擎，直接嵌进你的 App 里。
start-mobile-code = implementation("org.staypersonal:rns:0.1")
    pod 'PersonalRns', '~> 0.1'
start-mobile-target = personal-rns-ffi

start-game-headline = 我要把它放进游戏里
start-game-body = 面向 Unity、Godot、MonoGame 的 C# / .NET 绑定。不必架服务器，也能多人联机。
start-game-code = dotnet add package Personal.Rns
start-game-target = personal-rns-ffi

start-embedded-headline = 我要把它跑在单片机上
start-embedded-body = 引擎加上一个仅含三个方法的 Host trait。ESP32-C6 是参考实现，接下来是 S3、nRF、RP2040 与 STM32。
start-embedded-code = cargo add personal-rns --no-default-features
start-embedded-target = personal-rns + hosts/*

start-web-headline = 我在为 Web 或边缘构建
start-web-body = 一个既能在浏览器中、又能在 Cloudflare Workers、Fastly、Spin 这类边缘运行时上跑的 WebAssembly 构建。
start-web-code = npm install personal-rns
start-web-target = personal-rns (wasm32)

start-rust-headline = 我要嵌入到一个 Rust 应用里
start-rust-body = 开箱即用的完整 RNS 运行时，或者只取纯净的内核，自己围绕它搭出运行时——任选其一。
start-rust-code = cargo add personal-rnsd   # complete RNS runtime
    cargo add personal-rns      # pure core only
start-rust-target = personal-rnsd or personal-rns

start-lxmf-headline = 我想在网状网络上发送消息
start-lxmf-body = 位于 Reticulum 之上的 LXMF——身份、地址、投递。Sideband 与 Nomadnet 所栖身的那一层。
start-lxmf-code = cargo add personal-lxmf
start-lxmf-target = personal-lxmf

# 许可（页脚）
footer-license = 开源。MIT / Apache 2.0。

# 设计思想页
ethos-kicker = 我们的规矩
ethos-title = 我们如何构建
ethos-lead = 这是一封工程师写给工程师的备忘，谈这个项目背后的工作纪律：纯净的引擎、无分配器的内核、每一次改动都与 RNS 参考实现对照核验。在决定依赖它之前不妨先扫一眼——我们希望你心里清楚，自己将要走进的是一个什么样的项目。

# Crates 索引
crates-kicker = 构件
crates-title = 挑一块与你正在构建的事物相称的。
crates-lead = 每个 crate 都被设计成单独使用也能派上用场，即便你并不打算把其他部分一并带入。引擎是基底，其余在其之上层层叠加；随着套件的成长，会有更多构件加入。
crates-card-cta = 它能做什么 →
crates-back = 全部 crates
crates-not-found = 没有这个名字的 crate

# 各 crate 卡片
crate-rns-role = 引擎
crate-rns-blurb = 把 Reticulum 放进任何一个 Rust 项目里。确定性、no_std、无分配器；没有全局状态，没有内置 I/O——时钟与线路由你自带。
crate-rnsd-role = 守护进程
crate-rnsd-blurb = 凡 Linux 跑得起来的地方都跑得起来的 rnsd 替代品。与 RNS 参考实现走同一根线，可与你已有的节点并排部署，也可顶替它们。
crate-lxmf-role = 消息层
crate-lxmf-blurb = 位于 Reticulum 之上的 LXMF——Sideband 与 Nomadnet 所栖身的那一层。身份、地址、消息投递。
crate-ffi-role = 移动与 Python 绑定
crate-ffi-blurb = 一份 uniffi 接口同时生成 Kotlin (.aar)、Swift (.xcframework) 与 Python (.whl)。在 Android、iOS 或 Jupyter Notebook 里调用 Reticulum——同样的形态、同一台引擎。
crate-rvt-role = 可视化调试器
crate-rvt-blurb = 在虚拟时钟之下，看着数据包在模拟节点之间穿梭。确定性——同一场景，每次都是同一条轨迹。
crate-esp32c6-role = ESP32-C6 固件
crate-esp32c6-blurb = 面向 ESP32-C6 的裸机 Host 适配器。没有操作系统，也没有分配器——是引擎能跑在一颗内置无线电的五美元 RISC-V 芯片上的实证。

# 404
not-found-title = 这里还什么都没有。
not-found-cta = 返回首页
