handle @fs.read workspace:missing.rs with error {
  | NotFound {uri} -> display {kind: "text"} "missing #uri"
  | e -> rethrow e
}
