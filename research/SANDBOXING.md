# How Pluto’s sandbox evolved

The original question was how Pluto could run a command without giving it access to the Mac. The first design put individual commands in a disposable VM. **Pluto VM** now runs the assistant and its tools together, so the workspace and conversation survive a restart. **Pluto Desktop** is the Mac interface for both modes.

## Chosen sandbox design

```mermaid
flowchart TB
    subgraph Host["Mac host"]
        Desktop["Pluto Desktop"]
        Local["Standalone: Pluto library and tools"]
        Model["Optional local model server"]
        Lima["Lima: create, start, stop, restart"]
        Desktop -->|Standalone mode| Local
        Desktop -->|VM controls| Lima
    end
    subgraph VM["Pluto VM · Alpine Linux · one disk per instance"]
        Service["Pluto service: API, harness, and model routing"]
        Tools["Terminal and local MCP: separate tool account"]
        Disk[("Persistent disk: settings, skills, chats, workspace")]
        Service <-->|Approved calls and results| Tools
        Service -->|Saves settings and chats| Disk
        Tools -->|Reads and writes workspace| Disk
    end
    Desktop <-->|SSH to the private API: chat, settings, approvals| Service
    Lima -->|Manages VM lifecycle| VM
    Service <-->|Explicit SSH model tunnel| Model
    Service <-->|Configured outbound requests| Cloud["Hosted models and remote MCP"]
```

| Mode | Where Pluto runs | Where terminal commands and local MCP servers run |
| --- | --- | --- |
| Standalone | In the Pluto Desktop process on your Mac | On the Mac with your account’s permissions |
| Pluto VM | In the Pluto service inside a Linux VM | Inside the VM under a separate tool account |

## 1. Why the harness and tools run together

The first design proposed a small, disposable VM for each command. Pluto could import a CSV, run Python with networking off, and return a report. This would isolate the command while leaving chat, tool orchestration, and conversation history on the Mac.

The **harness** routes model requests, presents tool calls for approval, and feeds results back into the conversation. Moving it into the VM gives Desktop and future clients one place to send prompts and approvals. It also lets Pluto reopen a conversation together with the files its tools created.

| Design option | Benefit | Cost |
| --- | --- | --- |
| Keep the harness on the Mac and isolate commands in a VM | Smaller change to Desktop. Chat can start before the VM boots. | The harness and chat history stay on the Mac. Files and tool results must cross the boundary as needed. |
| **Run the harness and tools in Pluto VM** | Workspace and conversation survive together. A client can connect over the API. | VM mode waits for the VM to boot. Prompts, progress, and approvals need a service API. |

