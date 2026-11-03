# Kron

A desktop app for writing a résumé for each job you apply to, where every line
can be traced back to something you actually did.

Kron keeps **facts** about your work instead of résumés. You import your CV,
Kron finds the facts in it, and you review them. For each job you add, a model
writes a résumé from those facts, and every line cites the facts it came from.
Kron checks each line against them: a figure, year or count your facts don't
have keeps the line off the page, and it's listed as an **unsupported claim**
instead.

Written in Rust with [GPUI](https://www.gpui.rs/) and laid out with
[Typst](https://typst.app/).

## What it does

- **Import** a CV as PDF, Markdown or text, or paste it. It's read on your
  device; you check the text before it's saved.
- **Find facts** with an AI model. Your name and contact details are replaced
  before anything is sent, and you see what's hidden first. Each fact quotes
  the document, and anything Kron couldn't match word for word is marked for
  review.
- **Facts page:** search, edit, hide, merge duplicates, add your own notes.
- **Jobs:** paste a job's description and generate a résumé for it, with
  optional instructions ("lead with the open-source work").
- **Every line cites its facts.** Select a line to see them, where they came
  from, and the words the line adds. Lines the checker rejects are listed as
  unsupported claims, with a way to add the missing fact.
- **Edit any line.** If your edit says more than its facts, Kron tells you
  what; Revert brings back what was generated.
- **Templates** (Modern, Classic, Sidebar) and paper (A4 or US Letter) change
  the look, not the words. **Export PDF** saves what you see.
- **Track applications:** log applied, interviews and offers; Kron shows each
  job's stage and when to follow up.
- Undo and redo for every change (Ctrl+Z, Ctrl+Y), light and dark themes,
  keyboard shortcuts throughout (Ctrl+/ lists them).

## Privacy

- Your data stays in SQLite on your device.
- Only what a task needs is sent to the AI provider you set up, with your
  name, email, phone and links hidden. Company names and the job description
  are sent as written.
- Your API key is kept in the OS keyring, never in the database.
- Prompts and responses are never stored or logged; Kron records only tokens
  and cost per run.

## Status

Early and personal: it works from import to exported PDF on real CVs. Not
built yet: cover letters, fetching a job description from its link, a
model-based check of what a line means (today's checker catches invented
figures, years and counts, not overstated wording), and sync between devices.
DeepSeek is the only AI provider offered so far.

## Building and running

Needs a recent stable Rust (developed on 1.96) and, on Linux, the usual GPUI
dependencies (X11 or Wayland, Vulkan).

```sh
cargo run --release
```

To find facts and generate résumés, open **Settings → AI**, add a provider
and paste its API key.

Data lives in the platform's data directory: `~/.local/share/kron` on Linux,
`~/Library/Application Support/kron` on macOS, `%APPDATA%\kron` on Windows.

### Development

- Debug builds use `kron-dev/` instead and start with a sample persona (Alex
  Rahman) so there's something to look at. `KRON_NO_SAMPLE=1` starts empty,
  to see onboarding. Ctrl+D opens the dev overlay.
- To run without touching your own data, point `XDG_DATA_HOME` at a scratch
  folder.
- `cargo test` runs the model, database and UI tests. Tests that call a real
  model are ignored by default: `cargo test live_extraction -- --ignored
  --nocapture` uses the provider set up in the development data.

See [ARCHITECTURE.md](ARCHITECTURE.md) for how it's built, and
[CLAUDE.md](CLAUDE.md) for code conventions.
