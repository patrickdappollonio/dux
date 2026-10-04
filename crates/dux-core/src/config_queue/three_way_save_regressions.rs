//! Three-way save regressions: each test edits `config.toml` the way a person
//! would (by hand, between saves) while dux saves from memory, and pins what
//! the file must hold afterwards. Every case comes from a review that found a
//! save undoing a hand edit, losing one, or breaking the file.

use super::*;

fn read(path: &std::path::Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

/// A config file holding `text`, the config loaded from it, and a writer
/// whose base is that config.
fn setup(
    text: &str,
) -> (
    tempfile::TempDir,
    std::path::PathBuf,
    Config,
    ConfigWriteQueue,
) {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, text).unwrap();
    let loaded = crate::config::load_config_file(&path).unwrap();
    let queue = ConfigWriteQueue::with_base(path.clone(), &loaded);
    (dir, path, loaded, queue)
}

/// Save `memory` three times, adding a global environment variable each
/// time (a change no test here is about), and return the file after each.
fn three_unrelated_saves(
    path: &std::path::Path,
    queue: &ConfigWriteQueue,
    memory: &mut Config,
) -> Vec<String> {
    (0..3)
        .map(|round| {
            memory.env.insert(format!("UNRELATED_{round}"), "1".into());
            queue.save_eager(memory.clone()).unwrap();
            read(path)
        })
        .collect()
}

