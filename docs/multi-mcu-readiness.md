# 対応 MCU 拡張に向けた下準備

> 作成: 2026-11-28（v0.2.1 時点）
> 前提: ESP32 の build / flash が macOS / Windows / Linux の実機で成功している。MVP の設計は [mvp-plan.md](mvp-plan.md)
> 目的: ESP32 以外の MCU（nRF、STM32、RP2040、NXP など）を追加する**前に**、ESP32 前提で書かれている箇所を洗い出し、
> 「ベンダーを 1 つ足すたびにコードのあちこちを触る」状態を避けるための手順を決める

---

## 1. 結論（要約）

現状のコードは「ボード解決」「HAL / blobs」「SDK ツールチェーン」「doctor・環境分離」「テスト」の 5 か所で ESP32 前提が
ハードコードされている。新しい MCU を足す前に、次の順で手を入れる。

| # | ステップ | 狙い | 規模 |
|---|---|---|---|
| S1 | ベンダー非依存の型に置き換える（`Soc` enum → 文字列 ID、`Arch` の拡張） | 型レベルで ESP32 に閉じている状態を解く | 中 |
| S2 | ベンダープロファイル（データ）を導入し、ESP32 固有の値をそこへ移す | ベンダー追加 = プロファイル 1 つ追加、にする | 中 |
| S3 | ボード解決を「Zephyr ツリーが正」に寄せる（sync を zephyr 本体 → 解決 → モジュールの 2 段に） | ボード表を全ボード分持たなくて済むようにする | 大 |
| S4 | ツールチェーン決定と SDK の配置を一般化する（SDK 1.x も視野に） | arm / arm64 / riscv / xtensa などを同じ経路で扱う | 小〜中 |
| S5 | 書き込み・デバッグ用ホストツールの扱いを決める | ESP32 以外で `west flash` が「動かない」状態を doctor で検出・案内できるようにする | 中 |
| S6 | 環境分離・doctor・シリアル判定をプロファイル駆動にする | 他ベンダー SDK（nRF Connect SDK 等）との衝突を防ぐ | 小 |
| S7 | ファイル形式の互換性方針を決める（`zem.toml` / `zem.lock` / `state.json`） | 拡張のたびに既存プロジェクトを壊さない | 小 |
| S8 | テストをベンダー横断にする（フィクスチャ、E2E マトリクス、サポート Tier） | 拡張しても回帰を検出できるようにする | 中 |

S1・S2・S7 は**新しい MCU を足す前に必ず**済ませる。S3 は 2 ベンダー目を足す時点で、S5 は ESP32 以外の書き込みを
サポートすると宣言する時点で必要になる。実際の 2 ベンダー目は、書き込みに Python 製ツールで済み blobs のない
**RP2040（`rpi_pico`）か、pyOCD で書ける STM32 / nRF の評価ボード**から始めるのが一番楽（§4）。

---

## 2. 現状の ESP32 依存箇所

### 2.1 型・データ

| 箇所 | 内容 | 拡張時の問題 |
|---|---|---|
| `src/board/espressif.rs` の `Soc` enum | 7 種類の ESP32 SoC を列挙。名前・アーキテクチャ・ツールチェーン・未対応バージョンをすべて `match` で返す | SoC を 1 つ足すごとに enum と全ての `match` を変更する必要がある。数百 SoC には対応できない |
| `src/board/mod.rs` の `Arch` | `Xtensa` / `Riscv` の 2 種類だけ | Arm（Cortex-M / A）、ARC、x86、posix（`native_sim`）を表せない。`zem.toml` の `[target].arch` と `state.json` もこの enum でシリアライズしている |
| `src/board/resolve.rs:88-89` | `hal_modules` / `blob_modules` が常に `["hal_espressif"]` | ベンダーごとの HAL（`hal_nordic`、`hal_stm32`、`hal_rpi_pico` など）と依存モジュール（`cmsis` / `cmsis_6`）を選べない |
| `ResolvedTarget.toolchains: Vec<&'static str>` | `'static` 文字列 | ツリーから読んだ値（実行時に決まる文字列）を入れられない |
| `KNOWN_BOARDS` | 14 件の ESP32 ボードの平坦なリスト | `init` の選択肢がこのリストそのもの。ベンダーで分けたり絞り込んだりできない |
| `Soc::absent_through` | ESP32-H2 が v4.2 以前に存在しない、という知識をコードに埋め込み | SoC ごとの「最初に入った Zephyr バージョン」を手で追い続けることになる |

