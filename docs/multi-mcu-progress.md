# 対応 MCU 拡張の進捗記録

> 計画: [multi-mcu-readiness.md](multi-mcu-readiness.md)（ステップ S1〜S8 と PR の進め方）
> 最終更新: 2026-11-28。この文書は作業を中断・再開するための記録で、計画との差分と未解決事項もここに残す

---

## 1. 現在地

S1・S2・S8-1・S3 を実装し、PR #8〜#11 として**スタック**で出してある（いずれも未マージ、3 OS の CI は通過）。
**#8〜#11 のセルフレビューと修正は完了**（§6）。§5 の論点を決めてから #8 から順にマージし、S4 へ進む。

| PR | ブランチ | base | ステップ | 内容 |
|---|---|---|---|---|
| [#7](https://github.com/k-sukke/zem/pull/7) | `docs/multi-mcu-readiness` | `main` | — | 計画の文書化、`PLAN.md` → `docs/mvp-plan.md`（**マージ済み**） |
| [#8](https://github.com/k-sukke/zem/pull/8) | `refactor/vendor-neutral-target-types` | `main` | S1 | `Soc` enum → `SocId` newtype、`Arch` を Zephyr の全アーキテクチャに拡張、`ResolvedTarget` を所有文字列に |
| [#9](https://github.com/k-sukke/zem/pull/9) | `refactor/vendor-profile` | #8 | S2 + S6 | `src/vendor/` と `VendorProfile` を新設し、ESP32 固有の値（SoC・既知ボード・HAL・blobs・除外する環境変数と PATH・USB ID）を移動。`state.json` に `target.vendor` |
| [#10](https://github.com/k-sukke/zem/pull/10) | `refactor/vendor-neutral-wording` | #9 | S8-1 | 複数ベンダーの偽 Zephyr ツリー（`src/board/fixtures.rs`）、`soc/**/soc.yml` の探索、`board.yml` の `vendor`、エラー文言とテンプレートの一般化 |
| [#11](https://github.com/k-sukke/zem/pull/11) | `feat/tree-board-resolution` | #10 | S3 | 名前から SoC が分からないボードを sync 時にツリーから解決する 2 段 sync、CPU クラスタの補完、`absent_through` の削除、モジュールが空のときの `west.yml` の修正 |

**マージ手順**: #8 から順にマージする。下の PR をマージすると、GitHub が次の PR の base を自動で付け替える（付け替わらない場合は `gh pr edit <n> --base main`）。

## 2. 作業の進め方（合意事項）

- ステップごとに前のブランチから切って PR を積む（スタック）。各 PR は ESP32 の挙動を変えないことを基本とし、変える場合は PR 本文に明記する。
- 各ステップで、USB 接続の **Seeed XIAO ESP32C3**（`xiao_esp32c3`、macOS では `/dev/cu.usbmodem1101`、ESP USB-Serial/JTAG）を使い、
  `zem init → sync → west build → west flash` と起動ログ（`Hello World! xiao_esp32c3/esp32c3`）を確認する。
  起動ログは、zem の venv の pyserial で RTS を一度立ててリセットし、数秒読み取って確認している。
- ファイル形式（`zem.toml` / `zem.lock` / `state.json` / `~/.zem`）の互換性は試作段階のため考慮しない（S7）。

## 3. 計画からの変更点

| 変更 | 理由 |
|---|---|
| `soc.yml` 探索の一般化を S3（#11）ではなく #10 で実施 | 偽ツリーで他ベンダーのテストを書くのに必要だったため |
| S7 の PR（互換性方針の文書化）をなくし、PR 番号を詰めた | 互換性を考慮しない方針になったため |
| `SocId` は素の `String` ではなく、`board` の外では作れない newtype にした | 開いた集合を表しつつ、出どころを限定するため（readiness.md S1 に理由を記載） |
| ボードを後回しにしたとき、`validate` は警告を出さない | 警告にすると、正しく使えているボードでも sync や doctor のたびに出続けるため。案内は `init` / `--dry-run` / `doctor` の表示だけにした |
| `absent_through`（SoC が最初に入った Zephyr のバージョン）を削除 | 計画どおり。その結果、「v4.2.0 で ESP32-H2」は init では止まらず、sync の Board ステップで `NotInZephyr` になる |

## 4. 確認済みのこと

- 各 PR で clippy・単体テスト・CLI テスト・platform 境界チェックが通過。GitHub Actions の Linux / macOS / Windows もすべて通過。
- 実機（XIAO ESP32C3、macOS）で #8・#9・#10・#11 それぞれの `init → sync → build → flash` と起動を確認。
- #9: シェルに `IDF_PATH` / `IDF_TOOLS_PATH` がある状態で、doctor が警告し、ビルドには混入しない。
- #11: 実際の Zephyr v4.2.0 ツリーで、表にない `ttgo_t8c3` と `tdongle_s3`（`/esp32s3/procpu` に補完）の sync とビルドが通る。
  空の ZEM_HOME から `ttgo_t8c3` を sync すると、Board ステップで `hal_espressif` が追加される。2 回目の sync は何もせず、`--locked` も通る。
  `nucleo_f401re` は sync の Board ステップで `unsupported_soc` になる。

## 5. 振り返りで検討したい点

1. **CodeRabbit が #8〜#11 をレビューしていない**。base が `main` ではない PR は対象外になっているとみられる。
   `@coderabbitai review` で手動依頼するか、base が `main` に付け替わってから順にレビューを受けるか。
2. **プロファイルのない SoC のエラーが遅い**（#11）。Zephyr 本体を取得した後に初めて `unsupported_soc` になる。
   共有ワークスペースがなければ約 500 MB の取得が先に走る。`init` の時点で「未対応ベンダーの可能性がある」と伝えるべきか。
3. **CPU クラスタの補完は `soc.yml` の最初のクラスタ**（#11）。ESP32（`procpu`）と nRF5340（`cpuapp`）では妥当だが、
   順序に意味がある保証はない。プロファイルで既定のクラスタを持たせるべきか。
4. **静的なボード表（`KnownBoard`）の役割**。ツリーから解決できるようになり、表は `init` の候補とオフラインでの即時検証だけに使っている。
   `default_qualifiers` はツリーの補完と重複している。
5. **実機確認が macOS のみ**。Windows / Linux の実機と、`cargo test --test e2e -- --ignored` は #8〜#11 では実行していない。
6. **アウトオブツリーのボード**（アプリの `boards/`、モジュールの `board_root`）は、`find_board` がまだ `zephyr/boards` しか見ない。
   readiness.md S3 の残り。

## 6. セルフレビュー記録

CodeRabbit がレビューしないため、#8 から順にセルフレビューする。指摘は PR ごとにここへ記録し、
対応方針（修正 / 見送り / 後続ステップで対応）と状態を残す。

**修正の入れ方**: スタックなので、指摘はその PR のブランチで直し、上のブランチを順に rebase する（`git rebase --update-refs` が使える）。

### #8 S1: 型をベンダー非依存に

| # | 種別 | 指摘 | 方針 | 状態 |
|---|---|---|---|---|
| 8-1 | 要修正 | `Arch::Nios2` は Zephyr v4.0 以降には存在しない。v4.2.0 の `arch/` は `arc arm arm64 mips posix riscv rx sparc x86 xtensa`（他に `common/`）。zem は v4.0 以降が対象なので不要。readiness.md S1 の一覧も同様に誤り | `Nios2` を削除し、readiness.md の一覧も直す。`Arch` の一覧とツリーの照合は、S8-2 の実ツリー照合テストで行う | 対応済み |
| 8-2 | 軽微 | `SocId` の `PartialOrd` / `Ord` / `Hash` の derive が使われていない | 削除（必要になってから足す） | 対応済み |
| 8-3 | 確認 | `KnownBoard.soc` が `&str` になり、表の SoC 名の誤字はコンパイル時に検出できない | テスト（既知ボードの SoC が表にあること）と、`resolve_static` の `expect` で担保されている。現状維持 | 見送り |
| 8-4 | 確認 | `ResolvedTarget.soc` は `SocId`、`state.json` の `TargetState.soc` は `String`。型がそろっていない | `state.json` は保存用のデータなので文字列のままでよい。現状維持 | 見送り |
| 8-5 | 好み | `missing_toolchains` / `SdkStore::ensure` / `ensure_blobs` が `S: AsRef<str>` のジェネリクス。テストが `&[&str]` を渡すためだけ | `&[String]` に寄せると単純だが、どちらでもよい。現状維持 | 見送り |
| 8-6 | 確認 | readiness.md S1 の完了条件（ESP32 の名前が `espressif.rs` 以外に出ない）は #8 単体では満たしていない（`resolve.rs` の `hal_espressif`） | #9 で満たしている。PR 本文に明記済み | 対応済み（#9） |

### #9 S2: ベンダープロファイル

| # | 種別 | 指摘 | 方針 | 状態 |
|---|---|---|---|---|
| 9-1 | 要修正 | `VendorProfile.id` のコメントが「board.yml の `vendor` と `soc/<vendor>/` に一致する」となっているが、どちらとの照合にも使っていない。そもそも board.yml の `vendor` はボードのメーカー（`xiao_esp32c3` なら `seeed`）なので、SoC のベンダーとは一致しない | コメントを「SoC ベンダーの識別子。`state.json` に記録する」に直す | 対応済み |
| 9-2 | 軽微 | `ResolvedTarget` が `&'static VendorProfile` を持ち、`derive(Debug)` でプロファイル全体（ボード表など）が出力される。テスト失敗時や `{:?}` の出力が読みにくい | `VendorProfile` の `Debug` を `id` だけにする | 対応済み |
| 9-3 | 軽微 | 古い `state.json` を読むと「corrupt。ファイルを消して `zem sync`」と出るが、実際は `zem sync` だけで上書きされる | ヒントを「`zem sync` を実行」に寄せる | 対応済み |
| 9-4 | 確認 | プロファイルの環境変数が `env_prefixes` / `env_vars` / `notable_env_vars` の 3 つに分かれていて、やや複雑 | 除外（分離）と警告（doctor）で目的が違うので現状維持 | 見送り |
| 9-5 | 確認 | 全ベンダーの変数を除外するので、将来ランナー（J-Link 等）が必要とする変数まで消える可能性 | S5 でランナー要件と allowlist を扱うときに考える | S5 で対応 |

### #10 S8-1: 複数ベンダーのフィクスチャ・文言

| # | 種別 | 指摘 | 方針 | 状態 |
|---|---|---|---|---|
| 10-1 | **要修正（バグ）** | `soc.yml` のパーサーが `family → series → socs` しか読まない。本物の v4.2.0 では `family` 直下の `socs`、トップレベルの `series` / `socs` もあり、940 SoC 中 73 個を読めていない。うち 7 個はマルチコア（`max32655` `max32666` `max32680` `max32690` `max78000` `max78002` `openisa_rv32m1`）で、#11 のツリー解決ではクラスタの検証・補完を誤る（MVP 時点からの潜在バグ） | 3 つの形すべてを読む。フィクスチャに各形を入れてテストする | 対応済み |
| 10-2 | 要修正 | フィクスチャを本物と照合すると差分がある。`rpi_pico` に `w` バリアントがない、native の `soc.yml` は実際は `soc/native/inf_clock/soc.yml` で `family` 直下に `socs`、`native_sim` の board.yml は `boards:` のリスト形式 | 本物の v4.2.0 の内容に合わせる（10-1 のテストも兼ねる） | 対応済み |
| 10-3 | 軽微 | `board.yml` の `vendor` を読むようにしたが、#11 でもテスト以外で使っていない（ベンダーは SoC から決めている。9-1 のとおりボードのメーカーなので用途もない） | フィールドを削除する | 対応済み |
| 10-4 | 確認 | `soc_cpuclusters` は SoC が見つからないと空を返すため、`soc.yml` が読めないマルチコア SoC は「シングルコア扱い」になり、補完されないまま `west build` で失敗する | 10-1 を直せば実害はほぼない。現状維持 | 見送り |
| 10-5 | 確認 | `find_board` / `soc_cpuclusters` は呼ぶたびにツリーを全走査する（Board ステップで 0.3〜0.6 秒） | 許容範囲。現状維持 | 見送り |

### #11 S3: ツリーからのボード解決

| # | 種別 | 指摘 | 方針 | 状態 |
|---|---|---|---|---|
| 11-1 | 要修正 | sync はツリー解決に、静的解決で修飾子を補った名前を渡している。そのため静的表の SoC が誤っているときは `SocNotOnBoard` で失敗し、PR 本文の「食い違ったらツリーが正」になっていない（警告に届くのは修飾子なしで静的解決できた場合だけ） | ツリーにはユーザーが書いた名前（`zem.toml` の値）を渡す。ESP32 のマルチコア SoC では、`soc.yml` の最初のクラスタと静的表の既定値（`procpu` / `hpcore`）が一致することを v4.2.0 で確認済みなので、結果は変わらない | 対応済み |
| 11-2 | 確認 | 2 回目の `workspace.ensure` に `opts.update` を渡していない | 1 回目で更新済みなので不要。現状維持 | 見送り |
| 11-3 | 確認 | `state.json` の `target.source` は常に `board_yml` になり、`qualifier` などの値は `--dry-run` でしか見えない | 情報として十分。現状維持 | 見送り |

### まとめて直した対象（2026-11-28 対応済み）

8-1, 8-2, 9-1, 9-2, 9-3, 10-1, 10-2, 10-3, 11-1。それぞれ該当 PR のブランチに修正コミットを積み、`git rebase --update-refs` で上のブランチを付け替えて force push した。

- 各ブランチで clippy・テストが通過。
- 10-1 は、修正後のパーサーで本物の v4.2.0 を走査し、940 SoC・788 ボードがすべて見つかることを一時テストで確認した（このテストは S8-2 の実ツリー照合テストの原型になる）。
- 11-1 の後も、ボード名だけ書いた `esp32s3_devkitc` が `esp32s3_devkitc/esp32s3/procpu` に解決されることと、XIAO ESP32C3 での書き込み・起動を確認した。

## 7. 次の作業

1. §5 の論点と §6 のセルフレビュー（#8 → #9 → #10 → #11 の順）の結論を反映（必要なら #8〜#11 に追加コミット）し、#8 から順にマージする。
2. S4: アーキテクチャごとの既定ツールチェーン＋プロファイルの上書き、SDK のバージョンごとの配置（`SdkLayout`）。SDK 1.x の実物の確認が先。
3. 2 ベンダー目のプロファイル（RP2040 が最有力）、E2E のビルドマトリクス、サポート Tier（readiness.md S8-3・S8-4）。
4. S5: doctor の Flash セクション、pip で入るランナー。
