# Agent Instructions

## 1. Strict Read-Only Guardrails (The _reference Directory)
- The `_reference` directory and its contents are strictly for reference purposes. 
- You MUST NOT modify, manipulate, delete, or rename any files or folders inside the `_reference` directory. All new implementation code must live in the root package structure (e.g., `src/`, `Cargo.toml`).

## 2. Compilation Compatibility & Crate Selection
- **Decoupled Architecture:** `proj-core` is fundamentally architected as a decoupled, standalone CLI tool executed from R (via `projr`) through process delegation, rather than utilizing in-memory embedded bindings like `extendr`. This structural boundary ensures that the Rust execution context is strictly isolated from R's single-threaded runtime constraint.
- **Performance & Isolation Advantages:** This process separation enables stable, uninhibited multi-threaded workspace sweeps (utilizing `blake3` parallel hashing), guarantees frictionless CRAN check server validation passes by avoiding complex dynamic library linking, and provides isolated execution process spaces protecting both runtimes.
- **Handoff Protocol:** To prevent environment pollution from the active R session, the R wrapper (`projr`) implements a strict handoff protocol. It strips active runtime variables and passes down a completely clean environment canvas to the CLI execution context, preserving only an explicit whitelist (e.g., `PATH` and `HOME`) paired with the live R library path forwarded explicitly through the `R_LIBS` key.
- CRAN and enterprise environments (like Debian Stable or RHEL) enforce rigid, older compiler architectures. 
- Use stable, mature, and widely compatible crates (e.g., standard `serde`, `serde_yaml`, `clap`, `regex`) rather than bleeding-edge alternatives. Avoid unnecessary external dependencies to minimize compilation friction on older toolchains.

## 3. Library-First Architecture & Modular Organization
- **Library-First Constraint:** This project must be designed as a core shared library first (`src/lib.rs`), with a thin CLI binary wrapper (`src/main.rs`). All internal logic (parsing configurations, compiling ignores, computing paths) must execute through exposed, public module functions within the library layer. The binary entry point (`src/main.rs`) must only handle command-line shell argument parsing and routing.
- **Module Separation (Anti-Bloat Rule):** You MUST NOT write the core implementation logic directly inside `src/lib.rs`. Keep `src/lib.rs` reserved as a clean, high-level traffic controller that merely declares and re-exports submodules.
- **Dedicated Submodules:** Break distinct functional components out into their own dedicated files inside `src/` and declare them at the top of `src/lib.rs` using the `pub mod <name>;` syntax. For example:
  - `src/version.rs` for authority file parsing and formatting logic.
  - `src/yml.rs` for `_proj.yml` structural validation and default parameter injection.
  - `src/ignore.rs` for workspace file-system scanning and the demarcation slicing engine.

## 4. Testing Suite Organization
- **Unit Tests Module Block:** Keep lightweight, pure-function unit tests (such as version string parsing edge cases, validation mapping errors, or basic regex matches) wrapped inside an inline `mod tests` block flagged with `#[cfg(test)]` at the *bottom* of the specific submodule file (`src/version.rs`, `src/yml.rs`, etc.). 
- **Integration Tests Separation:** You MUST NOT place tests that read or write files to disk, modify active process environment states, or simulate workspace layouts inside the `src/` folder. All filesystem-dependent or multi-module workflow tests must be written as separate integration test files inside a root-level `tests/` directory.

## 5. Documentation Strategy
- For every struct, enum, and public function implemented, you must immediately author inline Rust documentation comments (`///`).
- Include a descriptive overview, details on expected parameters, edge cases/errors, and an executable markdown ````rust ... ```` code example block within the comments. This ensures that CLI help screens (`--help`) and API definitions stay perfectly synchronized.
- Run `cargo test --doc` to verify that the examples written inside your documentation comments actually compile and pass successfully.
- Verify that running `cargo doc --no-deps` generates clean, complete API reference pages without warnings.

## 6. Reference Website Architecture & Synchronization
- **Documentation Site Layout:** A Quarto-based documentation website lives in the root `docs/` directory. Whenever you introduce, modify, or extend a core framework capability or CLI subcommand, you must immediately create or update its corresponding user-facing page under this structure:
  - `docs/index.qmd` – High-level onboarding introduction and quick-start summary.
  - `docs/install.qmd` – Platform prerequisite setup guidelines (WSL2/Linux/macOS).
  - `docs/configuration.qmd` – Validation specifications for `_proj.yml` (prefixes, fallback rules, errors).
  - `docs/cli/` – Command usage manuals:
    - `docs/cli/version.qmd` – Reference documentation for `proj version get` options and JSON schemas.
    - `docs/cli/yml.qmd` – Reference documentation for evaluating configuration properties.
    - `docs/cli/path.qmd` – Reference documentation for path resolution and temporary build cache locations.
    - `docs/cli/ignore.qmd` – Reference documentation for `proj ignore` logic, protected lists, and boundaries.
    - *Add new `docs/cli` qmds as needed, and update their functional descriptions here.*
- **Continuous Alignment:** Do not leave pages blank or create placeholders. The technical narrative inside the `docs/` files must mirror the behavior of the active Rust code and the terminal `--help` output exactly as features are implemented.

## Ponytail

For coding, refactoring, bug-fixing, review and implementation design, read
`.agents/skills/ponytail/SKILL.md` and apply Ponytail in **full** mode by
default. Use **ultra** only when explicitly requested. Repository-specific
correctness, security, architecture and validation requirements take precedence
over generic simplification guidance.

## GitHub issues and Projects

For issue and Project administration, use the canonical
[MiguelRodo/github-projects-skill](https://github.com/MiguelRodo/github-projects-skill/tree/main/skills/github-projects)
skill and read `.projects/project.md` for this repository's routing contract.
The central issue store owns planning; this repository is its implementation mirror.