### 2.2 ボード解決

| 箇所 | 内容 | 拡張時の問題 |
|---|---|---|
| `resolve_static`（`src/board/resolve.rs`） | 修飾子 → 静的表 → ボード名の接頭辞（`esp32c3_*`）の順に SoC を決める。どれにも当たらなければエラー | 接頭辞の推測は ESP32 だからできている（`nucleo_f401re` や `rpi_pico` は名前から SoC が分からない）。ボード表を全ベンダー分持つのは保守できない |
| `board_yml::soc_cpuclusters`（`src/board/board_yml.rs:102`） | `soc/espressif/soc.yml` を直接読む | 他ベンダーの `soc/<vendor>/soc.yml`、さらにモジュールやアプリ側の `soc_root` / `board_root` にある SoC・ボードを見られない |
| `board_yml::find_board` | `zephyr/boards/**/board.yml` だけを探す | アウトオブツリーのボード（アプリの `boards/`、モジュールの `board_root`）に対応していない。製品開発ではむしろこちらが主流 |
| sync のステップ順（`src/sync/run.rs`） | ワークスペース取得の**前に**モジュール集合を決める必要がある（`name-allowlist`） | ボードのベンダーが分からないと HAL が決まらず、ワークスペースを作れない。これが静的表に依存せざるを得ない根本原因 |

### 2.3 SDK・ビルド

| 箇所 | 内容 | 拡張時の問題 |
|---|---|---|
| `Soc::toolchain` | ESP32 の Xtensa は SoC ごと、RISC-V は `riscv64-zephyr-elf` | Arm は `arm-zephyr-eabi`、arm64 は `aarch64-zephyr-elf` など、アーキテクチャ単位で決まるものが多い。Xtensa だけは SoC（ベンダー）ごと |
| `store/sdk.rs` の `toolchain_gcc` | `<sdk>/<toolchain>/bin/<toolchain>-gcc` で導入済みかを判定 | Zephyr SDK 1.x ではアセット名・ディレクトリ構成が 0.17 系から変わっている可能性がある（`gnu/` 配下への移動、LLVM の同梱など。要確認）。対応 Zephyr を上げると SDK も上がるので、MCU 追加と同時期に問題になる |
| `store/venv.rs` | `scripts/requirements-base.txt` だけを入れる | pyOCD など書き込み・デバッグ用の Python パッケージは `requirements-run-test.txt` などにあり、入っていない（ESP32 は hal_espressif 同梱の esptool を使うので問題が出ていない） |

### 2.4 書き込み・doctor・環境分離

| 箇所 | 内容 | 拡張時の問題 |
|---|---|---|
| 書き込み | ESP32 は hal_espressif 同梱の esptool（Python）だけで完結する | 他ベンダーは nrfutil / J-Link / OpenOCD / STM32CubeProgrammer / LinkServer / pyOCD / probe-rs / picotool など、多くが**ホスト側のネイティブツール**で、一部はプロプライエタリでライセンス同意が必要。zem は現状何も面倒を見ていない |
| `src/serial.rs` の `known_bridge` | CP210x / CH34x / FTDI / Espressif USB のみ | 未知の VID/PID のポートも一覧には出る（USB の product 文字列か「USB serial」で表示）が、ST-LINK の VCOM、J-Link の CDC、CMSIS-DAP（DAPLink）、nRF の USB などに分かりやすいラベルが付かない |
| `platform::serial_access`（doctor） | Linux の `dialout` / `uucp` グループを確認 | デバッグプローブには udev ルールが必要（グループだけでは足りない）。Windows では WinUSB ドライバ（Zadig 等）が必要なプローブもある |
| `src/env/sanitize.rs`、`src/doctor/checks.rs` の `CONFLICTING_VARS` | ESP-IDF・Zephyr・Python 系の変数と PATH を除外 | nRF Connect SDK（toolchain manager が `ZEPHYR_BASE` や独自の PATH を入れる）、STM32CubeIDE / MCUXpresso の同梱ツールチェーン、ホストの `arm-none-eabi-gcc` などを考慮していない（`GNUARMEMB_TOOLCHAIN_PATH` / `CROSS_COMPILE` だけは除外済み） |

