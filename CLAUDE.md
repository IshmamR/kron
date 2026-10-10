# carear

A GPUI (Zed's UI framework) desktop app.

## Rules

- **Never use `.unwrap()` (or `.expect()`) when the error can be handled cheaply.**
  Propagate with `?`, fall back with `unwrap_or` / `unwrap_or_default` / `if let` /
  `let ... else`, or log the error and carry on. Use `.expect("why this can't fail")` only
  when failure is truly impossible or unrecoverable, and say why in the message.
- Never silently drop errors with `let _ =` on fallible calls; propagate or log them.
  Be careful with indexing that can panic.

## Code structure

### Files and modules
- One file per logical component, named after its main type: `title_bar.rs` → `TitleBar`.
  Don't split into many tiny files; add to an existing file unless it's a new component.
- Never `mod.rs`. Use `src/foo.rs`, with submodules in `src/foo/bar.rs` declared at the top of `foo.rs`.

### Order inside a file
1. `mod` declarations
2. `use` imports (std, external crates, then `crate::`)
3. `const`s
4. `actions!(namespace, [ActionName, ...])`
5. `pub fn init(cx: &mut App)`: registers the component's key bindings, action handlers
   and globals. `main.rs` just calls each component's `init`.
6. The main `pub struct`, then its trait impls (`Render`, `Focusable`, `EventEmitter`)
   and its inherent `impl` (`new`, then methods)
7. Private helper types and functions
8. `#[cfg(test)] mod tests` last

### GPUI patterns
- Stateful views: `Entity<T>` where `T: Render`, created with `cx.new(...)`.
- Stateless pieces built just to be rendered: `#[derive(IntoElement)]` struct +
  `RenderOnce`, with a `new(...)` constructor and chainable builder setters
  (`fn color(mut self, color: Color) -> Self`).
- Parameter order: `window` before `cx`; callbacks after `cx`.
- Event handlers that mutate the view use `cx.listener(|this, event, window, cx| ...)`.
  Call `cx.notify()` after state changes that affect rendering.
- Conditional styling/children: `.when(cond, |this| ...)` and `.when_some(opt, |this, v| ...)`.
- Store subscriptions as `_subscriptions: Vec<Subscription>` (or a single `_name` field).
  Store `Task`s in fields to keep them alive, or `.detach()` them; a dropped task is cancelled.
- In async code, scope clones with shadowing:
  `cx.spawn({ let thing = thing.clone(); async move |cx| ... })`.

### Style
- Full-word names, no abbreviations (`queue`, not `q`).
- Comments explain *why*, only when non-obvious. No comments that restate the code.