/// The `[[projects]]` entries of `text` as (name, note) pairs.
fn names_and_notes(text: &str) -> Vec<(Option<String>, Option<String>)> {
    let doc: toml_edit::DocumentMut = text.parse().unwrap();
    doc.get("projects")
        .and_then(|projects| projects.as_array_of_tables())
        .map(|projects| {
            projects
                .iter()
                .map(|entry| {
                    let field = |name: &str| {
                        entry
                            .get(name)
                            .and_then(|value| value.as_str())
                            .map(String::from)
                    };
                    (field("name"), field("note"))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn some(text: &str) -> Option<String> {
    Some(text.to_string())
}

// ---------------------------------------------------------------------------
// Inline sections the user deleted or rewrote by hand
// ---------------------------------------------------------------------------

#[test]
fn a_deleted_inline_ui_section_stays_deleted() {
    let (_dir, path, loaded, queue) = setup("ui = { left_width_pct = 25 }\n[env]\nA = \"1\"\n");
    let mut memory = loaded.clone();
    memory.env.insert("B".into(), "2".into());
    queue.save_eager(memory.clone()).unwrap();
    let written = read(&path);
    let line = written
        .lines()
        .find(|line| line.starts_with("ui = {"))
        .expect("the inline ui section")
        .to_string();
    std::fs::write(&path, written.replace(&format!("{line}\n"), "")).unwrap();

    for text in three_unrelated_saves(&path, &queue, &mut memory) {
        assert!(!text.contains("ui = {"), "{text}");
        assert!(!text.contains("[ui]"), "{text}");
        assert!(!text.contains("left_width_pct"), "{text}");
    }
}

#[test]
fn a_deleted_inline_provider_stays_deleted() {
    let (_dir, path, loaded, queue) =
        setup("[providers]\nclaude = { command = \"claude\" }\n[env]\nA = \"1\"\n");
    let mut memory = loaded.clone();
    memory.env.insert("B".into(), "2".into());
    queue.save_eager(memory.clone()).unwrap();
    let written = read(&path);
    let line = written
        .lines()
        .find(|line| line.starts_with("claude = {"))
        .expect("the inline provider")
        .to_string();
    std::fs::write(&path, written.replace(&format!("{line}\n"), "")).unwrap();

    for text in three_unrelated_saves(&path, &queue, &mut memory) {
        assert!(!text.contains("claude = {"), "{text}");
        assert!(!text.contains("[providers.claude]"), "{text}");
    }
}

#[test]
fn an_inline_section_rewritten_as_a_table_without_a_key_keeps_it_out() {
    let (_dir, path, loaded, queue) = setup("ui = { left_width_pct = 25 }\n");
    let mut memory = loaded.clone();
    memory.env.insert("B".into(), "2".into());
    queue.save_eager(memory.clone()).unwrap();
    let written = read(&path);
    let line = written
        .lines()
        .find(|line| line.starts_with("ui = {"))
        .expect("the inline ui section")
        .to_string();
    assert!(line.contains("diff_tab_width"), "{line}");
    // The user turns the inline table into a `[ui]` table, leaving one key out.
    let mut table = toml_edit::DocumentMut::new();
    let inline: toml_edit::DocumentMut = line.parse().unwrap();
    let mut ui = inline["ui"]
        .as_inline_table()
        .expect("inline")
        .clone()
        .into_table();
    ui.remove("diff_tab_width");
    table["ui"] = toml_edit::Item::Table(ui);
    let rewritten = written.replace(&format!("{line}\n"), "") + "\n" + &table.to_string();
    std::fs::write(&path, rewritten).unwrap();

    for text in three_unrelated_saves(&path, &queue, &mut memory) {
        assert!(text.contains("[ui]"), "{text}");
        assert!(!text.contains("diff_tab_width"), "{text}");
    }
}

// ---------------------------------------------------------------------------
// Two hand-written projects for one folder
// ---------------------------------------------------------------------------

const TWO_PROJECTS_AT_ONE_PATH: &str = "[[projects]]\npath = \"/x\"\nname = \"one\"\nnote = \"n1\"\n\n[[projects]]\npath = \"/x\"\nname = \"two\"\nnote = \"n2\"\n";

#[test]
fn renaming_the_first_of_two_projects_at_one_path_to_the_seconds_name_never_swaps_them() {
    let (_dir, path, loaded, queue) = setup(TWO_PROJECTS_AT_ONE_PATH);
    let mut memory = loaded.clone();
    memory.projects[0].name = Some("two".into());
    queue.save_eager(memory.clone()).unwrap();
    for text in
        std::iter::once(read(&path)).chain(three_unrelated_saves(&path, &queue, &mut memory))
    {
        assert_eq!(
            names_and_notes(&text),
            vec![(some("two"), some("n1")), (some("two"), some("n2"))],
            "{text}"
        );
    }
}

#[test]
fn renaming_the_second_of_two_projects_at_one_path_to_the_firsts_name_never_swaps_them() {
    let (_dir, path, loaded, queue) = setup(TWO_PROJECTS_AT_ONE_PATH);
    let mut memory = loaded.clone();
    memory.projects[1].name = Some("one".into());
    queue.save_eager(memory.clone()).unwrap();
    for text in
        std::iter::once(read(&path)).chain(three_unrelated_saves(&path, &queue, &mut memory))
    {
        assert_eq!(
            names_and_notes(&text),
            vec![(some("one"), some("n1")), (some("one"), some("n2"))],
            "{text}"
        );
    }
}

/// Every edit memory makes to two id-less projects at one path keeps each
/// project's own keys on its own entry, over three saves.
#[test]
fn renaming_or_removing_projects_at_one_path_keeps_each_entrys_own_keys() {
    type Edit = fn(&mut Config);
    type Rows = Vec<(Option<String>, Option<String>)>;
    let cases: [(Edit, Rows); 4] = [
        (
            |memory| memory.projects[0].name = Some("two".into()),
            vec![(some("two"), some("n1")), (some("two"), some("n2"))],
        ),
        (
            |memory| {
                memory.projects[0].name = Some("two".into());
                memory.projects[1].name = Some("three".into());
            },
            vec![(some("two"), some("n1")), (some("three"), some("n2"))],
        ),
        (
            |memory| {
                memory.projects[0].name = Some("two".into());
                memory.projects[1].name = Some("one".into());
            },
            vec![(some("two"), some("n1")), (some("one"), some("n2"))],
        ),
        (
            |memory| {
                memory.projects.remove(1);
                memory.projects[0].name = Some("two".into());
            },
            vec![(some("two"), some("n1"))],
        ),
    ];
    for (edit, expected) in cases {
        let (_dir, path, loaded, queue) = setup(TWO_PROJECTS_AT_ONE_PATH);
        let mut memory = loaded.clone();
        edit(&mut memory);
        queue.save_eager(memory.clone()).unwrap();
        for text in
            std::iter::once(read(&path)).chain(three_unrelated_saves(&path, &queue, &mut memory))
        {
            assert_eq!(names_and_notes(&text), expected, "{text}");
        }
    }
}

// ---------------------------------------------------------------------------
// Hand edits to projects, env, macros, providers and inline sections
// ---------------------------------------------------------------------------

fn load(path: &std::path::Path) -> Config {
    crate::config::load_config_file(path).unwrap()
}

/// A writer whose base is the file as dux loaded it, as dux builds it.
fn queue_for(path: &std::path::Path) -> ConfigWriteQueue {
    ConfigWriteQueue::with_base(path.to_path_buf(), &load(path))
}

fn project(id: &str, path: &str, name: &str) -> crate::config::ProjectConfig {
    crate::config::ProjectConfig {
        id: id.into(),
        path: path.into(),
        name: Some(name.into()),
        default_provider: None,
        leading_branch: None,
        auto_reopen_agents: None,
        startup_command: None,
        env: Default::default(),
    }
}

/// The `[[projects]]` entries of `text` as (id, path, name).
fn project_rows(text: &str) -> Vec<(Option<String>, Option<String>, Option<String>)> {
    let doc: toml_edit::DocumentMut = text.parse().unwrap();
    doc.get("projects")
        .and_then(|projects| projects.as_array_of_tables())
        .map(|projects| {
            projects
                .iter()
                .map(|entry| {
                    let field = |name: &str| {
                        entry
                            .get(name)
                            .and_then(|value| value.as_str())
                            .map(String::from)
                    };
                    (field("id"), field("path"), field("name"))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Load `initial`, let a save fill in `diff_tab_width`, delete it by hand,
/// save twice more, and return the file.
fn fill_then_hand_delete_diff_tab_width(initial: &str) -> String {
    let (_dir, path, loaded, queue) = setup(initial);
    let mut memory = loaded.clone();
    memory.env.insert("Z".into(), "1".into());
    queue.save_eager(memory.clone()).unwrap();
    let text = read(&path);
    assert!(text.contains("diff_tab_width = 4"), "filled first:\n{text}");
    let deleted = text
        .replace("diff_tab_width = 4, ", "")
        .replace("diff_tab_width = 4\n", "");
    assert!(!deleted.contains("diff_tab_width"));
    std::fs::write(&path, &deleted).unwrap();
    memory.env.insert("Y".into(), "2".into());
    queue.save_eager(memory.clone()).unwrap();
    memory.env.insert("X".into(), "3".into());
    queue.save_eager(memory.clone()).unwrap();
    read(&path)
}

/// A variable deleted by hand from a `[projects.env]` table stays deleted over
/// three saves.
#[test]
fn a_project_env_variable_deleted_by_hand_stays_deleted() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[[projects]]\nid = \"p\"\npath = \"/tmp/p\"\n\n[projects.env]\nTOK = \"secret\"\nKEEP = \"1\"\n").unwrap();
    let q = queue_for(&path);
    let mut m = load(&path);
    std::fs::write(
        &path,
        "[[projects]]\nid = \"p\"\npath = \"/tmp/p\"\n\n[projects.env]\nKEEP = \"1\"\n",
    )
    .unwrap();
    for i in 0..3 {
        m.ui.right_width_pct = 30 + i;
        q.save_eager(m.clone()).unwrap();
    }
    let a = read(&path);
    assert!(!a.contains("TOK"), "{a}");
}

/// A project moved by hand keeps its new path, as one entry, when dux renames
/// it.
#[test]
fn a_project_moved_by_hand_keeps_its_new_path_when_dux_renames_it() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[[projects]]\nid = \"p\"\npath = \"/tmp/p\"\nname = \"a\"\n",
    )
    .unwrap();
    let q = queue_for(&path);
    let mut m = load(&path);
    std::fs::write(
        &path,
        "[[projects]]\nid = \"p\"\npath = \"/tmp/q\"\nname = \"a\"\n",
    )
    .unwrap();
    m.projects[0].name = Some("b".into());
    q.save_eager(m.clone()).unwrap();
    m.ui.right_width_pct = 33;
    q.save_eager(m.clone()).unwrap();
    let a = read(&path);
    assert_eq!(a.matches("[[projects]]").count(), 1, "{a}");
    assert!(a.contains("/tmp/q") && a.contains("\"b\""), "{a}");
}

/// Id-less projects get one id each, across a reload, and one added by hand
/// later stays.
#[test]
fn id_less_projects_settle_on_one_id_each_and_a_hand_added_one_stays() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[[projects]]\npath = \"/tmp/a\"\n\n[[projects]]\npath = \"/tmp/b\"\n",
    )
    .unwrap();
    let q = queue_for(&path);
    let mut m = load(&path);
    m.ui.right_width_pct = 31;
    q.save_eager(m.clone()).unwrap();
    let first = read(&path);
    assert_eq!(
        first.matches("id = ").count(),
        2,
        "each project settles on an id:\n{first}"
    );
    // reload
    let mut m2 = load(&path);
    q.set_base(m2.clone());
    m2.ui.right_width_pct = 32;
    q.save_eager(m2.clone()).unwrap();
    // hand-add id-less project c; memory (stale m2) saves again
    let t = read(&path) + "\n[[projects]]\npath = \"/tmp/c\"\n";
    std::fs::write(&path, &t).unwrap();
    m2.ui.right_width_pct = 34;
    q.save_eager(m2.clone()).unwrap();
    m2.ui.right_width_pct = 35;
    q.save_eager(m2.clone()).unwrap();
    let a = read(&path);
    assert_eq!(a.matches("[[projects]]").count(), 3, "{a}");
    assert_eq!(a.matches("id = ").count(), 2, "{a}");
}

/// A project dux added and the user then deleted by hand stays deleted.
#[test]
fn a_project_dux_added_and_the_user_deleted_stays_deleted() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[[projects]]\nid = \"a\"\npath = \"/tmp/a\"\n").unwrap();
    let q = queue_for(&path);
    let mut m = load(&path);
    let mut p = m.projects[0].clone();
    p.id = "n".into();
    p.path = "/tmp/n".into();
    m.projects.push(p);
    q.save_eager(m.clone()).unwrap();
    // hand delete n
    std::fs::write(&path, "[[projects]]\nid = \"a\"\npath = \"/tmp/a\"\n").unwrap();
    m.ui.right_width_pct = 31;
    q.save_eager(m.clone()).unwrap();
    m.ui.right_width_pct = 32;
    q.save_eager(m.clone()).unwrap();
    let a = read(&path);
    assert!(!a.contains("/tmp/n"), "{a}");
}

/// A `dux config set` landing between two saves survives them, with the comment
/// above it.
#[test]
fn a_config_set_between_two_saves_survives_with_its_comment() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "# top\n[ui]\n# keep me\nleft_width_pct = 20\ncopy_on_select = true\n",
    )
    .unwrap();
    let q = queue_for(&path);
    let mut m = load(&path);
    m.ui.copy_on_select = false;
    q.save_eager(m.clone()).unwrap();
    let key = crate::config_keys::lookup("ui.left_width_pct").unwrap();
    crate::config_keys::set_plain(&path, &key, "40").unwrap();
    m.ui.right_width_pct = 31;
    q.save_eager(m.clone()).unwrap();
    m.ui.copy_on_select = true;
    q.save_eager(m.clone()).unwrap();
    let a = read(&path);
    assert!(
        a.contains("left_width_pct = 40") && a.contains("# keep me"),
        "{a}"
    );
}