### 2.5 文言・テンプレート・テスト

- `BoardError` の help（「ESP32-series boards only」）、`config/template.rs` のコメント（「hal_espressif is added automatically」「Espressif binary blob licenses」）、`README.md` の説明が ESP32 前提。
- 単体テスト・CLI テスト・E2E のボード名がすべて ESP32。E2E の `poisoned_bin` も ESP-IDF の偽ツールだけ。
- CI は Linux / macOS / Windows で単体テストを回しているが、実際のビルドを伴う E2E は手動（`--ignored`）。

**依存がないもの（そのまま使える）**: ZEM_HOME の構成、共有ワークスペースと `project-filter`、venv / uv、ロックファイルの仕組み、
`platform` 層、`west` のパススルー、`board.yml` の修飾子検証（`valid_targets`）は ESP32 に依存していない。

---

## 3. ステップ詳細

### S1. ベンダー非依存の型に置き換える

**やること**

1. `Soc` enum を `SocId` newtype（例: `"esp32s3"`、`"nrf52840"`、`"stm32f401xe"`）に置き換える。SoC 固有の知識は S2 のプロファイルに移す。
   素の `String` にはせず、フィールドを非公開にして、作れる経路を「修飾子の解析」「soc.yml からの読み込み」「プロファイルの定数」に限る
   （`VendorId` も同様）。

   **enum をやめる理由**: SoC の集合は zem ではなく Zephyr のツリーが決める**開いた集合**である（リリースごとに増減し、モジュールや
   アプリの `soc_root` で外から足せる）。enum が保証していたのは「全 SoC について網羅的に `match` した」ことだが、それは
   「SoC ごとの知識をコードに持つ」ことの裏返しで、S3 でその知識をツリーから読むようにすると守るべき網羅性そのものがなくなる。
   ツリーから読んだ値が「存在する SoC」であることは、enum ではなく sync 時の `board.yml` / `soc.yml` の検証で保証する。

   **検討した代替案**: `enum Soc { Espressif(EspSoc), Nordic(NrfSoc), …, Other(SocId) }` のようにベンダーごとの enum を持つ形。
   既知の SoC については型の安全性が残るが、ベンダーを足すたびに型が増え、`Other` の経路も結局必要になるため採らない。
   一方、`Arch` は Zephyr の `arch/` に対応する閉じた小さな集合で、アーキテクチャごとに振る舞いが本当に変わる（ツールチェーンの既定値など）ので、
   enum のまま網羅的な `match` の恩恵を受ける。
2. `Arch` を Zephyr の `arch/` に合わせて拡張する: `arc`、`arm`、`arm64`、`mips`、`nios2`、`posix`、`riscv`、`rx`、`sparc`、`x86`、`xtensa`。
   Zephyr 側の一覧は有限で変化も遅いので enum のままでよい。ただし将来の追加に備え、デシリアライズで未知の値を受けたときの
   エラーメッセージを分かりやすくする。
3. `ResolvedTarget` の `&'static str` を `String`（または `Cow<'static, str>`）にし、ツリーから読んだ値を入れられるようにする。
   あわせて `vendor: String` と、どこから得た情報か（`source`）をフィールドごとではなく解決全体で持つ現在の形は維持する。
