## Quick orientation for AI coding agents

This file contains concise, actionable information to help an AI agent be productive in BletchMAME (Rust + Slint UI).

- Big picture: this is a Rust desktop GUI app with a Slint UI front end, a small backend runtime abstraction, and a number of background worker threads that interact with an external MAME binary.

- Important directories/files:
  - `src/main.rs` — CLI flags, sets up tokio runtime, Slint backend and runs the Slint event loop.
  - `src/appstate.rs` — the canonical application state (InfoDb builds, live session, failures). Central place for listxml (InfoDb) lifecycle (`infodb_rebuild`, `infodb_build_complete`).
  - `src/appwindow.rs` — bridges the `AppState` -> UI; contains `AppModel` and patterns used to update UI models (`with_items_table_model`, `modify_prefs`, `update_state`).
  - `src/backend/mod.rs` — abstraction over Slint backends (Winit vs optional Qt). Use `BackendRuntime::new()` to create and `--slint-backend` CLI to pick at runtime.
  - `ui/` — all Slint `.slint` UI definitions. `ui/main.slint` exports modules (e.g. `AppWindow`) included by `slint::include_modules!()` in `src/main.rs`.
  - `Cargo.toml` — lists important features: `diagnostics` (on by default) and `slint-qt-backend` (optional). Build deps include `slint-build`.
  - `.github/workflows/general.yml` — CI shows how tests are run (matrix includes qt true/false; `--all-features` used for qt matrix entries).

- Data & flow summary (most important):
  1. `InfoDb` is produced from MAME's `-listxml` output. InfoDb building is performed in background jobs and exposed via `AppState` (`src/appstate.rs`).
  2. `AppState` -> `AppModel` (in `src/appwindow.rs`) -> Slint `AppWindow` UI. UI updates always go through `update_state()` / `modify_prefs()` so downstream models (items table, collections) remain consistent.
  3. MAME integration: external `mame` binary is used for `-listxml` (diagnostics) and live sessions (`spawn_mame_session_thread` in `runtime::session`). Expect IO, long runs and cancellation handling.

- Concurrency & runtime notes:
  - A Tokio runtime is created in `main.rs` for background async work. The Slint UI runs its own event loop via `slint::run_event_loop()`; short-lived async tasks are started using `slint::spawn_local(...)` and long-running jobs use explicit `Job` threads (see `job.rs`).
  - Shared state uses `Rc`, `RefCell`, `Arc<Mutex<...>>` and channels. Prefer using the helpers in `src/appwindow.rs` (`with_items_table_model`, `update_state`) instead of mutating UI models directly.

- Build / test / run commands (project-specific):
  - Build (default features):
    - `cargo build` or `cargo build --release`
  - Run (common):
    - `cargo run -- [--process-listxml]` (see `src/main.rs` for `--process-listxml` and other flags)
    - Diagnostics run that mirrors CI: `mame -listxml | cargo run --release -- --process-listxml`
  - Test matrix / features:
    - CI uses `cargo test` and `cargo test --all-features` when testing the Qt backend. To include optional Qt backend locally: `cargo test --all-features` (or `cargo test --features slint-qt-backend`).
  - Lint/format (CI): `cargo clippy` and `cargo fmt -- --check` (see `.github/workflows/general.yml`).

- Runtime flags and logging:
  - CLI flags are parsed in `src/main.rs` (see `Opt`): `--prefs-path`, `--mame-windowing`, `--slint-backend`, `--process-listxml`, `--process-listxml-file`, `--log`, `--no-capture-mame-stderr`.
  - Logging can be controlled with `--log` or via `RUST_ENV` env var. Use these for tracing (e.g. `--log trace`).

- Project-specific patterns and examples to follow:
  - UI: Slint components are generated and accessed via `ui::AppWindow` (see `src/main.rs` and `src/appwindow.rs`). To update a Slint model, use the provided downcast helpers: `get_items_model()` then `downcast_ref::<ItemsTableModel>()` as shown in `AppModel::with_items_table_model()`.
  - Preferences: Always call `modify_prefs()` (see `AppModel::modify_prefs`) to persist and smoothly propagate preference changes to state and UI.
  - InfoDb lifecycle: trigger a rebuild with `AppState::infodb_rebuild()`; inspect progress using `infodb_build_progress()` and completion via `infodb_build_complete()`.
  - Backends: for platform-specific behavior use `BackendRuntime` helpers (e.g. `wait_for_window_ready`, `create_child_window`) rather than calling Slint/Winit directly.

- Integration & external deps to be aware of:
  - External MAME binary: required for diagnostics and InfoDb (`mame -listxml`). CI installs `mame` only for diagnostics job.
  - Optional Qt support via `slint-qt-backend` feature — requires Qt to be installed when enabled (CI uses `jurplel/install-qt-action`).

If any of these sections are unclear or you want more examples (call sites, common edits, or tests to add), tell me which area to expand and I will iterate the file.
