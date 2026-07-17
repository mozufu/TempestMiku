do {
  type Finding = | Hit {file: String, line: Int, text: String} | Ignored Path;
  let finding = Hit {file: "src/lib.rs", line: 42, text: "TODO"};
  match finding {
    | Hit {file, line, text} -> "#file:#line #text"
    | Ignored path -> "ignored #path"
  }
}
