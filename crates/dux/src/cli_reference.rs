//! The command-line reference page of the docs site, rendered from the
//! command tree's own definitions, and the test that keeps the committed page
//! equal to it.

use std::fmt::Write as _;

use clap::{Arg, Command};

use crate::commands::{ENVIRONMENT, exit_codes};

/// The page for the command tree `root`, with `config` standing in for its
/// `config` command, which clap never parses.
pub(crate) fn render(mut root: Command, mut config: Command) -> String {
    root.build();
    config.build();
    let mut page = String::from(
        "---
title: Command line reference
description: Every dux command, subcommand, argument and flag, with the same help text --help prints.
group: Reference
order: 90
editSource: crates/dux/src/commands.rs
---

<!-- Generated from the dux command line's own definitions; do not edit by hand. The help of the `dux config` subcommands is defined in crates/dux-tui/src/config_spec.rs. -->

Every command dux has, with the same text `dux <command> --help` prints. For how the
commands fit together, and examples, see [The command line](/docs/command-line).
",
    );
    common_flags(&mut page, &root);
    environment(&mut page);
    exit_code_table(&mut page);
    let name = root.get_name().to_string();
    section(&mut page, &root, &name, 2, Some(&config));
    page
}

fn common_flags(page: &mut String, root: &Command) {
    page.push_str(
        "\n## Flags every command takes\n\n| Flag | Value | Description |\n|---|---|---|\n",
    );
    for arg in root
        .get_arguments()
        .filter(|arg| arg.is_global_set() && !arg.is_hide_set())
    {
        option_row(page, arg);
    }
    page.push_str("| `-h`, `--help` | | Print the help for the command. |\n");
    page.push_str("| `-V`, `--version` | | Print the version; `dux` alone takes it. |\n");
    page.push_str(
        "\n`dux config` is the exception: it reads `--local` only before the word `config`, and \
         it refuses `--remote` (and any selected remote) unless `--local` is given, because it \
         always edits this machine's file.\n",
    );
}

fn environment(page: &mut String) {
    page.push_str("\n## Environment variables\n\n| Variable | Description |\n|---|---|\n");
    for var in ENVIRONMENT {
        let mut description = String::new();
        for line in var.lines {
            if !description.is_empty() {
                description.push_str(if line.starts_with(' ') { "<br>" } else { " " });
            }
            description.push_str(&cell(line.trim_start()));
        }
        let _ = writeln!(page, "| `{}` | {description} |", var.name);
    }
}

fn exit_code_table(page: &mut String) {
    page.push_str("\n## Exit codes\n\n| Code | Meaning |\n|---|---|\n");
    for (code, meaning) in exit_codes() {
        let _ = writeln!(page, "| `{code}` | {} |", cell(meaning));
    }
}

/// Whether clap made `command` itself: the `help` subcommand.
fn is_generated(command: &Command) -> bool {
    command.get_name() == "help"
}

/// Whether clap made `arg` itself: `--help` and `--version`.
fn is_generated_arg(arg: &Arg) -> bool {
    matches!(arg.get_id().as_str(), "help" | "version")
}

fn shown_subcommands(command: &Command) -> impl Iterator<Item = &Command> {
    command
        .get_subcommands()
        .filter(|sub| !sub.is_hide_set() && !is_generated(sub))
}

fn shown_arguments(command: &Command) -> impl Iterator<Item = &Arg> {
    command
        .get_arguments()
        .filter(|arg| !arg.is_hide_set() && !arg.is_global_set() && !is_generated_arg(arg))
}

/// The anchor `rehype-slug` gives a heading reading `path` in code.
fn anchor(path: &str) -> String {
    path.replace(' ', "-")
}