/// A whole `[env]` table deleted by hand keeps its old variables out when dux
/// adds a new one.
#[test]
fn a_deleted_env_table_keeps_its_old_variables_out_when_dux_adds_one() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[env]\nA = \"1\"\nB = \"2\"\n\n[ui]\nleft_width_pct = 20\n",
    )
    .unwrap();
    let q = queue_for(&path);
    let mut m = load(&path);
    std::fs::write(&path, "[ui]\nleft_width_pct = 20\n").unwrap();
    m.env.insert("C".into(), "3".into());
    q.save_eager(m.clone()).unwrap();
    m.ui.right_width_pct = 31;
    q.save_eager(m.clone()).unwrap();
    let a = read(&path);
    assert!(a.contains("C = \"3\"") && !a.contains("A = "), "{a}");
}

/// A project, an env variable, a macro and a provider added by hand survive
/// three saves.
#[test]
fn hand_added_projects_env_macros_and_providers_survive_three_saves() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[[projects]]\nid = \"a\"\npath = \"/tmp/a\"\n\n[env]\nA = \"1\"\n\n[macros]\nm1 = \"x\"\n",
    )
    .unwrap();
    let q = queue_for(&path);
    let mut m = load(&path);
    let t = read(&path)
        .replace("[env]\nA = \"1\"\n", "[env]\nA = \"1\"\nHAND_ENV = \"2\"\n")
        .replace("m1 = \"x\"\n", "m1 = \"x\"\nhand_macro = \"y\"\n")
        + "\n[[projects]]\nid = \"c\"\npath = \"/tmp/c\"\n\n[providers.handprov]\ncommand = \"foo\"\n";
    std::fs::write(&path, &t).unwrap();
    for i in 0..3 {
        m.ui.right_width_pct = 30 + i;
        q.save_eager(m.clone()).unwrap();
        let a = read(&path);
        assert!(a.contains("/tmp/c"), "save {i}: hand project kept:\n{a}");
        assert!(a.contains("HAND_ENV"), "save {i}: hand env kept:\n{a}");
        assert!(a.contains("hand_macro"), "save {i}: hand macro kept:\n{a}");
        assert!(a.contains("handprov"), "save {i}: hand provider kept:\n{a}");
    }
}

/// Projects whose paths use environment variables are not written twice.
#[test]
fn projects_with_environment_variables_in_their_paths_are_not_duplicated() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[[projects]]\npath = \"$HOME/p\"\n\n[[projects]]\nid = \"q\"\npath = \"$HOME/q\"\n",
    )
    .unwrap();
    let q = queue_for(&path);
    let mut m = load(&path);
    for i in 0..2 {
        m.ui.right_width_pct = 30 + i;
        q.save_eager(m.clone()).unwrap();
    }
    let a = read(&path);
    assert_eq!(a.matches("[[projects]]").count(), 2, "{a}");
}

/// The same with an id-less hand-added project, and after a reload.
#[test]
fn a_hand_added_id_less_project_survives_saves_and_a_reload() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[[projects]]\nid = \"a\"\npath = \"/tmp/a\"\n").unwrap();
    let q = queue_for(&path);
    let mut m = load(&path);
    let t = read(&path) + "\n[[projects]]\npath = \"/tmp/hand\"\n";
    std::fs::write(&path, &t).unwrap();
    for i in 0..3 {
        m.ui.right_width_pct = 30 + i;
        q.save_eager(m.clone()).unwrap();
        assert!(
            read(&path).contains("/tmp/hand"),
            "save {i}:\n{}",
            read(&path)
        );
    }
    // A reload: memory and base become the file as it is now (hand
    // project and all, its id minted by this read), then two saves.
    m = load(&path);
    q.set_base(m.clone());
    for i in 0..2 {
        m.ui.right_width_pct = 40 + i;
        q.save_eager(m.clone()).unwrap();
        assert!(
            read(&path).contains("/tmp/hand"),
            "after reload {i}:\n{}",
            read(&path)
        );
    }
}

/// A project dux removed, then added back to the file by hand, stays.
#[test]
fn a_project_removed_in_dux_then_added_back_by_hand_stays() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[[projects]]\nid = \"a\"\npath = \"/tmp/a\"\n\n[[projects]]\nid = \"b\"\npath = \"/tmp/b\"\n",
    )
    .unwrap();
    let q = queue_for(&path);
    let mut m = load(&path);
    m.projects.retain(|p| p.id != "b");
    q.save_eager(m.clone()).unwrap();
    assert!(!read(&path).contains("/tmp/b"), "removed by dux");
    let t = read(&path) + "\n[[projects]]\nid = \"b\"\npath = \"/tmp/b\"\n";
    std::fs::write(&path, &t).unwrap();
    for i in 0..3 {
        m.ui.right_width_pct = 30 + i;
        q.save_eager(m.clone()).unwrap();
        assert!(read(&path).contains("/tmp/b"), "save {i}:\n{}", read(&path));
    }
}

/// A `set` that lands between dux loading the file and building its
/// writer is not reverted: the writer's base is the loaded text, not a
/// fresh read.
#[test]
fn a_config_set_between_load_and_writer_start_survives() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[ui]\nleft_width_pct = 20\n").unwrap();
    let mut m = load(&path);
    let key = crate::config_keys::lookup("ui.left_width_pct").unwrap();
    crate::config_keys::set_plain(&path, &key, "40").unwrap();
    let q = ConfigWriteQueue::with_base(path.clone(), &m);
    m.ui.copy_on_select = !m.ui.copy_on_select;
    q.save_eager(m.clone()).unwrap();
    m.ui.right_width_pct = 31;
    q.save_eager(m).unwrap();
    assert!(
        read(&path).contains("left_width_pct = 40"),
        "{}",
        read(&path)
    );
}

