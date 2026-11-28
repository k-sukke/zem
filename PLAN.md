# zem (Zephyr Environment Manager) — MVP 実装計画

> ステータス: **Phase 3 完了**。`init → sync → build` が esp32 / esp32s3 / esp32c3 / esp32c6 で通り、ESP32 実機への書き込みと起動を確認済み。実装メモは各 Phase の「実装結果」、スパイクの結果は §9
> 対象ボード: ESP32 系（Xtensa: ESP32 / S2 / S3、RISC-V: C3 / C6 / H2）
> 対象 Zephyr: **v4.0 以降**
> ホスト OS: **設計はクロスプラットフォーム前提**（macOS / Linux / Windows）。**MVP の動作保証は macOS と Linux のみ**で、Windows 対応は MVP 後の Phase 5 で行う

---

## 0. 設計方針

| 方針 | 内容 |
|---|---|
| オーケストレーターに徹する | `west` / `uv` / `git` / `cmake` などを再実装しない。zem が担うのは「何を・どこに・どの環境変数で」実行するかの決定だけ |
| 宣言的・冪等 | `zem.toml` が唯一の入力。`zem sync` は何度実行しても同じ状態に収束し、未完了の部分だけを実行する |
| 隔離が既定 | 子プロセスには毎回 zem が組み立てた環境を渡す。ユーザーのシェルや `~/.westconfig`、CMake のユーザーパッケージレジストリ、ESP-IDF の環境変数には依存も変更もしない |
| **ZEM_HOME への一元化** | zem が作るもの（SDK、Zephyr ワークスペース、venv、各種キャッシュ）は**すべて `ZEM_HOME`（既定 `~/.zem/`）配下**に置く。zem の子プロセスが勝手に作るキャッシュ（uv、pip、ccache、Zephyr の CMake キャッシュ）も環境変数で `ZEM_HOME` 配下へ誘導する |
| **複数プロジェクトで共有** | Zephyr ワークスペース・venv・SDK は内容から決まるキー（content-addressed）で `ZEM_HOME` に置き、同じ構成のプロジェクト間で共有する。HAL などモジュールの中身もすべてグローバル側に置く |
| **プロジェクト側 `.zem/` は軽量ファイルのみ** | プロジェクト直下の `.zem/` には、解決結果・起動スクリプト・ログなど、小さくて**いつ消してもネットワークなしで再生成できる**ものだけを置く（§4.3） |
| **クロスプラットフォーム前提** | OS 依存のコードは `platform` モジュールに閉じ込める。Windows でも Phase 1 からコンパイルと単体テストを CI で通し、unix 専用のコードが他のモジュールに漏れていないことを継続的に保証する |
| 対応バージョンを絞る | Zephyr v4.0 以降のみ（HWMv2 のボード名修飾子が前提）。初期の検証対象は v4.2.0 |

---

## 1. ディレクトリ・モジュール構成

テストしやすいように `main.rs` は薄くし、ロジックはすべて `lib.rs` 配下に置く。

```
zem/
├── Cargo.toml
├── PLAN.md
├── src/
│   ├── main.rs              # エントリポイント。clap のパース → cli::dispatch のみ
│   ├── lib.rs
│   ├── cli/                 # サブコマンド定義（ここは薄いレイヤに留める）
│   │   ├── mod.rs           # clap derive・共通オプション（--verbose, --project-dir, --offline）
│   │   ├── init.rs
│   │   ├── sync.rs
│   │   ├── build.rs
│   │   ├── flash.rs
│   │   ├── env.rs           # `zem env`（シェル用 export を出力）
│   │   ├── run.rs           # `zem run -- <cmd>`（隔離環境で任意コマンドを実行）
│   │   ├── doctor.rs
│   │   └── cache.rs         # `zem cache list / gc`（Phase 4）
│   ├── config/              # zem.toml
│   │   ├── mod.rs
│   │   ├── schema.rs        # serde 構造体
│   │   ├── load.rs          # カレントから上位へ zem.toml を探索・読込
│   │   ├── validate.rs      # 意味検証（ボード名・バージョン形式・feature 名）
│   │   └── template.rs      # init 用の生成（toml_edit でコメント付き出力）
│   ├── board/               # ボード名 → SoC / アーキテクチャ / ツールチェーンの解決
│   │   ├── mod.rs           # ResolvedTarget 型
│   │   ├── qualifier.rs     # "esp32s3_devkitc/esp32s3/procpu" の分解
│   │   ├── espressif.rs     # 静的テーブル
│   │   ├── board_yml.rs     # sync 後に zephyr/boards/**/board.yml で裏取り
│   │   └── resolve.rs
│   ├── manifest/
│   │   ├── mod.rs
│   │   ├── features.rs      # feature 名 → west プロジェクト名
│   │   └── west_yml.rs      # 最小 west.yml の生成（serde_yaml）
│   ├── home/                # ZEM_HOME
│   │   ├── mod.rs           # ZemHome（ルートの決定とサブディレクトリのパス）
│   │   └── keys.rs          # 共有コンポーネントのキー算出（正規化 + sha256 の先頭 8 桁）
│   ├── project/             # プロジェクト側 .zem/（軽量ファイルのみ）
│   │   ├── mod.rs           # ProjectDir（.zem/ のパス、ZEM_HOME との衝突チェック）
│   │   ├── state.rs         # state.json の読み書き（config_hash による鮮度判定）
│   │   └── activate.rs      # activate.sh / activate.fish の生成
│   ├── store/               # ZEM_HOME 上の共有コンポーネント
│   │   ├── mod.rs           # Store トレイト・完了マーカー・ロック
│   │   ├── workspace.rs     # west init / update / モジュールの追加 / blobs
│   │   ├── venv.rs          # Python 本体 + venv + 依存
│   │   ├── sdk.rs           # Zephyr SDK とツールチェーン
│   │   └── registry.rs      # プロジェクト → 使用中コンポーネントの台帳（gc 用）
│   ├── sync/
│   │   ├── mod.rs           # EnvSpec に従って各 Store を順に ensure する
│   │   └── plan.rs          # zem.toml → EnvSpec の解決と --dry-run の表示
│   ├── env/
│   │   ├── mod.rs           # EnvBuilder（最終的な環境変数の組み立て）
│   │   ├── sanitize.rs      # 除外する変数と PATH のフィルタ
│   │   └── shell.rs         # ShellKind ごとの出力（bash/zsh/fish/json、将来 pwsh/cmd）
│   ├── platform/            # ★ OS 依存コードはここにしか書かない
│   │   ├── mod.rs           # OS 非依存の API（下記 §7 参照）
│   │   ├── unix.rs
│   │   └── windows.rs       # MVP ではコンパイルが通る最小実装 + Unsupported
│   ├── tools/               # 外部ツールの検出・バージョン取得・起動
│   │   ├── mod.rs           # ExternalTool トレイト（which + version probe）
│   │   ├── uv.rs
│   │   ├── west.rs
│   │   ├── git.rs
│   │   └── cmake.rs
│   ├── doctor/
│   │   ├── mod.rs
│   │   └── checks.rs        # 各チェック（OK / 警告 / 失敗 + OS 別の対処案内）
│   ├── process.rs           # Command ラッパ（ログ・エラー整形・置き換え起動）
│   ├── lock.rs              # ファイルロック（fd-lock、全 OS 対応）
│   ├── fsutil.rs            # アトミック書き込み（一時ファイル + rename、Windows 向けの再試行）
│   ├── ui.rs                # 進捗表示（indicatif）・プロンプト（dialoguer）
│   └── error.rs             # thiserror によるドメインエラー + miette で表示
└── tests/
    ├── cli_init.rs          # assert_cmd + tempfile
    ├── manifest_snapshot.rs # insta
    ├── keys.rs              # キーの安定性（OS・パス表記に依存しないこと）
    ├── env_sanitize.rs
    └── e2e.rs               # #[ignore]。ネットワークと実ツールが必要
```

### 主要クレート候補

| 用途 | クレート |
|---|---|
| CLI | `clap`（derive） |
| 設定 | `serde`、`toml`、`toml_edit`、`serde_yaml`、`serde_json` |
| エラー | `thiserror`、`miette` |
| UX | `dialoguer`、`indicatif`、`console` |
| パス・ツール | `home`（ホームディレクトリ）、`which`、`dunce`（Windows で `\\?\` 付きにならない canonicalize） |
| 冪等性・共有 | `sha2`、`fd-lock`、`tempfile` |
| プロセス | Windows 向けに `ctrlc`（親プロセスが Ctrl-C を無視し、子に処理を任せるため） |
| ログ | `tracing`、`tracing-subscriber` |
| テスト | `assert_cmd`、`predicates`、`insta`、`tempfile` |

すべて macOS / Linux / Windows に対応したクレートに限る。HTTP クライアントは **Zephyr SDK の取得専用**に導入する（§9 の結果。候補: `ureq` + rustls、展開に `tar` + xz、Windows 用に 7z）。それ以外のダウンロードは `west` / `uv` / `git` に任せる。

### sync の中核: EnvSpec と Store

`zem.toml` から「このプロジェクトに必要な環境」を表す `EnvSpec` を解決し、各共有コンポーネント（Store）を順に用意する。

```rust
/// zem.toml から決まる、このプロジェクトに必要な環境の完全な記述
pub struct EnvSpec {
    pub target: ResolvedTarget,
    pub workspace: WorkspaceKey,        // (zephyr url, revision)
    pub modules: BTreeSet<String>,      // このプロジェクトが使うモジュール（hal_espressif + features + extra）
    pub venv: VenvKey,                  // (WorkspaceKey, python version, extra_requirements, install_cmake_ninja)
    pub sdk: SdkKey,                    // (SDK version, toolchains)
}

