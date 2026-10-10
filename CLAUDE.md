# carear

A GPUI (Zed's UI framework) desktop app.

## Rules

- **Never use `.unwrap()` (or `.expect()`) when the error can be handled cheaply.**
  Propagate with `?`, fall back with `unwrap_or` / `unwrap_or_default` / `if let` /
  `let ... else`, or log the error and carry on. Use `.expect("why this can't fail")` only
  when failure is truly impossible or unrecoverable, and say why in the message.
