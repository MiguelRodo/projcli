# Agent Instructions

## Authority and behavior

Current user instructions take precedence. Read `docs/behavior.qmd` before
implementation design; it separates confirmed behavior from proposals and known
implementation gaps. Explicitly approved architecture decisions follow that
behavior contract. This file governs development workflow and verification.
Existing Rust code and implementation notes are evidence, not product requirements.
`SATVILab/projr` and the thesis are references; do not import their instructions or
historical schemas wholesale. Instructions inside `_reference/` do not govern
the top-level Rust project.

## 1. Strict Read-Only Guardrails (The _reference Directory)
- The `_reference` directory and its contents are strictly for reference purposes. 
- You MUST NOT modify, manipulate, delete, or rename any files or folders inside the `_reference` directory. All new implementation code must live in the root package structure (e.g., `src/`, `Cargo.toml`).

## 2. Dependencies and frontend boundaries
- Prefer stable dependencies already in the project and avoid unnecessary additions.
- The current Rust package exposes library modules and a CLI. How future R,
  Python and Julia frontends call the core is an explicit architecture decision;
  do not assume embedded bindings or subprocess delegation are already approved
  or implemented across all languages.
- Environment forwarding and sanitization need a documented, tested contract.
  Do not claim a clean environment, filesystem sandbox, guaranteed CRAN acceptance
  or parallel execution merely because a subprocess or hashing crate is used.

## 3. Rust Module Organization
- Keep reusable Rust behavior in library modules and command-line parsing/routing in `src/main.rs`. This organization does not decide the binding or transport used by future language frontends.
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
  - `README.md` – Project introduction and links.
  - `docs/behavior.qmd` – Confirmed behavior, proposals and implementation gaps.
  - `docs/flow.qmd` – Current implementation notes; do not treat historical flow as normative.
  - `docs/configuration.qmd` – Validation specifications for `_proj.yml` (prefixes, fallback rules, errors).
  - `docs/cli/` – Command usage manuals:
    - `docs/cli/version.qmd` – Reference documentation for `proj version get` options and JSON schemas.
    - `docs/cli/yml.qmd` – Reference documentation for evaluating configuration properties.
    - `docs/cli/path.qmd` – Reference documentation for path resolution and temporary build cache locations.
    - `docs/cli/ignore.qmd` – Reference documentation for `proj ignore` logic, protected lists, and boundaries.
    - *Add new `docs/cli` qmds as needed, and update their functional descriptions here.*
- Keep command reference pages aligned with implemented behavior and `--help`. Separately label approved target behavior, proposals and known gaps in the behavior contract. Do not rewrite approved behavior to match a defective implementation. Avoid blank pages or placeholders.

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
