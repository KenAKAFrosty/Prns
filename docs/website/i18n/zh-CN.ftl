# 导航
nav-ethos = 设计思想
nav-crates = Crates
nav-api = API

# 页脚
footer-tagline = 移植的是契约，而非实现。

# 首页
landing-kicker = 忠实移植 Reticulum
landing-title = 一个纯粹的引擎。每个平台带来轻薄的 host。
landing-subtitle = 从零开始的 Rust 移植版 Reticulum 网状网络契约——嵌入式优先、确定性、no_std 干净，依靠一条小小的 Host 接缝，让同一个引擎能在守护进程、微控制器和手机上运行。
landing-cta-ethos = 阅读设计思想
landing-cta-crates = 浏览 crates
landing-triumvirate-label = 三柱
landing-quote-label = 构建准则
landing-quote-body = 移植的是契约，而非实现。构建一个纯粹的引擎，让每个平台带来自己的轻薄 host。

# 三柱卡片
triumvirate-rns-role = 纯粹的引擎
triumvirate-rns-blurb = 协议契约、路由、announce、链接——纯粹的 tick/ingest，无 I/O，无需 std。
triumvirate-rnsd-role = 守护进程 host
triumvirate-rnsd-blurb = 基于 std 的轻薄 Host 适配器；展示了平台如何让引擎活起来的范例。
triumvirate-lxmf-role = 消息层
triumvirate-lxmf-blurb = 引擎之上的 LXMF 应用层——为应用开发者提供寻址、投递与身份。

# 设计思想页
ethos-kicker = 构建之道
ethos-title = 构建准则
ethos-lead = 这是整个套件每一个决定背后的工程思想。读一遍就能理解为什么架构是这个样子——纯粹的引擎、轻薄的 host、契约优先于实现。

# Crates 索引
crates-kicker = 套件
crates-title = Crates 一览
crates-lead = 套件由六个 crate 组成。引擎是基座；其余皆为轻薄的 host 或消费者。
crates-card-cta = 阅读更多 →
crates-back = 返回 crates 列表
crates-not-found = 没有这个名字的 crate

# 每个 crate 卡片
crate-rns-role = 纯粹的 Reticulum 引擎
crate-rns-blurb = 把协议契约与路由实现为纯粹的状态机。no_std + alloc，确定性，嵌入式优先。
crate-rnsd-role = 参考实现的守护进程 host
crate-rnsd-blurb = 基于 std 的 Host 适配器和 Linux 守护进程二进制。让引擎跑起来的范例。
crate-lxmf-role = LXMF 应用层
crate-lxmf-blurb = 为应用开发者提供寻址、投递与身份。位于 personal-rns 之上；被 Personal 等使用。
crate-ffi-role = Kotlin / Swift / Python 绑定
crate-ffi-blurb = 一个 UDL，借助 uniffi 生成三种语言的绑定。给非 Rust 使用者的 SDK 入口。
crate-rvt-role = 多节点仿真与开发工具
crate-rvt-blurb = Reticulum Visual Toolkit。当下是虚拟时钟的多节点仿真；不久后是实时调试器。基于 Dioxus，可移植到 Web。
crate-esp32c6-role = ESP32-C6 host 适配器
crate-esp32c6-blurb = bare metal 上的 no_std/no_main host。证明引擎可以装进真实的微控制器里。

# 404
not-found-title = 这里还什么都没有。
not-found-cta = 回到首页
