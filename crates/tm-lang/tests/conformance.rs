use std::{fs, path::Path};

use tm_lang::{CONFORMANCE_VERSION, parse};

#[test]
fn frozen_accept_corpus_parses() {
    assert_eq!(CONFORMANCE_VERSION, "tm-conformance-v2");
    for path in fixture_files("accept") {
        let source = fs::read_to_string(&path).unwrap();
        parse(&source).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    }
}

#[test]
fn frozen_reject_corpus_has_deterministic_codes() {
    for path in fixture_files("reject") {
        let source = fs::read_to_string(&path).unwrap();
        let expected = source
            .lines()
            .next()
            .unwrap()
            .trim_start_matches("-- expect: ");
        let error = parse(&source).unwrap_err();
        assert_eq!(error.code, expected, "{}", path.display());
    }
}

fn fixture_files(kind: &str) -> Vec<std::path::PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/tm-conformance-v2")
        .join(kind);
    let mut files: Vec<_> = fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "tm"))
        .collect();
    files.sort();
    files
}
