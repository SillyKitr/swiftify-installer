# Swiftify Installer
<p align="center">
  <img src="assets/swiftify-256.png" alt="Swiftify Installer" width="128">
</p>

An unofficial, community-made **WINDOWS** installer and updater for
[Swiftify](https://github.com/swiftify-org/swiftify). This project is not
affiliated with, endorsed by, or sponsored by the Swiftify project or its
maintainers.

## what does this do?

The installer checks for compatible Swiftify installations, downloads a
matching stable Windows release from the upstream GitHub repository, and
installs or updates it in a user-selected location. It also maintains
community installer metadata and Start Menu shortcuts for installations it
manages.

The default frontend is built with Rust and egui. An optional Qt 6 / Qt Quick
frontend is available behind the `qt` Cargo feature.

## installation

When a GitHub Release is published, download its Windows x86_64 package and
run the included installer. Choose an empty destination or an existing
Swiftify installation when prompted.

This project currently targets Windows x86_64 only. It has been developed for
Windows 10 and 11; no formal minimum Windows version has been declared.
Windows on ARM and 32-bit Windows are not supported.

## Development

### Prerequisites

- Windows 10 or 11.
- A current stable Rust toolchain with the MSVC target.
- For the optional Qt frontend: a matching Qt 6 MSVC kit, including
  `qmake6.exe`.
- PowerShell for the commands below.

The default egui frontend can be built and run with:

```powershell
cargo build --release
cargo run
```

The optional Qt frontend can be built by setting `QMAKE` to the Qt kit's
`qmake6.exe` and placing its `bin` directory on `PATH`:

```powershell
$env:QMAKE = 'C:\Qt\6.10.0\msvc2022_64\bin\qmake6.exe'
$env:PATH = "C:\Qt\6.10.0\msvc2022_64\bin;$env:PATH"
cargo build --release --no-default-features --features qt --bin swiftify-installer-qt
```

The release binaries are written to `target\release\`.

Run the Rust test suite with:

```powershell
cargo test
```

Before building this checkout, resolve the existing asset-name mismatch:
the source includes `assets\installer.ico`, while the repository currently
contains `assets\swiftify.ico`. Until that is resolved, `cargo test` and the
default build fail during compilation.

## Known limitations

- Only Windows x86_64 Swiftify release archives are supported.
- The project supports one community registration at a time.
- A clean uninstallation UI is not implemented.
- Automatic update checks, desktop shortcuts, UAC elevation, and final
  installer branding are not implemented.
- Damaged metadata and unrecognized non-empty destinations are refused rather
  than deleted or guessed at.
- The installer trusts the upstream GitHub release over HTTPS. CRC32 checks
  detect accidental damage; they do not authenticate the publisher.

## Contributing

Please read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a change.
Security reports should follow [SECURITY.md](SECURITY.md).

## License and third-party notices

This project is released under the [MIT License](LICENSE). Rust dependencies
remain under their respective upstream licenses. Qt 6 licensing and any
redistributed Qt runtime files must be reviewed for the chosen distribution
model. Swiftify names, releases, and upstream assets are not owned by this
project and may have separate terms.