4. `ResolvedTarget` を使う側（`sync/plan.rs`、`sync/run.rs`、`doctor`、`cli/init.rs`、`config/validate.rs`）が `soc.name()` などの
   ESP32 用メソッドに触れないようにする。

**完了条件**: `src/board/espressif.rs` 以外に ESP32 の SoC 名・ツールチェーン名・モジュール名が（テストを除いて）現れない。
CI で `grep -rnE 'hal_espressif|xtensa-espressif|esp32' src --include='*.rs'` を `src/board/vendors/espressif*` 以外で検査してもよい
（`check-platform-boundary.sh` と同じ方式）。

### S2. ベンダープロファイルを導入する

ベンダーごとの差分を**振る舞い（trait）ではなくデータ（静的な構造体）**で表す。ベンダー間で処理の流れは同じで、違うのは値だけだからである。

```rust
// src/board/vendors/mod.rs（案）
pub struct VendorProfile {
    /// board.yml の `vendor` / soc.yml の family と照合する ID（"espressif", "nordic", "st", "raspberrypi", ...）
    pub id: &'static str,
    /// このベンダーの SoC が必要とする west プロジェクト（HAL と依存）
    pub hal_modules: &'static [&'static str],          // 例: ["hal_nordic", "cmsis_6"]
    /// `west blobs fetch` が必要なモジュールと、ライセンス同意が必要か
    pub blob_modules: &'static [BlobModule],
    /// SoC（＋CPU クラスタ）とアーキテクチャから SDK のツールチェーンを決める。None ならアーキテクチャの既定
    pub toolchain_override: fn(soc: &str, cpucluster: Option<&str>) -> Option<&'static str>,
    /// init の候補とオフライン検証用の厳選ボード（全ボードではない）
    pub curated_boards: &'static [KnownBoard],
    /// 衝突しやすいホスト環境（IDF_* や NCS 系）の変数・PATH の目印
    pub conflicting_env: &'static [&'static str],
    pub conflicting_path_markers: &'static [&'static str],
    /// doctor が表示する USB VID/PID
    pub usb_ids: &'static [UsbId],
    /// 書き込み・デバッグに使うツールの要件（S5）
    pub runners: &'static [RunnerRequirement],
}
```

- 既存の ESP32 の値をすべて `espressif` プロファイルに移し、**挙動を変えずに**リファクタリングを終える（この段階で新ベンダーは足さない）。
- まずは Rust のコードとしてコンパイル時に埋め込む。外部 TOML 化（ユーザーが独自プロファイルを足せる）は、需要が見えてから検討する。
- プロファイルの選択は「SoC の所属ファミリー（soc.yml の配置ディレクトリ `soc/<vendor>/`）」で行う。ボード名の接頭辞は ESP32 用の補助としてのみ残す。

### S3. ボード解決を「Zephyr ツリーが正」に寄せる

全ベンダーのボード表を zem に持つのは保守できない。Zephyr のツリーにはすでに必要な情報がある。

| 必要な情報 | ツリー上の情報源 |
|---|---|
| ボードの実在・SoC・CPU クラスタ・バリアント | `boards/**/board.yml`、`soc/**/soc.yml`（実装済み。ただし ESP32 のパスに固定） |
| ベンダー | `board.yml` の `vendor`、SoC が属する `soc/<vendor>/` |
| アーキテクチャ | 各ボードターゲットの twister メタデータ（`boards/<vendor>/<board>/<board_target>.yaml` の `arch`）。要確認: 全ボードに存在するか、命名規則 |
| 対応ランナー | `board.cmake` の `include(${ZEPHYR_BASE}/boards/common/<runner>.board.cmake)` |

**sync の流れを 2 段にする**

```
現在:  モジュール集合を決める（静的解決が必要） → west update（zephyr + HAL） → board.yml で裏取り
変更後: west update（zephyr 本体 + 明示指定のモジュール） → ツリーからボードを解決 → HAL を和集合に追加して west update
```

