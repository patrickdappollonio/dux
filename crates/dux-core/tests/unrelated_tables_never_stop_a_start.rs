//! A table of the user's own that has nothing to do with the web password,
//! and that dux before the branch ignored, must not stop either surface.
use dux_core::config::{Surface, start_refusal};

#[test]
fn a_users_own_table_with_a_short_name_never_stops_a_start() {
    for body in [
        "[path]\nx = 1\n",
        "[math]\nx = 1\n",
        "[auto]\nenabled = true\n",
        "[oauth]\nclient_id = \"x\"\n",
        "[server.oauth]\nclient_id = \"x\"\n",
    ] {
        for surface in [Surface::TerminalUi, Surface::DuxServer] {
            assert_eq!(
                start_refusal(body, surface),
                None,
                "{body:?} stops {surface:?}, though it holds no password setting"
            );
        }
    }
}
