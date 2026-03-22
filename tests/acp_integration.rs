#[path = "../src/acp.rs"]
mod acp;

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use acp::AcpClient;

#[test]
fn acp_client_can_initialize_create_load_and_prompt() {
    let binary = env!("CARGO_BIN_EXE_mock_acp");
    let (tx, rx) = mpsc::channel();
    let client = AcpClient::spawn(binary, &[], Path::new("."), "app-session", tx).unwrap();

    client.initialize().unwrap();
    let session_id = client.new_session(Path::new(".")).unwrap();
    assert_eq!(session_id, "mock-session");

    let loaded = client.load_session(Path::new("."), "mock-session").unwrap();
    assert_eq!(loaded, "mock-session");

    client.prompt(
        "mock-session".to_string(),
        "hello".to_string(),
        mpsc::channel().0,
        "app-session".to_string(),
    );

    let first = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(first.message.contains("mock reply"));
}