pub trait Store {
    type Key;
    fn path(&self, home: &ZemHome, key: &Self::Key) -> PathBuf;
    /// 完了マーカー（各ディレクトリ直下の .zem-store.json）を見て、要求を満たしているか判定する
    fn is_ready(&self, home: &ZemHome, key: &Self::Key, req: &Requirement) -> Result<bool>;
    /// 排他ロックを取ってから不足分だけを用意する。何度呼んでも安全
    fn ensure(&self, home: &ZemHome, key: &Self::Key, req: &Requirement) -> Result<()>;
}
```

sync の手順:

1. **venv（bootstrap）**: venv を作成し `west`（`install_cmake_ninja` なら `cmake` / `ninja` も）を入れる
2. **workspace**: 共有ワークスペースを用意し、足りないモジュールだけを追加取得する
3. **ボードの裏取り**: `board.yml` と照合する
4. **venv（requirements）**: `zephyr/scripts/requirements-base.txt` を入れる（esptool は hal_espressif 同梱版が使われるので入れない。§9）
5. **blobs**: `west blobs fetch hal_espressif`（ワークスペースごとに 1 回）
6. **sdk**: 必要なツールチェーンだけを追加する
7. **registry**: このプロジェクトが使っているキーを台帳に記録する

8. **project**: プロジェクト側 `.zem/` に、解決済みの EnvSpec（`state.json`）と起動スクリプトを書き出す

状態の正（source of truth）は Store 側の完了マーカーにある。プロジェクト側の `.zem/state.json` は「前回の sync で何に解決されたか」のキャッシュにすぎない。`zem build` などはまず `zem.toml` のハッシュと `state.json` を比べ、一致していれば各 Store の `is_ready` の軽い確認だけで起動する。一致しない、あるいは `.zem/` が無い場合は「`zem sync` を実行してください」と案内する。

---

## 2. `zem.toml` スキーマ

### 2.1 例

```toml
schema = 1                      # スキーマのバージョン（将来の移行用）

[project]
name = "blinky"                 # アプリ（CMakeLists.txt / prj.conf）は zem.toml と同じディレクトリに置く

[zephyr]
version = "v4.2.0"              # タグまたはコミット SHA を推奨（ブランチ指定は警告）
# url = "https://github.com/zephyrproject-rtos/zephyr"   # フォーク利用時のみ

[target]
board = "esp32s3_devkitc/esp32s3/procpu"
# arch = "xtensa"               # 自動判別を上書きしたい場合だけ指定

[modules]
features = ["mbedtls"]          # 抽象名 → west プロジェクト名に展開
extra = []                      # west.yml のプロジェクト名をそのまま追加（逃げ道）
# hal_espressif はターゲットから自動追加される

[blobs]
accept_licenses = false         # true の場合だけ非対話でライセンスに同意する

[sdk]
version = "auto"                # "auto" なら zephyr/SDK_VERSION に従う

[python]
version = "3.12"
install_cmake_ninja = true      # cmake / ninja を PyPI ホイールで project venv に入れる
extra_requirements = []

[build]
dir = "build"
cmake_args = []

