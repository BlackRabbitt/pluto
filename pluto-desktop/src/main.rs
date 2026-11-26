mod desktop;

fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let (vm_instance, use_saved) = match args.as_slice() {
        #[cfg(unix)]
        [mode, instance] if mode == "--vm" => {
            let name = instance
                .to_str()
                .filter(|name| pluto::tools::valid_name(name) && !name.starts_with('-'))
                .ok_or_else(|| anyhow::anyhow!("Invalid Lima instance name"))?;
            (Some(name.to_owned()), false)
        }
        [] => (None, true),
        [mode] if mode == "--standalone" => (None, false),
        _ => anyhow::bail!(
            "Launch without arguments for the saved connection, use --vm INSTANCE for Pluto VM, or use --standalone."
        ),
    };
    desktop::run(vm_instance, use_saved).map_err(|error| anyhow::anyhow!("{error}"))
}
