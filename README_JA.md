<div align="center">

<img src="assets/logo.svg" alt="HeiGate Logo" width="128" />

# HeiGate

[![Go](https://img.shields.io/badge/Go-1.27.0-00ADD8.svg)](https://golang.org/)
[![Vue](https://img.shields.io/badge/Vue-3.4+-4FC08D.svg)](https://vuejs.org/)

**ローカル上流チャネルを管理する macOS デスクトップ AI API ゲートウェイ**

[English](README.md) | [中文](README_CN.md) | 日本語

</div>

> [!IMPORTANT]
> HeiGate は [Wei-Shaw/sub2api](https://github.com/Wei-Shaw/sub2api) を基にした派生プロジェクトです。sub2api の公式リポジトリではなく、独立してメンテナンスされています。帰属とライセンスの詳細は [NOTICE](NOTICE) を参照してください。

## ⚠️ 重要なお知らせ

本プロジェクトをご利用になる前に、以下の内容を必ずよくお読みください：

- **🚨 利用規約のリスク**：本プロジェクトの使用は、Anthropic をはじめとする上流プロバイダーの利用規約に違反する可能性があります。ご利用前に各プロバイダーのユーザー規約を必ずご確認ください。使用により生じるすべてのリスクはユーザーご自身が負うものとします。
- **⚖️ 法令遵守**：お住まいの国または地域の法令を遵守した上で本プロジェクトをご利用ください。いかなる違法な目的での使用も固く禁じます。
- **📖 免責事項**：本プロジェクトは技術的な学習および研究の目的でのみ提供されます。本プロジェクトの使用により生じたアカウントの停止、サービスの中断、データの損失、その他一切の直接的または間接的な損害について、作者は一切の責任を負いません。
- **🏷️ 商標またはサービスの推奨は含まれません**：LGPL はその条件に従った商用を含む利用を許可します。ただし、プロジェクト名、ロゴ、アップストリームアカウント、ホスティングサービス、プロバイダー関係への権利や、メンテナーによる推奨を与えるものではありません。
- **🤝 独立したメンテナンス**：明示的な記載がない限り、このドキュメントの機能、設定例、リンクは HeiGate の説明のみを目的とし、第三者によるスポンサー、提携、代理、推奨を意味しません。


## 概要

HeiGate は、ローカルの上流チャネルを管理し、コーディングクライアントからのリクエストをルーティングする macOS デスクトップ AI API ゲートウェイです。ゲートウェイの認証情報とリクエストデータはデフォルトで端末内に保存されます。

## 機能

- **マルチアカウント管理** - 複数の上流アカウントタイプ（OAuth、APIキー）をサポート
- **APIキー配布** - ユーザー向けの APIキーの生成と管理
- **精密な課金** - トークンレベルの使用量追跡とコスト計算
- **スマートスケジューリング** - スティッキーセッション付きのインテリジェントなアカウント選択
- **同時実行制御** - ユーザーごと・アカウントごとの同時実行数制限
- **レート制限** - 設定可能なリクエスト数およびトークンレート制限
- **管理ダッシュボード** - 監視・管理のための Web インターフェース
- **外部システム連携** - 外部システム（チケット管理など）を iframe 経由で管理ダッシュボードに埋め込み可能


## クイックスタート

### パッケージ版をインストール

```bash
open HeiGate-macOS-arm64-*.dmg
```

GitHub Release には `.dmg`、`.zip`、チェックサムが含まれます。現在のパッケージは Apple シリコン Mac 向けです。

### ソースからビルド

```bash
make build-desktop-macos
open dist/desktop/HeiGate.app
```

## 技術スタック

| コンポーネント | 技術 |
|-----------|------------|
| バックエンド | Go 1.27.0, Gin, Ent |
| フロントエンド | Vue 3.4+, Vite 5+, TailwindCSS |
| データベース | SQLite（ローカルデスクトップデータ） |
| デスクトップシェル | macOS Cocoa + WebKit |

---

## Antigravity サポート

HeiGate は [Antigravity](https://antigravity.so/) アカウントをサポートしています。認証後、Claude および Gemini モデル用の専用エンドポイントが利用可能になります。

### 専用エンドポイント

| エンドポイント | モデル |
|----------|-------|
| `/antigravity/v1/messages` | Claude モデル |
| `/antigravity/v1beta/` | Gemini モデル |

### Claude Code の設定

```bash
export ANTHROPIC_BASE_URL="http://localhost:8080/antigravity"
export ANTHROPIC_AUTH_TOKEN="sk-xxx"
```

### ハイブリッドスケジューリングモード

Antigravity アカウントはオプションの**ハイブリッドスケジューリング**をサポートしています。有効にすると、汎用エンドポイント `/v1/messages` および `/v1beta/` も Antigravity アカウントにリクエストをルーティングします。

> **⚠️ 警告**: Anthropic Claude と Antigravity Claude は**同じ会話コンテキスト内で混在させることはできません**。グループを使用して適切に分離してください。

---

## プロジェクト構成

```
HeiGate/
├── backend/                  # Go バックエンドサービス
│   ├── cmd/server/           # アプリケーションエントリ
│   ├── internal/             # 内部モジュール
│   │   ├── config/           # 設定
│   │   ├── model/            # データモデル
│   │   ├── service/          # ビジネスロジック
│   │   ├── handler/          # HTTP ハンドラー
│   │   └── gateway/          # API ゲートウェイコア
│   └── resources/            # 静的リソース
│
├── frontend/                 # Vue 3 フロントエンド
│   └── src/
│       ├── api/              # API 呼び出し
│       ├── stores/           # 状態管理
│       ├── views/            # ページコンポーネント
│       └── components/       # 再利用可能なコンポーネント
│
├── desktop/macos/            # ネイティブ macOS アプリシェル
└── .github/workflows/        # 自動デスクトップリリースワークフロー
```

## スター履歴

<a href="https://star-history.dera.page/#Heiheiaoo/HeiGate&Date">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://star-history.dera.page/svg?repos=Heiheiaoo/HeiGate&type=Date&theme=dark" />
   <source media="(prefers-color-scheme: light)" srcset="https://star-history.dera.page/svg?repos=Heiheiaoo/HeiGate&type=Date" />
   <img alt="Star History Chart" src="https://star-history.dera.page/svg?repos=Heiheiaoo/HeiGate&type=Date" />
 </picture>
</a>

---

## ライセンスと帰属

本プロジェクトは [GNU Lesser General Public License v3.0](LICENSE)（またはそれ以降のバージョン）の下でライセンスされています。

HeiGate には [Wei-Shaw/sub2api](https://github.com/Wei-Shaw/sub2api) を基にした作品が含まれます。元の著作権表示はそれぞれの権利者に帰属します。詳細は [NOTICE](NOTICE) を参照してください。

---

<div align="center">

**このプロジェクトが役に立ったら、ぜひスターをお願いします！**

</div>
