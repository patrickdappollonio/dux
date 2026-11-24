use std::io::Write as _;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

use super::*;
use crate::pty_child::{PtyChild, child_dir};

fn events_of_reads(reads: &[&[u8]]) -> Vec<Event> {
    let mut input = TerminalInput::default();
    let mut events = Vec::new();
    for read in reads {
        input.push_read(read);
        while let Some(event) = input.next_event() {
            events.push(event);
        }
    }
    events
}

fn key(code: KeyCode, modifiers: KeyModifiers) -> Event {
    Event::Key(KeyEvent::new(code, modifiers))
}

#[test]
fn a_lone_escape_ending_a_read_is_the_escape_key_and_one_mid_read_prefixes_alt() {
    assert_eq!(
        events_of_reads(&[b"\x1b"]),
        vec![key(KeyCode::Esc, KeyModifiers::NONE)]
    );
    assert_eq!(
        events_of_reads(&[b"\x1bx"]),
        vec![key(KeyCode::Char('x'), KeyModifiers::ALT)]
    );
}

#[test]
fn a_lone_escape_ending_a_full_read_waits_for_the_next_read() {
    let mut full = vec![b'a'; READ_SIZE - 1];
    full.push(0x1b);
    let events = events_of_reads(&[&full, b"x"]);
    assert_eq!(
        events.last(),
        Some(&key(KeyCode::Char('x'), KeyModifiers::ALT))
    );
    assert_eq!(events.len(), READ_SIZE);
}

#[test]
fn a_paste_whose_text_holds_a_numbered_csi_r_shape_stays_one_paste() {
    assert_eq!(
        events_of_reads(&[b"\x1b[200~1;5R", b" and more\x1b[201~"]),
        vec![Event::Paste("1;5R and more".to_string())]
    );
}

#[test]
fn bytes_taken_by_a_raw_reader_and_given_back_decode_as_if_never_taken() {
    let mut input = TerminalInput::default();
    input.push_read(b"\x1b[");
    assert_eq!(input.next_event(), None);
    let taken = input.take_bytes();
    assert_eq!(taken, b"\x1b[");
    input.give_back(&[taken.as_slice(), b"A"].concat());
    assert_eq!(
        input.next_event(),
        Some(key(KeyCode::Up, KeyModifiers::NONE))
    );
}

/// Once the bytes of a full read are all decoded, a lone Escape a raw reader
/// gives back is the Escape key: the full read it no longer ends says nothing
/// about it.
#[test]
fn a_full_read_decoded_to_its_end_leaves_no_wait_on_a_later_escape() {
    let mut input = TerminalInput::default();
    input.push_read(&[b'a'; READ_SIZE]);
    while input.next_event().is_some() {}
    input.give_back(b"\x1b");
    assert_eq!(
        input.next_event(),
        Some(key(KeyCode::Esc, KeyModifiers::NONE))
    );
}

/// Bytes given back were read before a sequence the decoder has begun since,
/// so they come first.
#[test]
fn bytes_given_back_come_before_a_sequence_already_begun() {
    let mut input = TerminalInput::default();
    input.push_read(b"\x1b[");
    assert_eq!(input.next_event(), None);
    input.give_back(b"x");
    input.push_read(b"A");
    assert_eq!(
        [input.next_event(), input.next_event()],
        [
            Some(key(KeyCode::Char('x'), KeyModifiers::NONE)),
            Some(key(KeyCode::Up, KeyModifiers::NONE))
        ]
    );
}

