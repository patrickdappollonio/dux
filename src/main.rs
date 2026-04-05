mod app;
mod clipboard;
mod config;
mod diff;
mod editor;
mod git;
mod keybindings;
mod logger;
mod model;
mod provider;
mod pty;
mod raw_input;
mod reset;
mod statusline;
mod storage;
mod theme;

use anyhow::Result;

fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();

    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return Ok(());
    }

    if args.first().map(|s| s.as_str()) == Some("reset") {
        let options = reset::ResetOptions::from_args(&args[1..])?;
        let paths = config::DuxPaths::discover()?;
        return reset::run(&paths, options);
    }

    let mut app = app::App::bootstrap()?;
    app.run()
}

fn print_help() {
    println!(
        "dux\n\n\
         Terminal UI for AI worktree sessions.\n\n\
         Usage:\n\
          dux              Launch the TUI\n\
          dux reset        Remove config and logs, keep saved agents and worktrees\n\
          dux reset --delete-agent-data\n\
                            Same as reset, and also remove sessions.sqlite3 and worktrees\n\n\
         Environment variables:\n\
           DUX_HOME    Override the config directory (must be an absolute path).\n\
                       When unset, defaults to:\n\
                         macOS: ~/.dux/\n\
                         Linux: $XDG_CONFIG_HOME/dux/ or ~/.config/dux/\n\n\
         First run writes a full default config to:\n\
           macOS: ~/.dux/config.toml\n\
           Linux: $XDG_CONFIG_HOME/dux/config.toml or ~/.config/dux/config.toml\n\
         Session state is stored in:\n\
           macOS: ~/.dux/sessions.sqlite3\n\
           Linux: $XDG_CONFIG_HOME/dux/sessions.sqlite3 or ~/.config/dux/sessions.sqlite3"
    );
}