/// A project deleted by hand is not written back when dux changes another
/// project with the same name.
#[test]
fn a_project_deleted_by_hand_is_not_rewritten_when_dux_changes_its_namesake() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[[projects]]\nid = \"a\"\npath = \"/x/api\"\nname = \"api\"\n\n[[projects]]\nid = \"b\"\npath = \"/y/api\"\nname = \"api\"\n").unwrap();
    let loaded = crate::config::load_config_file(&path).unwrap();
    let q = ConfigWriteQueue::with_base(path.clone(), &loaded);
    // hand delete A
    std::fs::write(
        &path,
        "[[projects]]\nid = \"b\"\npath = \"/y/api\"\nname = \"api\"\n",
    )
    .unwrap();
    let mut memory = loaded.clone();
    memory.projects[1].default_provider = Some("codex".into());
    q.save_eager(memory).unwrap();
    let text = read(&path);
    assert_eq!(text.matches("id = \"b\"").count(), 1, "{text}");
}

/// The user moves a project by hand: deletes the old entry, writes a new
/// one without an id but with the same name. The new entry must not
/// inherit the old id.
#[test]
fn a_project_moved_by_hand_without_an_id_does_not_take_the_old_id() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[[projects]]\nid = \"a\"\npath = \"/old/dux\"\nname = \"dux\"\n",
    )
    .unwrap();
    let loaded = crate::config::load_config_file(&path).unwrap();
    let q = ConfigWriteQueue::with_base(path.clone(), &loaded);
    std::fs::write(&path, "[[projects]]\npath = \"/new/dux\"\nname = \"dux\"\n").unwrap();
    let mut memory = loaded.clone();
    memory.ui.copy_on_select = !memory.ui.copy_on_select;
    q.save_eager(memory).unwrap();
    let text = read(&path);
    assert!(!text.contains("id = \"a\""), "{text}");
}

/// Env variables deleted by hand stay deleted over three saves, and while dux
/// adds and removes others.
#[test]
fn env_variables_deleted_by_hand_stay_deleted_while_dux_adds_and_removes_others() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[env]\nA = \"1\"\nB = \"2\"\n").unwrap();
    let loaded = crate::config::load_config_file(&path).unwrap();
    let q = ConfigWriteQueue::with_base(path.clone(), &loaded);
    std::fs::write(&path, "[env]\nB = \"2\"\nC = \"3\"\n").unwrap();
    let mut memory = loaded.clone();
    for i in 0..3 {
        memory.ui.copy_on_select = i % 2 == 0;
        q.save_eager(memory.clone()).unwrap();
    }
    memory.env.insert("D".into(), "4".into());
    q.save_eager(memory.clone()).unwrap();
    memory.env.remove("B");
    q.save_eager(memory.clone()).unwrap();
    let text = read(&path);
    assert!(!text.contains("A ="), "{text}");
    assert!(text.contains("C = \"3\""), "{text}");
    assert!(text.contains("D = \"4\""), "{text}");
    assert!(!text.contains("B ="), "{text}");
}

/// A project added by hand survives dux adding one project and removing two.
#[test]
fn a_hand_added_project_survives_dux_adding_and_removing_others() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[[projects]]\nid = \"a\"\npath = \"/a\"\nname = \"a\"\n",
    )
    .unwrap();
    let loaded = crate::config::load_config_file(&path).unwrap();
    let q = ConfigWriteQueue::with_base(path.clone(), &loaded);
    // hand adds c
    let mut t = read(&path);
    t.push_str("\n[[projects]]\npath = \"/c\"\nname = \"c\"\n");
    std::fs::write(&path, t).unwrap();
    // dux adds b
    let mut memory = loaded.clone();
    memory.projects.push(project("b", "/b", "b"));
    q.save_eager(memory.clone()).unwrap();
    // dux removes a
    memory.projects.remove(0);
    q.save_eager(memory.clone()).unwrap();
    // dux removes b
    memory.projects.remove(0);
    q.save_eager(memory.clone()).unwrap();
    let text = read(&path);
    assert!(text.contains("/c"), "{text}");
    assert!(!text.contains("\"/a\""), "{text}");
    assert!(!text.contains("\"/b\""), "{text}");
}

/// A project added by hand with the name of one dux removes stays.
#[test]
fn a_hand_added_project_named_like_a_removed_one_stays() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[[projects]]\nid = \"a\"\npath = \"/old/api\"\nname = \"api\"\n",
    )
    .unwrap();
    let loaded = crate::config::load_config_file(&path).unwrap();
    let q = ConfigWriteQueue::with_base(path.clone(), &loaded);
    let mut t = read(&path);
    t.push_str("\n[[projects]]\npath = \"/new/api\"\nname = \"api\"\n");
    std::fs::write(&path, t).unwrap();
    let mut memory = loaded.clone();
    memory.projects.clear();
    q.save_eager(memory).unwrap();
    let text = read(&path);
    assert!(
        text.contains("/new/api"),
        "hand-added project lost:\n{}",
        text.lines().take(8).collect::<Vec<_>>().join("\n")
    );
}

/// Hand-added project env var, then dux changes another var of that project's
/// env.
#[test]
fn a_hand_added_project_env_variable_survives_dux_adding_another() {
    let (_d, path, loaded, q) =
        setup("[[projects]]\nid = \"a\"\npath = \"/a\"\nenv = { A = \"1\" }\n");
    std::fs::write(
        &path,
        "[[projects]]\nid = \"a\"\npath = \"/a\"\nenv = { A = \"1\", B = \"2\" }\n",
    )
    .unwrap();
    let mut m = loaded.clone();
    m.projects[0].env.insert("C".into(), "3".into());
    q.save_eager(m).unwrap();
    let t = read(&path);
    assert!(t.contains("B = \"2\""), "{t}");
}

/// A project env written by hand as a commented subtable keeps its form, its
/// comment and its variables when dux adds one.
#[test]
fn a_hand_written_project_env_subtable_keeps_its_form_comment_and_variables() {
    let (_d, path, loaded, q) =
        setup("[[projects]]\nid = \"a\"\npath = \"/a\"\n[projects.env]\nA = \"1\"\n");
    std::fs::write(
        &path,
        "[[projects]]\nid = \"a\"\npath = \"/a\"\n# my env\n[projects.env]\nA = \"1\"\nB = \"2\"\n",
    )
    .unwrap();
    let mut m = loaded.clone();
    m.projects[0].env.insert("C".into(), "3".into());
    q.save_eager(m).unwrap();
    let t = read(&path);
    assert!(t.contains("B = \"2\""), "{t}");
    assert!(t.contains("C = \"3\""), "{t}");
    assert!(t.contains("# my env"), "the comment stays: {t}");
    assert!(t.contains("[projects.env]"), "the subtable form stays: {t}");
}

