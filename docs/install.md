# Installing and updating

On macOS or Linux:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/roccoagain/gerberus/releases/latest/download/gerberus-installer.sh | sh
```

On Windows, in PowerShell:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/roccoagain/gerberus/releases/latest/download/gerberus-installer.ps1 | iex"
```

The installer downloads the release built for your machine (macOS on Apple silicon or Intel, Linux on x86-64 or
ARM64 with glibc 2.35 or newer, Windows on x86-64) and puts `gerberus` and `gerberus-update` in `~/.cargo/bin`
(`$CARGO_HOME/bin` if you set it), adding that folder to your `PATH` if it isn't there. The [releases
page](https://github.com/roccoagain/gerberus/releases) also has each build as a plain archive, with checksums. To
build it yourself instead, with Rust installed:

```sh
cargo install --locked --git https://github.com/roccoagain/gerberus
```

The shared part library (`parts/`) and the derating policy (`policy.toml`) are built into the binary, so it needs
nothing else beside it. `gerberus datasheet verify` also needs poppler's `pdftotext` (`brew install poppler`,
`apt install poppler-utils`).

## Updating

```sh
gerberus update
```

fetches the latest release and installs it over the old one (on Windows, the running `gerberus.exe` is renamed
`gerberus.exe.old` to make room, and the next update removes it). It runs `gerberus-update`, which the installer put
next to `gerberus` and which reads the install receipt the installer left in your config folder, so it updates the
copy you installed, where you installed it. A copy built with `cargo install` has no updater: run the same
`cargo install` command again.

Once a day, `gerberus` also asks GitHub for the latest release and, after a command finishes, prints a line on
stderr when a newer one is out. It waits at most a second for GitHub, and only on the first run of the day; the
answer is kept in `~/.cache/gerberus/latest-release` (`%LOCALAPPDATA%\gerberus` on Windows). It doesn't check when
its output goes to a pipe or a file, or when `CI` is set. Set `GERBERUS_NO_UPDATE_CHECK=1` to turn it off.
`gerberus --version` prints the version you have.
