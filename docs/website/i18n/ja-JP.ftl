# ナビゲーション
nav-ethos = デザイン
nav-crates = クレート
nav-api = API

# フッター
footer-tagline = Personal チームがお届けしています。

# ランディング
landing-kicker = 止められないメッシュネットワークを、すべての人に
landing-title = Rust で書かれた Reticulum (RNS) の本番品質ポート。
landing-subtitle = 決定論的で no_std、アロケータを必要としないコア。RNS と LXMF を余すところなくカバーします。5 ドルのマイコンからクラウドノードまで、あらゆる Reticulum スタックが必要とする性能とバッテリー寿命を正面から面倒みる設計です。
landing-cta-ethos = クレートを選ぶ
landing-cta-crates = どう作っているか

# プルクオート
landing-quote-label = 私たちが向かう先
landing-quote-body = Reticulum は、私たちが築き上げれば手に入る明るい未来の、まさにその根幹を支える通信基盤です。本プロジェクトは、それをより多くの開発者の手に届け、その未来を共に形にしていくための私たちの取り組みです。

# 信頼できること
standards-section-label = 私たちの基準
standards-section-title = 安心して頼れること
standards-license-label = ライセンス
standards-license-headline = MIT / Apache 2.0
standards-license-body = デュアルライセンス、寛容な条件です。コピーレフトも、非商用制限もありません。
standards-coverage-label = カバレッジ
standards-coverage-headline = RNS と LXMF を完全サポート
standards-coverage-body = RNS だけではありません。LXMF をおまけ扱いにもしません。両方を、きちんと。
standards-core-label = コア
standards-core-headline = no_std、アロケータ不要
standards-core-body = アロケータが動けない場所でも走る、決定論的なコア。
standards-verification-label = 検証
standards-verification-headline = RNS との差分テスト
standards-verification-body = すべての変更をリファレンスと突き合わせて検証します。要となる部分には形式的証明も。

# どこから始めればいいですか？
start-section-label = 入り口
start-section-title = どこから始めればいいですか？
start-section-lead = いま作っているものに合う道を選んでください。各項目は当面ひとつのクレートへ案内しますが、専用のガイドも順次そろえていきます。

start-daemon-headline = Reticulum ノードを動かしたい
start-daemon-body = ビルド済みのデーモン。rnsd のドロップイン。既存のノードの隣に置いて、そのまま走らせてください。
start-daemon-code = apt install personal-rnsd
start-daemon-target = personal-rnsd

start-mobile-headline = モバイルアプリを作っている
start-mobile-body = Kotlin (.aar)、Swift (.xcframework)、Python (.whl) — デーモンが動かしているのと同じエンジンを、アプリの中に直接組み込めます。
start-mobile-code = implementation("org.staypersonal:rns:0.1")
    pod 'PersonalRns', '~> 0.1'
start-mobile-target = personal-rns-ffi

start-game-headline = ゲームに組み込みたい
start-game-body = Unity、Godot、MonoGame 向けの C# / .NET バインディング。サーバを立てずにマルチプレイヤーが動かせます。
start-game-code = dotnet add package Personal.Rns
start-game-target = personal-rns-ffi

start-embedded-headline = マイコンをターゲットにしている
start-embedded-body = エンジンに、メソッド 3 つだけの Host トレイト。リファレンスは ESP32-C6、続いて S3、nRF、RP2040、STM32 と並びます。
start-embedded-code = cargo add personal-rns --no-default-features
start-embedded-target = personal-rns + personal-hopspot

start-web-headline = Web またはエッジ向けに作っている
start-web-body = ブラウザ、そして Cloudflare Workers や Fastly、Spin といったエッジランタイムでも動く WebAssembly ビルド。
start-web-code = npm install personal-rns
start-web-target = personal-rns (wasm32)

start-rust-headline = Rust アプリに組み込む
start-rust-body = 箱を開けたら使える完全な RNS ランタイム。もしくは自前のランタイムを組み立てるための純粋なコア。どちらでも。
start-rust-code = cargo add personal-rnsd   # complete RNS runtime
    cargo add personal-rns      # pure core only
start-rust-target = personal-rnsd or personal-rns

start-lxmf-headline = メッシュ越しにメッセージを送りたい
start-lxmf-body = Reticulum の上に乗る LXMF — アイデンティティ、アドレス、配送。Sideband と Nomadnet がそのまま乗っているレイヤーです。
start-lxmf-code = cargo add personal-lxmf
start-lxmf-target = personal-lxmf

# ライセンス（フッター）
footer-license = オープンソース。MIT / Apache 2.0。

# 設計思想ページ
ethos-kicker = 規律
ethos-title = どう作っているか
ethos-lead = このプロジェクトを支える規律について、エンジニアからエンジニアへのメモです。純粋なエンジン、アロケータを必要としないコア、すべての変更を RNS リファレンスと突き合わせて検証。依存する前に一度目を通してください。どんなプロジェクトに足を踏み入れるのか、知っておいてもらえると安心です。

# クレート一覧
crates-kicker = 構成要素
crates-title = 作っているものに合うものを選んでください。
crates-lead = どのクレートも、ほかを引き入れなくても単体で役に立つよう設計しています。エンジンが土台で、その他はすべて上に積み上がります。スイートが育つにつれ、さらにピースが増えていきます。
crates-card-cta = 何をするか →
crates-back = すべてのクレート
crates-not-found = その名前のクレートはありません

# クレートカード
crate-rns-role = エンジン
crate-rns-blurb = Reticulum を任意の Rust プロジェクトに差し込んでください。決定論的、no_std、アロケータ不要。グローバル状態も、組み込みの I/O もありません — クロックと回線はご自身でお持ち込みください。
crate-rnsd-role = デーモン
crate-rnsd-blurb = Linux が動くところならどこでも動く rnsd ドロップイン。RNS リファレンスと同じワイヤなので、既存のノードと並べても、置き換えても構いません。
crate-lxmf-role = メッセージング
crate-lxmf-blurb = Reticulum の上に乗る LXMF — Sideband と Nomadnet がそのまま乗っているレイヤーです。アイデンティティ、アドレス、メッセージ配送。
crate-ffi-role = モバイルと Python のバインディング
crate-ffi-blurb = 1 つの uniffi インターフェイスから Kotlin (.aar)、Swift (.xcframework)、Python (.whl) が生まれます。Android でも iOS でも、Jupyter ノートブックでも、同じ形と同じエンジンで Reticulum を使えます。

# 404
not-found-title = ここにはまだ何もありません。
not-found-cta = ホームに戻る
