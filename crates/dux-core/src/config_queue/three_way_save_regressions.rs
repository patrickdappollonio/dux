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

// ---------------------------------------------------------------------------
// Arrays of tables dux does not own, and project entries told apart by what
// dux manages in them
// ---------------------------------------------------------------------------

/// Arrays of tables dux does not write (a hand-added `[[extra]]`, one nested
/// in a section dux writes, one under a provider) keep exactly their entries
/// over six saves and a reload: an entry with nothing that identifies it is
/// never written twice.
#[test]
fn arrays_of_tables_dux_does_not_own_keep_their_count_over_saves_and_a_reload() {
    let text = "[[extra]]\na = 1\n\n[[extra]]\nid = \"q\"\nb = 2\n\n[ui]\nleft_width_pct = 25\n\n[[ui.notes]]\ntext = \"hi\"\n\n[providers.claude]\ncommand = \"claude\"\n\n[[providers.claude.hooks]]\nrun = \"x\"\n";
    let counts = |text: &str| {
        (
            text.matches("[[extra]]").count(),
            text.matches("[[ui.notes]]").count(),
            text.matches("[[providers.claude.hooks]]").count(),
        )
    };
    let (_dir, path, loaded, queue) = setup(text);
    let mut memory = loaded.clone();
    for round in 0..6 {
        memory.env.insert(format!("R{round}"), "1".into());
        queue.save_eager(memory.clone()).unwrap();
        let written = read(&path);
        assert_eq!(counts(&written), (2, 1, 1), "save {round}:\n{written}");
    }
    let mut reloaded = load(&path);
    let queue = queue_for(&path);
    for written in three_unrelated_saves(&path, &queue, &mut reloaded) {
        assert_eq!(counts(&written), (2, 1, 1), "after a reload:\n{written}");
    }
}

/// A memory change to an array of tables dux does not own is written as a
/// whole; a hand edit to it while memory has not changed it is kept.
#[test]
fn an_array_of_tables_dux_does_not_own_is_merged_as_one_value() {
    let (_dir, path, loaded, queue) = setup("[[extra]]\na = 1\n");
    std::fs::write(&path, "[[extra]]\na = 1\n\n[[extra]]\na = 2\n").unwrap();
    let mut memory = loaded.clone();
    for written in three_unrelated_saves(&path, &queue, &mut memory) {
        assert_eq!(written.matches("[[extra]]").count(), 2, "{written}");
        assert!(written.contains("a = 2"), "{written}");
    }
}

const TWO_PROJECTS_WITH_THEIR_OWN_SETTINGS: [&str; 2] = [
    "[[projects]]\npath = \"/x\"\nname = \"api\"\ndefault_provider = \"claude\"\nnote = \"n1\"\n\n[[projects]]\npath = \"/x\"\nname = \"api\"\ndefault_provider = \"codex\"\nnote = \"n2\"\n",
    "[[projects]]\npath = \"/x\"\ndefault_provider = \"claude\"\nnote = \"n1\"\n\n[[projects]]\npath = \"/x\"\ndefault_provider = \"codex\"\nnote = \"n2\"\n",
];

/// Removing either of two id-less projects at one path (with the same name,
/// or with none) leaves the survivor with its own settings and its own keys,
/// never the removed one's.
#[test]
fn removing_one_of_two_projects_at_one_path_leaves_the_survivor_its_own_settings() {
    for text in TWO_PROJECTS_WITH_THEIR_OWN_SETTINGS {
        for (removed, kept_provider, kept_note) in [(0, "codex", "n2"), (1, "claude", "n1")] {
            let (_dir, path, loaded, queue) = setup(text);
            let mut memory = loaded.clone();
            memory.projects.remove(removed);
            queue.save_eager(memory.clone()).unwrap();
            for written in std::iter::once(read(&path)).chain(three_unrelated_saves(
                &path,
                &queue,
                &mut memory,
            )) {
                let doc: toml_edit::DocumentMut = written.parse().unwrap();
                let projects = doc["projects"].as_array_of_tables().unwrap();
                assert_eq!(projects.len(), 1, "{written}");
                let survivor = projects.get(0).unwrap();
                let field = |name: &str| survivor.get(name).and_then(|value| value.as_str());
                assert_eq!(field("default_provider"), Some(kept_provider), "{written}");
                assert_eq!(field("note"), Some(kept_note), "{written}");
            }
        }
    }
}

/// A change dux makes to the second of two projects at one path lands on
/// the second entry, which keeps its own keys.
#[test]
fn a_change_to_the_second_of_two_projects_at_one_path_lands_on_it() {
    let (_dir, path, loaded, queue) = setup(TWO_PROJECTS_WITH_THEIR_OWN_SETTINGS[0]);
    let mut memory = loaded.clone();
    memory.projects[1].startup_command = Some("make".into());
    queue.save_eager(memory.clone()).unwrap();
    for text in
        std::iter::once(read(&path)).chain(three_unrelated_saves(&path, &queue, &mut memory))
    {
        let doc: toml_edit::DocumentMut = text.parse().unwrap();
        let projects = doc["projects"].as_array_of_tables().unwrap();
        assert_eq!(projects.len(), 2, "{text}");
        let second = projects.get(1).unwrap();
        assert_eq!(
            second.get("note").and_then(|v| v.as_str()),
            Some("n2"),
            "{text}"
        );
        assert_eq!(
            second.get("startup_command").and_then(|v| v.as_str()),
            Some("make"),
            "{text}"
        );
        assert!(
            projects.get(0).unwrap().get("startup_command").is_none(),
            "{text}"
        );
    }
}

