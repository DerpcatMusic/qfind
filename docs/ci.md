# CI and local checks

The existing `conclusion` check summarizes four mandatory jobs: lint, test,
MSRV, and dependency policy. Every one must succeed; an unexpectedly skipped
job is not a passing check. PRs, merge-queue candidates, and main pushes keep
running the same coverage. CI uses Ubuntu 24.04 because GTK's selected API
requires GTK 4.12 or later.

```sh
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets --all-features
cargo doc --locked --workspace --no-deps
xvfb-run -a cargo nextest run --locked --workspace --all-features --no-tests=pass
cargo test --locked --workspace --all-features --doc
cargo deny check advisories bans licenses sources
```

Use `RUSTFLAGS='-D warnings'` and `RUSTDOCFLAGS='-D warnings'` to reproduce the
workflow's warning policy. The workflow lists native libraries to install and
installs nextest. Clippy is static analysis, nextest runs unit/integration
behavior, the separate `cargo test --doc` runs documentation examples, and deny
checks known advisories plus configured license/source/dependency rules.

The MSRV lane selects the highest member `rust-version` using numeric version
components. That is the minimum compiler which can compile the *entire*
workspace; a lexical minimum is wrong for mixed versions and for 1.100 versus
1.99. Today all members inherit the same workspace promise. If a future member
promises an older compiler, test that member separately at its own minimum too.
`--locked` ensures compiler checks use the committed dependency resolution.

Caches stay separated for lint, test and MSRV; only main writes them. Stale PRs
are cancelled, while merge-queue/main runs finish. No extra build pass, new
platform matrix, release action, ruleset, or deployment has been introduced.
Open Dependabot action-upgrade PRs are intentionally left independent.