fn section(
    page: &mut String,
    command: &Command,
    path: &str,
    level: usize,
    config: Option<&Command>,
) {
    let _ = writeln!(page, "\n{} `{path}`\n", "#".repeat(level));
    let aliases: Vec<String> = command
        .get_visible_aliases()
        .map(|alias| format!("`{alias}`"))
        .collect();
    if !aliases.is_empty() {
        let _ = writeln!(page, "Also: {}\n", aliases.join(", "));
    }
    let about = command
        .get_long_about()
        .or_else(|| command.get_about())
        .map(ToString::to_string);
    let before = command
        .get_before_long_help()
        .or_else(|| command.get_before_help())
        .map(ToString::to_string);
    for text in [about, before].into_iter().flatten() {
        let paragraphs: Vec<String> = text
            .split("\n\n")
            .map(|paragraph| sentence(prose(paragraph)))
            .collect();
        let _ = writeln!(page, "{}\n", paragraphs.join("\n\n"));
    }
    let usage = command.clone().render_usage().to_string();
    let usage: Vec<&str> = usage
        .lines()
        .map(|line| line.strip_prefix("Usage: ").unwrap_or(line).trim_start())
        .collect();
    let _ = writeln!(page, "```text\n{}\n```", usage.join("\n"));
    let positionals: Vec<&Arg> = shown_arguments(command)
        .filter(|arg| arg.is_positional())
        .collect();
    if !positionals.is_empty() {
        page.push_str("\n| Argument | Description |\n|---|---|\n");
        for arg in positionals {
            argument_row(page, arg);
        }
    }
    let options: Vec<&Arg> = shown_arguments(command)
        .filter(|arg| !arg.is_positional())
        .collect();
    if !options.is_empty() {
        page.push_str("\n| Flag | Value | Description |\n|---|---|---|\n");
        for arg in options {
            option_row(page, arg);
        }
    }
    // The bullet shows what this command's own `--help` lists; the section
    // below it is the grafted `config` command's.
    let subcommands: Vec<(&Command, &Command, String)> = shown_subcommands(command)
        .map(|listed| {
            let described = match config {
                Some(config) if listed.get_name() == config.get_name() => config,
                _ => listed,
            };
            (listed, described, format!("{path} {}", listed.get_name()))
        })
        .collect();
    if !subcommands.is_empty() {
        page.push('\n');
        for (listed, _, sub_path) in &subcommands {
            let about = listed
                .get_about()
                .map(ToString::to_string)
                .unwrap_or_default();
            let _ = writeln!(
                page,
                "- [`{sub_path}`](#{}): {}",
                anchor(sub_path),
                sentence(cell(&about))
            );
        }
    }
    for (_, sub, sub_path) in &subcommands {
        // The root and its nouns are h2, everything under a noun h3: the
        // docs table of contents lists those two levels.
        let sub_level = if path.contains(' ') { 3 } else { 2 };
        section(page, sub, sub_path, sub_level, None);
    }
}

fn value_name(arg: &Arg) -> String {
    arg.get_value_names().map_or_else(
        || format!("<{}>", arg.get_id().as_str()),
        |names| {
            names
                .iter()
                .map(|name| format!("<{name}>"))
                .collect::<Vec<_>>()
                .join(" ")
        },
    )
}

fn help_of(arg: &Arg) -> String {
    arg.get_long_help()
        .or_else(|| arg.get_help())
        .map(ToString::to_string)
        .unwrap_or_default()
}

/// An argument's help with the sentences about its values after it.
fn described(arg: &Arg) -> String {
    format!("{}{}", sentence(cell(&help_of(arg))), values(arg))
}

/// clap drops the period that ends a one-line doc comment; the page puts it
/// back so every description reads as a sentence.
fn sentence(text: String) -> String {
    if text.is_empty() || text.ends_with(['.', '!', '?', ':']) {
        text
    } else {
        text + "."
    }
}

fn argument_row(page: &mut String, arg: &Arg) {
    let need = if arg.is_required_set() {
        "Required."
    } else {
        "Optional."
    };
    let _ = writeln!(
        page,
        "| `{}` | {need} {} |",
        value_name(arg),
        described(arg)
    );
}

fn option_row(page: &mut String, arg: &Arg) {
    let mut flag = Vec::new();
    if let Some(short) = arg.get_short() {
        flag.push(format!("`-{short}`"));
    }
    if let Some(long) = arg.get_long() {
        flag.push(format!("`--{long}`"));
    }
    let value = if arg.get_action().takes_values() {
        format!("`{}`", value_name(arg))
    } else {
        String::new()
    };
    let _ = writeln!(
        page,
        "| {} | {value} | {} |",
        flag.join(", "),
        described(arg)
    );
}