/// A user key edited by hand stays edited while dux renames the project.
#[test]
fn a_hand_edited_user_key_survives_dux_renaming_its_project() {
    let (_dir, path, loaded, queue) = setup(
        "[[projects]]\npath = \"/x\"\nname = \"one\"\nnote = \"n1\"\n\n[[projects]]\npath = \"/y\"\nname = \"two\"\nnote = \"n2\"\n",
    );
    let mut memory = loaded.clone();
    std::fs::write(
        &path,
        read(&path).replace("note = \"n1\"", "note = \"edited\""),
    )
    .unwrap();
    memory.projects[0].name = Some("uno".into());
    queue.save_eager(memory.clone()).unwrap();
    for text in
        std::iter::once(read(&path)).chain(three_unrelated_saves(&path, &queue, &mut memory))
    {
        assert_eq!(
            names_and_notes(&text),
            vec![(some("uno"), some("edited")), (some("two"), some("n2"))],
            "{text}"
        );
    }
}

/// An id added by hand to one of two projects at one path does not move
/// keys between them when dux renames the other.
#[test]
fn an_id_added_by_hand_does_not_move_keys_between_projects_at_one_path() {
    let (_dir, path, loaded, queue) = setup(TWO_PROJECTS_AT_ONE_PATH);
    let mut memory = loaded.clone();
    std::fs::write(
        &path,
        read(&path).replace("name = \"two\"", "id = \"handid\"\nname = \"two\""),
    )
    .unwrap();
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

/// Two projects at one path reordered by hand keep their own keys when dux
/// renames one.
#[test]
fn projects_at_one_path_reordered_by_hand_keep_their_keys_through_a_rename() {
    let (_dir, path, loaded, queue) = setup(TWO_PROJECTS_AT_ONE_PATH);
    let mut memory = loaded.clone();
    std::fs::write(
        &path,
        "[[projects]]\npath = \"/x\"\nname = \"two\"\nnote = \"n2\"\n\n[[projects]]\npath = \"/x\"\nname = \"one\"\nnote = \"n1\"\n",
    )
    .unwrap();
    memory.projects[0].name = Some("two".into());
    queue.save_eager(memory.clone()).unwrap();
    for text in
        std::iter::once(read(&path)).chain(three_unrelated_saves(&path, &queue, &mut memory))
    {
        let mut rows = names_and_notes(&text);
        rows.sort();
        assert_eq!(
            rows,
            vec![(some("two"), some("n1")), (some("two"), some("n2"))],
            "{text}"
        );
    }
}

/// A section flipped from inline to a table and back by hand, each time
/// leaving a key out, keeps those keys out over saves and a reload.
#[test]
fn a_section_flipped_between_inline_and_table_by_hand_keeps_its_deletions() {
    let (_dir, path, loaded, queue) = setup("ui = { left_width_pct = 25 }\n");
    let mut memory = loaded.clone();
    memory.env.insert("B".into(), "2".into());
    queue.save_eager(memory.clone()).unwrap();
    let written = read(&path);
    let line = written
        .lines()
        .find(|line| line.starts_with("ui = {"))
        .unwrap()
        .to_string();
    let inline: toml_edit::DocumentMut = line.parse().unwrap();
    let mut ui = inline["ui"].as_inline_table().unwrap().clone().into_table();
    ui.remove("diff_tab_width");
    let mut table = toml_edit::DocumentMut::new();
    table["ui"] = toml_edit::Item::Table(ui);
    std::fs::write(
        &path,
        written.replace(&format!("{line}\n"), "") + "\n" + &table.to_string(),
    )
    .unwrap();
    memory.ui.left_width_pct = 31;
    queue.save_eager(memory.clone()).unwrap();
    let text = read(&path);
    assert!(
        text.contains("left_width_pct = 31") && !text.contains("diff_tab_width"),
        "{text}"
    );
    // Back to inline, this time also leaving `theme` out.
    let mut doc: toml_edit::DocumentMut = text.parse().unwrap();
    let mut ui = doc["ui"].as_table().unwrap().clone();
    ui.remove("theme");
    doc.remove("ui");
    std::fs::write(&path, format!("ui = {}\n{}", ui.into_inline_table(), doc)).unwrap();
    memory.ui.left_width_pct = 32;
    queue.save_eager(memory.clone()).unwrap();
    for text in
        std::iter::once(read(&path)).chain(three_unrelated_saves(&path, &queue, &mut memory))
    {
        assert!(text.contains("left_width_pct = 32"), "{text}");
        assert!(!text.contains("diff_tab_width"), "{text}");
        assert!(!text.contains("theme ="), "{text}");
    }
    let mut reloaded = load(&path);
    let queue = queue_for(&path);
    for text in three_unrelated_saves(&path, &queue, &mut reloaded) {
        crate::config::config_from_text_as_written(&text)
            .unwrap_or_else(|error| panic!("{text}: {}", error.reason()));
    }
}

/// Projects written as an inline array keep their count while dux renames
/// one and then removes another.
#[test]
fn projects_written_as_an_inline_array_keep_their_count() {
    let (_dir, path, loaded, queue) =
        setup("projects = [ { path = \"/x\", name = \"a\", note = \"n\" }, { path = \"/y\" } ]\n");
    let mut memory = loaded.clone();
    assert_eq!(memory.projects.len(), 2);
    memory.projects[0].name = Some("b".into());
    queue.save_eager(memory.clone()).unwrap();
    for text in
        std::iter::once(read(&path)).chain(three_unrelated_saves(&path, &queue, &mut memory))
    {
        let config = crate::config::config_from_text_as_written(&text)
            .unwrap_or_else(|error| panic!("{text}: {}", error.reason()));
        assert_eq!(config.projects.len(), 2, "{text}");
    }
    memory.projects.remove(1);
    queue.save_eager(memory.clone()).unwrap();
    let text = read(&path);
    let config = crate::config::config_from_text_as_written(&text)
        .unwrap_or_else(|error| panic!("{text}: {}", error.reason()));
    assert_eq!(config.projects.len(), 1, "{text}");
}

// ---------------------------------------------------------------------------
// What dux has seen of the file, over many saves
// ---------------------------------------------------------------------------

/// The text the writer keeps of what it has seen of the file.
fn seen_text(queue: &ConfigWriteQueue) -> String {
    queue
        .last_written()
        .1
        .and_then(|source| source.as_str().map(str::to_string))
        .unwrap_or_default()
}

const ARRAYS_OF_TABLES_BESIDE_SETTINGS: [&str; 12] = [
    "",
    "[[extra]]\na = 1\n",
    "[[ui.notes]]\ntext = \"hi\"\n",
    "[providers.claude]\ncommand = \"claude\"\n\n[[providers.claude.hooks]]\nrun = \"x\"\n",
    "[[projects]]\nid = \"p1\"\npath = \"/a\"\n\n[[projects.hooks]]\nrun = \"h1\"\n",
    "[[extra]]\na = 1\n\n[[projects]]\nid = \"p1\"\npath = \"/a\"\n\n[[projects.hooks]]\nrun = \"h1\"\n",
    "[[extra]]\na = 1\n\n[[ui2]]\nx = 1\n",
    "[[extra]]\na = 1\n\n[[extra.sub]]\nx = 1\n",
    "[providers.claude]\ncommand = \"claude\"\n\n[[providers.claude.hooks]]\nrun = \"x\"\n\n[[projects]]\nid = \"p1\"\npath = \"/a\"\n",
    "[[a]]\nx = 1\n\n[[b]]\ny = 1\n\n[[b.c]]\nz = 1\n",
    "[[a]]\nx = 1\n[[a.c]]\nz = 1\n\n[[a]]\nx = 2\n",
    "[[projects]]\nid = \"p1\"\npath = \"/a\"\n\n[[projects.hooks]]\nrun = \"h1\"\n\n[[projects]]\nid = \"p2\"\npath = \"/b\"\n",
];

/// A setting dux filled in and the user deleted stays deleted over eight
/// saves, whatever arrays of tables (top-level, nested, under a provider,
/// under a project) sit in the file beside it, and what dux has seen of the
/// file always parses.
#[test]
fn a_hand_deletion_stays_deleted_beside_any_array_of_tables() {
    for extra in ARRAYS_OF_TABLES_BESIDE_SETTINGS {
        let (_dir, path, loaded, queue) = setup(&format!("[ui]\nleft_width_pct = 25\n\n{extra}"));
        let mut memory = loaded.clone();
        memory.env.insert("Z".into(), "1".into());
        queue.save_eager(memory.clone()).unwrap();
        let text = read(&path);
        assert!(text.contains("diff_tab_width = 4\n"), "{text}");
        std::fs::write(&path, text.replace("diff_tab_width = 4\n", "")).unwrap();
        for round in 0..8 {
            memory.env.insert(format!("R{round}"), "1".into());
            queue.save_eager(memory.clone()).unwrap();
            let written = read(&path);
            assert!(
                !written.contains("diff_tab_width"),
                "save {round} beside {extra:?}:\n{written}"
            );
            let seen = seen_text(&queue);
            assert!(
                seen.parse::<toml_edit::DocumentMut>().is_ok(),
                "save {round} beside {extra:?} left what dux has seen unreadable:\n{seen}"
            );
        }
    }
}

/// What dux has seen of the file stays the size of the file over 300
/// saves, with arrays of tables dux does not own beside `[[projects]]`
/// entries carrying nested arrays of their own.
#[test]
fn what_dux_has_seen_stays_bounded_over_many_saves() {
    for text in [
        "[[extra]]\na = 1\n\n[[extra]]\na = 2\n",
        "[[extra]]\na = 1\n\n[[extra]]\na = 2\n\n[providers.x]\ncommand = \"x\"\n\n[[providers.x.hooks]]\nrun = \"p\"\n\n[[projects]]\nid = \"p1\"\npath = \"/a\"\n\n[[projects.hooks]]\nrun = \"h1\"\n",
    ] {
        let (_dir, path, loaded, queue) = setup(text);
        let mut memory = loaded.clone();
        let mut first = None;
        for round in 0..300u16 {
            memory.ui.left_width_pct = 20 + round % 2;
            queue.save_eager(memory.clone()).unwrap();
            let seen = seen_text(&queue);
            assert!(
                seen.parse::<toml_edit::DocumentMut>().is_ok(),
                "save {round}: what dux has seen does not parse:\n{seen}"
            );
            let first = *first.get_or_insert(seen.len());
            assert!(
                seen.len() <= first * 2,
                "save {round}: seen grew from {first} to {} bytes (file {} bytes)",
                seen.len(),
                read(&path).len()
            );
        }
        let written = read(&path);
        assert_eq!(written.matches("[[extra]]").count(), 2, "{written}");
    }
}

/// Macros, providers and keys keep what the user added by hand and every
/// comment, once each, over twenty saves of dux adding, reordering and
/// removing macros, changing a provider and adding keys, and a reload.
#[test]
fn macros_providers_and_keys_keep_hand_additions_and_comments_over_many_saves() {
    use crate::config::{MacroEntry, MacroSurface};
    let text = "[macros]\nfirst = { text = \"1\", surface = \"agent\" } # c1\n# above second\n[macros.second]\ntext = \"2\"\nsurface = \"both\"\n\n[providers.claude]\ncommand = \"claude\"\nargs = [\"--x\"]\nmine = 1\n\n[keys]\nzeta = [\"ctrl-z\"]\nalpha = [\"ctrl-a\"]\n";
    let (_dir, path, loaded, mut queue) = setup(text);
    let mut memory = loaded.clone();
    std::fs::write(
        &path,
        read(&path)
            + "\n[macros.hand]\ntext = \"h\"\nsurface = \"agent\"\n\n[providers.handp]\ncommand = \"hp\"\n",
    )
    .unwrap();
    for round in 0..20 {
        match round % 4 {
            0 => {
                memory.macros.entries.insert(
                    format!("m{round}"),
                    MacroEntry {
                        text: "x".into(),
                        surface: MacroSurface::Agent,
                    },
                );
            }
            1 => memory.macros.entries.reverse(),
            2 => {
                memory
                    .providers
                    .commands
                    .get_mut("claude")
                    .expect("claude")
                    .args
                    .push(format!("--r{round}"));
            }
            _ => {
                memory
                    .keys
                    .bindings
                    .insert(format!("k{round}"), vec!["ctrl-k".into()]);
                memory
                    .macros
                    .entries
                    .shift_remove(&format!("m{}", round - 3));
            }
        }
        queue.save_eager(memory.clone()).unwrap();
        let written = read(&path);
        let config = crate::config::config_from_text_as_written(&written)
            .unwrap_or_else(|error| panic!("{round}: {}\n{written}", error.reason()));
        for (needle, what) in [
            ("text = \"h\"", "the hand macro"),
            ("handp", "the hand provider"),
            ("# c1", "the inline macro's comment"),
            ("# above second", "the comment above a macro"),
            ("mine = 1", "the provider's own key"),
            ("zeta", "a key binding"),
        ] {
            assert_eq!(
                written.matches(needle).count(),
                1,
                "{what}, save {round}:\n{written}"
            );
        }
        // Within each form (inline entries, `[macros.<name>]` sections),
        // the file reads back in memory's order, the hand macro aside.
        let _ = &config;
        assert_order_within_each_form(&memory, &written, &["hand"]);
        if round == 9 {
            drop(queue);
            memory = load(&path);
            queue = queue_for(&path);
        }
    }
}

/// What dux has seen of the file, if it ever cannot be read back, fails
/// safe: every setting counts as seen, so a deletion is never undone by
/// filling the setting back in. And a union over such a text keeps it as it
/// is, so the next save fails safe too.
#[test]
fn an_unreadable_record_of_what_dux_has_seen_fills_nothing_in() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[ui]\nleft_width_pct = 25\n").unwrap();
    let loaded = load(&path);
    let unreadable = "[[a.b]]\nx = 1\n[a]\ny = 2\n[a]\n";
    assert!(unreadable.parse::<toml_edit::DocumentMut>().is_err());
    let mut memory = loaded.clone();
    memory.env.insert("Z".into(), "1".into());
    crate::config_write::patch_config_file_three_way(
        &path,
        Some(crate::config_write::SaveBase {
            config: &loaded,
            seen: Some(unreadable),
        }),
        &memory,
        crate::config_write::Durability::NoFsync,
    )
    .unwrap();
    let written = read(&path);
    assert!(written.contains("Z = \"1\""), "{written}");
    assert!(!written.contains("diff_tab_width"), "{written}");
    assert_eq!(
        crate::config_write::union_seen(Some(unreadable), &written, &[]),
        unreadable
    );
}

