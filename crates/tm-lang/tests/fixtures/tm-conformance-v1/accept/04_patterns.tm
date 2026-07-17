match {content: "ok", mime: "text/plain", extra: true} {
  | {content, mime: "text/plain", ...} -> content
  | _ -> "missing"
}