/// The sentences naming the values an argument takes and its default.
fn values(arg: &Arg) -> String {
    if !arg.get_action().takes_values() {
        return String::new();
    }
    let mut out = String::new();
    let possible: Vec<String> = arg
        .get_possible_values()
        .iter()
        .filter(|value| !value.is_hide_set())
        .map(|value| format!("`{}`", value.get_name()))
        .collect();
    if !possible.is_empty() {
        let _ = write!(out, " One of {}.", possible.join(", "));
    }
    let defaults: Vec<String> = arg
        .get_default_values()
        .iter()
        .map(|value| format!("`{}`", value.to_string_lossy()))
        .collect();
    if !defaults.is_empty() {
        let _ = write!(out, " Default: {}.", defaults.join(", "));
    }
    out
}

/// `text` as a Markdown paragraph: Markdown's own characters escaped outside
/// code spans, so a bare `<AGENT>` is not read as an HTML tag.
fn prose(text: &str) -> String {
    escape(text, Place::Paragraph)
}

/// `text` as one table cell: [`prose`], with a pipe escaped inside code spans
/// too (a table splits on it there) and paragraphs kept apart by line breaks.
fn cell(text: &str) -> String {
    escape(text, Place::TableCell)
        .split("\n\n")
        .map(|paragraph| paragraph.replace('\n', " "))
        .collect::<Vec<_>>()
        .join("<br><br>")
}

/// Where escaped text goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    Paragraph,
    TableCell,
}

/// A flag named in prose (`--stdin`) becomes a code span, or the site's
/// typography would turn its two dashes into a dash.
fn escape(text: &str, place: Place) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_code = false;
    let mut in_flag = false;
    let mut previous = ' ';
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if in_flag && !(character.is_ascii_alphanumeric() || character == '-') {
            out.push('`');
            in_flag = false;
        }
        let starts_flag = !in_code
            && !in_flag
            && character == '-'
            && matches!(previous, ' ' | '(' | '\n')
            && characters.peek() == Some(&'-')
            && {
                let mut ahead = characters.clone();
                ahead.next();
                ahead.next().is_some_and(|next| next.is_ascii_alphabetic())
            };
        if starts_flag {
            out.push('`');
            in_flag = true;
        }
        match character {
            '`' => in_code = !in_code,
            '|' if in_code && place == Place::TableCell => out.push('\\'),
            '\\' | '|' | '<' | '>' | '*' | '~' if !in_code && !in_flag => out.push('\\'),
            _ => {}
        }
        out.push(character);
        previous = character;
    }
    if in_flag {
        out.push('`');
    }
    out
}

/// What the golden test does when the committed page differs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OnMismatch {
    /// Fail, saying how to regenerate the page.
    Fail,
    /// Write the rendered page over the committed one.
    Regenerate,
}

const REGENERATE: &str = "website/docs/cli-reference.md no longer matches the command line's \
     definitions. Regenerate it with `DUX_REGENERATE_CLI_REFERENCE=1 cargo test -p dux \
     cli_reference`, then commit the file.";

/// Where the golden test runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Runner {
    /// CI, which only ever checks the page, so a regenerate variable left in
    /// a workflow cannot turn the check into a rewrite.
    Ci,
    /// Anywhere else.
    Local,
}

