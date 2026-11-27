# Pluto

<img src="pluto-desktop/assets/pluto-icon.png" alt="Pluto icon" width="112" height="112">

**Pluto** is a free, local-first assistant for everyday work with open models. Use it to draft a message, summarize notes, plan a project, or work on code. **Pluto Desktop** is the Mac app for chat and tool approvals.

## Get started

1. [Install Pluto Desktop on macOS](docs/installation.md#run-the-macos-installer-script) or [build and run from source](docs/installation.md#build-and-run-with-cargo).
2. Choose where Pluto runs.

   | Mode | Where tools run and files are kept |
   | --- | --- |
   | **Standalone** | On your computer. Terminal commands and local MCP servers use your account’s permissions. |
   | **[Pluto VM](docs/installation.md#set-up-pluto-vm)** | In a Linux VM with its own persistent disk. The VM setup uses Lima on Apple Silicon Macs. |

3. [Connect a model](docs/models.md) and send a message in **Chat**.

## Find a guide

| I want to… | Guide |
| --- | --- |
| Install, update, or launch Pluto Desktop | [Installation](docs/installation.md) |
| Connect llama.cpp, Ollama, or a hosted model | [Models and fallbacks](docs/models.md) |
| Reopen chats, change thinking, or check context usage | [Chat](docs/chat.md) |
| Add skills or MCP servers and manage approvals | [Skills and tools](docs/tools.md) |
| Find settings, chats, or diagnostic logs | [Troubleshooting](docs/troubleshooting.md) |
| Set up or operate the Linux guest | [Pluto VM guide](pluto/vm/README.md) |
| Transfer files or folders between your Mac and a VM | [VM import and export](pluto/vm/README.md#run-and-move-files) |
| Open a VM shell or set environment variables for models and tools | [VM environment variables](pluto/vm/README.md#environment-variables-in-pluto-vm) |
| Find source files, symbols, and focused tests | [Development guide](DEVELOPMENT.md) |
| Understand the sandbox design and benchmarks | [Sandboxing research](research/SANDBOXING.md) |

## Contribute

If you’ve tried a model or found a bug, include the model, runtime, OS, and steps to reproduce it in a [GitHub issue](https://github.com/BlackRabbitt/pluto/issues). For code changes, [start with the development guide](DEVELOPMENT.md#start-a-task).
