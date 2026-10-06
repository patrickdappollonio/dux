//! What a command prints, and the questions it asks before a change: the
//! three shapes of a list, picking one resource by id or name, confirming a
//! change, and making a path mean the same thing to the dux that receives it.

use std::io::{BufRead, Write};
use std::path::Path;

use super::{CliError, Exit};

/// One row of a list: its id (what `-q` prints), the table's cells in header
/// order, and the object `--format json` prints for it.
#[derive(Clone, Debug)]
pub struct Row {
    pub id: String,
    pub cells: Vec<String>,
    pub json: serde_json::Value,
}

/// A list as every `ls` prints it. Rows are already in the order the
/// resource's own list uses.
#[derive(Clone, Debug)]
pub struct Listing {
    pub headers: Vec<&'static str>,
    pub rows: Vec<Row>,
}

/// Which of the three forms a list is printed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// Aligned columns under a header; an empty list prints the header only.
    Table,
    /// One JSON array of objects.
    Json,
    /// The ids, one per line.
    Ids,
}

impl Shape {
    /// `-q` asks for the ids whatever the format says.
    pub fn of(json: bool, quiet: bool) -> Self {
        if quiet {
            Shape::Ids
        } else if json {
            Shape::Json
        } else {
            Shape::Table
        }
    }
}

/// The gap between two table columns.
const GUTTER: &str = "   ";

/// `listing` in `shape`, ending in a line break unless it is empty.
pub fn render(listing: &Listing, shape: Shape) -> String {
    match shape {
        Shape::Ids => listing
            .rows
            .iter()
            .map(|row| format!("{}\n", row.id))
            .collect(),
        Shape::Json => {
            let array: Vec<&serde_json::Value> = listing.rows.iter().map(|row| &row.json).collect();
            let mut text = serde_json::to_string_pretty(&array).unwrap_or_else(|_| "[]".into());
            text.push('\n');
            text
        }
        Shape::Table => table(listing),
    }
}

fn table(listing: &Listing) -> String {
    let width = |text: &str| text.chars().count();
    let mut widths: Vec<usize> = listing.headers.iter().map(|h| width(h)).collect();
    for row in &listing.rows {
        for (column, cell) in row.cells.iter().enumerate() {
            if let Some(w) = widths.get_mut(column) {
                *w = (*w).max(width(cell));
            }
        }
    }
    let line = |cells: &mut dyn Iterator<Item = &str>| {
        let mut out = String::new();
        for (column, cell) in cells.enumerate() {
            if column > 0 {
                out.push_str(GUTTER);
            }
            out.push_str(cell);
            let pad = widths.get(column).copied().unwrap_or(0)
                - width(cell).min(widths.get(column).copied().unwrap_or(0));
            out.extend(std::iter::repeat_n(' ', pad));
        }
        let mut out = out.trim_end().to_string();
        out.push('\n');
        out
    };
    let mut out = line(&mut listing.headers.iter().copied());
    for row in &listing.rows {
        out.push_str(&line(&mut row.cells.iter().map(String::as_str)));
    }
    out
}

/// One resource as `show` prints it: a `field: value` line per field that
/// holds something, a string as its text and anything else as JSON.
pub fn details(object: &serde_json::Value) -> String {
    let Some(fields) = object.as_object() else {
        return format!("{object}\n");
    };
    fields
        .iter()
        .filter(|(_, value)| !value.is_null())
        .map(|(field, value)| match value {
            serde_json::Value::String(text) => format!("{field}: {text}\n"),
            other => format!("{field}: {other}\n"),
        })
        .collect()
}

/// The one item of `items` that `query` names: the item with that id, else the only item
/// with that name.
///
/// # Errors
///
/// [`Exit::Failed`] when nothing matches; [`Exit::Usage`], listing their ids, when several
/// items share the name.
pub fn select<'a, T>(
    noun: &str,
    query: &str,
    items: &'a [T],
    id: impl Fn(&T) -> &str,
    name: impl Fn(&T) -> &str,
) -> Result<&'a T, CliError> {
    select_shown(noun, query, query, items, id, name)
}

