# Install and run Pluto

[Home](../README.md) · [Connect a model](models.md)

| What you want to do | Where to start |
| --- | --- |
| Install the Mac app | [Run the macOS installer script](#run-the-macos-installer-script) |
| Run directly from a source checkout | [Build and run with Cargo](#build-and-run-with-cargo) |
| Run the harness and its tools in a Linux guest | [Set up Pluto VM](#set-up-pluto-vm) |

## Run the macOS installer script

The script builds from a Git checkout or a source archive and installs the Desktop launcher and Terminal executable.

1. Install the Xcode Command Line Tools with `xcode-select --install`. Install Rustup if needed, then open a new Terminal window. [Apple Command Line Tools](https://developer.apple.com/documentation/xcode/installing-the-command-line-tools) · [Rustup](https://rustup.rs/)
2. Clone the repository or download and unzip the source for the branch or release you want. In Terminal, open the folder containing `pluto/` and `pluto-desktop/`. [Pluto on GitHub](https://github.com/BlackRabbitt/pluto)
3. Run the installer.

   ```sh
   sh pluto-desktop/install-macos.sh
   ```

Open **Pluto Desktop** from Spotlight or Applications. To inherit your shell’s API keys and command search path, launch `~/.cargo/bin/pluto-desktop` from Terminal. [MCP environment variables](tools.md#mcp-environment-variables-on-macos)

If you’re switching from a VM, open **Settings → Connection** and click **Connect** next to **Standalone**. Then [connect a model](models.md).

### Update the Mac app

Quit Pluto Desktop and run the same installer from the updated checkout or a newly unpacked source archive. It replaces the app and Terminal executable even when the version number is unchanged. Your settings and chats stay in place. [Local files](troubleshooting.md#local-files)

## Build and run with Cargo

Use Rust 1.100.0 and keep both `pluto/` and `pluto-desktop/` in the checkout. On Linux, Desktop needs a graphical desktop and OpenGL drivers. From the repository root, run:

```sh
cargo build --locked --manifest-path pluto-desktop/Cargo.toml
cargo run --locked --manifest-path pluto-desktop/Cargo.toml -- --standalone
```

For toolchain setup, package builds, and tests, see the [development guide](../DEVELOPMENT.md#package-boundaries).

## Set up Pluto VM

On an Apple Silicon Mac with Lima 2.3 or later, open **Settings → Connection → New VM** in Pluto Desktop. Enter an instance name, then click **Create VM**. When it appears in the list, click **Connect** and open the VM’s **Settings → Model**. Desktop remembers your connection and starts the VM when needed. [VM model setup](../pluto/vm/README.md#2-connect-a-model)

To install the default VM from Terminal, run this from the repository root:

```sh
pluto/vm/vm install
```

Run the same install command to apply the source in your checkout. Use `pluto/vm/vm install --latest` from a Git checkout with an upstream to install that branch’s latest source. [VM updates](../pluto/vm/README.md#update-pluto-vm) · [Manual builds](../pluto/vm/README.md#build-manually-from-source) · [VM API](../pluto/vm/README.md#api)

You can also [use Pluto from Terminal](../pluto/vm/README.md#use-pluto-without-desktop) or [rename an existing instance to `plutoVM`](../pluto/vm/README.md#name-or-rename-a-vm).

Once connected, **Settings → VM runtime** shows the VM’s status, allocated resources, and live CPU and RAM usage. Start, stop, or restart that VM from the same panel. [VM settings and runtime](../pluto/vm/README.md#configure-a-vm-in-desktop)
