use anyhow::Result;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("server") => {
            let paths = dux_core::config::DuxPaths::discover()?;
            std::fs::create_dir_all(&paths.root)?;
            let addr = "127.0.0.1:8080".parse()?;
            println!("dux server listening on http://{addr} — open it in your browser");
            dux_web::run_server(paths, addr)
        }
        _ => dux_tui::run(),
    }
}