- 1 段目では zephyr 本体に加えて、`zem.toml` で明示指定されたモジュール（`[modules].extra` と `features`）も取得する。
  モジュールの `board_root` / `soc_root` にしかないボード・SoC は、そのモジュールを取得するまでツリーから見つけられないため。
  アプリの `boards/` はプロジェクト内にあるので、取得の順序には影響しない。
- ワークスペースのモジュールは「追加のみ」の不変条件（mvp-plan.md §4.4）なので、2 段にしても既存の仕組みで安全に追加できる。
  追加の `west update` は no-op なら約 1.6 秒（スパイクの実測）。
- 静的表（`curated_boards`）は**オフラインでの即時検証・`init` の候補・`--dry-run`** 用の「ヒント」に格下げし、ツリーの結果と食い違えばツリーを正とする（現在の方針の延長）。
- 未知のボードを「推測できないのでエラー」にしている現在の挙動は、「ワークスペース取得後に解決する」に変わる。`init` 時は
  「ボード名はまだ検証されていない。`zem sync` で確定する」と警告するだけにする。
- `soc.yml` / `board.yml` の探索を `zephyr/` 直下に限らず、モジュールの `zephyr/module.yml` が宣言する `board_root` / `soc_root`
  と、アプリの `boards/`（`BOARD_ROOT`）にも広げる。アウトオブツリーのボードへの対応はこの延長で入る。
- `Soc::absent_through` のような「SoC の初出バージョン」は持たず、ツリーに無ければ `NotInZephyr` で落とす（既存のエラーで足りる）。

### S4. ツールチェーンと SDK を一般化する

1. ツールチェーンの決定を「アーキテクチャの既定 → プロファイルの上書き」の 2 段にする。
   - 既定: `arm` → `arm-zephyr-eabi`、`arm64` → `aarch64-zephyr-elf`、`riscv` → `riscv64-zephyr-elf`、`x86` → `x86_64-zephyr-elf`、`arc` → `arc-zephyr-elf` など
   - 上書き: Xtensa は SoC ごと（`xtensa-espressif_esp32s3_zephyr-elf`、`xtensa-nxp_*`、`xtensa-intel_*`）
   - `posix`（`native_sim`）はホストの gcc を使い SDK のツールチェーンが不要。`toolchains` が空でも sync が通ることをテストする
2. 複数コア・マルチイメージ（nRF5340 の `cpuapp` / `cpunet`、ESP32-C6 の `lpcore`、sysbuild + MCUboot）では、必要なツールチェーンが
   ボードターゲットの和集合になる。`toolchains: Vec` は既にあるので、和集合を取る場所を 1 か所に決めておく。
3. SDK のアセット名・ディレクトリ構成を、バージョン範囲ごとの小さな表（`SdkLayout`）にまとめる（mvp-plan.md §6 の対策を実装する）。
   SDK 1.x の実物で、アセット名・`sha256.sum`・展開後の構成・`Zephyr-sdkConfig.cmake` の位置を確認してから対応する。

### S5. 書き込み・デバッグ用ホストツール

ESP32 では esptool が Python で完結していたため問題にならなかったが、ほかのベンダーでは `west flash` の成否がホスト側の
ツールに左右される。zem の方針（オーケストレーターに徹する・勝手に巨大なものを入れない）に沿って、次の 3 区分で扱う。

| 区分 | 例 | zem の扱い |
|---|---|---|
| pip で入る | pyOCD、esptool（hal 同梱） | venv に入れる。`requirements-run-test.txt` などから必要なものだけ入れるか、プロファイルで指定したパッケージを `extra_requirements` に足す |
| zem が取得できる（配布条件が許す・単体バイナリ） | OpenOCD（Zephyr SDK の hosttools は Linux 版のみの可能性。要確認）、picotool、probe-rs | 当面は doctor の案内のみ。需要があれば `ZEM_HOME/tools` への取得を検討（mvp-plan.md §4.2 で予約済み） |
| ライセンス同意やインストーラが必要 | J-Link、nrfutil、STM32CubeProgrammer、LinkServer | 取得しない。doctor が検出と案内だけ行う |

