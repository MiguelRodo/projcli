# Preview.6: project setup and script groups

This development preview adds project setup helpers and makes it easier to select analysis scripts and publish their reports. It retains preview.5's local/GitHub archive, CAS and experimental Xet support.

- VERSION remains the authority. Existing DESCRIPTION Version fields follow it by default, with an explicit opt-out under settings.sync-description-version. Preview and failed-build recovery preserve the documented version state.
- Project setup supports a plain README by default, optional README.Rmd and DESCRIPTION, explicit citation authors, complete code-licence templates, optional Git/remote setup and separately selected renv initialization. Existing metadata and remotes are preserved.
- The R interface adds projr_init(), projr_init_plan(), projr_init_readme(), projr_init_license() and projr_init_renv(). Optional renv installation requires consent; ordinary metadata setup does not require it.
- Named scripts directories, ordered globs and ! exclusions select supported sources. A scripts-l1 group places independent reports under docs/l1. Whole Quarto projects preserve their output tree, navigation and shared resources.
- Source discovery excludes project metadata, including nested directories and Windows case variants. Generated-source ignore rules preserve hand-authored sources and existing user rules.

## Limits

For separate documents, proj copies the rendered output and its generated `<stem>_files` folder. If embedding is explicitly disabled, arbitrary authored images, CSS and downloads outside that folder are not collected automatically. Use embedded HTML or a Quarto project when you need a complete portable output tree.

This is not complete parity with every original projr helper. Multiple artifact destinations, broader initialization conveniences and general environment reproduction remain outside this release. One project per store is unchanged. Xet remains optional and experimental; its existing integrity and whole-store transfer limitations still apply.

The CLI provides Linux/WSL and Windows x86-64 binaries. The R interface is a development source package using the separate CLI; no CRAN release is claimed. Its package development version remains 0.0.0.9000, so identify this update by source revision and exported functions.

Checks and exact source/artifact evidence are recorded in the release PR and release-delivery page. Existing public download instructions stay pinned to preview.5 until preview.6 is published and verified.