/// Hold the page at `path` to `rendered`.
fn reconcile(
    path: &std::path::Path,
    rendered: &str,
    on_mismatch: OnMismatch,
    runner: Runner,
) -> Result<(), String> {
    if on_mismatch == OnMismatch::Regenerate {
        if runner == Runner::Ci {
            return Err(
                "DUX_REGENERATE_CLI_REFERENCE is set on CI, where the reference page is only \
                 checked; regenerate it locally and commit the file."
                    .to_string(),
            );
        }
        return std::fs::write(path, rendered)
            .map_err(|error| format!("could not write {}: {error}", path.display()));
    }
    let Ok(committed) = std::fs::read_to_string(path) else {
        return Err(format!(
            "{REGENERATE}\n{} could not be read.",
            path.display()
        ));
    };
    if committed == rendered {
        return Ok(());
    }
    let line = committed
        .lines()
        .zip(rendered.lines())
        .position(|(old, new)| old != new)
        .unwrap_or_else(|| committed.lines().count().min(rendered.lines().count()));
    Err(format!(
        "{REGENERATE}\nFirst difference at line {}:\n  committed: {}\n  rendered:  {}",
        line + 1,
        committed.lines().nth(line).unwrap_or("(end of file)"),
        rendered.lines().nth(line).unwrap_or("(end of file)"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory as _;

    fn scratch(test: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("dux-cli-reference-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("make a scratch folder");
        dir.join("page.md")
    }

    #[test]
    fn the_committed_page_is_the_rendered_one() {
        let path = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../website/docs/cli-reference.md"
        ));
        let rendered = render(crate::commands::Cli::command(), dux_tui::config_command());
        let on_mismatch =
            if std::env::var_os("DUX_REGENERATE_CLI_REFERENCE").is_some_and(|v| v == "1") {
                OnMismatch::Regenerate
            } else {
                OnMismatch::Fail
            };
        let runner = if std::env::var_os("CI").is_some() {
            Runner::Ci
        } else {
            Runner::Local
        };
        if let Err(message) = reconcile(path, &rendered, on_mismatch, runner) {
            panic!("{message}");
        }
    }

    #[test]
    fn regenerating_writes_the_page_except_on_ci() {
        let path = scratch("regenerate");
        std::fs::write(&path, "stale").expect("write");
        assert!(
            reconcile(&path, "fresh", OnMismatch::Fail, Runner::Local)
                .expect_err("stale")
                .contains("DUX_REGENERATE_CLI_REFERENCE=1 cargo test -p dux cli_reference")
        );

        let refused =
            reconcile(&path, "fresh", OnMismatch::Regenerate, Runner::Ci).expect_err("on CI");
        assert!(refused.contains("CI"), "{refused}");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "stale");

        reconcile(&path, "fresh", OnMismatch::Regenerate, Runner::Local).expect("written");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "fresh");
        reconcile(&path, "fresh", OnMismatch::Fail, Runner::Local).expect("now equal");

        let missing = path.with_file_name("missing.md");
        assert!(
            reconcile(&missing, "fresh", OnMismatch::Fail, Runner::Local)
                .expect_err("missing")
                .contains("Regenerate it with")
        );
    }

    fn tiny() -> Command {
        Command::new("tool")
            .arg(
                Arg::new("remote")
                    .long("remote")
                    .global(true)
                    .help("Global one"),
            )
            .subcommand(
                Command::new("go")
                    .about("Go somewhere")
                    .arg(
                        Arg::new("target")
                            .required(true)
                            .value_name("AGENT")
                            .help("Pipe `a|b` to <AGENT> or c|d, unless --dry-run (or --x)"),
                    )
                    .arg(
                        Arg::new("secret")
                            .long("secret")
                            .hide(true)
                            .action(clap::ArgAction::SetTrue),
                    )
                    .arg(
                        Arg::new("speed")
                            .long("speed")
                            .value_parser(["slow", "fast"])
                            .default_value("slow")
                            .help("How fast"),
                    ),
            )
            .subcommand(Command::new("ghost").hide(true))
    }

    #[test]
    fn a_cell_keeps_its_code_span_and_escapes_what_would_break_the_table() {
        let page = render(tiny(), Command::new("config"));
        assert!(
            page.contains(
                "| `<AGENT>` | Required. Pipe `a\\|b` to \\<AGENT\\> or c\\|d, unless `--dry-run` (or `--x`). |\n"
            ),
            "{page}"
        );
        assert!(
            page.contains(
                "| `--speed` | `<speed>` | How fast. One of `slow`, `fast`. Default: `slow`. |\n"
            ),
            "{page}"
        );
    }

    #[test]
    fn global_hidden_and_clap_made_arguments_and_commands_are_left_out_of_sections() {
        let page = render(tiny(), Command::new("config"));
        let go = &page[page.find("## `tool go`").expect("go section")..];
        assert!(!go.contains("--remote"), "{go}");
        assert!(!go.contains("--secret"), "{go}");
        assert!(!go.contains("--help"), "{go}");
        assert!(!page.contains("ghost"), "{page}");
        assert!(!page.contains("`tool help`"), "{page}");
        assert!(
            page.contains("| `--remote` | `<remote>` | Global one. |"),
            "{page}"
        );
    }
}
