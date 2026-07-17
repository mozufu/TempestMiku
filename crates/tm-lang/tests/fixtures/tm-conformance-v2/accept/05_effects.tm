handle @fs.read workspace:missing.rs with error {
  | NotFoundError {uri, ...} -> display {kind: "text"} "missing #uri"
  | e -> rethrow e
}
