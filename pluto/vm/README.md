# Set up Pluto VM

Pluto VM is the optional Alpine Linux sandbox for Pluto, the assistant. Its virtual disk keeps your workspace and conversation between restarts. Connect with Pluto Desktop, the Mac app, or use the API from Terminal.

Already set up? [Update Pluto in your existing VM](#update-pluto-vm).

## Quick start

You’ll need an Apple Silicon Mac and Lima 2.3 or later. This example uses `llama-server` and a tool-capable GGUF model on the Mac. Running Pluto Desktop from source also requires Rust 1.100.0. [Lima installation](https://lima-vm.io/docs/installation/) · [Tested llama.cpp setup](../../README.md#tested-local-setup)

### 1. Install Pluto VM

From the repository root on your Mac, run:

```sh
pluto/vm/vm install
```

This builds Pluto from the current checkout in a separate Lima builder and installs it in the runtime VM, named `pluto` by default. Rerunning it updates the existing Pluto service. For more control over the build, see [Build manually from source](#build-manually-from-source).

You can also open **Settings → Connection → New VM** in Pluto Desktop on Apple Silicon. Enter a name and click **Create VM**. Keep Desktop open while it builds and provisions the VM. When the VM appears in the connection list, click **Connect** and configure its model. Lima must already be installed.

### 2. Connect a model

Start llama.cpp in its own terminal on the Mac. Replace the example GGUF path with the path to your model. Keep this terminal open. The `--alias` value is the model name Pluto will request. [llama.cpp server options](https://github.com/ggml-org/llama.cpp/blob/b12426/tools/server/README.md)

```sh
llama-server -m "/path/to/Qwen3.5-27B-Q4_K_M.gguf" \
  --jinja --ctx-size 32768 --alias qwen3.5-27b --host 127.0.0.1 --port 8080
```

In a second terminal at the repository root, check that the server lists `qwen3.5-27b`.

```sh
curl -fsS http://127.0.0.1:8080/v1/models
```

Forward port 8080 from the VM to llama.cpp on your Mac. Leave this command running in the second terminal.

```sh
pluto/vm/vm forward-model 8080
```

### 3. Select the VM in Pluto Desktop

Open the installed Pluto Desktop app, or run it from the repository root.

```sh
cargo run --locked --manifest-path pluto-desktop/Cargo.toml
```

1. Open **Settings → Connection** and click **Connect** next to **pluto**.
2. In the VM’s **Settings → Model**, choose **Local only** and **llama.cpp**. Set **Server URL** to `http://127.0.0.1:8080`, click **Load models**, and choose `qwen3.5-27b`. You can also enter the model name directly.
3. Click **Save settings**, open **Chat**, and send “Say hello in one sentence.”

Desktop remembers the selected connection. Click **Connect** next to **Standalone** to switch back, or launch with `--vm INSTANCE` or `--standalone` for a one-time override.

Use **New chat** to start a fresh conversation and **Recent chats** to reopen a saved one. A new chat is saved when its first message is sent. All chats share the VM workspace and model settings. Update both Desktop and Pluto VM to use these controls.

If you stop the VM, reopen the model tunnel before chatting. Closing Desktop ends the current turn. Changes a tool already made can persist.

## Use Pluto without Desktop

Pluto runs as a service inside the VM. Start it from Terminal and read its history to check the connection. A fresh VM returns an empty conversation.

```sh
pluto/vm/vm start
printf '{"type":"history"}\n' | pluto/vm/vm api
```

Use the same CLI to configure a model and call the API. If your model server runs on the Mac, keep the port forward open while chatting. [Configure from Terminal](#configure-from-terminal) · [VM API](#api)

## Configure a VM in Desktop

Connect to the instance you want to configure. In **Settings**, the instance name appears above its settings. Changes are saved on that VM and take effect on the next turn. New VMs start with their own settings.

| Section | What you can configure |
| --- | --- |
| **Model** | Local or OpenAI-compatible profile, server URL, model catalog, context limit, token environment variable, and hosted fallbacks. Discovery and fallback checks run inside the VM. |
| **Skills** | Add a folder or `SKILL.md` using an absolute path in the VM. **Import from Mac** copies a portable `SKILL.md` up to 32 KiB. Put scripts and other companion files inside the VM separately. |
| **MCP servers** | Add, edit, or remove local stdio commands and remote HTTP endpoints. Install local commands inside the VM and set token variables there. |
| **Tool approvals** | Choose whether calls ask for approval in this VM. |

Click **Save settings** to apply your changes on the next turn. The service reads its settings file for each turn, so saving doesn’t restart the VM. To use these controls with an older install, update both Pluto Desktop and the Pluto service.

In **Settings → VM runtime**, check the instance status, allocated vCPUs, and allocated memory. While the VM is running and Settings is open, Desktop samples Linux CPU and RAM usage about every three seconds. This shows usage inside the VM and excludes Mac processes such as a host model server. Use **Stop VM** or **Restart VM** while the VM is running, and **Start VM** when it is stopped. Use **Refresh status** to check changes made outside Desktop. Restarting or stopping can close a model tunnel to the Mac. Reopen the tunnel before chatting.

## Other model options

- **Ollama on the Mac.** Use `provider = "ollama"`, `url = "http://127.0.0.1:11434"`, and the exact name of a downloaded model, such as `qwen3:4b`. Run `pluto/vm/vm forward-model 11434`. [Ollama model page](https://ollama.com/library/qwen3:4b)
- **Hosted model.** Set `mode = "open_ai_compatible"` and configure `[models.openai]` with the API URL, model name, and `token_env` if needed. Put credential exports in the root-owned `/etc/conf.d/pluto` inside the VM with mode `0600`. As root in the VM, run `rc-service pluto restart`.

### Configure from Terminal

To configure a model without Desktop, upload a `tools.toml` file. The `configure` command replaces that file inside the VM. On an existing VM, merge these model settings into your current file first.

```sh
pluto_model_config=$(mktemp)
cat > "$pluto_model_config" <<'EOF'
[models]
mode = "local_only"

[models.local]
provider = "llama_cpp"
url = "http://127.0.0.1:8080"
name = "qwen3.5-27b"
EOF

pluto/vm/vm configure "$pluto_model_config" && rm "$pluto_model_config"
```

Use VM paths for registered skills and local MCP commands. Changes to `tools.toml` take effect on the next turn.

## Try the notes skill and MCP fixture

These examples run the repo’s test tools with your configured model. Run the commands from the repository root on your Mac. Set `PLUTO_INSTANCE` to the VM you want to use and keep its model connection available.

```sh
export PLUTO_INSTANCE=plutoVM
export LIMA_HOME="${PLUTO_LIMA_HOME:-$HOME/.pluto-lima}"
pluto/vm/vm start
limactl shell --tty=false --workdir=/ "$PLUTO_INSTANCE" sudo -n apk add --no-progress python3
```

Copy the fixtures into the VM and put the notes files at the paths the skill expects.

```sh
pluto/vm/vm import pluto/tests/test-notes/SKILL.md pluto-demo-SKILL.md
pluto/vm/vm import pluto/tests/test-notes/notes.py pluto-demo-notes.py
pluto/vm/vm import pluto/tests/fixture.py pluto-demo-mcp.py

limactl shell --tty=false --workdir=/ "$PLUTO_INSTANCE" sudo -n -u pluto-tool -- sh -ec '
  cd /var/lib/pluto-workspace
  mkdir -p tests/test-notes
  mv pluto-demo-SKILL.md tests/test-notes/SKILL.md
  mv pluto-demo-notes.py tests/test-notes/notes.py
'
```

1. In Desktop, connect to the same VM. Under **Settings → Skills → Path in VM**, enter `/var/lib/pluto-workspace/tests/test-notes/SKILL.md` and click **Add skill**.
2. Under **MCP servers**, choose **Local process (stdio)**. Enter `demo` as the server name, `python3` as the command, and the following two arguments on separate lines.

   ```text
   /var/lib/pluto-workspace/pluto-demo-mcp.py
   stdio
   ```

3. Click **Add MCP server**, then **Save settings**. Open a new chat and try the prompts below. Approve the tool calls when asked.

| Test | Prompt | Expected result |
| --- | --- | --- |
| Notes skill | Use `test-notes` to create a note titled Shopping with the body Buy milk, then search my test notes for milk. | The search finds Shopping in the sample database. |
| MCP server | Call the `lookup` tool on the `demo` MCP server and report its exact result. | `fixture lookup succeeded` |

## Update Pluto VM

Finish any active chat turn first. The update restarts the Pluto service and keeps the VM disk, workspace, conversation, and model settings. In a Git checkout with a tracking branch, this command fetches and installs the latest upstream source without changing your checkout.

```sh
pluto/vm/vm install --latest
```

To install the version in your current checkout or source archive, run `pluto/vm/vm install`. To install a Linux ARM64 binary you already built, run `pluto/vm/vm setup PATH_TO_BINARY`. Reopen Desktop after updating it from the same source version if needed. If your model tunnel is closed, start it again.

## Build manually from source

Run these commands from the repository root on your Mac. `pluto-builder` holds the build tools. Pluto VM runs the assistant and stores your workspace. Its default Lima instance is named `pluto`.

**Set up the build VM once.** This gives the compiler 3 GiB of RAM and an 8 GiB disk. [Lima start options](https://lima-vm.io/docs/reference/limactl_start/) · [Rustup](https://rustup.rs/)

```sh
export LIMA_HOME="${PLUTO_LIMA_HOME:-$HOME/.pluto-lima}"
limactl start --tty=false --name=pluto-builder --memory=3 --disk=8 pluto/vm/lima.yaml
limactl shell --tty=false --workdir=/ pluto-builder sh -ec '
  sudo -n apk add --no-cache build-base curl ca-certificates
  curl --proto "=https" --tlsv1.2 -fsSL https://sh.rustup.rs -o /tmp/pluto-rustup.sh
  sh /tmp/pluto-rustup.sh -y --profile minimal --default-toolchain 1.100.0
'
```

### Build the Linux executable

This copies Pluto’s manifest, lockfile, and source into the running build VM and saves the Linux ARM64 executable at `target/vm/pluto` on your Mac.

```sh
mkdir -p target/vm
COPYFILE_DISABLE=1 tar -C pluto -cf - Cargo.toml Cargo.lock src |
  limactl shell --tty=false --workdir=/ pluto-builder sh -ec '
    source_dir=$(mktemp -d /tmp/pluto-build.XXXXXX)
    trap "rm -rf -- \"$source_dir\"" EXIT
    cd "$source_dir"
    tar -xf -
    . "$HOME/.cargo/env"
    build_dir="$HOME/.cache/pluto-build"
    cargo +1.100.0 build --locked --release --target-dir "$build_dir"
    strip "$build_dir/release/pluto"
    cat "$build_dir/release/pluto"
  ' > target/vm/pluto.tmp &&
  mv target/vm/pluto.tmp target/vm/pluto
```

**Install the executable.** After the build succeeds, stop the builder and install the assistant in Pluto VM.

```sh
limactl stop pluto-builder
pluto/vm/vm setup target/vm/pluto
```

Use the same Lima home and runtime instance name to install a rebuilt binary into an existing VM.

## Run and move files

| Task | Command from the repository root |
| --- | --- |
| Start for file transfers or model forwarding | `pluto/vm/vm start` |
| Import a file as `sales.csv` in the VM | `pluto/vm/vm import sales.csv sales.csv` |
| Export a report Pluto wrote | `pluto/vm/vm export report.csv > report.csv` |
| Stop the VM | `pluto/vm/vm stop` |

Setup copies earlier OpenRC settings to `/etc/conf.d/pluto` if that file is absent.

## Name or rename a VM

To create a new instance called `plutoVM`, enter that name under **New VM** in Desktop, or run `pluto/vm/vm create plutoVM` from the repository root. Both refuse an existing name. Each new instance gets its own disk, chat history, and model settings.

To rename the existing `pluto` instance and keep its disk, finish any active turn and run these commands from the repository root.

```sh
export LIMA_HOME="${PLUTO_LIMA_HOME:-$HOME/.pluto-lima}"
limactl stop pluto &&
  limactl rename --tty=false pluto plutoVM &&
  export PLUTO_INSTANCE=plutoVM &&
  pluto/vm/vm start
```

Use `PLUTO_INSTANCE=plutoVM` for future CLI commands, including updates and model forwarding. In Desktop, select `plutoVM` under **Settings → Connection**, or launch with `--vm plutoVM`. [Lima rename reference](https://lima-vm.io/docs/reference/limactl_rename/)

Keep `PLUTO_LIMA_HOME` the same to use the existing storage location. Set `PLUTO_BUILDER` to reuse a differently named builder with `vm install` or `vm build`. [VM defaults](lima.yaml)

## Files and diagnostics

Each instance has its own primary disk image on the Mac, usually `~/.pluto-lima/<instance>/disk`. Lima initializes it from the Alpine image and gives it 4 GiB of capacity by default. The file is sparse, so its host storage grows as data is written. Stop, restart, and Pluto updates reuse the same disk. [Lima instance storage](https://lima-vm.io/docs/dev/internals/)

These paths are inside the VM.

| Path | Contents |
| --- | --- |
| `/var/lib/pluto/tools.toml` | Service settings |
| `/var/lib/pluto/chats/` | Saved conversations |
| `/var/lib/pluto-workspace/` | Imported files and tool output |
| `/var/lib/pluto/diagnostics.jsonl` | Model, tool, and approval events |
| `/var/lib/pluto/service.log` | OpenRC service errors |

For startup problems, run `rc-service pluto status` as root inside the VM. For turn problems, check `diagnostics.jsonl` and its rotated copy, `diagnostics.previous.jsonl`.

Pluto VM uses 2 CPUs, 512 MiB RAM, and a sparse 4 GiB disk by default. It has outbound networking. Lima disables shared mounts, automatic port forwards, and the host-loopback alias. Terminal commands and local stdio MCP servers run under a separate account with a clean environment. They can reach the explicitly forwarded model port.

## API

`pluto/vm/vm api` relays newline-delimited JSON over SSH to Pluto’s private Unix socket. Use one connection per request.

| Action | Message |
| --- | --- |
| Read the latest conversation and list saved chats | `{"type":"history"}` |
| Open a saved chat | `{"type":"open_chat","chat_id":"…"}` |
| Send to a saved chat or start a fresh one | `{"type":"send_chat","chat_id":"…","prompt":"..."}`. Use `null` for a fresh chat. |
| Read model settings | `{"type":"model_settings"}` |
| Save model settings | `{"type":"save_model_settings","previous":{…},"models":{…}}` |
| Read or save editable VM settings | `{"type":"vm_settings"}` or `{"type":"save_vm_settings","previous":{…},"settings":{…}}` |
| Load a model catalog or check fallback metadata | `{"type":"list_models","mode":"local_only","profile":{…}}` or `{"type":"check_fallback_model","model":{…}}` |
| Validate or import a skill | `{"type":"inspect_skill","path":"/path/in/vm/SKILL.md"}` or `{"type":"import_skill","text":"…"}` |
| Send to the latest conversation | `{"type":"send","prompt":"..."}` |
| Answer an `approval` or `retry` event | `{"type":"decision","id":1,"accept":true}` with the event’s ID. Set `accept` to `false` to decline. |

Keep the input stream open during a turn. The service sends history and progress events and finishes with `complete` or `error`. A history response includes the active `chat_id` when a chat exists. The `chats` list contains saved chat IDs and titles, and is included when you open a chat or start a turn. [API types](../src/api.rs)

## Verify

The smoke test replaces the model configuration and imports files. Use a disposable VM.

```sh
export PLUTO_INSTANCE=pluto-smoke
pluto/vm/vm setup target/vm/pluto
python3 -B pluto/tests/vm_smoke.py
pluto/vm/vm stop
```

The fixture checks model settings and stale saves, terminal and MCP tool accounts, approvals, and file and separate chat persistence after a cold restart. For Desktop’s approval buttons, run `cargo test --locked --manifest-path pluto-desktop/Cargo.toml vm_approval_dialog`.