/// The files a guard reads, under this crate's `src`.
fn source_files(dir: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read the source folder") {
        let path = entry.expect("a source entry").path();
        if path.is_dir() {
            source_files(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
}

/// Whether `source` reaches crossterm's event reader, as far as its text shows:
/// a qualified `crossterm::event::poll` or `read` (through ratatui's re-export
/// too), `read` or `poll` named in a `use crossterm::event::{…}` list, an
/// `EventStream`, or `event::poll(` or `event::read(` in a file that imports
/// crossterm's `event` module.
fn reaches_crossterm_s_reader(source: &str) -> bool {
    let imports_module = source.contains("use crossterm::event;")
        || source.contains("use crossterm::event::{self")
        || source.contains("use ratatui::crossterm::event;");
    let imports_reader = source
        .match_indices("crossterm::event::{")
        .any(|(at, opening)| {
            let list = &source[at + opening.len()..];
            let list = &list[..list.find('}').unwrap_or(list.len())];
            list.split(',').any(|item| {
                let name = item.split_whitespace().next().unwrap_or_default();
                name == "read" || name == "poll"
            })
        });
    source.contains("crossterm::event::read")
        || source.contains("crossterm::event::poll")
        || source.contains("EventStream")
        || imports_reader
        || (imports_module && (source.contains("event::poll(") || source.contains("event::read(")))
}

/// crossterm's event reader, once polled, reads the terminal into a queue of
/// its own that no other reader sees, so a byte it took would never reach the
/// buffer every reader of the terminal UI shares. `dux config` runs as its own
/// command, and this file's differential test drives crossterm in a child.
#[test]
fn nothing_in_the_terminal_ui_reads_through_crossterm_s_event_reader() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    source_files(&src, &mut files);
    let allowed = ["config_cli.rs", "terminal_input_tests.rs"];
    let readers: Vec<_> = files
        .iter()
        .filter(|path| {
            !allowed
                .iter()
                .any(|name| path.file_name().is_some_and(|file| file == *name))
        })
        .filter(|path| {
            reaches_crossterm_s_reader(&std::fs::read_to_string(path).expect("read a source file"))
        })
        .collect();
    assert!(readers.is_empty(), "reads through crossterm: {readers:#?}");
    assert!(
        reaches_crossterm_s_reader(
            &std::fs::read_to_string(src.join("config_cli.rs")).expect("read config_cli.rs")
        ),
        "the guard no longer sees the one reader it allows"
    );
}

const CROSSTERM_CHILD: &str = "terminal_input::tests::child_records_crossterm_events";

/// Run as a child in a pseudo-terminal, it appends a line per event crossterm's
/// own reader returns to `<dir>/events`.
#[test]
#[ignore = "runs only as the child of a test in this file, in a pseudo-terminal"]
fn child_records_crossterm_events() {
    let Some(dir) = child_dir() else {
        return;
    };
    crossterm::terminal::enable_raw_mode().expect("raw mode on the child's terminal");
    let mut out = std::fs::File::create(dir.join("events")).expect("events file");
    std::fs::write(dir.join("ready"), b"").expect("ready marker");
    loop {
        let event = crossterm::event::read().expect("crossterm read");
        writeln!(out, "{event:?}").expect("event line");
        out.flush().expect("flush");
    }
}

/// Written last: once crossterm has decoded it, it has decoded everything
/// before it. No corpus or fuzzed item makes Ctrl-Alt-Shift-F12.
const END_OF_ITEMS: &[u8] = b"\x1b[24;8~";

/// The most bytes one write to the child carries. Each write waits for the
/// child to have decoded the one before, so the child reads it whole, in one
/// read, from an otherwise empty terminal.
const WRITE_SIZE: usize = 256;

/// Whole items grouped into writes of at most [`WRITE_SIZE`] bytes.
fn writes_of(items: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut writes: Vec<Vec<u8>> = vec![Vec::new()];
    for item in items {
        let last = writes.last_mut().expect("a write");
        if !last.is_empty() && last.len() + item.len() > WRITE_SIZE {
            writes.push(item.clone());
        } else {
            last.extend_from_slice(item);
        }
    }
    writes
}

/// The events dux decodes from `writes`, one read each, as lines.
fn our_events(writes: &[Vec<u8>]) -> Vec<String> {
    let reads: Vec<&[u8]> = writes.iter().map(Vec::as_slice).collect();
    events_of_reads(&reads)
        .iter()
        .map(|event| format!("{event:?}"))
        .collect()
}

fn lines_of(path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// Feed `writes` to crossterm's real reader in a child, each once the child
/// has decoded the one before as dux does, and return what it decoded. A write
/// that crossterm decodes differently stops the feed once its wait runs out.
fn crossterm_events_of(writes: &[Vec<u8>]) -> Vec<String> {
    let child = PtyChild::start(CROSSTERM_CHILD, "");
    let events = child.dir.path().join("events");
    for (sent, write) in writes.iter().enumerate() {
        child.type_bytes(write);
        let want = our_events(&writes[..=sent]).len();
        let deadline = Instant::now() + Duration::from_secs(5);
        while lines_of(&events).len() < want && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        if lines_of(&events).len() != want {
            break;
        }
    }
    child.type_bytes(END_OF_ITEMS);
    let end = format!("{:?}", events_of_reads(&[END_OF_ITEMS])[0]);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let mut lines = lines_of(&events);
        if lines.last() == Some(&end) {
            lines.pop();
            return lines;
        }
        assert!(
            Instant::now() < deadline,
            "crossterm never decoded the end marker; it decoded {lines:#?}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Deterministic fragments a fuzzed item is built from. No `0` digit and no bare
/// `M`: crossterm subtracts one from a zero coordinate, which panics in a debug build.
const FRAGMENTS: &[&[u8]] = &[
    b"a",
    b"Z",
    b"7",
    b" ",
    b"\r",
    b"\n",
    b"\t",
    b"\x7f",
    b"\x00",
    b"\x01",
    b"\x07",
    b"\x08",
    b"\x1c",
    b"\x1f",
    b"\x1b",
    b"\x1b\x1b",
    b"\x1bO",
    b"\x1b[",
    b"\x1b[[",
    b"\x1b[<",
    b"\x1b[?",
    b";",
    b":",
    b"1",
    b"5",
    b"9",
    b"13",
    b"97",
    b"103",
    b"57441",
    b"~",
    b"u",
    b"A",
    b"B",
    b"D",
    b"F",
    b"H",
    b"P",
    b"R",
    b"S",
    b"m",
    b"I",
    b"O",
    b"Z",
    b"c",
    b"\xc3\xa9",
    b"\xe6\xbc\xa2",
    b"\xf0\x9f\x8e\x89",
    b"\x80",
    b"\xff",
    b"\xe6",
    b"\x1b[200~",
    b"\x1b[201~",
    b"\x1b[201~",
    b"\x1b[<1;2;3M",
    b"\x1b[<64;5;5M",
    b"\x1b[M!!!",
    b"\x1b[M@@@",
    b"\x1b[32;5;5M",
];

/// Fixed, hand-picked sequences: every kind of key, report and paste dux reads.
fn corpus() -> Vec<Vec<u8>> {
    let mut items: Vec<Vec<u8>> = Vec::new();
    for byte in 0u8..=0x7f {
        if byte != 0x1b {
            items.push(vec![byte]);
        }
    }
    for text in [
        "é",
        "漢字",
        "🎉",
        "Ünïcødé",
        "\x1ba",
        "\x1bA",
        "\x1b\x1b",
        "\x1b\x1b[A",
        "\x1b\x7f",
        "\x1b\r",
        "\x1b\t",
        "\x1b\x01",
        "\x1bé",
        "\x1bOA",
        "\x1bOP",
        "\x1bOx",
        "\x1b[A",
        "\x1b[Z",
        "\x1b[R",
        "\x1b[1;5A",
        "\x1b[1;2D",
        "\x1b[1;5R",
        "\x1b[5R",
        "\x1b[15~",
        "\x1b[3;5~",
        "\x1b[200~multi\nline\rpaste\x1b[201~",
        "\x1b[I",
        "\x1b[O",
        "\x1b[<0;10;5M",
        "\x1b[<0;10;5m",
        "\x1b[<32;11;5M",
        "\x1b[<64;1;1M",
        "\x1b[<65;1;1M",
        "\x1b[<4;3;3M",
        "\x1b[M #(",
        "\x1b[32;12;7M",
        "\x1b[103;6u",
        "\x1b[105;5u",
        "\x1b[109;5u",
        "\x1b[9u",
        "\x1b[9;2u",
        "\x1b[13;2u",
        "\x1b[27u",
        "\x1b[103;9u",
        "\x1b[103;17u",
        "\x1b[103;33u",
        "\x1b[97;1:3u",
        "\x1b[97;5:2u",
        "\x1b[57441;2u",
        "\x1b[57399u",
        "\x1b[57428u",
        "\x1b[97:65;2u",
        "\x1b[127;1u",
        "\x1b[?1u",
        "\x1b[?62;4c",
        "\x1b[[A",
        "\x1b[[E",
        "\x1b[P",
        "\x1b[1;3P",
        "\x1b[999;999;999u",
        "\x1b]11;rgb:0/0/0\x07",
    ] {
        items.push(text.as_bytes().to_vec());
    }
    items.push(b"\x1b[200~\x1b[A\x1b[201~".to_vec());
    items.push(b"\x1b[200~".to_vec());
    items.push(b"half a paste".to_vec());
    items.push(b"\x1b[201~".to_vec());
    items
}

/// Pseudo-random items from [`FRAGMENTS`], never ending in a lone Escape: that
/// is the one byte whose meaning depends on where the read ends, and the
/// child's reads may not end where the items do.
fn fuzzed(count: usize, mut seed: u64) -> Vec<Vec<u8>> {
    let mut next = || {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        usize::try_from(seed >> 33).expect("31 bits fit a usize")
    };
    let mut items = Vec::new();
    while items.len() < count {
        let mut item = Vec::new();
        for _ in 0..=next() % 6 {
            item.extend_from_slice(FRAGMENTS[next() % FRAGMENTS.len()]);
        }
        if item.last() != Some(&0x1b) {
            items.push(item);
        }
    }
    items
}

fn assert_same_as_crossterm(items: &[Vec<u8>]) {
    let writes = writes_of(items);
    let ours = our_events(&writes);
    let theirs = crossterm_events_of(&writes);
    let first_difference = ours.iter().zip(&theirs).position(|(a, b)| a != b);
    assert!(
        ours == theirs,
        "decoded {} events, crossterm {}; first difference at {first_difference:?}:\nours   {:?}\ntheirs {:?}",
        ours.len(),
        theirs.len(),
        first_difference.map(|i| &ours[i]),
        first_difference.map(|i| &theirs[i]),
    );
}

// These compare dux's decoder with the crossterm in Cargo.lock, so run them
// again after every crossterm upgrade.
#[test]
fn every_hand_picked_sequence_decodes_as_crossterm_reads_it() {
    assert_same_as_crossterm(&corpus());
}

#[test]
fn fuzzed_sequences_decode_as_crossterm_reads_them() {
    assert_same_as_crossterm(&fuzzed(1500, 0x00d0_c0de));
}
