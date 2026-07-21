# 13. End-to-end example

**User:** *"Which of our last 100 support tickets mention 'refund', and what's the median
resolution time for those?"*

```
turn 1  assistant → execute:
        let cards = @tools.search "support tickets list";
        cards |> map (fun c -> c.name) |> display {kind: "table"}
  tool ← ["tickets.list","tickets.get"]                              # ~30 tokens, not the schemas

turn 2  assistant → execute:
        let t = @tickets.list {limit: 100};                            # 100 tickets, ~600 KB
        let refunds = t |> filter (fun x -> contains "refund" x.subject);
        let mins = refunds |> map (fun x -> (x.resolvedAt - x.createdAt) / 60000) |> sort;
        let median = match mins { | [] -> null | _ -> mins |> at (length mins / 2) };
        display {matched: length refunds, medianMinutes: median}
  tool ← { matched: 12, median_minutes: 87 }                          # ~15 tokens; 600 KB never entered context

turn 3  assistant → (final answer)
        "12 of the last 100 tickets mention refunds; median resolution ≈ 1h27m."
```

The 600 KB payload was fetched, filtered, and reduced **in the sandbox**. Context spent tokens
only on two tiny `display()` results and the answer. That is the entire point.