- doctor に「Flash」セクションを追加し、`board.cmake` から得たランナー一覧と、ホストで見つかったツールを突き合わせて表示する
  （「このボードは jlink / pyocd / openocd で書き込める。見つかったのは pyocd」）。
- strict モードでは J-Link などのツールが使う変数・PATH を通す必要がある。プロファイルの `runners` に必要な変数を持たせ、
  allowlist に足せるようにする。
- Linux の udev ルール、Windows の WinUSB ドライバの有無を doctor でできる範囲で確認・案内する（`platform` 層に閉じ込める）。
- `zem flash` のような独自ラッパーは作らない（`zem west flash` のパススルーを維持する）。

### S6. 環境分離・doctor・シリアル判定をプロファイル駆動にする

- `sanitize.rs` の `DENY_EXACT` / PATH の目印、`doctor` の `CONFLICTING_VARS` を「共通（Zephyr・Python）＋全プロファイルの和集合」で組み立てる。
  ターゲットのベンダーだけでなく**全ベンダー分**を除外する（ESP32 のプロジェクトでも nRF Connect SDK の PATH は除外したい）。
- `serial.rs` の `known_bridge` をプロファイルの `usb_ids` から引く。プローブ（ST-LINK、J-Link、CMSIS-DAP）も表示する。
- `doctor` の「target」行、`init` の出力、エラーの help 文言をベンダー非依存にする。

### S7. ファイル形式の互換性方針

拡張の途中でフィールドが増えるので、先に方針を決めて文書化する。

| ファイル | 方針 |
|---|---|
| `zem.toml`（`schema = 1`） | フィールドの**追加**は `#[serde(default)]` で後方互換にし、`schema` は上げない。意味が変わる変更・削除だけ `schema` を上げる。`[target].arch` に新しい値が増えるのは追加扱い |
| `zem.lock`（`version = 1`） | 同上。ターゲット情報（ボード・ツールチェーン）を lock に記録するかを決める（再現性の観点では、SDK バージョンだけでなくツールチェーン集合も記録したほうがよい） |
| `.zem/state.json`（`schema = 1`） | いつ消しても再生成できるので、形式が変わったら `schema` を上げて「再 sync を求める」だけでよい。`TargetState` に `vendor` を足す |
| `~/.zem` の構成 | ワークスペースのキーは `(url, revision)` のまま。2 段 sync にしてもキーは変わらないことを確認する |

`deny_unknown_fields` を使っているため、**新しい zem が書いたファイルを古い zem が読むとエラーになる**。これは意図どおり
（黙って無視するよりよい）だが、エラーメッセージに「zem を更新してください」と出るようにしておく。

### S8. テストをベンダー横断にする

1. **フィクスチャのツリー**: 現在の `board_yml.rs` のテストのような小さな偽ツリーを、ESP32 / nRF（マルチコア）/ STM32 /
   RP2040 / `native_sim` / アウトオブツリーのボード分用意し、解決ロジックを単体テストで網羅する。
2. **実ツリーとの照合テスト（nightly）**: 対応する Zephyr リリースのツリーを取得し、全ボードターゲットについて
   「解決できる」「ツールチェーンが SDK の `sha256.sum` に存在する」「プロファイルが見つかる」を検査する。
   静的表（`curated_boards`）の陳腐化もここで検出する。
3. **E2E のビルドマトリクス**: 代表ボードを 1 アーキテクチャ・1 ベンダー 1 つ程度に絞る。

   | ボード | 狙い |
   |---|---|
   | `esp32_devkitc/esp32/procpu` | Xtensa、blobs あり（現状） |
   | `esp32c3_devkitm` | RISC-V |
   | `nrf52840dk/nrf52840` | Arm Cortex-M、`hal_nordic` + `cmsis` |
   | `nucleo_f401re` | ボード名から SoC が分からないケース、`hal_stm32` |
   | `rpi_pico` | blobs なし、picotool / UF2 |
   | `native_sim` | SDK ツールチェーン不要（Linux のみ） |

   ビルドだけなら実機は不要なので、GitHub Actions の nightly で Linux / macOS / Windows に回せる（キャッシュに `ZEM_HOME` を使う）。
