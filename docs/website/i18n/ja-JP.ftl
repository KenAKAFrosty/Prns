# ナビゲーション
nav-ethos = デザイン
nav-crates = クレート
nav-api = API

# フッター
footer-tagline = Personal チームがお届けします。

# ランディング
landing-kicker = 止められないメッシュネットワークを、人々のために
landing-title = Rust で書かれた Reticulum (RNS) のプロダクションレベルの移植版。
landing-subtitle = 決定論的で no_std、アロケータ不要のコア。RNS と LXMF を余すところなくカバーします。Kotlin、Swift、Python、TypeScript、C# のネイティブバインディング付き。ブラウザやエッジランタイム向けの WebAssembly も。5 ドルのマイコンからクラウドノードまで、どの Reticulum スタックでも必要な性能とバッテリー寿命を狙って作られています。rnsd のドロップイン置き換えも同梱。
landing-cta-ethos = クレートを選ぶ
landing-cta-crates = どう作っているか

# プルクオート
landing-quote-label = 何に向かって作っているか
landing-quote-body = Reticulum は、私たちが築き上げられる明るい未来の根幹となる通信インフラです——築き上げる気があれば。これは、より多くの開発者の手にそれを届け、その未来を共に実現していくための私たちの試みです。

# 信頼できること
standards-section-label = 私たちの基準
standards-section-title = 信頼できること
standards-license-label = ライセンス
standards-license-headline = MIT / Apache 2.0
standards-license-body = デュアルライセンスで寛容。コピーレフトも、非商用制限もありません。
standards-coverage-label = カバレッジ
standards-coverage-headline = RNS と LXMF を完全サポート
standards-coverage-body = RNS だけではなく、LXMF は脇役でもありません。両方を、きちんと。
standards-core-label = コア
standards-core-headline = no_std、アロケータ不要
standards-core-body = アロケータが動けないところでも動く、決定論的なコア。
standards-verification-label = 検証
standards-verification-headline = RNS との差分テスト
standards-verification-body = すべての変更はリファレンスと照らし合わせて検証。必要なところには形式的証明も。

# どこから始める？
start-section-label = 入口
start-section-title = どこから始める？
start-section-lead = 作っているものに合う道を選んでください。今はそれぞれ単一のクレートに着地しますが、ガイドも一緒に整えていきます。

start-daemon-headline = Reticulum ノードを動かしたい
start-daemon-body = ビルド済みのデーモン。rnsd のドロップイン。既存のノードの隣でそのまま走らせてください。
start-daemon-code = apt install personal-rnsd
start-daemon-target = personal-rnsd

start-mobile-headline = モバイルアプリを作っている
start-mobile-body = Kotlin (.aar)、Swift (.xcframework)、または Python (.whl) — デーモンが動かしているのと同じエンジンを、アプリの中に直接埋め込めます。
start-mobile-code = implementation("org.staypersonal:rns:0.1")
    pod 'PersonalRns', '~> 0.1'
start-mobile-target = personal-rns-ffi

start-game-headline = ゲームに組み込みたい
start-game-body = Unity、Godot、MonoGame 向けの C# / .NET バインディング。サーバを立てずにマルチプレイヤー。
start-game-code = dotnet add package Personal.Rns
start-game-target = personal-rns-ffi

start-embedded-headline = マイコンをターゲットにしている
start-embedded-body = エンジンに、3 つのメソッドからなる Host トレイト。リファレンスは ESP32-C6、続いて S3、nRF、RP2040、STM32。
start-embedded-code = cargo add personal-rns --no-default-features
start-embedded-target = personal-rns + hosts/*

start-web-headline = Web またはエッジ向けに作っている
start-web-body = ブラウザや Cloudflare Workers、Fastly、Spin といったエッジランタイムで動く WebAssembly ビルド。
start-web-code = npm install personal-rns
start-web-target = personal-rns (wasm32)

start-rust-headline = Rust アプリに組み込む
start-rust-body = 箱を開けてすぐ使える完全な RNS ランタイム、または自前のランタイムを組み立てるための純粋なコア。
start-rust-code = cargo add personal-rnsd   # complete RNS runtime
    cargo add personal-rns      # pure core only
start-rust-target = personal-rnsd or personal-rns

start-lxmf-headline = メッシュ越しにメッセージを送りたい
start-lxmf-body = Reticulum の上に乗る LXMF — アイデンティティ、アドレス、配送。Sideband と Nomadnet が乗っている層です。
start-lxmf-code = cargo add personal-lxmf
start-lxmf-target = personal-lxmf

# ライセンス（フッター）
footer-license = オープンソース。MIT / Apache 2.0。

# 設計思想ページ
ethos-kicker = 規律
ethos-title = どう作っているか
ethos-lead = このプロジェクトを支える規律について、エンジニアからエンジニアへのメモです——純粋なエンジン、アロケータ不要のコア、すべての変更を RNS リファレンスと照合。依存する前にざっと目を通してください。何に踏み込むのかを知っておいてほしいのです。

# クレート一覧
crates-kicker = 構成要素
crates-title = 作っているものに合うものを選んでください。
crates-lead = どのクレートも、ほかを引き込まなくても単体で役に立つように作っています。エンジンが基盤、ほかはその上に積み上がります。スイートが育つにつれて、さらに増えていきます。
crates-card-cta = 何をするか →
crates-back = すべてのクレート
crates-not-found = その名前のクレートはありません

# クレートカード
crate-rns-role = エンジン
crate-rns-blurb = Reticulum をどんな Rust プロジェクトにも差し込めます。決定論的、no_std、アロケータ不要。グローバル状態も、組み込みの I/O もありません——クロックと回線はあなたが用意します。
crate-rnsd-role = デーモン
crate-rnsd-blurb = Linux が動くところならどこでも動く rnsd ドロップイン。RNS リファレンスと同じワイヤなので、既存のノードと並べても入れ替えても使えます。
crate-lxmf-role = メッセージング
crate-lxmf-blurb = Reticulum の上に乗る LXMF — Sideband と Nomadnet が乗っている層です。アイデンティティ、アドレス、メッセージ配送。
crate-ffi-role = モバイルと Python のバインディング
crate-ffi-blurb = 1 つの uniffi インターフェイスから Kotlin (.aar)、Swift (.xcframework)、Python (.whl) が生まれます。Android でも iOS でも、Jupyter ノートブックでも、同じ形と同じエンジンで Reticulum を使えます。
crate-rvt-role = ビジュアルデバッガ
crate-rvt-blurb = 仮想クロック上のシミュレートされたノードの間をパケットが移動する様子を眺められます。決定論的——同じシナリオなら、毎回同じトレースに。
crate-esp32c6-role = ESP32-C6 ファームウェア
crate-esp32c6-blurb = ESP32-C6 向けのベアメタル Host アダプタ。OS もアロケータもなし——無線機を内蔵した 5 ドルの RISC-V チップ上でエンジンが動くという証拠です。

# 404
not-found-title = ここにはまだ何もありません。
not-found-cta = ホームに戻る