/// Hand edits (an env variable, a project, a deleted name) and dux project
/// changes mixed over four saves.
#[test]
fn hand_edits_and_dux_project_changes_mixed_over_several_saves() {
    let (_d, path, loaded, q) = setup(
        "[env]\nX = \"1\"\n\n[[projects]]\nid = \"a\"\npath = \"/a\"\nname = \"api\"\n\n[[projects]]\nid = \"b\"\npath = \"/b\"\nname = \"api\"\n",
    );
    let mut t = read(&path);
    t = t.replace("[env]\nX = \"1\"\n", "[env]\nX = \"1\"\nH = \"hand\"\n");
    t.push_str("\n[[projects]]\npath = \"/c\"\nname = \"api\"\n");
    std::fs::write(&path, &t).unwrap();
    let mut m = loaded.clone();
    m.projects.retain(|p| p.id != "a");
    q.save_eager(m.clone()).unwrap();
    // hand deletes b's name, adds key
    let t = read(&path).replace("name = \"api\"\n", "");
    std::fs::write(&path, &t).unwrap();
    m.ui.copy_on_select = !m.ui.copy_on_select;
    q.save_eager(m.clone()).unwrap();
    m.env.insert("Y".into(), "2".into());
    q.save_eager(m.clone()).unwrap();
    m.projects.push(crate::config::ProjectConfig {
        id: "d".into(),
        path: "/c".into(),
        name: Some("c".into()),
        default_provider: None,
        leading_branch: None,
        auto_reopen_agents: None,
        startup_command: None,
        env: Default::default(),
    });
    q.save_eager(m.clone()).unwrap();
    let t = read(&path);
    assert!(t.contains("H = \"hand\""));
    assert!(!t.contains("\"/a\""));
    assert_eq!(t.matches("\"/c\"").count(), 1, "{t}");
    assert!(!t.contains("name = \"api\""), "{t}");
}

/// Hand-added provider and macro, dux edits macros.
#[test]
fn a_hand_added_macro_and_provider_survive_saves() {
    let (_d, path, loaded, q) = setup("[macros]\none = \"1\"\ntwo = \"2\"\n");
    let t = read(&path) + "three = \"3\"\n\n[providers.mine]\ncommand = \"mine\"\n";
    std::fs::write(&path, &t).unwrap();
    let mut m = loaded.clone();
    q.save_eager(m.clone()).unwrap();
    m.ui.copy_on_select = !m.ui.copy_on_select;
    q.save_eager(m.clone()).unwrap();
    let t = read(&path);
    assert!(t.contains("three"), "{t}");
    assert!(t.contains("providers.mine"), "{t}");
}

/// A key deleted by hand after the startup sync wrote the file stays deleted.
#[test]
fn a_key_deleted_by_hand_after_the_startup_sync_wrote_stays_deleted() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[ui]\nleft_width_pct = 20\n").unwrap();
    let loaded = crate::config::load_config_file(&path).unwrap();
    // simulate startup sync write
    let mut cfg = loaded.clone();
    cfg.env.insert("S".into(), "1".into());
    let w = crate::config_write::save_config_three_way(
        &path,
        Some(crate::config_write::SaveBase::read(&loaded)),
        &cfg,
        crate::config_write::Durability::Fsync,
    )
    .unwrap();
    cfg.source_text = crate::config::SourceText::written(&w, cfg.clone());
    let q = ConfigWriteQueue::with_base(path.clone(), &cfg);
    // hand delete left_width_pct and copy_on_select
    let t = read(&path).replace("left_width_pct = 20\n", "");
    std::fs::write(&path, &t).unwrap();
    let mut m = cfg.clone();
    m.env.insert("T".into(), "2".into());
    q.save_eager(m.clone()).unwrap();
    let t = read(&path);
    assert!(!t.contains("left_width_pct"), "{t}");
}

/// A project env variable deleted by hand stays deleted when dux changes
/// another variable of the same project, across saves.
#[test]
fn a_project_env_variable_deleted_by_hand_stays_deleted_while_dux_adds_another() {
    let (_d, path, loaded, q) =
        setup("[[projects]]\nid = \"a\"\npath = \"/a\"\nenv = { A = \"1\", B = \"2\" }\n");
    std::fs::write(
        &path,
        "[[projects]]\nid = \"a\"\npath = \"/a\"\nenv = { A = \"1\" }\n",
    )
    .unwrap();
    let mut m = loaded.clone();
    m.projects[0].env.insert("C".into(), "3".into());
    q.save_eager(m.clone()).unwrap();
    m.ui.copy_on_select = !m.ui.copy_on_select;
    q.save_eager(m).unwrap();
    let t = read(&path);
    assert!(!t.contains("B = "), "{t}");
    assert!(t.contains("A = \"1\"") && t.contains("C = \"3\""), "{t}");
}

/// A change whose save failed is still a change: the base is the config
/// of the last save that succeeded, so a later save writes it.
#[test]
fn a_change_whose_save_failed_is_written_by_a_later_save() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, path, loaded, q) = setup("[env]\nA = \"1\"\n");
    let mut m = loaded.clone();
    m.env.insert("B".into(), "2".into());
    q.save_eager(m.clone()).unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
    let mut failed = m.clone();
    failed.ui.left_width_pct = 41;
    let result = q.save_eager(failed.clone());
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err(), "the write failed");
    // What the engine hands the writer after a reload whose deferred
    // save failed: the surfaced config (with the failed change), its base
    // the last successful write.
    let (_, last) = q.last_written();
    let mut surfaced = failed.clone();
    surfaced.source_text = last.expect("a write succeeded");
    q.set_base(surfaced.clone());
    q.save_eager(surfaced).unwrap();
    assert!(
        read(&path).contains("left_width_pct = 41"),
        "{}",
        read(&path)
    );
}

/// A project moved by hand and a new one added at its old path, while dux
/// removes the project: the new one is not taken for the removed one.
#[test]
fn a_project_moved_by_hand_and_a_new_one_at_its_old_path_are_told_apart() {
    let (_d, path, loaded, q) = setup("[[projects]]\nid = \"a\"\npath = \"/old\"\n");
    std::fs::write(
        &path,
        "[[projects]]\nid = \"a\"\npath = \"/new\"\n\n[[projects]]\npath = \"/old\"\n",
    )
    .unwrap();
    let mut m = loaded.clone();
    m.projects.clear();
    q.save_eager(m).unwrap();
    let t = read(&path);
    assert!(
        t.contains("\"/old\""),
        "the new project at the old path stays: {t}"
    );
}

/// The same with the new entry above the moved one: file order decides nothing.
#[test]
fn a_project_moved_by_hand_and_a_new_one_at_its_old_path_are_told_apart_in_either_order() {
    let (_d, path, loaded, q) = setup("[[projects]]\nid = \"a\"\npath = \"/old\"\n");
    std::fs::write(&path, "[[projects]]\npath = \"/old\"\nname = \"newone\"\n\n[[projects]]\nid = \"a\"\npath = \"/new\"\n").unwrap();
    let mut m = loaded.clone();
    m.projects.clear();
    q.save_eager(m).unwrap();
    let t = read(&path);
    assert!(
        t.contains("\"/old\""),
        "the new project at the old path stays: {t}"
    );
    assert!(
        !t.contains("id = \"a\""),
        "removed project a stays removed: {t}"
    );
}

/// A project with no env gets one by hand and one from dux: both variables
/// stay.
#[test]
fn a_project_env_started_by_hand_and_by_dux_keeps_both() {
    let (_d, path, loaded, q) = setup("[[projects]]\nid = \"a\"\npath = \"/a\"\n");
    std::fs::write(
        &path,
        "[[projects]]\nid = \"a\"\npath = \"/a\"\nenv = { B = \"2\" }\n",
    )
    .unwrap();
    let mut m = loaded.clone();
    m.projects[0].env.insert("C".into(), "3".into());
    q.save_eager(m).unwrap();
    let t = read(&path);
    assert!(t.contains("B = \"2\""), "hand-added B kept: {t}");
    assert!(t.contains("C = \"3\""), "{t}");
}

