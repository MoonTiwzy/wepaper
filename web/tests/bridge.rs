//! The page script, tested in a scripted page: runs `bridge.test.js` with node.

use std::path::Path;
use std::process::Command;

#[test]
fn the_bridge_behaves_in_a_scripted_page_when_node_is_installed() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/bridge.test.js");
    let Ok(output) = Command::new("node").arg(&script).output() else {
        eprintln!("node is not installed: skipped");
        return;
    };
    let report = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{report}{}", String::from_utf8_lossy(&output.stderr));
}
