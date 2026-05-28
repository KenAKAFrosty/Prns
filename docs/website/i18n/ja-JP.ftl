# ナビゲーション
nav-ethos = 設計思想
nav-crates = クレート
nav-api = API

# フッター
footer-tagline = 実装ではなく契約を移植する。

# ランディングページ
landing-kicker = Reticulum を忠実に移植
landing-title = 純粋なエンジンが一つ。各プラットフォームが薄いホストを持ち寄る。
landing-subtitle = Reticulum のメッシュネットワーク契約をゼロから Rust で移植。組み込み優先、決定論的、no_std クリーン。小さな Host 接続点があれば、同じエンジンがデーモン、マイコン、スマートフォンで動きます。
landing-cta-ethos = 設計思想を読む
landing-cta-crates = クレート一覧へ
landing-triumvirate-label = 三本柱
landing-quote-label = 設計指針
landing-quote-body = 実装ではなく契約を移植する。純粋なエンジンを一つだけ作り、各プラットフォームには薄いホストを持ち込ませる。

# 三本柱カード
triumvirate-rns-role = 純粋なエンジン
triumvirate-rns-blurb = ワイヤ契約、ルーティング、announce、リンク——純粋な tick/ingest、I/O なし、std 不要。
triumvirate-rnsd-role = デーモンホスト
triumvirate-rnsd-blurb = std ベースの薄い Host アダプタ。プラットフォームがエンジンに命を吹き込む正典的な例。
triumvirate-lxmf-role = メッセージ層
triumvirate-lxmf-blurb = エンジン上の LXMF アプリケーション層——アプリ開発者のための宛先、配信、アイデンティティ。

# 設計思想ページ
ethos-kicker = 構築の流儀
ethos-title = 設計指針
ethos-lead = スイート内のあらゆる意思決定の背後にあるエンジニアリング哲学です。なぜこのアーキテクチャがそうなっているのか——純粋なエンジン、薄いホスト、実装より契約を——を一度読めば理解できます。

# クレート一覧
crates-kicker = スイート
crates-title = クレート一覧
crates-lead = スイートは 6 つのクレートで構成されています。エンジンが基盤、それ以外はすべて薄いホストまたは消費者です。
crates-card-cta = 続きを読む →
crates-back = クレート一覧へ戻る
crates-not-found = その名前のクレートはありません

# 各クレートカード
crate-rns-role = 純粋な Reticulum エンジン
crate-rns-blurb = ワイヤ契約とルーティングを純粋な状態機械として実装。no_std + alloc、決定論的、組み込み優先。
crate-rnsd-role = 参照実装のデーモンホスト
crate-rnsd-blurb = std ベースの Host アダプタと Linux デーモンバイナリ。エンジンを動かすための正典的な例。
crate-lxmf-role = LXMF アプリケーション層
crate-lxmf-blurb = アプリ開発者のための宛先、配信、アイデンティティ。personal-rns の上に乗り、Personal などが利用。
crate-ffi-role = Kotlin / Swift / Python バインディング
crate-ffi-blurb = 一つの UDL から uniffi で 3 言語のバインディングを生成。Rust 以外の利用者のための SDK 入口。
crate-rvt-role = マルチノードシミュレーションと開発ツール
crate-rvt-blurb = Reticulum Visual Toolkit。今は仮想時計のマルチノードシム、まもなくライブデバッガに。Dioxus 製で Web にも展開可能。
crate-esp32c6-role = ESP32-C6 ホストアダプタ
crate-esp32c6-blurb = bare metal 上の no_std/no_main ホスト。エンジンが実機マイコンに収まることの証明。

# 404
not-found-title = ここにはまだ何もありません。
not-found-cta = ホームに戻る