/// The same with the hand-written env as a subtable.
#[test]
fn a_project_env_subtable_started_by_hand_and_by_dux_keeps_both() {
    let (_d, path, loaded, q) = setup("[[projects]]\nid = \"a\"\npath = \"/a\"\n");
    std::fs::write(
        &path,
        "[[projects]]\nid = \"a\"\npath = \"/a\"\n# mine\n[projects.env]\nB = \"2\"\n",
    )
    .unwrap();
    let mut m = loaded.clone();
    m.projects[0].env.insert("C".into(), "3".into());
    q.save_eager(m).unwrap();
    let t = read(&path);
    assert!(t.contains("B = \"2\""), "hand-added B kept: {t}");
    assert!(t.contains("C = \"3\""), "{t}");
}

/// An inline global `env` keeps the variables added by hand and by dux, each
/// once.
#[test]
fn an_inline_global_env_keeps_hand_and_dux_variables_once_each() {
    let (_d, path, loaded, q) = setup("env = { A = \"1\" }\n");
    std::fs::write(&path, "env = { A = \"1\", B = \"2\" }\n").unwrap();
    let mut m = loaded.clone();
    m.env.insert("C".into(), "3".into());
    q.save_eager(m.clone()).unwrap();
    let t = read(&path);
    assert!(t.contains("B = \"2\"") && t.contains("C = \"3\""), "{t}");
    m.ui.copy_on_select = !m.ui.copy_on_select;
    q.save_eager(m).unwrap();
    let t = read(&path);
    assert!(t.contains("B = \"2\""), "{t}");
    assert!(t.contains("C = \"3\""), "{t}");
    assert_eq!(t.matches("A = \"1\"").count(), 1, "{t}");
}

/// A project env the user rewrote as a subtable, deleting one variable, keeps
/// that edit and later hand additions while dux changes another variable.
#[test]
fn a_project_env_rewritten_as_a_subtable_by_hand_keeps_its_edits() {
    let (_d, path, loaded, q) =
        setup("[[projects]]\nid = \"a\"\npath = \"/a\"\nenv = { A = \"1\", B = \"2\" }\n");
    std::fs::write(
        &path,
        "[[projects]]\nid = \"a\"\npath = \"/a\"\n[projects.env]\nB = \"2\"\n",
    )
    .unwrap();
    let mut m = loaded.clone();
    m.projects[0].env.insert("C".into(), "3".into());
    q.save_eager(m.clone()).unwrap();
    let t = read(&path).replace("B = \"2\"\n", "B = \"2\"\nD = \"4\"\n");
    std::fs::write(&path, &t).unwrap();
    m.projects[0].env.insert("C".into(), "33".into());
    q.save_eager(m.clone()).unwrap();
    m.ui.copy_on_select = !m.ui.copy_on_select;
    q.save_eager(m).unwrap();
    let t = read(&path);
    assert!(!t.contains("A = "), "{t}");
    assert!(t.contains("D = \"4\"") && t.contains("C = \"33\""), "{t}");
}

/// Two id-less hand projects at /x then dux removes the one it knew (base by
/// path).
#[test]
fn two_id_less_projects_at_one_path_both_survive_an_unrelated_save() {
    let (_d, path, loaded, q) = setup("[[projects]]\npath = \"/x\"\nname = \"one\"\n");
    let t = read(&path) + "\n[[projects]]\npath = \"/x\"\nname = \"two\"\n";
    std::fs::write(&path, &t).unwrap();
    let mut m = loaded.clone();
    m.ui.copy_on_select = !m.ui.copy_on_select;
    q.save_eager(m.clone()).unwrap();
    let t = read(&path);
    assert!(t.contains("\"one\"") && t.contains("\"two\""), "{t}");
}

/// Every top-level section written inline by hand (a dotted or inline
/// table before any header) is patched in its own form, and no save
/// panics the writer: later saves still work.
#[test]
fn odd_top_level_shapes_are_patched_and_never_panic_the_writer() {
    for text in [
        "ui = { left_width_pct = 25 }\n",
        "defaults = { provider = \"claude\" }\n",
        "macros = { hi = \"hello\" }\n",
        "server = { port = 3890 }\n",
        "providers = { claude = { command = \"claude\" } }\n",
        "keys = { }\n",
        "ui = 5\nenv = 3\nmacros = []\n",
        "projects = [{ id = \"a\", path = \"/a\" }]\n",
        "providers = 1\n",
    ] {
        let (_d, path, loaded, q) = setup(text);
        let mut m = loaded.clone();
        m.ui.left_width_pct = 31;
        m.env.insert("C".into(), "3".into());
        q.save_eager(m.clone())
            .unwrap_or_else(|e| panic!("{text}: {e:#}"));
        m.ui.copy_on_select = !m.ui.copy_on_select;
        q.save_eager(m).unwrap_or_else(|e| panic!("{text}: {e:#}"));
        let t = read(&path);
        assert!(t.contains("left_width_pct = 31"), "{text}:\n{t}");
        assert!(t.contains("C = \"3\""), "{text}:\n{t}");
        crate::config::config_from_text_as_written(&t)
            .unwrap_or_else(|e| panic!("{t}: {}", e.reason()));
    }
}

/// Inline sections and providers keep their form and every comment over four
/// saves.
#[test]
fn inline_sections_keep_their_form_and_comments_over_saves() {
    let text = "# top comment\nui = { left_width_pct = 25, copy_on_select = true } # trailing\nenv = { A = \"1\" } # envc\n\n[providers]\n# claude comment\nclaude = { command = \"claude\", args = [] } # cc\n\n# codex hdr\n[providers.codex]\ncommand = \"codex\" # cx\n";
    let (_d, path, loaded, q) = setup(text);
    let mut m = loaded.clone();
    for round in 0..4 {
        m.ui.left_width_pct = 30 + round;
        m.env.insert(format!("V{round}"), "x".into());
        if let Some(p) = m.providers.commands.get_mut("claude") {
            p.args = vec![format!("--r{round}")];
        }
        q.save_eager(m.clone()).unwrap();
        let t = read(&path);
        crate::config::config_from_text_as_written(&t).unwrap_or_else(|e| panic!("{:?}", e));
    }
    let t = read(&path);
    for c in [
        "# top comment",
        "# trailing",
        "# envc",
        "# claude comment",
        "# cc",
        "# codex hdr",
        "# cx",
    ] {
        assert!(t.contains(c), "lost {c}:\n{t}");
    }
    assert!(t.contains("ui = {"), "{t}");
    assert!(t.contains("claude = {"), "{t}");
}

/// Sections written as dotted keys at the top of the file stay readable over
/// saves.
#[test]
fn dotted_top_level_keys_round_trip() {
    let text = "ui.left_width_pct = 25\nproviders.claude.command = \"x\"\nenv.A = \"1\"\n";
    let (_d, path, loaded, q) = setup(text);
    let mut m = loaded.clone();
    for round in 0..3 {
        m.ui.left_width_pct = 30 + round;
        m.env.insert(format!("V{round}"), "x".into());
        q.save_eager(m.clone()).unwrap();
        let t = read(&path);
        let c =
            crate::config::config_from_text_as_written(&t).unwrap_or_else(|e| panic!("{:?}", e));
        assert_eq!(c.ui.left_width_pct, 30 + round);
    }
}

