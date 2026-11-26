# Connect a model

[Home](../README.md) · [Installation](installation.md)

- [Configure a model in Desktop](#configure-a-model-in-desktop)
- [Tested llama.cpp setup](#tested-local-setup)
- [Cloud models and fallbacks](#find-free-cloud-models-and-configure-fallbacks)
- [Connect over your local network](#connect-to-a-model-on-your-local-network-from-macos)

## Configure a model in Desktop

Both modes use the same **Model**, **Skills**, **MCP servers**, and **Tool approvals** tabs. The heading shows which environment you’re editing. The steps below are for standalone mode. [VM model setup](../pluto/vm/README.md#2-connect-a-model)

1. Open **Settings** and configure **Local model**, **OpenAI-compatible model**, or both. Enter the server URL, then click **Load models** or enter the model ID.
2. Choose **Local only** to use llama.cpp or Ollama. **OpenAI-compatible** tries the primary model, enabled backups, and then the saved local model if configured.
3. Set the **Terminal working directory** if you plan to run commands. Click **Save settings** and open **Chat**.

| Provider | Server URL |
| --- | --- |
| **llama.cpp** | The server base URL, for example `http://localhost:8080` |
| **Ollama** | The server base URL, for example `http://localhost:11434` |
| **OpenAI-compatible** | The API base URL for a server offering a free, open model, including its required path such as `/v1` |

For an authenticated model or MCP server, export the key before launching Pluto Desktop. In Settings, enter the **variable name**, such as `MODEL_API_KEY`. If Pluto can load the model list but chat returns a 404, check that your model ID supports text chat on that endpoint.

### Tested local setup

Pluto has been tested with **Qwen3.5-27B Q4_K_M** on llama.cpp build **b12426**, with Jinja templates enabled and a **32,768-token** server context. The tested GGUF is about **16.7 GB**. Qwen publishes the base model under Apache 2.0. [Qwen model card](https://huggingface.co/Qwen/Qwen3.5-27B) · [Unsloth GGUF](https://huggingface.co/unsloth/Qwen3.5-27B-GGUF/blob/3221f178a6b842d04f1fb42f1c413534adcc0a6a/Qwen3.5-27B-Q4_K_M.gguf)

| Pluto setting | Tested value |
| --- | --- |
| **Runtime** | llama.cpp |
| **Server URL** | `http://localhost:8080` on the machine running llama.cpp |
| **Model name** | `qwen3.5-27b` when launched with `--alias qwen3.5-27b` |
| **Token environment variable** | Blank for an unauthenticated local server |

With the GGUF already downloaded, run a same-machine server with:

```sh
llama-server -m /path/to/Qwen3.5-27B-Q4_K_M.gguf --jinja --ctx-size 32768 \
  --alias qwen3.5-27b --host 127.0.0.1 --port 8080
```

### Find free cloud models and configure fallbacks

The [FreeLLM registry](https://freellm.net/models/) lists free tiers across providers. Pick a free, openly licensed model and copy its provider's API base URL and exact model ID. Provider quotas and available models can change.

Under **OpenAI-compatible model**, enable **Try other free models automatically** to add up to four backups in preference order. Use **Check model metadata** to see the provider's reported context limit and tool support. Supply verified values when the provider omits them. Confirm each backup is free and openly licensed, then save.

Pluto honors `Retry-After` when an inference server returns HTTP 429. It checks estimated context fit and tool support before picking a cloud backup. Each switched model receives the current chat and tool results so Pluto can continue without repeating completed tools. Chat shows the selected destination.

### Connect to a model on your local network from macOS

Open the installed **Pluto Desktop** app from Spotlight. In Settings, enter the model server’s LAN address and port, then click **Load models**. Use `localhost` when the model server runs on the same Mac. For a server on another device, make sure it listens on its LAN interface and accepts connections on that port.

If macOS asks to allow local network access, choose **Allow**. macOS lists Pluto Desktop under **System Settings → Privacy & Security → Local Network** after it attempts a connection that requires permission. If Pluto Desktop is already listed, make sure its switch is on. [Apple’s local network privacy guide](https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy)

Source installs use ad hoc signing by default, so macOS can ask again after a rebuild. If you have an Apple-issued code-signing identity in your keychain, set `PLUTO_SIGNING_IDENTITY` to its name when running the installer. To compare whether the issue is specific to the app’s local network permission, try the same server from `~/.cargo/bin/pluto-desktop` in Terminal. macOS grants local network access to command-line tools launched from Terminal. [Apple’s macOS signing and local network guidance](https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy#Build-time-considerations)
