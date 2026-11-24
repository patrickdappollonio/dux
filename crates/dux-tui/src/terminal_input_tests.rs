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

const CROSSTERM_CHILD: &str = "terminal_input::tests::child_records_crossterm_events";

/// Not a test on its own: run as a child in a pseudo-terminal, it appends a
/// line per event crossterm's own reader returns to `<dir>/events`.
#[test]
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

/// Feed `items` to crossterm's real reader in a child, one write each, and
/// return the lines it recorded once they stop coming.
fn crossterm_events_of(items: &[Vec<u8>]) -> Vec<String> {
    let child = PtyChild::start(CROSSTERM_CHILD, "");
    for item in items {
        child.type_bytes(item);
        std::thread::sleep(Duration::from_millis(2));
    }
    let events = child.dir.path().join("events");
    let mut last = (u64::MAX, Instant::now());
    loop {
        let len = std::fs::metadata(&events).map_or(0, |meta| meta.len());
        if len != last.0 {
            last = (len, Instant::now());
        } else if last.1.elapsed() > Duration::from_millis(800) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    std::fs::read_to_string(events)
        .expect("events")
        .lines()
        .map(str::to_string)
        .collect()
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
    let reads: Vec<&[u8]> = items.iter().map(Vec::as_slice).collect();
    let ours: Vec<String> = events_of_reads(&reads)
        .iter()
        .map(|event| format!("{event:?}"))
        .collect();
    let theirs = crossterm_events_of(items);
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

#[test]
fn every_hand_picked_sequence_decodes_as_crossterm_reads_it() {
    assert_same_as_crossterm(&corpus());
}

#[test]
fn fuzzed_sequences_decode_as_crossterm_reads_them() {
    assert_same_as_crossterm(&fuzzed(1500, 0x00d0_c0de));
}
