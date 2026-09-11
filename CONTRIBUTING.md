# Contributing

Bug reports and PRs are welcome. For a bug, include your Hematite version, the input format, the command or menu option you used, and the relevant log output. Say what you expected and what happened. Keep private paths and personal information out of logs.

For a larger change, open an issue first so we can agree on the approach.

## Working on a fix

Follow the build steps in the [README](README.md#build-and-contribute). The [developer guide](DEVELOPER.md) covers the engine and transform framework.

- Prefer a rule in `config/fix_config.toml` when it can express the fix. The top-level `enabled_fixes` list controls the defaults.
- Keep changes focused and match the surrounding code.
- Add a regression test for a behaviour change. Use synthetic fixtures, and do not commit game assets.
- File-format fixes belong in [RitoShark-Crates](https://github.com/RitoShark/RitoShark-Crates).

Before submitting Rust changes, run the same checks as CI:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace -- -D warnings -A clippy::needless_return
cargo test --workspace
```

Use scoped Conventional Commits, such as `fix(core): preserve referenced animations` or `doc(readme): clarify batch output`. The changelog is built from commit messages. Explain the change and how you checked it in the PR.

Contributions are provided under Hematite's [AGPL-3.0 license and dependency exception](LICENSE). Please follow the [Code of Conduct](CODE_OF_CONDUCT.md).
