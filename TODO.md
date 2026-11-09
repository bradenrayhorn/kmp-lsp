# Fork maintenance TODO

Local improvements are currently committed on `improvements` in `81a2766`, based on upstream v0.27.0 (`cd086c8`). Keep this list updated as patches are replaced or upstreamed.

## Prepare for upstream updates

- [ ] Separate upstream submissions into focused patches: Java parsing/inference, hover, documentation recovery, indexing correctness, and dependency-manifest support. Do not rewrite shared history without coordination.
- [ ] Record each patch's regression test, upstream issue/PR, and whether upstream has superseded it.
- [ ] Move the useful LSP reproduction scripts out of `/tmp` into durable tests or documented developer tools. Use minimal fixtures rather than private application source or machine-specific paths.

## Highest-risk code

- [ ] Review the fluent-chain resolver in `src/features/hover.rs` for reuse of upstream's shared inference/navigation APIs. Avoid maintaining an independent hover type engine indefinitely.
- [ ] Preserve declaration identity through nested Java types and generic returns, rather than reducing types to ambiguous names such as `Builder`.
- [ ] Keep regressions for multiline chains, inherited generic methods, source-JAR-only dependencies, competing same-named types, instance versus companion access, and constructor/factory receivers.
- [ ] Check source-JAR return qualification against multiple same-named nested types and explicitly qualified return types when changing inference.

## Indexing and cache lifecycle

- [ ] Preserve exact manifest-selected versions and artifact variants; do not select the newest version in the shared cache.
- [ ] Verify that manifest mode excludes unrelated compiled JARs, source JARs, and default extracted sources in both LSP and CLI paths.
- [ ] Test dependency removal, shrinking/empty manifests, invalid manifests, root changes, and repeated reindexing. Removed symbols must not remain in secondary indexes.
- [ ] Check cleanup when the native sidecar is unavailable, especially when removing a previously configured manifest. The current early-return path can bypass source-JAR cleanup.
- [ ] Keep JDK/external source files and compiled-JAR state intact when pruning deselected source JARs.
- [ ] Recheck source/compiled duplicate handling and declaration precedence after upstream indexing changes.
- [ ] Bump the relevant cache versions when parser output, serialized models, or cached metadata semantics change. Test cold and warm caches.
- [ ] Keep on-demand documentation recovery off ordinary completion-list paths; avoid retaining whole dependency source trees merely to show Javadoc.

## Upstreaming order

- [ ] Submit small Java superclass/type-argument parsing and return-type extraction fixes first.
- [ ] Submit documentation recovery and source classification/deduplication fixes independently where possible.
- [ ] Propose the dependency-manifest format and replacement semantics separately from correctness fixes.
- [ ] Upstream hover improvements after reconciling them with shared resolver architecture.

## For each upstream update

- [ ] Compare upstream changes against the patch inventory before rebasing or merging. Drop superseded implementations while retaining useful regression coverage.
- [ ] Run formatting, `cargo test`, and `cargo clippy -- -D warnings`. Investigate failures rather than assuming they are unrelated.
- [ ] Exercise real LSP requests through Neovim: builder completion, `K`, completion-item Javadoc, and JDK `Timestamp` static/instance members.
- [ ] Verify the resolved dependency versions and confirm that unrelated cached libraries do not reappear.
- [ ] Measure first-use and warm completion separately under the same CPU quota and without competing builds. Include the full editor path; unit-test speed is not completion latency.
- [ ] Build a release binary, install it atomically alongside a compatible native JAR indexer, restart the LSP, and verify the executable Neovim actually launched.
- [ ] Keep a known-working binary/configuration for rollback until verification passes.

## Local deployment upkeep

- [ ] Refresh the Gradle manifest after dependency changes or deleting `build/`, then reindex/restart the LSP. Local helper: `~/.local/lib/kmp-lsp-patched/refresh-dependencies`.
- [ ] Refresh extracted platform sources when changing JDK versions. The current setup includes Java 25's `java.base` and `java.sql`, not every JDK module.
- [ ] Keep the two-CPU quota and Blink documentation settings separate from upstream server patches.
- [ ] Update `~/.local/lib/kmp-lsp-patched/README.md` when build, installation, configuration, or rollback instructions change.