/// [`select`] for a query that may not be repeated as typed: `shown` is how
/// every refusal names it.
pub fn select_shown<'a, T>(
    noun: &str,
    query: &str,
    shown: &str,
    items: &'a [T],
    id: impl Fn(&T) -> &str,
    name: impl Fn(&T) -> &str,
) -> Result<&'a T, CliError> {
    if let Some(item) = items.iter().find(|item| id(item) == query) {
        return Ok(item);
    }
    let named: Vec<&T> = items.iter().filter(|item| name(item) == query).collect();
    match named.as_slice() {
        [one] => Ok(one),
        [] => Err(CliError::new(
            Exit::Failed,
            format!("no {noun} has the id or name {shown}"),
        )),
        many => {
            let ids: Vec<&str> = many.iter().map(|item| id(item)).collect();
            Err(CliError::new(
                Exit::Usage,
                format!(
                    "{} {noun}s are named {shown}; name one by its id: {}",
                    many.len(),
                    ids.join(", ")
                ),
            ))
        }
    }
}

/// Ask `question` about `target` before a change, on `terminal` (the prompt to stderr, the
/// answer from stdin); `yes` answers for the user.
///
/// # Errors
///
/// [`Exit::Refused`] when the answer is no, or when there is no terminal to ask on.
pub fn confirm(
    question: &str,
    target: &str,
    yes: bool,
    terminal: Option<(&mut dyn BufRead, &mut dyn Write)>,
) -> Result<(), CliError> {
    if yes {
        return Ok(());
    }
    let Some((input, prompt)) = terminal else {
        return Err(CliError::new(
            Exit::Refused,
            format!(
                "{question} on {target}? There is no terminal to ask on, so nothing was changed; \
                 add --yes to go ahead"
            ),
        ));
    };
    let _ = write!(prompt, "{question} on {target}? [y/N] ");
    let _ = prompt.flush();
    let mut answer = String::new();
    let _ = input.read_line(&mut answer);
    if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        Err(CliError::new(Exit::Refused, "nothing was changed"))
    }
}