4. **サポート Tier を README に明記する**:
   - Tier 1: 実機での書き込みまで確認（現在は `esp32_devkitc` のみ）
   - Tier 2: CI の E2E でビルドを確認
   - Tier 3: ツリーから解決できる（未検証）
   ボードごとの確認状況は `docs/hardware-support.md`（新設）に表で持ち、README の WARNING はそこへのリンクにする。

---

## 4. 2 ベンダー目の選び方

S1〜S3・S7 が済んだら、次の順で足すのが摩擦が少ない。

| 候補 | 良い点 | 注意点 |
|---|---|---|
| RP2040 / RP2350（`rpi_pico` など） | blobs なし、ツールチェーンは `arm-zephyr-eabi`、安価で実機確認しやすい | 書き込みは UF2（ドラッグ＆ドロップ）か picotool / OpenOCD。RP2350 は Arm と RISC-V の両方を持つ |
| nRF52 / nRF53（`nrf52840dk` など） | Zephyr で最も整備されている | 書き込みは J-Link / nrfutil が中心（ライセンス）。nRF Connect SDK との環境衝突を防ぐ必要がある |
| STM32（Nucleo） | ボード名から SoC が分からない、を検証できる | ST-LINK の書き込みは OpenOCD / STM32CubeProgrammer / pyOCD |

最初の 1 つは「ビルドまで Tier 2」で足し、書き込み（S5）は doctor の案内から始めると、範囲を小さく保てる。

---

## 5. 進め方（PR の単位）

| PR | 内容 | 挙動の変化 |
|---|---|---|
| 1 | S7 の方針を文書化。`Arch` の拡張と、未知の値のエラーメッセージ | なし |
| 2 | S1: `SocId` と `ResolvedTarget` の文字列化 | なし（純粋なリファクタリング） |
| 3 | S2: `VendorProfile` の導入と ESP32 の値の移動。S6 の環境分離・doctor・シリアルのプロファイル化 | なし |
| 4 | S8-1: ベンダー横断のフィクスチャと、ESP32 依存の文言の一般化 | 文言のみ |
| 5 | S3: 2 段 sync とツリーからの解決、`soc.yml` 探索の一般化 | 未知ボードの扱いが「init で拒否」から「sync で解決」に変わる |
| 6 | S4: ツールチェーンの既定表と `SdkLayout` | なし（ESP32 では） |
| 7 | 2 ベンダー目のプロファイル + E2E マトリクス（S8-3）+ サポート Tier（S8-4） | 対応 MCU の追加 |
| 8 | S5: doctor の Flash セクション、pip で入るランナー | 書き込みの案内 |

PR 2〜4 は ESP32 の挙動を一切変えないことが条件。各 PR で既存の CLI テストと、手元の ESP32 実機での `init → sync → build → flash` を確認する。

---

## 6. 未確認事項（着手前に調べる）

- 全ボードターゲットに twister メタデータ（`<board_target>.yaml` の `arch`）があるか。無い場合のアーキテクチャの決め方（SoC の Kconfig を読む等）
- Zephyr SDK 1.x のアセット名・ディレクトリ構成・`sha256.sum` の形式、Zephyr の各リリースが要求する SDK バージョン
- Zephyr SDK の hosttools（OpenOCD 等）が macOS / Windows 向けに提供されているか
- `hal_nordic` / `hal_stm32` / `hal_rpi_pico` が依存する west プロジェクト（`cmsis` / `cmsis_6` 等）と、それが Zephyr のリリースごとに変わるか
- `west blobs fetch` がライセンス同意を要求するベンダー（`hal_nxp` 等）の挙動
- モジュールの `board_root` / `soc_root` にあるボードを `project-filter` で inactive にした場合、Zephyr のボード列挙から消えるか