[env]
passthrough = []                # 隔離時にも残したい変数（例: "HTTPS_PROXY"）
set = {}                        # 追加で設定する変数
```

v1 からの変更点: `[sdk].location` を削除した（SDK は常に `ZEM_HOME` 配下）。

### 2.2 Rust 構造体（案）

```rust
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZemConfig {
    pub schema: u32,
    pub project: Project,
    pub zephyr: Zephyr,
    pub target: Target,
    #[serde(default)] pub modules: Modules,
    #[serde(default)] pub blobs: Blobs,
    #[serde(default)] pub sdk: Sdk,
    #[serde(default)] pub python: Python,
    #[serde(default)] pub build: Build,
    #[serde(default)] pub env: EnvConfig,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub name: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Zephyr {
    pub version: String,          // v4.0 未満のタグは validate.rs でエラー
    #[serde(default = "default_zephyr_url")] pub url: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub board: String,
    pub arch: Option<Arch>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Arch { Xtensa, Riscv }

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Modules {
    #[serde(default)] pub features: Vec<Feature>,
    #[serde(default)] pub extra: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum Feature { Mbedtls, Lvgl, Littlefs, Mcuboot, Segger }

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Blobs { #[serde(default)] pub accept_licenses: bool }

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Sdk { #[serde(default)] pub version: SdkVersion }   // Auto | Exact(String)

#[derive(Debug, Deserialize, Serialize)]
pub struct Python {
    pub version: String,
    pub install_cmake_ninja: bool,
    pub extra_requirements: Vec<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Flash { pub port: Option<String>, pub baud: Option<u32> }

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct EnvConfig {
    #[serde(default)] pub passthrough: Vec<String>,
    #[serde(default)] pub set: BTreeMap<String, String>,
}
```

- 未知のキーは `deny_unknown_fields` でエラーにする（タイポの早期検出）。
- `zem.toml` 内のパスは常に `/` 区切りの相対パスとする（`RelPath` 型）。Windows のユーザーが書いた `\` も読込時に受け付けるが、`init` などで書き出すときは `/` にする。これで同じ `zem.toml` を全 OS で共有できる。
- CLI 引数で上書きできる項目は `--board` のみ。書き込みポートなど west のランナー固有の設定は zem で持たず、`zem west flash <west flash 引数>` か `[env.set]`（`ESPTOOL_PORT` など）で渡す。

---

## 3. ボード名 → アーキテクチャ / ツールチェーン解決仕様

### 3.1 解決は 2 段階で行う

1. **静的解決（sync 前、ネットワーク不要）**
   - ボード文字列を `name[@revision][/soc[/cpucluster[/variant]]]` として分解する。
   - SoC は次の優先順で決める: ① 修飾子の 2 番目の要素 → ② 既知ボードの静的テーブル → ③ ボード名の接頭辞（`esp32c3_*` → `esp32c3`）。
   - SoC から、アーキテクチャと必要なツールチェーンを表で引く。
   - 用途: `init` 時の検証と、sync で何をインストールするかの決定。
2. **裏取り（ワークスペース準備後）**
   - `zephyr/boards/espressif/*/board.yml` でボード・SoC・CPU クラスタの実在を確認する。
   - 静的解決と食い違う場合は **board.yml を正とし**、警告を出す。
   - 用途: Zephyr のバージョン間でのボード名変更への耐性（例: `esp32_devkitc_wroom` → `esp32_devkitc`）。

### 3.2 SoC → アーキテクチャ → ツールチェーン

| SoC | アーキテクチャ | Zephyr SDK のツールチェーン | 備考 |
|---|---|---|---|
| `esp32` | xtensa | `xtensa-espressif_esp32_zephyr-elf` | `procpu` / `appcpu` |
| `esp32s2` | xtensa | `xtensa-espressif_esp32s2_zephyr-elf` | シングルコア |
| `esp32s3` | xtensa | `xtensa-espressif_esp32s3_zephyr-elf` | `procpu` / `appcpu` |
| `esp32c2` | riscv | `riscv64-zephyr-elf` | 一応対応（未検証扱い） |
| `esp32c3` | riscv | `riscv64-zephyr-elf` | |
| `esp32c6` | riscv | `riscv64-zephyr-elf` | `hpcore` / `lpcore` とも同じ |
| `esp32h2` | riscv | `riscv64-zephyr-elf` | **Zephyr v4.2.0 には存在しない**（§9） |

### 3.3 静的テーブル（初期値）

| ボード（完全修飾の例） | SoC |
|---|---|
| `esp32_devkitc/esp32/procpu` | esp32 |
| `esp32s2_saola` | esp32s2 |
| `esp32s3_devkitc/esp32s3/procpu` | esp32s3 |
| `esp32s3_devkitm/esp32s3/procpu` | esp32s3 |
| `esp32c3_devkitm` | esp32c3 |
| `esp32c6_devkitc/esp32c6/hpcore` | esp32c6 |
| `esp32h2_devkitm` | esp32h2 |

> この表は Phase 1 で対象 Zephyr バージョン（v4.0 / v4.1 / v4.2 …）の `boards/espressif` と突き合わせて確定させる。

### 3.4 解決結果の型

```rust
pub struct ResolvedTarget {
    pub board: String,              // west build -b にそのまま渡す
    pub soc: Soc,                   // Esp32 | Esp32s2 | Esp32s3 | Esp32c2 | Esp32c3 | Esp32c6 | Esp32h2
    pub arch: Arch,
    pub toolchains: Vec<&'static str>,   // 将来の sysbuild / LP コアに備えて Vec
    pub hal_modules: Vec<&'static str>,  // ["hal_espressif"]
    pub blob_modules: Vec<&'static str>, // ["hal_espressif"]
    pub source: ResolutionSource,   // Qualifier | StaticTable | Heuristic | BoardYml
}
```

ESP32 以外のボードは「MVP では未対応」として明確なエラーを返す（推測では進めない）。

---

## 4. ZEM_HOME のレイアウトと共有・分離の設計

### 4.1 ZEM_HOME の決定

| 優先順位 | 値 |
|---|---|
| 1 | 環境変数 `ZEM_HOME`（CI や完全に閉じた環境が欲しい場合は `ZEM_HOME=./.zem-home` のように指定できる。プロジェクト側 `.zem/` と同じ場所はエラー） |
| 2 | 既定値: `~/.zem`（macOS / Linux）、`%USERPROFILE%\.zem`（Windows） |

OS 標準のキャッシュディレクトリ（`~/Library/Caches` や `%LOCALAPPDATA%`）は**あえて使わない**。全 OS で同じ構造の 1 か所にまとめ、ユーザーが場所を把握でき、丸ごと削除すれば完全に元に戻せることを優先する。

### 4.2 ディレクトリレイアウト

```
<project>/
├── zem.toml
├── zem.lock                        # sync が生成（コミット対象）。zephyr / モジュールの SHA、SDK、Python パッケージ
├── CMakeLists.txt, prj.conf, src/ ...
├── build/                          # west build の出力（[build].dir）。init が .gitignore に追加
└── .zem/                           # 軽量ファイルのみ（§4.3）。中に "*" だけの .gitignore を置き、自身を無視させる
    ├── .gitignore                  # 内容は "*"
    ├── state.json                  # 解決済み EnvSpec・各 Store の絶対パス・zem.toml のハッシュ
    ├── activate.sh                 # source 用（zem env の出力を固定化したもの）。fish 用も生成。Windows 用は Phase 5
    ├── activate.fish
    └── logs/
        └── sync-last.log           # 直近の sync ログ（1 世代のみ保持）

~/.zem/                             # ZEM_HOME。zem が作るものはすべてここ
├── config.toml                     # 将来用（グローバル設定）。MVP では読むだけで未使用
├── projects.json                   # 台帳: プロジェクトのパス → 使用中のキー・最終同期日時
├── workspaces/
│   └── zephyr-v4.2.0-1a2b3c4d/     # キー = (url, revision) の slug + ハッシュ先頭 8 桁
│       ├── .zem-store.json         # 完了マーカー（取得済みモジュール一覧、blobs 取得済みか など）
│       ├── .west/config
│       ├── manifest/west.yml       # zem が生成（取得済みモジュールの和集合）
│       ├── zephyr/
│       └── modules/hal/espressif/  # blobs も含む
├── venvs/
│   ├── py3.12-5e6f7a8b/            # キー = (workspace キー, python, extras, install_cmake_ninja)
│   └── py3.12-5e6f7a8b-9c0d1e2f/   # zem.lock のパッケージが共有 venv と違う場合だけ作る専用 venv
├── sdk/
│   └── zephyr-sdk-0.17.x/          # ツールチェーンは必要な分だけ追加される
├── python/                         # uv が管理する Python 本体（UV_PYTHON_INSTALL_DIR）
├── tools/                          # 将来用: zem が取得するホストツール（Windows の dtc など）
├── cache/
│   ├── git/                        # 将来用: git ミラー（west の --name-cache は既存キャッシュを読むだけで作らないため、zem が管理する。Phase 4）
│   ├── downloads/                  # SDK の sha256.sum（アーカイブは展開後に削除）
│   ├── uv/                         # UV_CACHE_DIR
│   ├── ccache/                     # CCACHE_DIR
│   └── zephyr/                     # Zephyr の CMake ユーザーキャッシュ（-DUSER_CACHE_DIR）
├── westconfig                      # WEST_CONFIG_GLOBAL / SYSTEM の差し替え先（空ファイル）
├── trash/                          # gc が削除前に Store を移す場所（削除が中断しても壊れた Store を残さない）
└── locks/                          # Store ごとの排他ロック
```

パスはディレクトリ名を短く（ハッシュ 8 桁）保つ。Windows の 260 文字制限への備え。

### 4.3 プロジェクト側 `.zem/` のルール

プロジェクト側 `.zem/` は「このプロジェクトがグローバル側のどれを使っているか」を指し示す軽量な索引であり、実体は置かない。

| ルール | 内容 |
|---|---|
| 置いてよいもの | 解決結果（`state.json`）、起動スクリプト（`activate.*`）、直近のログ。いずれも小さなテキストファイル |
| 置かないもの | ダウンロードしたもの・展開したもの全般（Zephyr 本体、HAL、blobs、venv、SDK、git オブジェクト）。これらは必ず ZEM_HOME 側に置く |
| 再生成可能性 | `.zem/` を丸ごと消しても、グローバル側が揃っていれば `zem sync` がネットワークなしで数秒で復元できる |
| サイズ上限 | 目安として全体で 1 MB 未満。ログは 1 世代のみ保持し、サイズ上限で切り詰める |
| バージョン管理 | `.zem/.gitignore`（内容 `*`）で自身を無視させる。ルートの `.gitignore` を書き換えなくても誤コミットされない |
| シンボリックリンク | 使わない（Windows 対策）。グローバル側の場所は `state.json` に絶対パスで記録する |

**`state.json` の例**

```json
{
  "zem_version": "0.1.0",
  "config_hash": "sha256:…",
  "zem_home": "/Users/me/.zem",
  "target": { "board": "esp32s3_devkitc/esp32s3/procpu", "soc": "esp32s3", "arch": "xtensa", "source": "board_yml" },
  "workspace": { "key": "zephyr-v4.2.0-1a2b3c4d", "path": "/Users/me/.zem/workspaces/zephyr-v4.2.0-1a2b3c4d" },
  "modules": {
    "hal_espressif": "/Users/me/.zem/workspaces/zephyr-v4.2.0-1a2b3c4d/modules/hal/espressif"
  },
  "venv": { "key": "py3.12-5e6f7a8b", "path": "/Users/me/.zem/venvs/py3.12-5e6f7a8b" },
  "sdk": { "version": "0.17.x", "path": "/Users/me/.zem/sdk/zephyr-sdk-0.17.x", "toolchains": ["xtensa-espressif_esp32s3_zephyr-elf"] },
  "synced_at": "2026-11-25T10:40:00+09:00"
}
```

`modules` のキーは、プロジェクト用 west 設定の `manifest.project-filter` の組み立てに使う（§4.6）。

**ZEM_HOME との衝突**: 既定の ZEM_HOME（`~/.zem`）とプロジェクト側 `.zem/` は名前が同じである。そのため、ホームディレクトリ自体をプロジェクトルートにした場合（`~/zem.toml`）は、`init` と `sync` をエラーにする。また、`zem.toml` を探して上位ディレクトリをたどるときも、ホームディレクトリより上には行かない。

### 4.4 共有ワークスペースの設計

**キーは `(zephyr url, revision)` のみ**とし、モジュールは**和集合で追加していく**。

- プロジェクト A（hal_espressif）とプロジェクト B（hal_espressif + mbedtls）が同じ v4.2.0 を使う場合、ワークスペースは 1 つ。B の sync で mbedtls が追加される。
- 追加時は、和集合で west.yml を再生成し、`west update --narrow -o=--depth=1` を**引数なし**で実行する。import 経由のプロジェクトは名前を指定した `west update <名前>` を west が拒否するため。既存のモジュールは SHA 固定なので再取得されず、HEAD も変わらない（スパイクで確認済み。no-op 時は約 1.6 秒）。
- モジュールのリビジョンは zephyr の west.yml で固定されているため、和集合にしても各モジュールの中身は一意に決まる。
- **ビルド時は、そのプロジェクトのモジュールだけに絞る**: プロジェクト用 west 設定の `manifest.project-filter`（`-.*,+zephyr,+<モジュール>...`）で、他プロジェクト用のモジュールを inactive にする。Zephyr のモジュール列挙（`zephyr_module.py`）は inactive なプロジェクトを除外するので、ビルドには混ざらない。
- 生成する west.yml（例: 和集合が hal_espressif + mbedtls の場合）:

```yaml
manifest:
  version: "1.2"
  remotes:
    - name: zephyrproject-rtos
      url-base: https://github.com/zephyrproject-rtos
  projects:
    - name: zephyr
      remote: zephyrproject-rtos
      revision: v4.2.0
      clone-depth: 1          # タグ指定のときだけ付ける（SHA 指定では付けない）
      import:
        name-allowlist:
          - hal_espressif
          - mbedtls
  self:
    path: manifest
```

**ワークスペースの不変条件**（共有を安全にするためのルール）

1. 一度作ったワークスペースの zephyr リビジョンは変えない（別リビジョンは別ワークスペース）。
2. モジュールは追加のみ。削除は `zem cache gc` だけが行い、台帳から誰も参照していないことを確認してからにする。
3. ワークスペースへの書き込み（init / update / blobs）は、そのワークスペースの排他ロックを取ってから行う。ビルドはワークスペースを読むだけなので、ロックを取らない。
4. ビルド成果物は必ずプロジェクト側の `build/` に出す。共有領域には何も書き込まない。
5. ブランチ指定（`main` など）は、共有したまま動くと他プロジェクトに影響する。そのため sync 時にコミットを解決してワークスペースのキーを SHA にし、`zem.lock` に記録する。新しいコミットに進むのは `zem sync --update` を明示したときだけ（Phase 4 で実装済み）。

> **検証済み（§9）**: `-DZEPHYR_MODULES` を明示すると、指定したモジュールだけが使われる。逆に指定しないと、他プロジェクトが追加したモジュールがビルドに混入する（`CONFIG_ZEPHYR_MBEDTLS_MODULE=y` が生える）。そのため、この指定は**必須**とする。パスは canonicalize して渡す（`/tmp` と `/private/tmp` の表記揺れで `.config` が変わるため）。

### 4.5 venv と SDK の共有

- **venv**: キーは `(workspace キー, python version, extra_requirements, install_cmake_ninja)`。zephyr のリビジョンが同じなら requirements も同じなので、同じ構成のプロジェクト間で 1 つを共有する。
- **SDK**: `sdk/zephyr-sdk-<ver>/` を全プロジェクトで共有する。**`west sdk install` は使わず、zem が直接取得する**（§9: GitHub API が 504 で失敗し続けたこと、内部で `setup.sh -c` を必ず実行して `~/.cmake/packages` に登録してしまうこと、の 2 点から）。
  1. `https://github.com/zephyrproject-rtos/sdk-ng/releases/download/v<ver>/sha256.sum` を取得する
  2. `zephyr-sdk-<ver>_<host>_minimal.tar.xz` と `toolchain_<host>_<toolchain>.tar.xz` を `cache/downloads/` に取得し、sha256 を照合する
  3. minimal を `sdk/` に、ツールチェーンを `sdk/zephyr-sdk-<ver>/` に展開する。一時ディレクトリに展開してから rename し、途中で失敗しても壊れた状態が残らないようにする
  4. インストール済みかどうかは `<toolchain>/bin/<toolchain>-gcc` の存在で判定する
  - `<host>` は `macos-aarch64` / `macos-x86_64` / `linux-x86_64` / `linux-aarch64` / `windows-x86_64`（Windows は `.7z`）。platform 層が決める
  - SDK のバージョンは `zephyr/SDK_VERSION` に従う（v4.2.0 では `0.17.2`）
- Zephyr SDK の `setup.sh` / `setup.cmd` は実行しない（実行すると `~/.cmake/packages` などに登録され、ユーザー環境が変わるため）。

### 4.6 環境変数の組み立て（EnvBuilder）

子プロセスに渡す環境は次の順で作る。キーの大小文字の扱いは platform 層に任せる（Windows では大小文字を区別しない。`Path` と `PATH` が同一になる）。

1. **親プロセスの環境をコピー**し、以下を**削除**する（denylist 方式。MVP の既定）
   - ESP-IDF 関連: `IDF_*` 全般、`ESP_IDF_VERSION`、`ESP_ROM_ELF_DIR`、`OPENOCD_SCRIPTS`
   - 他の Zephyr 環境: `ZEPHYR_*` 全般、`ESPRESSIF_TOOLCHAIN_PATH`、`GNUARMEMB_TOOLCHAIN_PATH`、`CROSS_COMPILE`
   - Python 汚染: `VIRTUAL_ENV`、`PYTHONPATH`、`PYTHONHOME`、`CONDA_*`、`PIP_*`
   - west: `WEST_CONFIG_*`
   - ただし `[env].passthrough` に列挙されたものは残す
2. **PATH をフィルタ**する（`std::env::split_paths` / `join_paths` を使い、区切り文字を OS に任せる）
   - 次に一致するエントリを除外: `.espressif`、`esp-idf`、`zephyr-sdk-*`、元の `$VIRTUAL_ENV` 配下、conda の `bin`
   - 先頭に venv の実行ファイルディレクトリを追加する（Unix は `bin`、Windows は `Scripts`。`platform::venv_bin_dir`）
3. **zem の値を設定**する

| 変数 | 値 |
|---|---|
| `ZEM_HOME` | 解決済みの ZEM_HOME |
| `ZEPHYR_BASE` | `<ZEM_HOME>/workspaces/<key>/zephyr` |
| `ZEPHYR_SDK_INSTALL_DIR` | `<ZEM_HOME>/sdk/zephyr-sdk-<ver>` |
| `ZEPHYR_TOOLCHAIN_VARIANT` | `zephyr` |
| `VIRTUAL_ENV` | `<ZEM_HOME>/venvs/<key>` |
| `WEST_CONFIG_GLOBAL` / `WEST_CONFIG_SYSTEM` | `<ZEM_HOME>/westconfig` |
| `PYTHONNOUSERSITE` | `1` |
| `UV_CACHE_DIR` / `UV_PYTHON_INSTALL_DIR` | `<ZEM_HOME>/cache/uv` / `<ZEM_HOME>/python` |
| `CCACHE_DIR` | `<ZEM_HOME>/cache/ccache` |
| `ZEM_ACTIVE` | `1`（入れ子実行の検出とプロンプト表示用） |

4. `[env].set` の値を最後に上書きで適用する。

プロジェクトの環境（`zem west` / `zem run` / `zem shell` / `zem env`）では、さらに `WEST_CONFIG_LOCAL=<project>/.zem/west-config` を設定する。`zem sync` の west には設定しない（共有ワークスペース本来の `.west/config` で管理するため）。`.zem/west-config` は `zem.toml` と `state.json` から Session を開くたびに生成し直す。

| west 設定 | 値・目的 |
|---|---|
| `manifest.path` / `manifest.file`、`zephyr.base` | ワークスペースの `.west/config` と同じ値（差し替えると読まれなくなるため） |
| `manifest.project-filter` | `-.*,+zephyr,+<このプロジェクトのモジュール>`。共有ワークスペースから、このプロジェクトのモジュールだけを使う |
| `build.board` | 修飾子を補完したボード名 |
| `build.dir-fmt` | `[build].dir` の絶対パス。`west flash` / `west espressif monitor` もここからビルドを探す |
| `build.cmake-args` | `-DUSER_CACHE_DIR=<ZEM_HOME>/cache/zephyr`（Zephyr のキャッシュを ZEM_HOME に集約）、`-DCMAKE_FIND_USE_PACKAGE_REGISTRY=OFF`（別の Zephyr / SDK を拾わない）、`[build].cmake_args` |

`[env].isolation = "strict"`（allowlist 方式）は Phase 4 で実装する。残す変数の表は platform 層が持つ（Unix: `HOME`、`USER`、`LANG`、`LC_*`、`TERM`、`TMPDIR`、`SHELL`、`SSH_AUTH_SOCK`。Windows: `SYSTEMROOT`、`COMSPEC`、`PATHEXT`、`TEMP`、`TMP`、`USERPROFILE`、`APPDATA`、`LOCALAPPDATA`）。

### 4.7 コマンドごとの使われ方

| コマンド | 動作 |
|---|---|
| `zem env [--shell ...]` | export 文を出力（`eval "$(zem env)"`）。削除対象の変数は unset として出力する。シェルは自動判別（Unix は `$SHELL`）。PowerShell / cmd 形式は Phase 5 |
| `zem run -- <cmd...>` | 任意コマンドを隔離環境で実行（`west boards`、`menuconfig` など） |
| `zem west <args...>` | 隔離環境の west をプロジェクトルートで、引数を変えずに実行する（`zem west build` / `zem west flash` / `zem west espressif monitor -d` など）。ボード・モジュール・ビルドディレクトリはプロジェクト用 west 設定から west 自身が読む |

`west` / `run` / `shell` は `process::run_replacing` で起動する。Unix では `exec` で zem 自身を置き換えるので、シグナルと終了コードがそのまま伝わる。Windows では `exec` がないため、子プロセスを起動して待ち、終了コードを返す。その間、親は Ctrl-C を無視して子に処理を任せる。

> **検証済み（§9）**: プロジェクトルート（ワークスペースの外）から、`ZEPHYR_BASE` を設定して `west build` を実行すれば動く。cwd はプロジェクトルートとする。

---

## 5. 実装フェーズ

### Phase 1: 骨組み・設定・ボード解決・platform 層・doctor（ネットワーク不要）

**ゴール**: `zem init` で妥当な `zem.toml` を作成でき、`zem doctor` でホストの状態が分かる。3 OS の CI でビルドと単体テストが通る。前提としている west / Zephyr の挙動を手作業で確認しておく。

**タスク**
1. Cargo プロジェクトを作成（edition 2024、`lib.rs` + `main.rs`、clippy / rustfmt）
2. **CI マトリクス**（GitHub Actions）: `ubuntu-latest` / `macos-14` / `windows-latest` で `cargo build`、`cargo clippy --all-targets -D warnings`、`cargo test`（E2E を除く）
3. `platform/`: §7 の API を unix で実装し、windows は「コンパイルが通る実装 + 実行時は `Unsupported`」
4. `home/`: ZEM_HOME の決定、キー算出（パス表記・OS に依存しない正規化）
5. `config/`: スキーマ、`RelPath`、上位ディレクトリからの探索、検証、テンプレート生成
6. `board/`: 修飾子パーサ、静的テーブル、`ResolvedTarget`
7. `cli/init.rs`: 引数（`--name --board --zephyr-version --yes`）と対話モード。`.gitignore` に `build/` を追記
8. `tools/`、`doctor/`
   - 必須: `git`（uv と Python は zem が管理するため、ホストに無くてよい）
   - `install_cmake_ninja = false` の場合のみ必須: `cmake>=3.20.5`、`ninja`
   - 推奨: `dtc`（OS ごとの導入コマンドを案内）
   - Linux のみ: シリアルデバイスへの権限（`dialout` / `uucp` グループ）
   - 衝突警告: `IDF_PATH` などが設定されていれば「zem 実行時には除外される」と表示
   - ZEM_HOME の場所（使用量の表示は走査コストがあるため Phase 4 の `zem cache list` で行う）。同期フォルダ（iCloud / OneDrive / Dropbox）上なら警告
9. **スパイク（手作業・結果を本書に反映）** — ✅ 2026-11-25 に Zephyr v4.2.0 で実施済み。結果は §9 を参照。確認した項目:
   - ワークスペース外からの `west build`（§4.7）
   - `-DZEPHYR_MODULES` によるモジュールの絞り込み（§4.4）
   - `-DUSER_CACHE_DIR` が効くか
   - 既存ワークスペースへのモジュール追加（`west update <名前>`）が、既存のモジュールに触れずに済むか
   - `west sdk install` のオプションと、その SDK バージョンでのディレクトリ構成
   - `west blobs fetch` のライセンス同意の有無と、非対話で同意する手段
   - esptool が `scripts/requirements-*.txt` のどこに含まれているか

**動作確認**
- 単体テスト: 修飾子パーサ、SoC → ツールチェーン表の全網羅、設定の検証、`RelPath` の往復、キーの安定性（同じ入力なら全 OS で同じキーになること）
- `insta` で `init` 出力のスナップショット比較
- `assert_cmd` で tempdir に `zem init --yes ...` を実行し、出力を再パースできること
- Windows CI では `zem init` / `zem doctor` が動き、`zem sync` は「Windows は未対応」というエラーで終了すること

### Phase 2: `zem sync` 前半（venv + 共有ワークスペース + blobs）

**ゴール**: `zem sync` で venv・共有ワークスペース・blobs が `ZEM_HOME` に揃う。2 つ目のプロジェクトは既存のものを再利用する。

**タスク**
1. `store/mod.rs`（完了マーカー、ロック）、`fsutil.rs`（アトミック書き込み）、`sync/plan.rs`（`--dry-run` で「何を作るか・何を再利用するか」を表示）
2. `store/venv.rs`
   - uv は zem が固定版（`UV_VERSION`）を取得して使う。ホストの uv / python は使わない
   - venv は常に `uv venv <path> --python <ver> --python-preference only-managed --no-project` で作り、`uv pip install --python <venv の python>` で入れる（スパイクでは、指定しないとホストの Python 3.12.7 が使われた。再現性のため uv 管理の Python に固定する）
   - 共有の west venv（`tools/west`）には `west` だけを入れる。project venv はワークスペース準備後に作り、`requirements-base.txt` と `cmake` / `ninja`（`install_cmake_ninja`）と extras を入れる
   - venv は最終パスで組み立て、marker を最後に書いて公開する。staging してから rename はしない（uv / pip が書く実行スクリプトの shebang に venv の絶対パスが入り、移動後は `bad interpreter` になるため。実機で確認した）
3. `manifest/`: feature → プロジェクト名、和集合での west.yml 生成
4. `store/workspace.rs`: 初回は `west init -l` + `west update --narrow -o=--depth=1`、モジュール追加時は west.yml を再生成して引数なしの `west update` を実行する
5. ボードの裏取り（`board_yml.rs`）
6. blobs（ワークスペースごとに 1 回。ライセンス同意が必要なら、設定かフラグがない限り止まって内容を表示する）
7. `store/registry.rs`: `projects.json` に記録する
8. プロジェクト側 `.zem/` の生成（`.gitignore`、`state.json`、`logs/sync-last.log`）

**動作確認**
- `insta` で west.yml の生成結果をスナップショット比較（esp32s3 / c3 × features あり・なし × タグ / SHA × 和集合）
- 単体テスト: 和集合の計算、追加分の算出、完了マーカーの判定
- E2E（`#[ignore]`、`ZEM_HOME` を tempdir に向けて実行）
  - プロジェクト A で `init` → `sync`: `workspaces/*/zephyr/VERSION` と blobs が存在すること
  - A で再度 `sync`: ネットワークアクセスなしで数秒以内に終わること
  - 同じ Zephyr バージョンのプロジェクト B（`features = ["mbedtls"]`）で `sync`: **ワークスペースが 1 つのまま**で mbedtls だけが追加されること。zephyr の再 clone が起きないこと
  - `ZEM_HOME` とプロジェクト側 `.zem/` の外に何も作られないこと（`HOME` を tempdir に差し替えて実行し、`~/.cache` などが作られないことを確認）
  - プロジェクト側 `.zem/` が 1 MB 未満であること。`.zem/` を削除しても、オフラインの `sync` で復元できること
- 手動確認: ディスク使用量と所要時間を記録する（hal_espressif が大きいため）

**実装結果（2026-11-25、macOS arm64）**
- 計画からの変更: Phase 3 予定だった `env/`（EnvBuilder と sanitize）と `process.rs`（ログ付き実行）を前倒しした。sync 中の uv / west もホストの `IDF_PATH`・`VIRTUAL_ENV`・`.espressif` の PATH から切り離して実行する必要があったため。シェル別の出力と `run_replacing` は Phase 3 のまま
- sync の手順: Python 環境 → ワークスペース → ボード（`board.yml` / `soc.yml`）→ Zephyr の requirements → blobs → `projects.json` と `state.json`。ロックはワークスペース → venv の順に取り、sync の間保持する
- `state.json` の `modules` はそのプロジェクトが使うモジュールだけを持つ（ワークスペースの和集合ではない）。Phase 3 の `-DZEPHYR_MODULES` はここから作る
- ZEM_HOME は解決時に正規化する（macOS の `/tmp` → `/private/tmp`）。west が返すパスと表記を揃えるため
- 計測（E2E、空の ZEM_HOME から）: A（esp32c3）の初回 sync から B（esp32s3 + mbedtls）の追加 sync までで計 75 秒。変更なしの再 sync は 0.4 秒。ZEM_HOME は約 1.1 GB（workspaces 680 MB、cache 172 MB、venvs 159 MB、python 51 MB）
- 未実施の確認: `HOME` を差し替えて ZEM_HOME の外に何も作られないことの検査、`.zem/` 削除後のオフライン復元の検査。Phase 3 の E2E に含める

### Phase 3: SDK + 環境注入 + build / flash / env

**ゴール**: `init → sync → build → flash` が esp32s3 と esp32c3 で通る。共有ワークスペース上の 2 プロジェクトが互いに影響しない。`IDF_PATH` などが設定されたシェルでも結果が変わらない。

**タスク**
1. `store/sdk.rs`: `zephyr/SDK_VERSION` から SDK バージョンを決め、足りないツールチェーンを追加する
2. `env/`: EnvBuilder、sanitize、シェル別の出力（bash / zsh / fish / json）
3. `process.rs` の `run_replacing`、`cli/build.rs`、`cli/flash.rs`、`cli/run.rs`、`cli/env.rs`
4. `zem build` 実行時に `.zem/state.json` の `config_hash` と各 Store の完了マーカーを検査し、不一致や不足があれば「`zem sync` を実行してください」と止める（自動では sync しない）
5. `.zem/activate.sh` / `activate.fish` の生成

**動作確認**
- 単体テスト: sanitize（偽の `IDF_PATH`、`.espressif` を含む PATH、passthrough、Windows 風に大小文字の違うキー）、シェル出力のスナップショット
- 統合テスト: `zem run -- env` の出力を検査し、除外と注入が意図どおりか確認する
- E2E（`#[ignore]`）
  - `samples/hello_world` を `esp32s3_devkitc/esp32s3/procpu` と `esp32c3_devkitm` でビルド
  - `build/CMakeCache.txt` の `CMAKE_C_COMPILER` が `ZEM_HOME/sdk` 配下を指していること
  - **共有の分離**: Phase 2 のプロジェクト A（mbedtls なし）のビルドで、`ZEPHYR_MODULES` に mbedtls が含まれていないこと
  - **汚染テスト**: `IDF_PATH=/bogus` と、偽の `xtensa-esp32s3-elf-gcc` を置いたディレクトリを PATH の先頭に入れた状態でビルドし、成功かつ zem 側のコンパイラが使われていること
- 実機（手動チェックリスト）: ESP32-S3-DevKitC と ESP32-C3-DevKitM に書き込み、シリアル出力で `Hello World!` を確認（macOS と Linux）

**実装結果（2026-11-25、macOS arm64）**
- コマンド: `zem build [-p] [-- <west 引数> [-- <CMake 引数>]]`、`zem flash [--port] [--baud] [-- <west 引数>]`、`zem run <cmd...>`、`zem env [--shell bash|zsh|sh|fish|json]`。build / flash / run は `platform::run_replacing`（Unix は exec）で west などに置き換わる
- 起動前の検査（`session.rs`）: `state.json` の有無、`config_hash`、ZEM_HOME の一致、zephyr / venv / モジュール / ツールチェーンの gcc の存在を確認し、満たさなければ「`zem sync` を実行してください」で止まる。`zem doctor` の Project 欄にも同じ結果を出す
- SDK: sync の手順に「Zephyr SDK」を追加（全 7 手順）。アーカイブは sha256 を照合して `.zem-extract-*` に展開してから rename する。展開後はアーカイブを削除し、`cache/downloads/` には `sha256.sum` だけ残す
- 計画からの変更
  - **HTTP の TLS**: Unix は rustls（同梱のルート証明書）、Windows は SChannel（Windows の証明書ストア）。Windows ターゲットへのクロスチェックで ring / liblzma の C ビルドが通らないため。TLS 設定と SDK アーカイブの展開（Unix: tar.xz、Windows: .7z は Phase 5）は platform 層に置き、`liblzma` / `tar` は Unix 専用の依存にした
  - **activate スクリプト**: `zem env` の出力を固定化せず、`zem env` を呼び出す薄いスクリプトにした。固定化すると、sync した時点のシェルの PATH が焼き込まれるため
  - **`zem init` が hello world を生成**: `CMakeLists.txt` が無い場合だけ `CMakeLists.txt` / `prj.conf` / `src/main.c` を書く。`init → sync → build` を 1 回で通すため
  - **PATH の除外を追加**: 名前ではなく構造で判定し、`pyvenv.cfg` か `conda-meta` が隣にある `bin` を除外する（PlatformIO の `penv` などを除外できた）
- 計測（ワークスペース・venv は Phase 2 の E2E で作成済みの状態から）: esp32c3 と esp32s3 の sync（SDK 取得を含む）+ ビルド 2 回で 119 秒。esp32c6 の hello world のフルビルドは約 28 秒
- **SDK のサイズ**: `riscv64-zephyr-elf` は展開後 1.0 GB（multilib を多数含む）。xtensa（esp32s3）は 193 MB。C3 / C6 を使うと ZEM_HOME が 1 GB 増える
- **実機確認（ESP32-D0WD-V3 / `esp32_devkitc/esp32/procpu`、`/dev/cu.usbserial-10`）**: 新規プロジェクトで `init → sync → build → flash` が通り、シリアルに `Hello World! esp32_devkitc/esp32/procpu` が 1 秒ごとに出力された。所要時間は sync 10 秒（ESP32 用ツールチェーンの追加のみ）、build 37 秒、flash 8 秒。`--port` を省略しても esptool の自動検出で同じポートに書き込めた
- 未確認: ESP32-S3 / C3 の実機、fish の activate（fish が未インストール）、Linux での E2E

### Phase 4: 再現性・キャッシュ管理・使い勝手

**ゴール**: チーム内で再現性を持って配布でき、ZEM_HOME の肥大化を管理できる。

**タスク（優先度順）**
1. `zem.lock`: zephyr とモジュールの SHA、SDK バージョン、Python パッケージ（`uv pip compile`）を固定。`zem sync --locked` で厳密に再現。ワークスペースのキーは SHA ベースになる
2. `zem cache list`（Store ごとの使用量と参照元のプロジェクト）/ `zem cache gc`（台帳上でどのプロジェクトからも参照されていない Store を、ロックを取ってから削除）
3. エラー UX: どの手順で失敗したか・次に何をすべきかを表示。`--verbose` で子プロセスのコマンドラインを表示
4. `zem monitor`（`west espressif monitor` のラップ）、`zem shell`（隔離環境のサブシェル）
5. `[env].isolation = "strict"`
6. シリアルポートの自動検出（`serialport` クレート。全 OS 対応）
7. `doctor --fix`（OS ごとのコマンドを提示するだけ。自動実行はしない）
8. 配布: `cargo-dist` で macOS / Linux 向けバイナリと Homebrew tap

**実装結果（2026-11-25、macOS arm64。タスク 1・2・4 の monitor）**
- **`zem monitor [--port] [--baud] [--no-decode] [-- <idf_monitor 引数>]` と `zem flash --monitor`**: `west espressif monitor` はビルドディレクトリを自分で探すため使わず、hal_espressif の `tools/idf_monitor/idf_monitor.py` を venv の Python で直接起動する。`--toolchain-prefix` に zem の SDK を渡すので、バックトレースのアドレスがソース行に変換される。ポートは `[flash].port`、なければ hal 同梱の esptool で ESP デバイスを自動検出する。実機（ESP32）で自動検出・リセット後のブートログ・Ctrl+] での終了（終了コード 0）を確認した
- **`zem.lock`**: sync のたびに生成・更新する。中身は `[zephyr]`（url・revision・commit）、`[modules]`（このプロジェクトのモジュールの SHA。`west list -f "{name} {sha}"`）、`[sdk].version`、`[python]`（version・install-cmake-ninja・extra-requirements・`packages`＝インストール後の freeze 結果）
  - lock の各節は、zem.toml の対応する設定が変わっていなければ適用する（例: `[python].extra_requirements` を変えると Python の固定だけ外れる）
  - 不一致の扱い: 通常の sync は警告して lock を更新する。`zem sync --locked` は lock が無い・古い・結果が一致しない場合にエラーで止まり、lock を書き換えない（CI 向け）。`--update` は lock を無視して取り直す（`--locked` とは併用不可）
  - **ブランチ指定**: sync 時に `git ls-remote` でコミットを解決し、ワークスペースのキーを SHA にする（共有ワークスペースが動くブランチを追従しない）。以降は lock のコミットを使い、`--update` で最新に進める
  - **Python パッケージ**: lock の `packages` を制約ファイル（`.zem/constraints.txt`、`pip install -c`）として渡す。共有 venv が別のバージョンで入っている場合は、`<venv キー>-<パッケージのハッシュ 8 桁>` の専用 venv を作り、他プロジェクトの venv を書き換えない
  - 計画からの変更: `uv pip compile` ではなく、インストール後の freeze を記録する（Zephyr の requirements には範囲指定しかなく、実際に入ったものを固定するほうが確実なため）
- **`zem cache list` / `zem cache gc [--dry-run]`**: 参照元は台帳（`projects.json`）の各プロジェクトの `.zem/state.json`。読めなければ台帳の記録で代用し、その SDK のツールチェーンはすべて残す。`zem.toml` が消えたプロジェクトは台帳から外す
  - 削除対象: 未参照のワークスペース・venv・SDK（バージョンごと）・ツールチェーン、1 時間以上前の `.zem-staging-*`（ロックが空いているもの）、`trash/` の残骸。uv の Python 本体と `cache/` は表示のみで消さない
  - 安全策: gc の間は台帳のロックを保持する（sync が新しい参照を登録できない）。各 Store は `try_lock` で取れなければ「使用中」として飛ばす。削除は `trash/` へ rename してから行う
  - 実測: E2E 用 ZEM_HOME（2.5 GiB）で lock の検証時に作った専用 venv 2 個（280 MiB）を削除。SDK のロックを別プロセスで保持した状態では、未使用の esp32s3 ツールチェーンが「使用中」として残ることを確認した。`cache list` は 2.6 秒（ワークスペースの走査が大半）

**実装結果（続き。タスク 3〜7）**
- **エラー UX**: sync が失敗すると、どの手順で止まったか（`✗ Python requirements failed (step 4/7, 0.3s)`）を表示してから、コマンド・出力の末尾・ログの場所・`--verbose` での再実行を案内する。`--verbose` では子プロセスのコマンドラインと出力をそのまま流す
- **`zem shell [--shell PATH]`**: 隔離環境でサブシェルを起動する。ユーザーの rc ファイル（pyenv や conda の初期化で PATH を書き換えるもの）を読んだ**後で** `zem env` を再適用し、プロンプトに `(zem:<name>)` を付ける。zsh は `ZDOTDIR` を `.zem/shell/zsh/` に向けて元の `.zshenv` / `.zshrc` を読み込み、bash は `--rcfile`、fish は `-C` を使う。入れ子は `ZEM_SHELL` で検出してエラーにする。zsh と bash で実際に確認した
- **`[env].isolation = "strict"`**: zem が起動するコマンド（sync / build / flash / monitor / run / shell）には、platform 層の allowlist・PATH・`[env].passthrough`・zem 自身の変数だけを渡す。`zem env` と activate スクリプトはユーザー自身のシェルに適用するものなので、strict でも denylist 方式のまま（個人の変数を unset しないため）。allowlist にプロキシの小文字版・`ALL_PROXY`・`SSL_CERT_FILE` / `SSL_CERT_DIR` を追加した。実機プロジェクトで `-p` ビルドが通ることを確認した
- **シリアルポート**: `serialport` クレート（libudev なし）で USB シリアルを列挙する。macOS の `/dev/tty.*` と `/dev/cu.*` の重複は `cu` だけ残す。ポートが 1 つなら flash / monitor はそれを使い、複数あるときだけ esptool で ESP チップを探す。見つからないときは、接続中のポートを列挙して `--port` を案内する。`zem doctor` にもポートとブリッジチップ（CP210x / CH34x / ESP USB-JTAG）を表示する
- **変更（2026-11-25）: `zem monitor`・`zem flash --monitor/--port/--baud`・`[flash]` を削除**。`west espressif monitor` / `west flash` の esp32 ランナーと二重管理になるため、ESP 固有の引数の翻訳は zem から外した
  - `idf_monitor` の Zephyr 版はアドレス変換に `--toolchain-prefix` ではなく `CMakeCache` の `CMAKE_ADDR2LINE` を使うので、`west espressif monitor -d` でも SDK の addr2line でソース行に変換される。`--target` は stack dump 変換（既定で無効）にしか使われない
  - ポートは esp32 ランナーの esptool が自動検出する。固定したい場合は `[env.set] ESPTOOL_PORT` か `zem west flash --esp-device P`。`zem init` は `[flash]` を書き出さない（既存の zem.toml に残っていれば未知のキーとしてエラーになる）
  - `west espressif monitor` はビルドディレクトリを cwd から探すため、`zem west <args...>` を追加した。build / flash と同じくプロジェクトルートで west を起動する（`zem run` は cwd を変えない）
  - USB シリアルの列挙は `zem doctor` の表示用にだけ残す
- **変更（2026-11-25）: `zem build` / `zem flash` と `[project].app_dir` を削除**。`west build` / `west flash` のラッパーも二重管理になるため、`zem west build` / `zem west flash` に一本化した
  - `zem build` が付けていた `-b` / `-d` / `-DZEPHYR_MODULES` / CMake 引数は、プロジェクト用 west 設定（`.zem/west-config`、`WEST_CONFIG_LOCAL`）の `build.board` / `build.dir-fmt` / `manifest.project-filter` / `build.cmake-args` に移した（§4.6）。`zem shell` や `zem env` を適用したシェルの素の `west build` も同じ設定で動く
  - west の設定にはアプリのディレクトリを指定する項目がなく、`west build` は cwd をアプリとして扱う。そのため「zem.toml とアプリは同じディレクトリ」と決め、`app_dir` を削除した
  - 確認: `project-filter` で外したモジュールは `west list` と `zephyr_module.py` の出力から消える。実プロジェクト（esp32_devkitc）でサブディレクトリから `zem west build` を実行し、アプリがプロジェクトルート、モジュールが `hal_espressif` だけ、`USER_CACHE_DIR` が ZEM_HOME 配下になることを確認した。`zem west flash --context` と `zem west espressif monitor --help` も同じビルドディレクトリを拾う
  - `west update` は zem.lock で固定されたリビジョンに揃えるだけで、inactive なモジュールにも触れないが、共有ワークスペースの更新は `zem sync` で行う
- **`zem doctor --fix`**: 警告・失敗の項目について、解決コマンドを重複なしで列挙する（実行はしない）。インストール案内は platform 層で「コマンド」と「代替案」に分けて持つ

**動作確認**
- lock の往復テスト（sync → lock 生成 → 別の ZEM_HOME で `--locked` sync → 同一の SHA になること）— 同じ ZEM_HOME での `--locked`、SHA を書き換えた lock での失敗と更新、パッケージを書き換えた lock での専用 venv 作成は確認済み。別の ZEM_HOME での往復と、ブランチ指定での実際の取得は未実施（数 GB の取得が必要なため）
- gc テスト（参照中の Store は消えず、参照が外れた Store だけが消えること）— 単体テストで確認済み
- CI の非ネットワークテストは毎回、E2E は nightly で実行（Linux / macOS）

### Phase 5（MVP 後）: Windows 対応

**ゴール**: Windows 10/11（x86_64）で `init → sync → build → flash` が通る。

**タスク**
1. `platform/windows.rs` の本実装（`Unsupported` を外す）
2. `zem env --shell powershell|cmd`、`ShellKind` の自動判別（`PSModulePath` など）
3. ホストツールの入手経路を決める: git（必須。案内のみ）、Python（uv が管理）、cmake / ninja（pip ホイール）、dtc（winget / choco / MSYS2 を doctor で案内するか、`ZEM_HOME/tools` に取得するか）
4. doctor の Windows 向けチェック: `LongPathsEnabled`、`git config core.longpaths`、`core.autocrlf`、USB シリアルドライバ（CP210x / CH34x）
5. CI の E2E に `windows-latest` を追加。配布物に Windows 向け zip / winget / scoop を追加

**進捗**
- 済: uv の取得（Windows は `uv-<target>.zip`、`uv.exe` がトップレベル）、SDK の `.7z` 展開（`sevenz-rust2`。v0.17.4 の minimal と nios2 ツールチェーンの実物で確認）、`run_replacing` 中の Ctrl-C を子に任せる（`ctrlc`）、`zem env --shell powershell|cmd`、`.zem/activate.ps1` / `.zem/activate.bat`、`zem shell` の PowerShell（`-EncodedCommand`）/ cmd 対応、`ensure_supported` の解禁、doctor の `LongPathsEnabled` と `core.longpaths` チェック、配布物に `x86_64-pc-windows-msvc` の zip と PowerShell インストーラを追加
- Windows では `$SHELL` を無視し、既定のシェルは PowerShell とする（MSYS / Git Bash の `$SHELL` はネイティブのプログラムから起動できないため）
- 未: 実機での `init → sync → build → flash` の確認、Windows 向けの CLI 統合テスト（偽の west / uv を `.cmd` で用意する必要がある）、E2E の `windows-latest`、dtc の入手経路、`core.autocrlf` と USB シリアルドライバのチェック、winget / scoop

---

## 6. 技術的リスクと割り切りポイント

### リスク

| リスク | 影響 | 対策 |
|---|---|---|
| `ZEPHYR_MODULES` による絞り込みが想定どおり動かない | 共有ワークスペースで他プロジェクトのモジュールが混ざる | Phase 1 のスパイクで確認。駄目ならキーにモジュール集合を含める方式に切り替える（git キャッシュで取得コストを抑える） |
| ワークスペース外からの `west build` が動かない | build / flash の設計に影響 | 代替として「cwd = 共有ワークスペース + 絶対パス指定」を用意 |
| 共有ワークスペースの変更が他プロジェクトに波及する | 再現性の崩れ | 不変条件（リビジョン固定・追加のみ・ロック・ブランチは明示更新のみ）を Store の実装で強制する。Phase 4 で SHA に固定する |
| SDK のアセット名やディレクトリ構成がバージョンで変わる（0.17 系 → 1.x） | SDK の取得が壊れる | `store/sdk.rs` に閉じ込め、URL の組み立てとレイアウトはバージョンごとの表で持つ。存在確認は gcc を探す方式にする。1.x 系は対応時に検証する |
| Zephyr のバージョンでボード名が変わる | 解決に失敗する | 静的テーブル + board.yml による裏取り |
| hal_espressif が大きく、初回 sync が遅い | 初回体験が悪い | 共有で 2 プロジェクト目以降は即座に終わる。初回は `--narrow` と `clone-depth: 1` で緩和（スパイク実測: ワークスペース 36 秒 + blobs 57 秒） |
| ZEM_HOME の肥大化 | ディスクを圧迫 | `zem cache list / gc`（Phase 4）。doctor で使用量を表示 |
| ZEM_HOME がネットワークドライブや同期フォルダ（iCloud / OneDrive）上にある | ロックや性能の問題 | doctor で警告。`ZEM_HOME` で移動できるようにする |
| blobs のライセンス同意 | 黙って同意すると問題 | 明示的な設定かフラグがない限り自動で同意しない |
| Windows 固有の問題（パス長、大小文字を区別しない環境変数、`\\?\` 付きパス、改行コード） | Phase 5 で手戻りが起きる | §7 の platform 層ルールを Phase 1 から守り、Windows CI でコンパイルと単体テストを常に通す |

### 割り切り（MVP ではやらないこと）

- **Windows での実行**（設計とコンパイルは対応済みにしておき、実行時は `Unsupported` を返す。本対応は Phase 5）
- **Zephyr v4.0 未満**
- **dtc の自前提供**: OS ごとの導入方法を doctor が案内するだけ
- **gperf / OpenOCD / QEMU / JTAG デバッグ**: 対象外。書き込みは esptool（シリアル）のみ
- **sysbuild / MCUboot / マルチイメージ**: 対象外（`cmake_args` や `zem run` で使うことは妨げない）
- **単一ターゲット**: `zem.toml` に書けるボードは 1 つ
- **HTTP 通信は SDK 取得のみ自前で行う**: それ以外のダウンロードは west / uv / git 経由。プロキシは `HTTPS_PROXY` 環境変数に従う
- **ESP-IDF との共存は「除外」のみ**: IDF の環境を読み取って再利用するような統合はしない
- **自動 sync はしない**: `build` は不整合を検出したら止めるだけ（暗黙の巨大ダウンロードを避ける）

---

## 7. クロスプラットフォーム設計（Windows を見据えたルール）

### 7.1 platform モジュールの API

OS 依存の処理はすべてここを経由する。他のモジュールには `cfg(unix)` / `cfg(windows)` を書かない（CI で `rg 'cfg\((unix|windows|target_os)' src --glob '!src/platform/**'` を実行し、ヒットしたら失敗させる）。

```rust
// src/platform/mod.rs
pub fn ensure_supported() -> Result<()>;               // MVP の Windows では Unsupported を返す
pub fn default_zem_home() -> Result<PathBuf>;          // ~/.zem または %USERPROFILE%\.zem
pub fn exe_name(base: &str) -> OsString;               // "west" → "west" / "west.exe"
pub fn venv_bin_dir(venv: &Path) -> PathBuf;           // bin / Scripts
pub fn venv_python(venv: &Path) -> PathBuf;            // bin/python / Scripts\python.exe
pub fn env_key_eq(a: &OsStr, b: &OsStr) -> bool;       // Windows では大小文字を区別しない
pub fn path_var_name() -> &'static str;                // "PATH"（Windows では既存のキー表記を保持）
pub fn strict_env_allowlist() -> &'static [&'static str];
pub fn to_cmake_path(p: &Path) -> String;              // Windows では "/" 区切りに変換（-D 引数の値のエスケープ対策）
pub fn canonicalize(p: &Path) -> Result<PathBuf>;      // dunce を使い、\\?\ を付けない
pub fn run_replacing(cmd: Command) -> Result<!>;       // Unix: exec / Windows: spawn + wait + exit
pub fn detect_shell() -> ShellKind;
pub fn serial_port_hint() -> &'static str;             // "/dev/ttyUSB0" / "/dev/cu.usbserial-*" / "COM3"
```

### 7.2 コーディング規約

| 項目 | ルール |
|---|---|
| パス | 常に `PathBuf` / `Path::join` を使う。文字列の `/` 連結は禁止。PATH 系は `std::env::split_paths` / `join_paths` を使う |
| 外部に渡すパス | CMake の `-D` 値には `platform::to_cmake_path` を使う。canonicalize には `platform::canonicalize` を使う |
| 実行ファイル | 名前は `platform::exe_name`、検索は `which` クレートを使う（Windows の `PATHEXT` に対応） |
| シンボリックリンク | **使わない**（Windows では権限が必要なため）。共有コンポーネントは絶対パスで参照する |
| パス長 | ZEM_HOME 配下のディレクトリ名は短く（ハッシュ 8 桁）。深いネストを作らない |
| 生成ファイル | UTF-8・LF で書く。`zem.toml` 内のパスは `/` 区切り |
| ファイル置換 | `fsutil` のアトミック書き込みを使う（Windows では対象ファイルを開いているプロセスがあると rename が失敗するので、短い再試行を入れる） |
| ロック | 標準の `File::lock` / `try_lock`（Rust 1.89 で安定化。全 OS 対応）を `lock.rs` 経由で使う。`flock` を直接呼ばない |
| プロセス | 置き換え起動は `platform::run_replacing` のみ。シェル経由（`sh -c`）の起動はしない（引数は常に配列で渡す） |
| 環境変数 | キーの比較は `platform::env_key_eq`。EnvBuilder の内部表現は、Windows では大小文字を区別しないマップにする |
| 改行・出力 | 端末出力の装飾は `console` クレートに任せる（Windows の ANSI 対応を含む） |

### 7.3 CI での担保

| ジョブ | Linux | macOS | Windows |
|---|---|---|---|
| build / clippy / 単体テスト | Phase 1〜 | Phase 1〜 | Phase 1〜 |
| platform 境界チェック（rg） | Phase 1〜 | — | — |
| E2E（nightly） | Phase 2〜 | Phase 2〜 | Phase 5〜 |

---

## 8. 決定事項と残りの論点

### 決定済み

| 論点 | 決定 |
|---|---|
| 対応 OS | クロスプラットフォーム前提で設計し、MVP の動作保証は macOS / Linux。Windows は Phase 5 |
| ワークスペースの共有 | MVP から複数プロジェクトで共有する（§4.4） |
| キャッシュの場所 | すべて `ZEM_HOME`（既定 `~/.zem/`）配下に集約する。OS 標準のキャッシュディレクトリは使わない |
| HAL・モジュールの実体 | グローバル側（`ZEM_HOME/workspaces/`）に置く |
| プロジェクト側 `.zem/` | 軽量ファイル（`state.json`・起動スクリプト・ログ）のみ。消しても再生成できる（§4.3） |
| SDK の場所 | `ZEM_HOME/sdk/` で全プロジェクト共有 |
| Zephyr の対応範囲 | v4.0 以降。`init` の既定値と初期の検証対象は v4.2.0 |
| SDK の取得方法 | `west sdk install` は使わず、リリースアセットを直接取得し sha256 を照合する（§4.5、§9） |
| esptool | pip では入れない。hal_espressif 同梱版を Zephyr が使う |
| cmake / ninja の入手 | 既定は PyPI ホイールを venv に入れる（`install_cmake_ninja = false` でホスト版を使用） |
| 環境変数の分離 | 既定は denylist 方式。strict（allowlist）は Phase 4 で実装済み（zem が起動するコマンドのみ。`zem env` は常に denylist） |

### 残りの論点

1. ESP32-H2 は Zephyr v4.2.0 に存在しない。H2 を MVP の検証対象に含める場合は、対応する Zephyr のバージョン（v4.3 以降と思われる）を決める必要がある

---

## 9. Phase 1 スパイク結果（2026-11-25、macOS arm64、Zephyr v4.2.0）

`/tmp/zem-spike` を ZEM_HOME に見立て、zem が行う予定の手順を手作業で再現した。

### 9.1 確認結果

| 項目 | 結果 | 設計への反映 |
|---|---|---|
| ワークスペース外からの `west build` | ✅ `ZEPHYR_BASE` を設定すればプロジェクトルートから動く | cwd はプロジェクトルート（§4.7） |
| `-DZEPHYR_MODULES` による絞り込み | ✅ 指定したモジュールだけが使われる。指定しないと共有ワークスペース内の他モジュール（mbedtls）が混入する | 必須の CMake 引数とする。パスは canonicalize する（§4.4） |
| モジュールの追加 | ⚠️ `west update mbedtls` は「import 経由のプロジェクトは更新できない」と拒否される。引数なしの `west update` なら 5 秒で追加でき、既存の HEAD は変わらない | 追加時は引数なしの `west update` を使う |
| `--name-cache` | ⚠️ 既存のキャッシュを読むだけで、作成はしない | 当面は使わない。git ミラーの管理は Phase 4 |
| `-DUSER_CACHE_DIR` | ✅ `ToolchainCapabilityDatabase` が ZEM_HOME 側に作られ、`~/Library/Caches/zephyr` は更新されない | そのまま採用 |
| `west sdk install` | ❌ GitHub API（`releases?per_page=100`）が 504 で 3 回連続失敗。さらに内部で必ず `setup.sh -c` を実行し、`~/.cmake/packages` に登録する | 使わない。直接取得に変更（§4.5） |
| SDK の直接取得 | ✅ `sha256.sum` で照合して展開。minimal 5 KB + esp32s3 ツールチェーン 36 MB、展開後 193 MB、約 3 秒 | `store/sdk.rs` で実装 |
| `west blobs fetch hal_espressif` | ✅ ライセンスのクリックスルーなし（stdin を閉じても成功）。全 SoC 分の blobs を取得（35 MB、57 秒）。同意が必要なモジュール向けに `-a/--auto-accept` がある | `accept_licenses = true` の場合だけ `-a` を付ける |
| esptool | ℹ️ どの `requirements-*.txt` にも含まれない。ビルドも `west flash` も `<hal_espressif>/tools/esptool_py/esptool.py`（v4.8.1）を使う | pip では入れない |
| PyPI 版 cmake / ninja | ✅ cmake 4.4.5 / ninja 1.13.2 で v4.2.0 のビルドが成功 | `install_cmake_ninja = true` を既定のまま維持 |
| uv の Python | ⚠️ `--python 3.12` だけだとホストの Python 3.12.7 が使われた | `--python-preference only-managed` を付ける |
| 汚染環境でのビルド | ✅ 実際の `IDF_PATH`（`~/.espressif/v6.1/esp-idf`）と偽の `xtensa-esp32s3-elf-gcc` が PATH にあっても、zem の SDK のコンパイラが使われた。ただし venv を PATH の先頭に置かないと偽の `cmake` が拾われて失敗した | venv を PATH の先頭に置く設計を維持 |
| 古い CMake レジストリ | ℹ️ ユーザー環境に、存在しない `~/zephyr-sdk-1.0.1` を指す `Zephyr-sdk` の登録が残っていた。`ZEPHYR_SDK_INSTALL_DIR` の指定が優先され、影響はなかった | `-DCMAKE_FIND_USE_PACKAGE_REGISTRY=OFF` も念のため維持 |
| 既知ボード表 | ✅ 14 件中 13 件が v4.2.0 に存在。❌ `esp32h2_devkitm` は存在しない（`soc/espressif/esp32h2` 自体がない） | v4.2 以前の H2 は検証時にエラーにした |
| 修飾子なしのボード名 | ✅ `esp32s3_devkitc` や `esp32s3_devkitc/esp32s3` は「Board qualifiers not found」で失敗する | zem による完全名への補完が必要（実装済み） |

### 9.2 所要時間とサイズ（初回）

| 手順 | 時間 | サイズ |
|---|---|---|
| venv 作成 + west / cmake / ninja | 約 4 秒 | — |
| `west update`（zephyr + hal_espressif、depth 1） | 約 36 秒 | zephyr 484 MB、hal_espressif 105 MB |
| `requirements-base.txt` | 約 2 秒 | — |
| blobs | 約 57 秒 | 35 MB |
| SDK（minimal + esp32s3 ツールチェーン） | 約 3 秒 | 193 MB |
| `hello_world` のビルド（esp32s3） | 約 28 秒 | — |
| mbedtls の追加（2 プロジェクト目） | 約 5 秒 | 55 MB |