/// An inline `server` with an inline `auth` keeps the password and the bans
/// over saves.
#[test]
fn an_inline_server_auth_keeps_its_password_and_bans() {
    let hash = crate::auth::hash_password(&crate::auth::Password::new(
        "correct horse battery staple 99".to_string(),
    ))
    .unwrap();
    let text = format!(
        "server = {{ port = 3890, auth = {{ password_hash = \"{hash}\", blocked_addresses = [\"203.0.113.7\"] }} }}\n"
    );
    let (_d, path, loaded, q) = setup(&text);
    let mut m = loaded.clone();
    assert!(!m.server.auth.blocked_addresses.is_empty());
    for round in 0..3 {
        m.server.port = 4000 + round;
        m.ui.left_width_pct = 30 + round;
        q.save_eager(m.clone()).unwrap();
        let t = read(&path);
        assert!(t.contains(&hash), "password lost:\n{t}");
        assert!(t.contains("203.0.113.7"), "block lost:\n{t}");
    }
}

/// The full patch (no base) keeps a file of inline sections readable.
#[test]
#[allow(deprecated)] // the full patch, the path a sync-direct writer takes
fn the_full_patch_handles_inline_sections() {
    let text = "ui = { left_width_pct = 25 } # trailing\nproviders = { claude = { command = \"claude\" } }\nkeys = { quit = [\"q\"] }\nmacros = { hi = { text = \"hello\", surface = \"both\" } }\n";
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, text).unwrap();
    let mut c = crate::config::load_config_file(&path).unwrap();
    c.ui.left_width_pct = 40;
    for _ in 0..3 {
        crate::config_write::patch_config_file(&path, &c).unwrap();
        let t = read(&path);
        crate::config::config_from_text_as_written(&t).unwrap_or_else(|e| panic!("{:?}", e));
    }
}

/// Every shape a user can give a section dux writes is loaded (each wrong
/// value read as its default), saved without a panic, and left readable. A
/// `[server.auth]` of the wrong shape is the one exception: it is refused at
/// load, because dux never starts on an auth section it cannot read.
#[test]
fn section_shapes_a_user_can_write_never_panic_a_save() {
    for text in [
        "providers = { claude = 1 }\n",
        "[providers]\nclaude = 1\n",
        "[providers]\nclaude = [1]\n",
        "keys = 1\n",
        "macros = 1\n",
        "projects = 1\n",
        "[[projects]]\npath = \"/a\"\nenv = 1\n",
        "[[projects]]\npath = \"/a\"\n[projects.env]\nA = { x = 1 }\n",
        "env = { A = { B = 1 } }\n",
        "[providers.claude]\ncommand = \"c\"\nargs = 1\n",
        "ui = { a = { b = { c = 1 } } }\n",
        "[[providers]]\nx = 1\n",
        "[[ui]]\nx = 1\n",
        "[[env]]\nx = 1\n",
        "[[macros]]\nx = 1\n",
    ] {
        let (_dir, path, loaded, queue) = setup(text);
        let mut memory = loaded.clone();
        memory.ui.left_width_pct = 31;
        memory.env.insert("C".into(), "3".into());
        let saved = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            queue.save_eager(memory.clone())
        }));
        saved
            .unwrap_or_else(|_| panic!("a save of {text:?} panicked"))
            .unwrap_or_else(|error| panic!("a save of {text:?} failed: {error:#}"));
        let written = read(&path);
        crate::config::config_from_text_as_written(&written)
            .unwrap_or_else(|error| panic!("{text:?} saved as unreadable: {}", error.reason()));
    }
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "server = { auth = 1 }\n").unwrap();
    assert!(crate::config::load_config_file(&path).is_err());
}

/// Two projects sharing an id are removed one at a time, each removal taking
/// its own entry.
#[test]
fn projects_sharing_an_id_are_removed_one_at_a_time() {
    let (_d, path, loaded, q) = setup(
        "[[projects]]\nid = \"a\"\npath = \"/p\"\nname = \"one\"\n\n[[projects]]\nid = \"a\"\npath = \"/q\"\nname = \"two\"\n",
    );
    let mut m = loaded.clone();
    assert_eq!(m.projects.len(), 2, "both entries load");
    m.ui.copy_on_select = !m.ui.copy_on_select;
    q.save_eager(m.clone()).unwrap();
    m.projects.remove(1);
    q.save_eager(m.clone()).unwrap();
    assert!(
        project_rows(&read(&path))
            .iter()
            .all(|p| p.1.as_deref() != Some("/q")),
        "{}",
        read(&path)
    );
    m.projects.remove(0);
    q.save_eager(m.clone()).unwrap();
    assert!(project_rows(&read(&path)).is_empty(), "{}", read(&path));
}

#[test]
fn removing_one_of_two_id_less_projects_at_one_path_keeps_the_other() {
    let (_d, path, loaded, q) = setup(
        "[[projects]]\npath = \"/x\"\nname = \"one\"\n\n[[projects]]\npath = \"/x\"\nname = \"two\"\n",
    );
    let mut m = loaded.clone();
    assert_eq!(m.projects.len(), 2, "both entries load");
    m.projects.remove(0);
    q.save_eager(m.clone()).unwrap();
    let p = project_rows(&read(&path));
    assert_eq!(p.len(), 1, "{}", read(&path));
    assert_eq!(p[0].2.as_deref(), Some("two"), "{}", read(&path));
}

/// Projects reordered by hand stay reordered when dux renames one.
#[test]
fn a_hand_reorder_survives_dux_renaming_a_project() {
    let (_d, path, loaded, q) = setup(
        "[[projects]]\nid = \"a\"\npath = \"/a\"\nname = \"A\"\n\n[[projects]]\nid = \"b\"\npath = \"/b\"\nname = \"B\"\n",
    );
    std::fs::write(&path, "[[projects]]\nid = \"b\"\npath = \"/b\"\nname = \"B\"\n\n[[projects]]\nid = \"a\"\npath = \"/a\"\nname = \"A\"\n").unwrap();
    let mut m = loaded.clone();
    m.projects[0].name = Some("A2".into());
    q.save_eager(m.clone()).unwrap();
    let p = project_rows(&read(&path));
    assert_eq!(p.len(), 2);
    assert!(p.contains(&(Some("a".into()), Some("/a".into()), Some("A2".into()))));
}

/// Two projects whose paths the user swapped keep their ids, and dux then
/// removes the right one.
#[test]
fn projects_whose_paths_were_swapped_by_hand_keep_their_ids() {
    let (_d, path, loaded, q) = setup(
        "[[projects]]\nid = \"a\"\npath = \"/a\"\nname = \"A\"\n\n[[projects]]\nid = \"b\"\npath = \"/b\"\nname = \"B\"\n",
    );
    std::fs::write(&path, "[[projects]]\nid = \"a\"\npath = \"/b\"\nname = \"A\"\n\n[[projects]]\nid = \"b\"\npath = \"/a\"\nname = \"B\"\n").unwrap();
    let mut m = loaded.clone();
    m.ui.copy_on_select = !m.ui.copy_on_select;
    q.save_eager(m.clone()).unwrap();
    let p = project_rows(&read(&path));
    assert_eq!(p.len(), 2, "{}", read(&path));
    // and dux removes b
    m.projects.retain(|p| p.id != "b");
    q.save_eager(m.clone()).unwrap();
    let p = project_rows(&read(&path));
    assert_eq!(
        p,
        vec![(Some("a".into()), Some("/b".into()), Some("A".into()))],
        "{}",
        read(&path)
    );
}

