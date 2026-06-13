# ナビゲーション
nav-contributing = 貢献
nav-crates = Crates
nav-api = API

# フッター
footer-tagline = Personal チームがお届けします。

# ランディング
landing-kicker = 人々のための止まらないメッシュネットワーク
landing-kicker-prefix = 人々のための止まらないメッシュネットワーク
landing-title = 安全な Rust で書かれた、プロダクション品質の Reticulum (RNS) ポート。
landing-subtitle = 決定的で no_std、アロケータ不要のコア。5ドルのマイクロコントローラからクラウドサーバーまで、あらゆる Reticulum ノードに必要な性能と安定性のために作られています。
landing-cta-ethos = crate を選ぶ
landing-cta-contributing = 貢献する

# 引用
landing-quote-label = 私たちが目指しているもの
landing-quote-body = Reticulum は、私たち全員が作り続ける限り手にできる明るい未来の、基礎となる通信インフラです。これは Personal チームが RNS をより多くのビルダーの手に届け、その未来の実現を助けるための取り組みです。

# 信頼できる基準
standards-section-label = 私たちの基準
standards-section-title = 信頼できること
standards-license-label = ライセンス
standards-license-headline = MIT / Apache 2.0
standards-license-body = デュアルライセンスで permissive。コピーレフトや商用利用の制限はありません。
standards-safety-label = 安全性
standards-safety-headline = #![forbid(unsafe_code)]
standards-safety-body = 私たちの crate は unsafe を一切含まず、コンパイラがそれを強制します。依存関係内の unsafe は Miri で UB を検査し、cargo-geiger で監査します。
standards-correctness-label = 正しさ
standards-correctness-headline = RNS との差分テスト済み
standards-correctness-body = すべての変更をリファレンスと照合し、そのうえでプロパティテスト、ファズテスト、ミューテーションテストにかけ、重要な箇所では Kani の証明も使います。
standards-benchmarked-label = 性能
standards-benchmarked-headline = 主張ではなく測定
standards-benchmarked-body = 性能は公開された形で追跡され、自分でも実行できるハーネスで測定されます。
standards-benchmarked-cta = ベンチマークを見る →

# どこから始める？
start-section-label = 入り口
start-section-title = どこから始める？
start-section-lead = 作っているものに合う道を選んでください。今はそれぞれ 1 つの crate に着地しますが、今後さらにガイドを並べていきます。

start-daemon-headline = Reticulum ノードを動かしたい
start-daemon-body = 事前ビルド済み daemon。rnsd のドロップイン。既存のノードの横で動かせます。
start-daemon-code = apt install personal-rnsd
start-daemon-target = personal-rnsd

start-mobile-headline = モバイルアプリを作っている
start-mobile-body = Kotlin (.aar)、Swift (.xcframework)、Python (.whl) — daemon と同じエンジンをアプリに直接組み込めます。
start-mobile-code = implementation("org.staypersonal:rns:0.1")
    pod 'PersonalRns', '~> 0.1'
start-mobile-target = personal-rns-ffi

start-game-headline = ゲームに組み込んで出荷したい
start-game-body = Unity、Godot、MonoGame 向けの C# / .NET バインディング。サーバーを立てずにマルチプレイヤーを実現します。
start-game-code = dotnet add package Personal.Rns
start-game-target = personal-rns-ffi

start-embedded-headline = マイクロコントローラを対象にしている
start-embedded-body = エンジンと、3 つのメソッドだけの Host trait。ESP32-C6 がリファレンスで、S3、nRF、RP2040、STM32 が次に続きます。
start-embedded-code = cargo add personal-rns --no-default-features
start-embedded-target = personal-rns + personal-hopspot

start-web-headline = Web や edge 向けに作っている
start-web-body = ブラウザや Cloudflare Workers、Fastly、Spin のような edge runtime で動く WebAssembly ビルドです。
start-web-code = npm install personal-rns
start-web-target = personal-rns (wasm32)

start-rust-headline = Rust アプリに組み込みたい
start-rust-body = そのまま使える完全な RNS runtime、または自分の runtime を組み立てるための純粋なコア。
start-rust-code = cargo add personal-rnsd   # complete RNS runtime
    cargo add personal-rns      # pure core only
start-rust-target = personal-rnsd or personal-rns

start-lxmf-headline = メッシュ上でメッセージを送りたい
start-lxmf-body = Reticulum の上にある LXMF — identity、address、delivery。Sideband と Nomadnet が乗る層です。
start-lxmf-code = cargo add personal-lxmf
start-lxmf-target = personal-lxmf

# プラットフォーム ("Runs on") — ヒーローのマーキーラベル + CTA、専用ページ
landing-platforms-label = 対応環境
landing-platforms-cta = すべて見る →
platforms-title = Prns が動く場所
platforms-lead = 1 つのエンジン、多くの居場所。いくつかは今日出荷されており、残りはロードマップ上にあります — 私たちが目指す北極星です。塗りつぶしのチップは現在動作し、点線のものは次に続きます。
platforms-legend-shipping = 現在出荷中
platforms-legend-roadmap = ロードマップ

# ベンチマークページ
benchmarks-kicker = 性能
benchmarks-title = オープンにベンチマーク
benchmarks-lead = 私たちは性能を形容詞ではなく数値として扱います。ここにある数値はすべて、repo 内の決定的なハーネスから得たもので、実機で測定し、公平に比較できる場所では RNS リファレンスとも照合しています。数値は suite が安定するにつれて追加されます。下には、それらが従う方法論を示しています。

# ライセンス表示 (フッター)
footer-license = Open source. MIT / Apache 2.0.

# 貢献ページ
contributing-kicker = 基準
contributing-title = 貢献
contributing-lead = 貢献のしかた — 私たちが大切にしていること、コードが従う規約、そしてすべての変更が満たす基準。人間の貢献者にも自動化された貢献者にも同じです。

# Crates index
crates-kicker = 部品
crates-title = 作っているものに合うものを選んでください。
crates-lead = 各 crate は、残りを取り込まなくても単体で役に立つように作られています。エンジンが基盤で、その上にすべてが積み上がり、suite の成長に合わせてさらに部品が増えていきます。
crates-card-cta = 何をするか →
crates-back = すべての crates
crates-not-found = その名前の crate はありません

# crate ごとのカード
crate-rns-role = エンジン
crate-rns-blurb = どんな Rust プロジェクトにも Reticulum を入れられます。決定的、no_std、アロケータ不要。グローバル状態も組み込み I/O もありません — clock と wire はあなたが用意します。
crate-rnsd-role = daemon
crate-rnsd-blurb = Linux が動く場所ならどこでも動く rnsd のドロップイン。RNS リファレンスと同じ wire です。既存のノードの横で、または代わりに使えます。
crate-lxmf-role = メッセージング
crate-lxmf-blurb = Reticulum の上にある LXMF — Sideband と Nomadnet が乗る層。identity、address、message delivery。
crate-ffi-role = モバイル + Python バインディング
crate-ffi-blurb = 1 つの uniffi interface から Kotlin (.aar)、Swift (.xcframework)、Python (.whl) を生成します。Android、iOS、Jupyter notebook から Reticulum を使えます — 同じ形、同じエンジンです。

# 404
not-found-title = ここにはまだ何もありません。
not-found-cta = ホームへ戻る
