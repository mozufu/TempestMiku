let paths = [workspace:a, workspace:b]
---
paths |> par map @fs.read
