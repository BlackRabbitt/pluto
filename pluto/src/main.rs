#[cfg(unix)]
mod vm;

fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    match args.as_slice() {
        #[cfg(unix)]
        [mode, state, socket] if mode == "--vm-service" => vm::serve(state.into(), socket.into()),
        #[cfg(unix)]
        [mode, socket] if mode == "--vm-connect" => vm::connect(socket.into()),
        #[cfg(unix)]
        [mode, executable, args @ ..] if mode == "--vm-run-tool" => vm::run_tool(executable, args),
        _ => anyhow::bail!("Use --vm-service STATE_DIR SOCKET or --vm-connect SOCKET"),
    }
}
