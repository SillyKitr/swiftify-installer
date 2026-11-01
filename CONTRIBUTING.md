# Contributing

Thanks for helping improve this unofficial community project.

## Development setup

Development requires Windows 10 or 11, a current stable Rust toolchain with
the MSVC target, and PowerShell. The optional Qt frontend additionally
requires a matching Qt 6 MSVC kit. See the [README](README.md) for build
commands and the current asset-name issue.

Run the test suite with:

```powershell
cargo test
```

Use `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` when
reviewing Rust changes.

## Changes and pull requests

- Keep commits focused and use short, imperative subjects such as
  `Document Qt build setup`.
- Explain the user-visible problem and the approach in the pull request.
- Include relevant test results and note any platform-specific validation.
- Keep changes limited to the requested scope; avoid unrelated refactoring.
- Update documentation when behavior, prerequisites, or supported platforms
  change.

Please do not claim affiliation with Swiftify or redistribute assets without
reviewing their applicable terms. Maintainers review changes for correctness,
clarity, platform safety, test coverage, and consistency with the project's
documented behavior.
