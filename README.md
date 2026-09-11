# Hematite

Fix broken League of Legends custom skins. Drop a mod onto Hematite, let it repair what it can, then install the result with your mod manager.

![Hematite's terminal menu, with options to fix a mod, check it, or keep its original paths](docs/shots/hematite-cli.png)

[Download](https://github.com/RitoShark/Hematite/releases/latest) · [CLI commands](#cli-commands) · [Developer guide](DEVELOPER.md) · [License](#license)

## Download and use

Grab `hematite-cli.exe` from the [latest release](https://github.com/RitoShark/Hematite/releases/latest). Windows builds are ready to run, with no installer.

Drag a mod onto the executable to apply the default fixes and repath its assets. Or open Hematite to choose between fixing, checking without changes, and fixing without repathing.

Results go into `Hematite-Fixed` next to the input. For a batch folder, that directory sits inside it and keeps the same subfolder structure. Originals are left alone by default.

Hematite accepts `.fantome`, `.modpkg`, mod `.zip` archives, `.wad.client` files or folders, and individual `.bin` files. Pass a folder to process its supported files in one run. WAD output is an unpacked `.wad.client` folder.

It detects your League install automatically and uses it to recover missing files. Fixes that need game data are skipped when no install is available. The first run downloads the hash dictionary, and fix rules update from GitHub with a local fallback.

## What it fixes

- Missing health bars, white models, broken icons, and outdated material or shader references.
- Old VFX layouts and BIN fields that Riot changed from path strings to file hashes.
- DDS textures, invalid TEX dimensions, and old mesh formats.
- Missing gear, voiceover data, and asset references that can be recovered from your League install.
- Stale champion data and unused animation or incompatible audio files, while keeping files the mod still references.

Repathing moves mod assets under a separate prefix and updates their references to avoid collisions with game files. Use **Fix without repathing** when you want to keep the original paths.

## CLI commands

Open PowerShell in the folder containing `hematite-cli.exe`. Replace the example paths with your own, keeping quotes around paths that contain spaces.

| What you want to do | Command |
| --- | --- |
| Open the menu | `.\hematite-cli.exe` |
| Fix a mod with the default rules | `.\hematite-cli.exe "skin.fantome"` |
| Check a mod without changing it | `.\hematite-cli.exe "skin.fantome" --check` |
| Preview fixes without writing output | `.\hematite-cli.exe "skin.fantome" --dry-run` |
| Fix a mod and keep its asset paths | `.\hematite-cli.exe "skin.fantome" --no-repath` |
| Process a folder and its subfolders | `.\hematite-cli.exe "C:\mods" --no-pause` |
| Choose an output folder | `.\hematite-cli.exe "skin.fantome" -o "C:\fixed-mods"` |
| Point to your League install | `.\hematite-cli.exe "skin.fantome" --game-path "C:\Riot Games\League of Legends"` |
| Fix without reading your League install | `.\hematite-cli.exe "skin.fantome" --no-live` |
| Recover missing animations from the game | `.\hematite-cli.exe "skin.fantome" --restore-anm` |
| Get a check report as JSON | `.\hematite-cli.exe "skin.fantome" --check --json` |
| Show detailed logs | `.\hematite-cli.exe "skin.fantome" -v verbose` |
| Check for updates | `.\hematite-cli.exe --check-version` |
| Show every option | `.\hematite-cli.exe --help` |

The same commands work with the other supported inputs. `--no-pause` skips the exit prompt; `--json` does this automatically.

## Build and contribute

Install stable Rust and the Visual Studio C++ build tools on Windows, then:

```powershell
git clone https://github.com/RitoShark/Hematite.git
cd Hematite
cargo build --release --bin hematite-cli
```

The executable is at `target/release/hematite-cli.exe`. The engine is also available as Rust crates for use in other tools.

Fix rules live in [config/fix_config.toml](config/fix_config.toml). Start with [CONTRIBUTING.md](CONTRIBUTING.md) for PRs and [DEVELOPER.md](DEVELOPER.md) for the engine and rule format. See the [repair notes](docs/repath-and-ltk-review.md) for known gaps.

Please follow the [Code of Conduct](CODE_OF_CONDUCT.md). Report security issues as described in [SECURITY.md](SECURITY.md).

## Credits

Made by [SirDexal](https://github.com/SirDexal) and the Hematite contributors. Built on [RitoShark-Crates](https://github.com/RitoShark/RitoShark-Crates), with hashes from [CommunityDragon](https://www.communitydragon.org) and [lmdb-hashes](https://github.com/RitoShark/lmdb-hashes).

Hematite is not affiliated with Riot Games. League of Legends and its assets belong to Riot.

## License

[AGPL-3.0 with a dependency exception](LICENSE).

You can use Hematite as a dependency in an open or closed source application without publishing your application's source or changing its license. This includes static linking, dynamic linking, and calling the CLI.

Hematite itself stays under AGPL-3.0. If you distribute it, you still need to provide its source as the license requires. If you modify Hematite and distribute it or let people use that modified version over a network, those changes must be available under AGPL-3.0. Private changes do not need to be published. Third-party dependencies keep their own licenses.