/// `path` as the dux on the other end should read it: absolute against `cwd` for the local
/// dux, which does not share this shell's folder.
///
/// # Errors
///
/// [`Exit::Usage`] for a relative path sent to the remote `remote` names.
pub fn path_for_target(path: &str, remote: Option<&str>, cwd: &Path) -> Result<String, CliError> {
    let typed = Path::new(path);
    if typed.is_absolute() {
        return Ok(path.to_string());
    }
    if let Some(remote) = remote {
        return Err(CliError::new(
            Exit::Usage,
            format!(
                "{path} is a relative path, and {remote} is another machine where this folder \
                 means nothing; give the full path"
            ),
        ));
    }
    Ok(cwd.join(typed).to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, cells: &[&str]) -> Row {
        Row {
            id: id.to_string(),
            cells: cells.iter().map(|c| c.to_string()).collect(),
            json: serde_json::json!({ "id": id, "name": cells[1] }),
        }
    }

    fn listing() -> Listing {
        Listing {
            headers: vec!["ID", "NAME", "PATH"],
            rows: vec![
                row("a1", &["a1", "web", "/src/web"]),
                row("b22", &["b22", "api-server", "/src/api"]),
            ],
        }
    }

    #[test]
    fn a_list_prints_in_each_of_its_three_shapes() {
        assert_eq!(
            render(&listing(), Shape::Table),
            "ID    NAME         PATH\n\
             a1    web          /src/web\n\
             b22   api-server   /src/api\n"
        );
        assert_eq!(render(&listing(), Shape::Ids), "a1\nb22\n");
        let json: serde_json::Value =
            serde_json::from_str(&render(&listing(), Shape::Json)).expect("one valid document");
        assert_eq!(
            json,
            serde_json::json!([
                { "id": "a1", "name": "web" },
                { "id": "b22", "name": "api-server" },
            ])
        );
        let empty = Listing {
            headers: vec!["ID", "NAME"],
            rows: vec![],
        };
        assert_eq!(render(&empty, Shape::Table), "ID   NAME\n");
        assert_eq!(render(&empty, Shape::Json).trim(), "[]");
        assert_eq!(render(&empty, Shape::Ids), "");
    }

    #[test]
    fn a_shown_resource_prints_a_line_per_field_that_holds_something() {
        let shown = details(&serde_json::json!({
            "name": "claude",
            "args": ["--fast"],
            "install_hint": null,
            "forward_scroll": false,
        }));
        assert_eq!(
            shown,
            "args: [\"--fast\"]\nforward_scroll: false\nname: claude\n"
        );
    }

    #[test]
    fn quiet_asks_for_ids_whatever_the_format() {
        assert_eq!(Shape::of(true, true), Shape::Ids);
        assert_eq!(Shape::of(false, true), Shape::Ids);
        assert_eq!(Shape::of(true, false), Shape::Json);
        assert_eq!(Shape::of(false, false), Shape::Table);
    }

    #[test]
    fn a_resource_is_picked_by_id_or_by_a_name_only_it_has() {
        let items = [("a1", "web"), ("b2", "api"), ("c3", "api"), ("web", "docs")];
        fn id<'a>(item: &'a (&'static str, &'static str)) -> &'a str {
            item.0
        }
        fn name<'a>(item: &'a (&'static str, &'static str)) -> &'a str {
            item.1
        }

        assert_eq!(select("agent", "b2", &items, id, name).unwrap().0, "b2");
        assert_eq!(
            select("agent", "web", &items, id, name).unwrap().0,
            "web",
            "an id wins over another item's name"
        );
        assert_eq!(select("agent", "docs", &items, id, name).unwrap().0, "web");

        let ambiguous = select("agent", "api", &items, id, name).unwrap_err();
        assert_eq!(ambiguous.exit, Exit::Usage);
        assert!(ambiguous.message.contains("b2"), "{}", ambiguous.message);
        assert!(ambiguous.message.contains("c3"), "{}", ambiguous.message);

        let missing = select("agent", "nope", &items, id, name).unwrap_err();
        assert_eq!(missing.exit, Exit::Failed);
        assert!(missing.message.contains("nope"), "{}", missing.message);
    }

    #[test]
    fn a_change_without_a_terminal_or_yes_is_refused() {
        let refused = confirm("Delete agent web", "this machine's dux", false, None).unwrap_err();
        assert_eq!(refused.exit, Exit::Refused);
        assert!(refused.message.contains("--yes"), "{}", refused.message);
        assert!(refused.message.contains("Delete agent web"));

        assert_eq!(confirm("Delete agent web", "x", true, None), Ok(()));

        let mut prompt = Vec::new();
        let mut yes = std::io::Cursor::new(b"y\n".to_vec());
        assert_eq!(
            confirm(
                "Delete agent web",
                "remote work",
                false,
                Some((&mut yes, &mut prompt))
            ),
            Ok(())
        );
        let asked = String::from_utf8(prompt).unwrap();
        assert!(asked.contains("Delete agent web"), "{asked}");
        assert!(
            asked.contains("remote work"),
            "the prompt names its target: {asked}"
        );

        let mut prompt = Vec::new();
        let mut no = std::io::Cursor::new(b"\n".to_vec());
        let declined =
            confirm("Delete agent web", "x", false, Some((&mut no, &mut prompt))).unwrap_err();
        assert_eq!(declined.exit, Exit::Refused);
    }

    #[test]
    fn a_relative_path_is_made_absolute_locally_and_refused_for_a_remote() {
        let cwd = Path::new("/home/me/src");
        assert_eq!(
            path_for_target("app", None, cwd).unwrap(),
            "/home/me/src/app"
        );
        assert_eq!(
            path_for_target("/srv/app", Some("work"), cwd).unwrap(),
            "/srv/app"
        );
        let refused = path_for_target("app", Some("work"), cwd).unwrap_err();
        assert_eq!(refused.exit, Exit::Usage);
        assert!(refused.message.contains("work"), "{}", refused.message);
    }
}
