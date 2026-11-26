# リファクタリング計画

`pip` と `uv` の併用を前提にしていた初期設計から、**`uv` のみを使用する前提**へ移行したことに伴う簡略化と、コードベース全体のデッドコード削除・重複整理を行うための計画。

- 前提: `zem.toml` / `zem.lock` / `.zem/state.json` などの後方互換性は維持しなくてよい。
- 進め方: 各 Phase ごとに `cargo test`・`cargo clippy --all-targets`・`./scripts/check-platform-boundary.sh` が通る状態を保ちながら段階的に進める。

---

## Phase 1: 進行中変更の完結とテスト復旧

現在の未コミット変更（`VenvMarker` から `installer` / `python` / `bootstrap` を削除し `packages` を非 `Option` 化）に追従して、テストを復旧する。

- [x] `tests/cli.rs` の `venv_rebuild` モジュールを新しい `VenvMarker` に合わせて修正
  - `installer` / `python` / `bootstrap` フィールドの参照を削除
  - `for installer in ["venv", "uv"]` の旧インストーラ移行テストを `uv` 前提に整理
- [x] `cargo test` / `cargo clippy --all-targets` がエラーゼロで通ることを確認

---

## Phase 2: デッドコード削除と重複実装の統合

振る舞いを変えずに不要コードと重複を削る。

### デッドコード削除
- [x] `src/home/mod.rs`: `ZemHome::git_cache_dir()`、`ZemHome::config_path()` を削除
- [x] `src/platform/{mod,unix,windows}.rs`: `serial_port_hint()` を削除
- [x] `src/env/mod.rs`: `EnvBuilder::remove()` を削除
- [x] `src/process.rs`: `Runner::verbose()`、`Runner::log_path()` を削除
- [x] `src/board/qualifier.rs`: `BoardId::cpucluster()` を削除
- [x] `Cargo.toml`: `repository` と重複している `homepage` を削除

### 重複実装の統合・整理
- [x] シェルクォート関数の共通化（`src/env/shell.rs`, `src/project/activate.rs`, `src/doctor/checks.rs`, `src/project/west_config.rs`）
- [x] `hex()` 関数の統合（`src/home/keys.rs` と `src/store/download.rs`）
- [x] `src/doctor/checks.rs`: `CONFLICTING_VARS` の独自リストを廃止し、`env::is_denied()` ベースに統一
- [x] `src/store/mod.rs`: `tempfile::Builder::new().prefix(EXTRACT_PREFIX).tempdir_in(...)` をヘルパー化し、`uv.rs` / `venv.rs` / `sdk.rs` から利用
- [x] `src/manifest/features.rs`: `module_for()` を削除し、`Feature::as_str()` を直接使用
- [x] `src/sync/run.rs`: `run_steps` 内で3回呼ばれている `sync_env()` を整理

---

## Phase 3: `uv` 専用化に伴う簡略化

- [ ] `[python].host_tools_via_pip` の廃止（常に `uv` 経由で `west` / `cmake` / `ninja` を venv に入れる前提にする）
  - `src/config/schema.rs` (`Python`)
  - `src/lockfile.rs` (`PythonLock`)
  - `src/home/keys.rs` (`VenvKey`)
  - `src/store/venv.rs` (`bootstrap_packages` を定数化または簡略化)
  - `src/doctor/checks.rs` (`build_tool_checks` から `host_tools_via_pip = false` 分岐を削除)
  - `src/tools/mod.rs` (`MIN_CMAKE` 等、不要になった定義の整理)
  - `src/platform/{unix,windows}.rs` (`cmake` / `ninja` のインストールヒント整理)
  - 関連テスト・スナップショットの更新
- [ ] `src/tools/mod.rs` の `ToolSpec` 簡略化
  - 複数候補探索 (`candidates`) と `version_args` を廃止し、`probe(name: &str)` / `require_in_env(env, name)` に単純化
- [ ] `src/env/sanitize.rs` に `"UV_"` を `DENY_PREFIXES` として追加
  - ホストの `UV_INDEX_URL` / `UV_OFFLINE` / `UV_SYSTEM_PYTHON` 等が `zem` 管理の `uv` に影響しないようにする（`zem_env_from` で `UV_CACHE_DIR` / `UV_PYTHON_INSTALL_DIR` を明示設定する）
- [ ] コメント・メッセージ内の旧 `pip` 表記を整理（`src/lockfile.rs` 等）

---

## Phase 4: スキーマ・内部データモデルの簡略化

後方互換不要の前提で、冗長な設定項目と状態フィールドを削除する。

- [ ] `[target].arch` を削除
  - `src/config/schema.rs` (`Target.arch`)
  - `src/config/validate.rs` / `src/board/resolve.rs` (`arch_override` 引数)
  - `src/board/mod.rs` (`BoardError::ArchMismatch`)
- [ ] `ProjectState.sdk` / `ProjectEntry.sdk` を `Option<...>` から必須フィールドへ変更
  - `src/project/state.rs`, `src/store/registry.rs`, `src/session.rs`, `src/doctor/checks.rs`, `src/store/gc.rs`
- [ ] `TargetState.source` (`ResolutionSource`) の保存を廃止
  - `src/project/state.rs`, `src/sync/run.rs`, `src/store/gc.rs`

---

## Phase 5: 仕上げ（可視性・ドキュメント・最終検証）

- [ ] `src/lib.rs` 及び各モジュールの可視性 (`pub` vs `pub(crate)`) を見直し、今後のデッドコードをコンパイラが検出できるようにする
- [ ] `README.md` 等のドキュメントから廃止した設定（`host_tools_via_pip`, `[target].arch` 等）の記述を更新
- [ ] 最終検証
  - `cargo test`
  - `cargo clippy --all-targets -- -D warnings`
  - `./scripts/check-platform-boundary.sh`