Standalone mode preserves the existing Mac workflow. The repository has two Rust projects with separate lockfiles. `pluto` owns the harness, tools, model routing, and headless service. `pluto-desktop` owns the Mac interface and uses the Pluto library directly in standalone mode. Both modes run the same harness code and keep separate local state. [Package boundaries](../DEVELOPMENT.md#package-boundaries)

## 2. What crosses the VM boundary

- **Approvals** control whether a tool call runs. Terminal commands and local MCP servers run under a separate Linux account with a clean environment. That account can write to the workspace and has restricted access to the service’s settings and state. [Account and launcher setup](../pluto/vm/setup-vm.sh)
- **Files and network.** Lima plain mode disables shared host mounts and automatic port forwarding. The VM can make outbound requests and reach a model port you explicitly forward. [VM defaults](../pluto/vm/lima.yaml)
- **Inference.** A local model server can use the Mac’s GPU through the forwarded port. Pluto can also use a configured hosted provider. Model discovery and fallback checks originate inside the VM and use its credentials. The VM ships without model weights. [Model setup](../pluto/vm/README.md#2-connect-a-model)
- **Settings.** Desktop uses the same settings tabs in both modes. The VM writes its own settings to its disk and reads them for each new turn. Desktop separately uses Lima for lifecycle controls and reads live guest CPU and RAM over SSH. [VM settings and runtime](../pluto/vm/README.md#configure-a-vm-in-desktop)

## 3. Why Lima runs the build and runtime VMs

The prototype uses **Lima with Apple Virtualization and Alpine ARM64** to test the service, approvals, and persistence on Apple Silicon. Lima handles image provisioning, startup, the virtual disk, and SSH access, so the assistant could be tested before investing in a dedicated Mac launcher.

| Option | Rationale |
| --- | --- |
| **Lima with Apple Virtualization** | It gave the prototype a ready-to-use VM lifecycle and SSH connection. Users need a separate Lima installation. |
| Direct Apple Virtualization launcher | Would give Desktop control over setup, startup, and recovery. It would also require image management, networking, a control channel, and signing. Any startup improvement needs measurement. |
| QEMU or a KVM-based launcher | Candidates for Linux or broader host support. Each would need its own provisioning and verification. |

The early Docker build was replaced with a separate **Lima builder VM**. It compiles Pluto natively on Alpine ARM64 with **3 GiB RAM** and an **8 GiB disk**. Each **runtime VM** gets its own disk. The installer replaces Pluto’s executable in place, so upgrades keep that disk. Desktop’s **New VM** uses the same builder and provisioning steps. [Install Pluto VM](../pluto/vm/README.md#1-install-pluto-vm)

Two ideas from Abhishek Bhardwaj’s *From Fork to Fleet* informed the prototype. A VM provides a separate kernel, and a durable disk keeps work after shutdown. One local VM was enough to test both. Checkpointing and fleet scheduling solve different problems and stayed outside the prototype. [VM isolation](https://www.youtube.com/watch?v=OqM67QG_Ikk&t=1109s) · [Durable disks](https://www.youtube.com/watch?v=OqM67QG_Ikk&t=1804s)

## 4. What the CSV smoke test verified

An OpenAI-compatible fixture drove terminal and local MCP calls. An approved command turned an imported CSV into a report. A declined command left no file. After a cold restart, the test reopened the chat, exported the report, and checked that the next model request included the saved conversation. The follow-up test also checked that model settings survived restart and update. [VM smoke test](../pluto/tests/vm_smoke.py)

```mermaid
flowchart LR
    A["1. Import a CSV"] --> B["2. Approve the report command"]
    B --> C["3. Write the report in Pluto VM"]
    C --> D["4. Stop and restart the VM"]
    D --> E["5. Reopen chat and export report"]
```

| Apple Silicon measurement | Result | Scope |
| --- | ---: | --- |
| Cold start to first Pluto API response, initial prototype | **About 21 s** | Boot from stopped. The API answered before any model inference. |
| Cold restart to first Pluto API response, tool-launcher check | **22.10 s** | Individual run after updating the tool launcher. |
| Cold restart to first Pluto API response, installer check | **19.09 s** | Individual run after adding one-command install and VM model settings. |
| Cold restart to first Pluto API response, settings and catalog check | **22.04 s** | Individual run after testing separate chats, per-VM settings, and model discovery. |
| Alpine VM download | **183 MiB** | Initial prototype download. Installed disk use and caches vary. |
| Headless Pluto executable | **6.5 MiB** | Initial prototype Linux ARM64 binary. |
| Pluto service peak resident memory | **4.9 MiB** | Short follow-up inside the VM. This measures the service process only. |
| Original Pluto Desktop executable | **18.6 MiB** | Mac release binary before the package split. |
| Original Pluto Desktop peak resident memory | **56 MiB** | A 10-second Mac launch with fresh settings and no model connected. |
| Sparse runtime disk on the Mac | **246 MiB used** | One disposable VM with a 4 GiB virtual disk. Host usage varies with data written. |

**Runtime defaults** are **2 vCPUs, 512 MiB RAM, and a 4 GiB sparse disk**. The boot timings end at the first API response. Total host memory, disk growth over time, and time to the first reply with a live model remain open measurements. [Desktop benchmark](../README.md#desktop-benchmark) · [Runtime defaults](../pluto/vm/lima.yaml)

## 5. Proposed host integrations

Two ideas from the research remain proposals:

1. **Mac-only tools.** A Mac-side MCP service could expose one operation such as “summarize up to 10 unread emails from today.” The Mac would keep the existing login and validate the request. Pluto would ask for approval before calling the tool. The returned content would enter the selected model’s context, including when that model is hosted.
2. **Memory indexing.** The proposed design keeps the index and small jobs inside Pluto VM. If profiling shows embedding batches need acceleration, Pluto could send selected text to a local Mac service and store the returned vectors on the VM’s disk. This would allow testing Mac GPU acceleration without changing the VM launcher.

End-to-end response time with a live model, total host memory during a tool-heavy turn, and disk growth after repeated work can inform whether to keep Lima as the user-facing launcher and how much work to move to the host.
