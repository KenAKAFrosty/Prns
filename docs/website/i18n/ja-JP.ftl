# ナビゲーション
nav-benchmarks = ベンチマーク
nav-contributing = 貢献
site-early-english-note = Prns はまだ初期段階です。完全なドキュメントは GitHub とソースコードにあり、今のところ英語のみです。

# フッター
footer-tagline = KenAKAFrosty と Personal/Prns チームがお届けします。
footer-flash = Hopspot をフラッシュ（英語のみ）
footer-playground = ブラウザプレイグラウンド（英語のみ）

# ランディング
landing-kicker = あなたのものになるメッシュネットワーク
landing-kicker-prefix = あなたのものになるメッシュネットワーク
landing-title = あらゆるデバイスで動くように作られた、高性能な Reticulum (RNS)。
landing-title-lead = 高性能な Reticulum (RNS)、
landing-title-accent = あらゆるデバイスで動くように。
landing-subtitle = 5ドルのマイクロコントローラからクラウドサーバークラスタまで、あらゆる Reticulum ノードに必要な性能、安定性、エネルギー効率のために作られています。ひとつのエンジンとひとつの API が、組み込み、デスクトップ、モバイル、ゲーム、Web で同じように動きます。
landing-cta-ethos = Prns で自分の道を見つける
landing-cta-standards = 私たちの基準
# 引用
landing-quote-label = 私たちが目指しているもの
landing-quote-body = Reticulum は、私たち全員が作り続ける限り手にできる明るい未来の、基礎となる通信インフラです。これは Personal チームが RNS をより多くのビルダーの手に届け、その未来の実現を助けるための取り組みです。

# インターフェース
interfaces-section-label = インターフェース
interfaces-section-title = メッシュが現実世界と出会う場所
interfaces-section-lead = Prns はビルダーがすでに知っている RNS 互換インターフェースを保ち、新しいデバイスとネットワーク向けのネイティブリンクでその地図を広げます。
interfaces-section-hot-note = Prns のインターフェースはホットスワップ可能です。ノードを再起動せずに、インターフェースを追加、削除、変更できます。

interfaces-radio-label = 無線
interfaces-radio-headline = デバイスとボード向けの近距離リンク
interfaces-radio-body = Bluetooth LE Auto-interface、ESP-NOW、LoRa が、近くのデバイス、ボード群、長距離 RF リンクをひとつの Reticulum メッシュへつなぎます。

interfaces-lan-label = LAN
interfaces-lan-headline = 自動発見されるローカルリンクのピア
interfaces-lan-body = Wi-Fi Auto-interface は multicast、mDNS、gateway rendezvous を使って近くのノードを見つけ、ローカルネットワークをメッシュに取り込みます。

interfaces-cable-label = ケーブル + パケット無線
interfaces-cable-headline = ケーブル、TNC、無線モデム
interfaces-cable-body = USB Auto-interface、シリアルフレーミング、KISS、AX.25、RNode が、小さなデバイスとパケット無線ハードウェアを同じメッシュにつなぎます。

interfaces-host-label = ルーティングされた IP
interfaces-host-headline = Internet、WAN、backbone リンク
interfaces-host-body = TCP client/server、UDP、WebSocket、Backbone により、遠くのピアも private WAN、VPN、public Internet relay、ブラウザ統合を通じてメッシュへ参加できます。

# 信頼できる基準
standards-section-label = 私たちの基準
standards-section-title = 信頼できること
standards-license-label = ライセンス
standards-license-headline = MIT / Apache 2.0
standards-license-body = パーミッシブなデュアルライセンスです。コピーレフトや商用利用の制限はありません。
standards-safety-label = 安全性
standards-safety-headline = まず強制、そして監査
standards-safety-body = エンジンでは panic、unwrap、根拠のない unsafe は決してコンパイルされません。禁止できないものは監査します。依存関係内の unsafe は cargo-geiger で、未定義動作は Miri で、セキュリティ勧告は cargo-deny で確認します。
standards-correctness-label = 正しさ
standards-correctness-headline = RNS との差分テスト済み
standards-correctness-body = すべての変更をリファレンスと照合し、そのうえでユニットテスト、プロパティテスト、ファズテスト、ミューテーションテストにかけ、重要な箇所では Kani の証明も使います。
standards-benchmarked-label = 性能
standards-benchmarked-headline = 主張ではなく測定
standards-benchmarked-body = 性能は公開された形で追跡され、自分でも実行できるハーネスで測定されます。
standards-benchmarked-cta = ベンチマークを見る →

# どこから始める？
start-section-label = 入り口
start-section-title = ここで何をしますか？
start-section-lead = Prns が自分の仕事にどう入るかに合わせて道を選んでください。フラッシュするハードウェア、動かすインフラ、作るソフトウェアのどれかです。

start-daemon-headline = daemon を動かす
start-daemon-body = デスクトップ、LXMF アプリ、backbone VPS などのための高速な Reticulum daemon をインストールします。
start-daemon-code = 既存アプリにドロップイン
    ~/.reticulum を読み込み
    インターフェースをライブ編集
    メトリクス内蔵
start-daemon-target = Prnsd を実行

start-embedded-headline = Hopspot をフラッシュする
start-embedded-body = 対応ボードを選び、無線とバッテリーのトレードオフを比較して、専用メッシュデバイスをフラッシュします。
start-embedded-code = ボードマトリクス
    Web フラッシャー
    ローカルフラッシュ
start-embedded-target = Hopspot をフラッシュ（英語のみ）

start-web-headline = ブラウザノードのプレイグラウンドを使う
start-web-body = 共有 Rust エンジンを WebAssembly で動かす TypeScript API を試し、Auto Wi-Fi または USB Auto で接続して、ローカルノードの動作をリアルタイムに確認できます。
start-web-code = WebAssembly runtime
    Auto Wi-Fi + USB Auto
    TypeScript サンプル
start-web-target = プレイグラウンドを開く（英語のみ）

start-rust-headline = Reticulum の上に構築する
start-rust-body = エンジンとバインディングで、アプリ、ツール、サービス、ゲームにメッシュネットワークを組み込めます。
start-rust-target = README を読む
start-rust-target-source = ソースをダウンロード

# プラットフォーム ("Runs on") — ヒーローのマーキーラベル + CTA、専用ページ
landing-platforms-label = 動作環境
landing-platforms-cta = すべて見る →
platforms-title = Prns が動く場所
platforms-lead = ひとつのエンジン、たくさんの居場所。このクイックビューは、ランタイムのプラットフォーム対応と、個々の Hopspot ボード対応を分けて示します。
platforms-board-support-link = ボード対応と bring-up を見る →

# Hopspot フラッシュページ
flash-back = プラットフォーム
flash-back-boards = ボード
flash-card-action = フラッシュ

# ベンチマークページ
benchmarks-kicker = 性能
benchmarks-title = 公開の場でベンチマーク
benchmarks-lead = 以下の数値はすべて、repo に公開された結果に基づいており、自分でも実行できるハーネスによって実機で測定されています。ここから先の内容は、今のところ英語のみです。

# ライセンス表示 (フッター)
footer-license = オープンソース。MIT / Apache 2.0。
footer-trademarks = 第三者のロゴ、商標、製品画像は、それぞれの所有者に帰属します。これらはプラットフォーム、ハードウェア、互換性対象を識別するためだけに表示しています。推奨や承認を主張または示唆するものではありません。

# 404
not-found-title = ここにはまだ何もありません。
not-found-cta = ホームへ戻る