/// The macros `text` reads back, in the order a load reads them.
fn macro_order(text: &str) -> Vec<String> {
    crate::config::config_from_text_as_written(text)
        .unwrap_or_else(|error| panic!("{}\n{text}", error.reason()))
        .macros
        .entries
        .keys()
        .cloned()
        .collect()
}

/// The macros of `text` written as `[macros.<name>]` sections.
fn macro_sections(text: &str) -> Vec<String> {
    let doc: toml_edit::DocumentMut = text.parse().unwrap();
    doc.get("macros")
        .and_then(toml_edit::Item::as_table)
        .map(|macros| {
            macros
                .iter()
                .filter(|(_, item)| item.as_table().is_some_and(|table| !table.is_dotted()))
                .map(|(name, _)| name.to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Within each form a macro can be written in, `text` reads its macros back
/// in `memory`'s order (macros named in `skip` aside): inline entries
/// always print before sections, which is the one order TOML cannot show.
fn assert_order_within_each_form(memory: &Config, text: &str, skip: &[&str]) {
    let sections = macro_sections(text);
    let read_back = macro_order(text);
    for section_form in [false, true] {
        let pick = |names: Vec<String>| -> Vec<String> {
            names
                .into_iter()
                .filter(|name| !skip.contains(&name.as_str()))
                .filter(|name| sections.contains(name) == section_form)
                .collect()
        };
        let wanted = pick(memory.macros.entries.keys().cloned().collect());
        let got = pick(read_back.clone());
        assert_eq!(got, wanted, "sections: {section_form}\n{text}");
    }
}

const MIXED_MACROS: &str = "[macros]\na = { text = \"1\", surface = \"agent\" }\nb = { text = \"2\", surface = \"agent\" }\n\n[macros.c]\ntext = \"3\"\nsurface = \"agent\"\n";

/// The user reorders a mixed `[macros]` by hand while dux has not
/// reordered anything: an unrelated save keeps the user's order and the
/// file's macros exactly as written.
#[test]
fn a_hand_reorder_of_macros_is_kept_with_no_change_of_form() {
    let (_dir, path, loaded, queue) = setup(MIXED_MACROS);
    let mut memory = loaded.clone();
    let reordered = "[macros]\nb = { text = \"2\", surface = \"agent\" }\na = { text = \"1\", surface = \"agent\" }\n\n[macros.c]\ntext = \"3\"\nsurface = \"agent\"\n";
    std::fs::write(&path, reordered).unwrap();
    for written in three_unrelated_saves(&path, &queue, &mut memory) {
        assert_eq!(macro_order(&written), vec!["b", "a", "c"], "{written}");
        assert!(written.contains(reordered), "{written}");
    }
}

/// Macro shapes a load reads in an order of their own (a section above the
/// `[macros]` header, dotted keys) are left exactly as written by saves
/// that do not reorder macros.
#[test]
fn unrelated_saves_leave_macro_shapes_exactly_as_written() {
    // Each case: the file, and the pieces of it that must read exactly as
    // written after every save (a first save fills other sections in
    // between, so the pieces, not the whole, stay contiguous).
    for (macros, pieces) in [
        (
            "[macros.b]\ntext = \"2\"\nsurface = \"agent\"\n\n[macros]\na = { text = \"1\", surface = \"agent\" }\n",
            vec![
                "[macros.b]\ntext = \"2\"\nsurface = \"agent\"\n",
                "[macros]\na = { text = \"1\", surface = \"agent\" }\n",
            ],
        ),
        (
            "[macros]\na.text = \"1\"\na.surface = \"agent\"\nb = { text = \"2\", surface = \"agent\" }\n",
            vec![
                "[macros]\na.text = \"1\"\na.surface = \"agent\"\nb = { text = \"2\", surface = \"agent\" }\n",
            ],
        ),
    ] {
        let (_dir, path, loaded, queue) = setup(macros);
        let mut memory = loaded.clone();
        let order = macro_order(macros);
        for written in three_unrelated_saves(&path, &queue, &mut memory) {
            for piece in &pieces {
                assert!(written.contains(piece), "{piece:?} changed:\n{written}");
            }
            assert_eq!(macro_order(&written), order, "{written}");
        }
    }
}

/// A reorder in dux keeps every comment on the macros: above a section,
/// trailing its header, and trailing a value inside it.
#[test]
fn reordering_macros_keeps_every_comment_and_form() {
    let text = "[macros]\na = { text = \"1\", surface = \"agent\" } # inline note\n\n# above c\n[macros.c] # header note\ntext = \"3\" # value note\nsurface = \"agent\"\n";
    let (_dir, path, loaded, queue) = setup(text);
    let mut memory = loaded.clone();
    memory.macros.entries.reverse();
    queue.save_eager(memory.clone()).unwrap();
    let written = read(&path);
    for comment in [
        "# inline note",
        "# above c",
        "# header note",
        "# value note",
    ] {
        assert_eq!(written.matches(comment).count(), 1, "{comment}:\n{written}");
    }
    assert!(written.contains("[macros.c]"), "{written}");
    assert_order_within_each_form(&memory, &written, &[]);
}

/// Macros written as sections, with or without a `[macros]` header of
/// their own, take memory's order through where their sections sit.
#[test]
fn section_macros_take_a_reorder_made_in_dux() {
    for text in [
        "[macros.a]\ntext = \"1\"\nsurface = \"agent\"\n\n[macros.b]\ntext = \"2\"\nsurface = \"agent\"\n",
        "[macros]\n\n[macros.a]\ntext = \"1\"\nsurface = \"agent\"\n\n[macros.b]\ntext = \"2\"\nsurface = \"agent\"\n\n[ui]\nleft_width_pct = 30\n",
    ] {
        let (_dir, path, loaded, queue) = setup(text);
        let mut memory = loaded.clone();
        memory.macros.entries.reverse();
        queue.save_eager(memory.clone()).unwrap();
        for written in
            std::iter::once(read(&path)).chain(three_unrelated_saves(&path, &queue, &mut memory))
        {
            assert_eq!(macro_order(&written), vec!["b", "a"], "{written}");
            assert!(
                written.contains("[macros.a]") && written.contains("[macros.b]"),
                "{written}"
            );
        }
    }
}

/// A reorder in dux of a mixed `[macros]` puts each form in memory's order
/// (here every macro reads back in memory's order, because the order needs
/// no section ahead of an inline entry).
#[test]
fn a_reorder_in_dux_of_mixed_macros_orders_each_form() {
    let text = "[macros]\na = { text = \"1\", surface = \"agent\" }\n\n[macros.b]\ntext = \"2\"\nsurface = \"agent\"\n\n[macros.c]\ntext = \"3\"\nsurface = \"agent\"\n";
    let (_dir, path, loaded, queue) = setup(text);
    let mut memory = loaded.clone();
    let c = memory.macros.entries.shift_remove("c").unwrap();
    memory.macros.entries.shift_insert(1, "c".into(), c);
    queue.save_eager(memory.clone()).unwrap();
    let written = read(&path);
    assert_eq!(macro_order(&written), vec!["a", "c", "b"], "{written}");
    let mut sections = macro_sections(&written);
    sections.sort();
    assert_eq!(sections, vec!["b", "c"], "the forms stay:\n{written}");
}

/// Hand-added section macros and reorders in dux over forty saves and
/// reloads: every hand macro and the comment above a section stay, once.
#[test]
fn macros_stay_stable_over_many_saves_with_hand_additions_and_reorders() {
    let text = "[macros]\na = { text = \"1\", surface = \"agent\" }\n# about b\n[macros.b]\ntext = \"2\"\nsurface = \"agent\"\n";
    let (_dir, path, loaded, mut queue) = setup(text);
    let mut memory = loaded.clone();
    for round in 0..40 {
        if round % 7 == 3 {
            std::fs::write(
                &path,
                read(&path) + &format!("\n[macros.h{round}]\ntext = \"h\"\nsurface = \"agent\"\n"),
            )
            .unwrap();
        }
        if round % 5 == 1 {
            memory.macros.entries.reverse();
        }
        memory.env.insert(format!("E{round}"), "1".into());
        queue.save_eager(memory.clone()).unwrap();
        let written = read(&path);
        assert_eq!(
            written.matches("# about b").count(),
            1,
            "{round}\n{written}"
        );
        let read_back = macro_order(&written);
        for hand in (0..=round).filter(|r| r % 7 == 3) {
            let name = format!("h{hand}");
            assert_eq!(
                read_back
                    .iter()
                    .filter(|macro_name| **macro_name == name)
                    .count(),
                1,
                "{round}\n{written}"
            );
        }
        if round % 10 == 9 {
            drop(queue);
            memory = load(&path);
            queue = queue_for(&path);
        }
    }
}

/// A setting deleted by hand stays deleted while dux reorders macros and
/// saves again.
#[test]
fn a_hand_deletion_survives_a_macro_reorder_and_later_saves() {
    let text = "[ui]\nleft_width_pct = 25\nright_width_pct = 30\n[macros]\na = { text = \"1\", surface = \"agent\" }\n[macros.b]\ntext = \"2\"\nsurface = \"agent\"\n";
    let (_dir, path, loaded, queue) = setup(text);
    let mut memory = loaded.clone();
    std::fs::write(&path, read(&path).replace("right_width_pct = 30\n", "")).unwrap();
    memory.macros.entries.reverse();
    queue.save_eager(memory.clone()).unwrap();
    for written in three_unrelated_saves(&path, &queue, &mut memory) {
        assert!(!written.contains("right_width_pct"), "{written}");
    }
}

/// What dux has seen stays bounded while memory replaces its one project
/// on every save: a project neither memory nor the file has any more is
/// forgotten.
#[test]
fn what_dux_has_seen_stays_bounded_while_projects_come_and_go() {
    let (_dir, _path, loaded, queue) = setup("[ui]\nleft_width_pct = 25\n");
    let mut memory = loaded.clone();
    let mut first = None;
    for round in 0..300 {
        memory.projects = vec![project(
            &format!("id{round}"),
            &format!("/p/{round}"),
            &format!("n{round}"),
        )];
        queue.save_eager(memory.clone()).unwrap();
        let seen = seen_text(&queue).len();
        let first = *first.get_or_insert(seen);
        assert!(
            seen <= first * 2,
            "save {round}: seen grew from {first} to {seen}"
        );
    }
}

/// A setting a save filled in, deleted by hand and then reloaded (the way
/// a hand edit takes effect while dux runs), stays deleted: a reload adds
/// what it read to what dux has seen, never replacing it.
#[test]
fn a_hand_deletion_survives_a_reload() {
    let (_dir, path, loaded, queue) = setup("[ui]\nleft_width_pct = 25\n");
    let mut memory = loaded.clone();
    memory.env.insert("A".into(), "1".into());
    queue.save_eager(memory.clone()).unwrap();
    let text = read(&path);
    assert!(text.contains("diff_tab_width = 4\n"), "filled in:\n{text}");
    std::fs::write(&path, text.replace("diff_tab_width = 4\n", "")).unwrap();
    let reloaded = load(&path);
    queue.set_base(reloaded.clone());
    let mut memory = reloaded;
    for written in three_unrelated_saves(&path, &queue, &mut memory) {
        assert!(!written.contains("diff_tab_width"), "{written}");
    }
}

/// A randomized model of hand edits, memory changes, saves and reloads,
/// checked against the three-way rules after every step, plus the dotted-key
/// shapes it found worth pinning.
mod randomized_model {
    use super::*;

    /// Seeds per randomized model. Each run is a few disk saves per step, so
    /// this is kept small enough to add only seconds to every test run (the
    /// review that wrote the model ran 300 of each, about 45 s); the seeds are
    /// fixed, so every run checks the same sequences.
    const SEEDS: u64 = 40;

    fn file(text: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, text).unwrap();
        (dir, path)
    }

    use crate::config_keys;

    #[test]
    fn dotted_top_level_keys_survive_a_save() {
        let (_dir, path) = file("ui.left_width_pct = 20\n");
        let loaded = crate::config::load_config_file(&path).unwrap();
        let q = ConfigWriteQueue::with_base(path.clone(), &loaded);
        let mut memory = loaded.clone();
        memory.ui.copy_on_select = !memory.ui.copy_on_select;
        q.save_eager(memory.clone()).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let back: Config = toml::from_str(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
        assert_eq!(back.ui.left_width_pct, 20, "{text}");
        assert_eq!(back.ui.copy_on_select, memory.ui.copy_on_select, "{text}");
    }

    #[test]
    fn a_dotted_auth_inside_server_survives_a_set_and_a_save() {
        let (_dir, path) = file("[server]\nport = 3890\nauth.require = \"tailnet\"\n");
        let key = config_keys::lookup("server.auth.cookie_secure").unwrap();
        config_keys::set_plain(&path, &key, "always").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let auth = crate::config::auth_section_of(&text).unwrap();
        assert_eq!(auth.require.as_str(), "tailnet", "{text}");
        assert_eq!(auth.cookie_secure.as_str(), "always", "{text}");
        let loaded = crate::config::load_config_file(&path).unwrap();
        let q = ConfigWriteQueue::with_base(path.clone(), &loaded);
        let mut memory = loaded.clone();
        memory.server.port = 4000;
        q.save_eager(memory).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let auth = crate::config::auth_section_of(&text).unwrap();
        assert_eq!(auth.require.as_str(), "tailnet", "{text}");
    }

    #[test]
    fn a_hand_added_key_survives_dux_changing_its_neighbour() {
        let (_dir, path) = file("[ui]\nleft_width_pct = 20\n");
        let loaded = crate::config::load_config_file(&path).unwrap();
        let q = ConfigWriteQueue::with_base(path.clone(), &loaded);
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(
            &path,
            text.replace(
                "left_width_pct = 20",
                "left_width_pct = 20\nterminal_pane_height_pct = 44",
            ),
        )
        .unwrap();
        let mut memory = loaded.clone();
        memory.ui.left_width_pct = 30;
        q.save_eager(memory).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.ui.terminal_pane_height_pct, 44, "{text}");
        assert_eq!(back.ui.left_width_pct, 30, "{text}");
    }

    // ---------------------------------------------------------------------------
    // Randomized three-way save model
    // ---------------------------------------------------------------------------

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    const KEYS: [(&str, &str); 4] = [
        ("ui", "left_width_pct"),
        ("ui", "right_width_pct"),
        ("server", "port"),
        ("ui", "copy_on_select"),
    ];
    const ENV: [&str; 3] = ["AA", "BB", "CC"];

    fn mem_get(c: &Config, i: usize) -> String {
        match i {
            0 => c.ui.left_width_pct.to_string(),
            1 => c.ui.right_width_pct.to_string(),
            2 => c.server.port.to_string(),
            _ => c.ui.copy_on_select.to_string(),
        }
    }
    fn mem_set(c: &mut Config, i: usize, rng: &mut Rng) {
        match i {
            0 => c.ui.left_width_pct = 15 + rng.below(20) as u16,
            1 => c.ui.right_width_pct = 15 + rng.below(20) as u16,
            2 => c.server.port = 3000 + rng.below(900) as u16,
            _ => c.ui.copy_on_select = !c.ui.copy_on_select,
        }
    }
    fn random_literal(i: usize, rng: &mut Rng) -> String {
        match i {
            0 | 1 => (15 + rng.below(20)).to_string(),
            2 => (3000 + rng.below(900)).to_string(),
            _ => (rng.below(2) == 0).to_string(),
        }
    }

    /// Disk value of a tracked key, as TOML literal text, or None when absent.
    fn disk_get(text: &str, section: &str, key: &str) -> Option<String> {
        let doc: toml::Table = toml::from_str(text).unwrap();
        doc.get(section)?.get(key).map(|v| v.to_string())
    }
    fn disk_env(text: &str, key: &str) -> Option<String> {
        let doc: toml::Table = toml::from_str(text).unwrap();
        doc.get("env")?
            .get(key)
            .and_then(|v| v.as_str())
            .map(String::from)
    }

    fn hand_set(path: &std::path::Path, section: &str, key: &str, literal: Option<&str>) {
        let text = std::fs::read_to_string(path).unwrap();
        let mut doc: toml_edit::DocumentMut = text.parse().unwrap();
        if doc.get(section).is_none() {
            doc[section] = toml_edit::table();
        }
        let table = doc[section].as_table_like_mut().unwrap();
        match literal {
            Some(lit) => {
                let v: toml_edit::Value = lit.parse().unwrap();
                table.insert(key, toml_edit::Item::Value(v));
            }
            None => {
                table.remove(key);
            }
        }
        std::fs::write(path, doc.to_string()).unwrap();
    }

    #[test]
    fn random_hand_edits_saves_and_reloads_follow_the_three_way_rules() {
        let mut failures = Vec::new();
        for seed in 1..=SEEDS {
            let mut rng = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1);
            let (_dir, path) = file(
                "[ui]\nleft_width_pct = 20\nright_width_pct = 25\ncopy_on_select = true\n\n[server]\nport = 3890\n\n[env]\nAA = \"a\"\nBB = \"b\"\n",
            );
            let loaded = crate::config::load_config_file(&path).unwrap();
            let q = ConfigWriteQueue::with_base(path.clone(), &loaded);
            let mut memory = loaded.clone();
            // What the file agreed with at the last save or reload.
            let mut base = loaded.clone();
            // Expected disk state for tracked keys.
            let text0 = std::fs::read_to_string(&path).unwrap();
            let mut expect: Vec<Option<String>> =
                KEYS.iter().map(|(s, k)| disk_get(&text0, s, k)).collect();
            let mut expect_env: Vec<Option<String>> =
                ENV.iter().map(|k| disk_env(&text0, k)).collect();
            let mut log = Vec::new();
            for step in 0..25 {
                match rng.below(6) {
                    0 => {
                        let i = rng.below(4) as usize;
                        let (s, k) = KEYS[i];
                        if rng.below(3) == 0 {
                            hand_set(&path, s, k, None);
                            expect[i] = None;
                            log.push(format!("hand delete {s}.{k}"));
                        } else {
                            let lit = random_literal(i, &mut rng);
                            hand_set(&path, s, k, Some(&lit));
                            expect[i] = Some(lit.clone());
                            log.push(format!("hand set {s}.{k}={lit}"));
                        }
                    }
                    1 => {
                        let i = rng.below(3) as usize;
                        if rng.below(3) == 0 {
                            hand_set(&path, "env", ENV[i], None);
                            expect_env[i] = None;
                            log.push(format!("hand delete env.{}", ENV[i]));
                        } else {
                            let v = format!("h{}", rng.below(100));
                            hand_set(&path, "env", ENV[i], Some(&format!("\"{v}\"")));
                            expect_env[i] = Some(v.clone());
                            log.push(format!("hand set env.{}={v}", ENV[i]));
                        }
                    }
                    2 => {
                        let i = rng.below(4) as usize;
                        mem_set(&mut memory, i, &mut rng);
                        log.push(format!("mem set {}={}", KEYS[i].1, mem_get(&memory, i)));
                    }
                    3 => {
                        let i = rng.below(3) as usize;
                        if rng.below(3) == 0 {
                            memory.env.remove(ENV[i]);
                            log.push(format!("mem remove env.{}", ENV[i]));
                        } else {
                            let v = format!("m{}", rng.below(100));
                            memory.env.insert(ENV[i].to_string(), v.clone());
                            log.push(format!("mem set env.{}={v}", ENV[i]));
                        }
                    }
                    4 => {
                        q.save_eager(memory.clone()).unwrap();
                        for (i, slot) in expect.iter_mut().enumerate() {
                            if mem_get(&memory, i) != mem_get(&base, i) {
                                *slot = Some(mem_get(&memory, i));
                            }
                        }
                        for (i, k) in ENV.iter().enumerate() {
                            let m = memory.env.get(*k).cloned();
                            let b = base.env.get(*k).cloned();
                            if m != b {
                                expect_env[i] = m;
                            }
                        }
                        base = memory.clone();
                        log.push("save".into());
                    }
                    _ => {
                        // reload
                        memory = crate::config::load_config_file(&path).unwrap();
                        q.set_base(memory.clone());
                        base = memory.clone();
                        log.push("reload".into());
                    }
                }
                let text = std::fs::read_to_string(&path).unwrap();
                let got: Vec<Option<String>> =
                    KEYS.iter().map(|(s, k)| disk_get(&text, s, k)).collect();
                let got_env: Vec<Option<String>> = ENV.iter().map(|k| disk_env(&text, k)).collect();
                if got != expect || got_env != expect_env {
                    failures.push(format!(
                        "seed {seed} step {step}: got {got:?} {got_env:?}, expected {expect:?} {expect_env:?}\nlog: {log:#?}\nfile:\n{text}"
                    ));
                    break;
                }
            }
        }
        assert!(
            failures.is_empty(),
            "{} failures; first:\n{}",
            failures.len(),
            failures[0]
        );
    }

    const PFIELDS: [&str; 3] = ["name", "startup_command", "default_provider"];

    fn proj_mem_get(c: &Config, p: usize, f: usize) -> Option<String> {
        let id = ["p1", "p2"][p];
        let project = c.projects.iter().find(|x| x.id == id)?;
        match f {
            0 => project.name.clone(),
            1 => project.startup_command.clone(),
            _ => project.default_provider.clone(),
        }
    }
    fn proj_mem_set(c: &mut Config, p: usize, f: usize, v: Option<String>) {
        let id = ["p1", "p2"][p];
        let project = c.projects.iter_mut().find(|x| x.id == id).unwrap();
        match f {
            0 => project.name = v,
            1 => project.startup_command = v,
            _ => project.default_provider = v,
        }
    }
    fn proj_disk_get(text: &str, p: usize, f: usize) -> Option<String> {
        let doc: toml::Table = toml::from_str(text).unwrap();
        let id = ["p1", "p2"][p];
        doc.get("projects")?
            .as_array()?
            .iter()
            .find(|e| e.get("id").and_then(|v| v.as_str()) == Some(id))?
            .get(PFIELDS[f])
            .and_then(|v| v.as_str())
            .map(String::from)
    }
    fn proj_hand_set(path: &std::path::Path, p: usize, f: usize, v: Option<&str>) {
        let text = std::fs::read_to_string(path).unwrap();
        let mut doc: toml_edit::DocumentMut = text.parse().unwrap();
        let id = ["p1", "p2"][p];
        let arr = doc["projects"].as_array_of_tables_mut().unwrap();
        let entry = arr
            .iter_mut()
            .find(|e| e.get("id").and_then(|v| v.as_str()) == Some(id))
            .unwrap();
        match v {
            Some(v) => {
                entry.insert(PFIELDS[f], toml_edit::value(v));
            }
            None => {
                entry.remove(PFIELDS[f]);
            }
        }
        std::fs::write(path, doc.to_string()).unwrap();
    }

    #[test]
    fn random_project_field_edits_saves_and_reloads_follow_the_three_way_rules() {
        let dir0 = tempfile::TempDir::new().unwrap();
        let r1 = dir0.path().join("r1");
        let r2 = dir0.path().join("r2");
        std::fs::create_dir_all(&r1).unwrap();
        std::fs::create_dir_all(&r2).unwrap();
        let mut failures = Vec::new();
        for seed in 1..=SEEDS {
            let mut rng = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1);
            let initial = format!(
                "[[projects]]\nid = \"p1\"\npath = \"{}\"\nname = \"one\"\nstartup_command = \"make\"\n\n[[projects]]\nid = \"p2\"\npath = \"{}\"\nname = \"two\"\ndefault_provider = \"codex\"\n",
                r1.display(),
                r2.display()
            );
            let (_dir, path) = file(&initial);
            let loaded = crate::config::load_config_file(&path).unwrap();
            let q = ConfigWriteQueue::with_base(path.clone(), &loaded);
            let mut memory = loaded.clone();
            let mut base = loaded.clone();
            let text0 = std::fs::read_to_string(&path).unwrap();
            let mut expect: Vec<Vec<Option<String>>> = (0..2)
                .map(|p| (0..3).map(|f| proj_disk_get(&text0, p, f)).collect())
                .collect();
            let mut log = Vec::new();
            for step in 0..25 {
                let p = rng.below(2) as usize;
                let f = rng.below(3) as usize;
                match rng.below(4) {
                    0 => {
                        let v = if rng.below(3) == 0 {
                            None
                        } else {
                            Some(format!("h{}", rng.below(50)))
                        };
                        proj_hand_set(&path, p, f, v.as_deref());
                        expect[p][f] = v.clone();
                        log.push(format!("hand p{} {}={v:?}", p + 1, PFIELDS[f]));
                    }
                    1 => {
                        let v = if rng.below(3) == 0 {
                            None
                        } else {
                            Some(format!("m{}", rng.below(50)))
                        };
                        proj_mem_set(&mut memory, p, f, v.clone());
                        log.push(format!("mem p{} {}={v:?}", p + 1, PFIELDS[f]));
                    }
                    2 => {
                        q.save_eager(memory.clone()).unwrap();
                        for (p, fields) in expect.iter_mut().enumerate() {
                            for (f, slot) in fields.iter_mut().enumerate() {
                                let m = proj_mem_get(&memory, p, f);
                                if m != proj_mem_get(&base, p, f) {
                                    *slot = m;
                                }
                            }
                        }
                        base = memory.clone();
                        log.push("save".into());
                    }
                    _ => {
                        memory = crate::config::load_config_file(&path).unwrap();
                        q.set_base(memory.clone());
                        base = memory.clone();
                        log.push("reload".into());
                    }
                }
                let text = std::fs::read_to_string(&path).unwrap();
                let got: Vec<Vec<Option<String>>> = (0..2)
                    .map(|p| (0..3).map(|f| proj_disk_get(&text, p, f)).collect())
                    .collect();
                if got != expect {
                    failures.push(format!(
                        "seed {seed} step {step}: got {got:?}, expected {expect:?}\nlog: {log:#?}\nfile:\n{text}"
                    ));
                    break;
                }
            }
        }
        assert!(
            failures.is_empty(),
            "{} failures; first:\n{}",
            failures.len(),
            failures[0]
        );
    }
}
