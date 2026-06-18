// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn maps_routes_to_files() {
    assert_eq!(map_route("/"), "index.html");
    assert_eq!(map_route(""), "index.html");
    assert_eq!(map_route("/index.html"), "index.html");
    assert_eq!(map_route("/chat"), "chat.html");
    assert_eq!(map_route("/chat.html"), "chat.html");
    assert_eq!(map_route("/styles.css"), "styles.css");
    assert_eq!(map_route("/dist/board.js"), "dist/board.js");
    assert_eq!(map_route("/styles.css?v=2"), "styles.css");
}

#[test]
fn content_types_for_known_extensions() {
    assert_eq!(
        content_type_for(Path::new("a.html")),
        "text/html; charset=utf-8"
    );
    assert_eq!(
        content_type_for(Path::new("a.js")),
        "text/javascript; charset=utf-8"
    );
    assert_eq!(
        content_type_for(Path::new("a.css")),
        "text/css; charset=utf-8"
    );
    assert_eq!(
        content_type_for(Path::new("a.unknown")),
        "application/octet-stream"
    );
}

#[test]
fn resolves_and_reads_a_file_under_root() {
    let dir = std::env::temp_dir().join(format!("temper-web-static-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("dist")).unwrap();
    std::fs::write(dir.join("index.html"), b"<!doctype html>").unwrap();
    std::fs::write(dir.join("dist/board.js"), b"console.log(1)").unwrap();

    let index = resolve(&dir, "/").expect("index resolves");
    assert_eq!(index.content_type, "text/html; charset=utf-8");
    assert_eq!(index.bytes, b"<!doctype html>");

    let js = resolve(&dir, "/dist/board.js").expect("bundle resolves");
    assert_eq!(js.content_type, "text/javascript; charset=utf-8");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn rejects_path_traversal() {
    let dir = std::env::temp_dir();
    assert_eq!(resolve(&dir, "/../etc/passwd"), Err(StaticError::Forbidden));
    assert_eq!(
        resolve(&dir, "/dist/../../secret"),
        Err(StaticError::Forbidden)
    );
}

#[test]
fn missing_file_is_not_found() {
    let dir = std::env::temp_dir();
    assert_eq!(
        resolve(&dir, "/does-not-exist.js"),
        Err(StaticError::NotFound)
    );
}
