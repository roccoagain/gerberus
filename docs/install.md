# Installing and updating

Gerberus is built from source. Install [Rust](https://rustup.rs) and git, then:

```sh
git clone https://github.com/roccoagain/gerberus
cd gerberus
cargo install --locked --path .
```

This builds `gerberus` and puts it in `~/.cargo/bin` (`$CARGO_HOME/bin` if you set it), which the Rust installer
adds to your `PATH`. It builds on macOS, Linux and Windows. To run it from the clone without installing, use
`cargo run --release -- <command>`, such as `cargo run --release -- check board.kicad_sch`.

The shared part library (`parts/`) and the derating policy (`policy.toml`) are built into the binary, so it needs
nothing else beside it. `gerberus datasheet verify` also needs poppler's `pdftotext` (`brew install poppler`,
`apt install poppler-utils`). Reading a tscircuit board's `.tsx` needs the tscircuit CLI, `tsci`
(`npm install -g tscircuit`, with [Node.js](https://nodejs.org)), which runs on [Bun](https://bun.sh); its `circuit.json` needs nothing.

## Updating

In the clone, run `git pull`, then the same `cargo install --locked --path .` again.
`gerberus --version` prints the version you have.
