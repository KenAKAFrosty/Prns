# ナビゲーション
nav-contributing = 貢献
nav-api = API

# フッター
footer-tagline = Personal チームがお届けします。

# ランディング
landing-kicker = 人々のための止まらないメッシュネットワーク
landing-kicker-prefix = 人々のための止まらないメッシュネットワーク
landing-title = 安全な Rust で書かれた、高性能な Reticulum (RNS) ポート。
landing-title-lead = A high-performance port of Reticulum (RNS).
landing-title-accent = Runs on any device.
landing-subtitle = 決定的で no_std、アロケータ不要のコア。5ドルのマイクロコントローラからクラウドサーバーまで、あらゆる Reticulum ノードに必要な性能と安定性のために作られています。
landing-cta-ethos = crate を選ぶ
# 引用
landing-quote-label = 私たちが目指しているもの
landing-quote-body = Reticulum は、私たち全員が作り続ける限り手にできる明るい未来の、基礎となる通信インフラです。これは Personal チームが RNS をより多くのビルダーの手に届け、その未来の実現を助けるための取り組みです。

# インターフェース
interfaces-section-label = インターフェース
interfaces-section-title = メッシュが現実世界と出会う場所
interfaces-section-lead = Prns は builder がすでに知っている RNS 互換インターフェースを保ち、新しいデバイスとネットワーク向けのネイティブリンクでその地図を広げます。
interfaces-section-hot-note = Prns のインターフェースはホットスワップ可能です。ノードを再起動せずに、インターフェースを追加、削除、変更できます。

interfaces-radio-label = 無線
interfaces-radio-headline = デバイスとボード向けの近距離リンク
interfaces-radio-body = Bluetooth LE Auto-interface、ESP-NOW、LoRa が、近くのデバイス、ボード群、長距離リンクをひとつの Reticulum メッシュへつなぎます。

interfaces-lan-label = LAN
interfaces-lan-headline = 自動発見されるローカルリンクのピア
interfaces-lan-body = Wi-Fi Auto-interface は multicast、mDNS、gateway rendezvous を使って近くのノードを見つけ、ローカルネットワークをメッシュに取り込みます。

interfaces-cable-label = ケーブル + パケット無線
interfaces-cable-headline = ケーブル、TNC、無線モデム
interfaces-cable-body = USB Auto-interface、シリアルフレーミング、KISS、AX.25、RNode が、小さなデバイスとパケット無線ハードウェアを同じメッシュにつなぎます。

interfaces-host-label = ルーティングされた IP
interfaces-host-headline = Internet、WAN、backbone リンク
interfaces-host-body = TCP client/server、UDP、Backbone により、遠くの peer も private WAN、VPN、public Internet relay 越しにメッシュへ参加できます。

# 信頼できる基準
standards-section-label = 私たちの基準
standards-section-title = 信頼できること
standards-license-label = ライセンス
standards-license-headline = MIT / Apache 2.0
standards-license-body = デュアルライセンスで permissive。コピーレフトや商用利用の制限はありません。
standards-safety-label = 安全性
standards-safety-headline = 強制、そして監査
standards-safety-body = エンジンでは panic、unwrap、根拠のない unsafe は決してコンパイルされません。禁止できないものは監査します。依存関係内の unsafe は cargo-geiger で、未定義動作は Miri で、セキュリティ勧告は cargo-deny で確認します。
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
start-daemon-target = prnsd

start-embedded-headline = Flash a Hopspot
start-embedded-body = Pick a supported board, compare radio and battery tradeoffs, then flash a dedicated mesh device.
start-embedded-code = Board matrix
    Web flasher
    Local flash
start-embedded-target = Flash a Hopspot

start-web-headline = ブラウザノードのプレイグラウンドを使う
start-web-body = 共有 Rust エンジンを WebAssembly で動かす TypeScript API を試し、Auto Wi-Fi または USB Auto で接続して、ローカルノードの動作をリアルタイムに確認できます。
start-web-code = WebAssembly runtime
    Auto Wi-Fi + USB Auto
    TypeScript サンプル
start-web-target = プレイグラウンドを開く

start-rust-headline = Rust アプリに組み込みたい
start-rust-body = そのまま使える完全な RNS runtime、または自分の runtime を組み立てるための純粋なコア。
start-rust-target = prnsd or personal-rns

# プラットフォーム ("Runs on") — ヒーローのマーキーラベル + CTA、専用ページ
landing-platforms-label = Runs on
landing-platforms-cta = See all →
platforms-title = Where Prns runs
platforms-lead = One engine, many homes. This quick view separates runtime platform support from specific Hopspot board support.
platforms-board-support-link = View board support & bring-up →

# Flash a Hopspot page
flash-back = Platforms
flash-back-boards = Boards
flash-card-action = Flash

# ベンチマークページ
benchmarks-kicker = 性能
benchmarks-title = オープンにベンチマーク
benchmarks-lead = 私たちは性能を形容詞ではなく数値として扱います。ここにある数値はすべて、repo 内の決定的なハーネスから得たもので、実機で測定し、公平に比較できる場所では RNS リファレンスとも照合しています。数値は suite が安定するにつれて追加されます。下には、それらが従う方法論を示しています。

# ライセンス表示 (フッター)
footer-license = Open source. MIT / Apache 2.0.
footer-trademarks = 第三者のロゴおよび商標は、それぞれの所有者に帰属します。これらはプラットフォーム、ハードウェア、互換性対象を識別するためだけに表示しています。推奨や承認を主張または示唆するものではありません。

# 貢献ページ
contributing-kicker = 基準
contributing-title = 貢献
contributing-lead = 貢献のしかた — 私たちが大切にしていること、コードが従う規約、そしてすべての変更が満たす基準。人間の貢献者にも自動化された貢献者にも同じです。

# 404
not-found-title = ここにはまだ何もありません。
not-found-cta = ホームへ戻る