/// Hand removes the id from an entry, dux removes that project.
#[test]
fn a_project_whose_id_was_removed_by_hand_is_still_removed_by_dux() {
    let (_d, path, loaded, q) = setup(
        "[[projects]]\nid = \"a\"\npath = \"/a\"\n\n[[projects]]\nid = \"b\"\npath = \"/b\"\n",
    );
    std::fs::write(
        &path,
        "[[projects]]\npath = \"/a\"\n\n[[projects]]\nid = \"b\"\npath = \"/b\"\n",
    )
    .unwrap();
    let mut m = loaded.clone();
    m.projects.retain(|p| p.id != "a");
    q.save_eager(m.clone()).unwrap();
    let p = project_rows(&read(&path));
    assert_eq!(p.len(), 1, "{}", read(&path));
}

#[test]
fn removing_the_second_of_two_projects_at_one_path_and_renaming_the_first_keeps_the_firsts_keys() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[[projects]]\npath = \"/x\"\nname = \"one\"\nnote = \"n1\"\n\n[[projects]]\npath = \"/x\"\nname = \"two\"\nnote = \"n2\"\n").unwrap();
    let loaded = crate::config::load_config_file(&path).unwrap();
    let q = ConfigWriteQueue::with_base(path.clone(), &loaded);
    let mut m = loaded.clone();
    m.projects.remove(1);
    q.save_eager(m.clone()).unwrap();
    let mut m2 = m.clone();
    m2.projects[0].name = Some("uno".into());
    q.save_eager(m2.clone()).unwrap();
    let t = read(&path);
    assert!(!t.contains("two") && !t.contains("n2"), "{t}");
    assert!(t.contains("uno") && t.contains("n1"), "{t}");
}

#[test]
fn removing_the_first_of_two_projects_at_one_path_keeps_the_seconds_keys() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[[projects]]\npath = \"/x\"\nname = \"one\"\nnote = \"n1\"\n\n[[projects]]\npath = \"/x\"\nname = \"two\"\nnote = \"n2\"\n").unwrap();
    let loaded = crate::config::load_config_file(&path).unwrap();
    let q = ConfigWriteQueue::with_base(path.clone(), &loaded);
    let mut m = loaded.clone();
    m.projects.remove(0);
    q.save_eager(m.clone()).unwrap();
    let t = read(&path);
    assert!(!t.contains("one") && !t.contains("n1"), "{t}");
    assert!(t.contains("two") && t.contains("n2"), "{t}");
}

#[test]
fn a_default_dux_filled_into_a_table_section_stays_deleted_by_hand() {
    let t = fill_then_hand_delete_diff_tab_width("[ui]\nleft_width_pct = 25\n");
    assert!(!t.contains("diff_tab_width"), "table form refilled:\n{t}");
}

#[test]
fn a_default_dux_filled_into_an_inline_section_stays_deleted_by_hand() {
    let t = fill_then_hand_delete_diff_tab_width("ui = { left_width_pct = 25 }\n");
    assert!(!t.contains("diff_tab_width"), "inline form refilled:\n{t}");
}

#[test]
fn a_project_field_dux_set_and_the_user_deleted_stays_deleted() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[[projects]]\nid = \"a\"\npath = \"/a\"\n").unwrap();
    let loaded = crate::config::load_config_file(&path).unwrap();
    let q = ConfigWriteQueue::with_base(path.clone(), &loaded);
    let mut m = loaded.clone();
    m.projects[0].startup_command = Some("make setup".into());
    q.save_eager(m.clone()).unwrap();
    let t = read(&path);
    assert!(t.contains("make setup"), "{t}");
    std::fs::write(&path, t.replace("startup_command = \"make setup\"\n", "")).unwrap();
    m.ui.copy_on_select = !m.ui.copy_on_select;
    q.save_eager(m.clone()).unwrap();
    let t = read(&path);
    assert!(
        !t.contains("make setup"),
        "hand-deleted project field came back:\n{t}"
    );
    // A third save keeps it deleted too.
    m.ui.copy_on_select = !m.ui.copy_on_select;
    q.save_eager(m.clone()).unwrap();
    let t = read(&path);
    assert!(
        !t.contains("make setup"),
        "hand-deleted project field came back:\n{t}"
    );
}

#[test]
fn a_project_env_variable_dux_set_and_the_user_deleted_stays_deleted() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[[projects]]\nid = \"a\"\npath = \"/a\"\nenv = { A = \"1\" }\n",
    )
    .unwrap();
    let loaded = crate::config::load_config_file(&path).unwrap();
    let q = ConfigWriteQueue::with_base(path.clone(), &loaded);
    let mut m = loaded.clone();
    m.projects[0].env.insert("B".into(), "2".into());
    q.save_eager(m.clone()).unwrap();
    let t = read(&path);
    assert!(t.contains("B = \"2\""), "{t}");
    std::fs::write(&path, t.replace(", B = \"2\"", "")).unwrap();
    m.ui.copy_on_select = !m.ui.copy_on_select;
    q.save_eager(m.clone()).unwrap();
    let t = read(&path);
    assert!(
        !t.contains("B = \"2\""),
        "hand-deleted env var came back:\n{t}"
    );
    // A third save keeps it deleted too.
    m.ui.copy_on_select = !m.ui.copy_on_select;
    q.save_eager(m.clone()).unwrap();
    let t = read(&path);
    assert!(
        !t.contains("B = \"2\""),
        "hand-deleted env var came back:\n{t}"
    );
}

#[test]
fn a_key_dux_filled_into_an_inline_provider_stays_deleted_by_hand() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[providers]\nclaude = { command = \"claude\" }\n").unwrap();
    let loaded = crate::config::load_config_file(&path).unwrap();
    let q = ConfigWriteQueue::with_base(path.clone(), &loaded);
    let mut m = loaded.clone();
    m.ui.copy_on_select = !m.ui.copy_on_select;
    q.save_eager(m.clone()).unwrap();
    let t = read(&path);
    assert!(t.contains("resume_args = [\"--continue\"], "), "{t}");
    std::fs::write(&path, t.replace("resume_args = [\"--continue\"], ", "")).unwrap();
    m.ui.copy_on_select = !m.ui.copy_on_select;
    q.save_eager(m.clone()).unwrap();
    let t = read(&path);
    assert!(
        t.lines()
            .any(|l| l.starts_with("claude = {") && !l.contains("resume_args")),
        "hand-deleted inline provider key came back:\n{t}"
    );
    // A third save keeps it deleted too.
    m.ui.copy_on_select = !m.ui.copy_on_select;
    q.save_eager(m.clone()).unwrap();
    let t = read(&path);
    assert!(
        t.lines()
            .any(|l| l.starts_with("claude = {") && !l.contains("resume_args")),
        "hand-deleted inline provider key came back:\n{t}"
    );
}
