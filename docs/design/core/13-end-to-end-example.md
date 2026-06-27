# 13. End-to-end example

**User:** *"Which of our last 100 support tickets mention 'refund', and what's the median
resolution time for those?"*

```
turn 1  assistant → execute:
        const cards = await tools.search("support tickets list");
        display(cards.map(c => c.name));
  tool ← ["tickets.list","tickets.get"]                              # ~30 tokens, not the schemas

turn 2  assistant → execute:
        const t = await tools.call("tickets.list", { limit: 100 });   # 100 tickets, ~600 KB
        const refunds = t.filter(x => /refund/i.test(x.subject + x.body));
        const mins = refunds.map(x => (x.resolved_at - x.created_at) / 60000).sort((a,b)=>a-b);
        const median = mins.length ? mins[Math.floor(mins.length/2)] : null;
        display({ matched: refunds.length, median_minutes: median });
  tool ← { matched: 12, median_minutes: 87 }                          # ~15 tokens; 600 KB never entered context

turn 3  assistant → (final answer)
        "12 of the last 100 tickets mention refunds; median resolution ≈ 1h27m."
```

The 600 KB payload was fetched, filtered, and reduced **in the sandbox**. Context spent tokens
only on two tiny `display()` results and the answer. That is the entire point.
