# Improvement plan

From the user-journey review of 2026-11-15. It fixes, in this order: first
impressions, AI access without a key, trust in the checker, and release
readiness. Then it adds features a chat app can't copy.

Two tracks can run side by side:
- **Track A** (phases 1 → 2 → 4): first impression and AI access.
- **Track B** (phase 3): the checker.
- Phase 5 comes after both.

Every fix gets a regression test.

## Decisions made

- **Free credits:** an anonymous allowance for each install, and more after an
  optional email sign-up. Asking for an email up front would undercut the
  privacy pitch and lose people at the verification step.
- **The relay comes before Sign in with ChatGPT.** ChatGPT sign-in ships only
  through OpenAI's official route, with Kron's own client identity. Don't use
  rig's `providers::chatgpt`, because it signs in as Codex.
- **The default dark palette is Moss.** Linen & Sage stays the light default.
- **Extraction worst case is about 200K tokens per run.** The relay caps each
  request: input at 100K, so Haiku's above-100K price never applies, and output
  too.

## Phase 0: Measure (days)

- [ ] **Measure the cost per new user.** Run the live extraction and generation
  tests on two or three real-sized CVs and read `llm_runs`. This sizes the free
  allowance, and replaces the typical figures on the AI setup step ("$5 is the
  least it takes", "a few cents").
- [ ] **Find what pushes an extraction toward 200K tokens:** long documents,
  retries, or the prompt itself. A two-page CV is about 2K tokens.
- [ ] **Confirm Sign in with ChatGPT access** in OpenAI's developer docs.
- [ ] **Decide the licence.**

## Phase 1: The first two minutes (Track A), done

**Goal:** a new user sees the "Left out" moment before being asked for
anything, and gets their first résumé without opening Settings.

Branch `phase-1-first-minutes`:
- [x] **An example job in release builds** (`81138d5`). "See an example" on the
  welcome screen opens Alex's résumé for Cinder Labs, built in databases of its
  own under `<data>/example`, so it never touches the user's data.
- [x] **A welcome line that answers "why not ChatGPT"** (`81138d5`): "A chatbot
  can promise not to make things up. Kron checks every line of your résumé
  against your own work, and shows you what it kept off."
- [x] **The AI choice before the privacy review** (`d15bb96`). It recommends
  OpenAI with a three-step guide to getting a key. "Use another provider" opens
  Settings, and "Skip" carries on without one.
- [x] **A guided first résumé** (`7b4fddd`). After the name step it asks for a
  job, writes the résumé, and shows how to read it until a line is selected.
- [x] **The cost of each run** (`799f60f`), shown after finding facts and after
  writing a résumé, and in the job header. It's hidden when a call wasn't
  priced.

Still open:
- [x] Checked end to end with a real DeepSeek key: finding facts and writing a
  résumé took about 2 seconds and cost under 1¢ each.
- [x] "Show in Facts" in the example opens Alex's own facts, inside the
  example; a résumé under "Used in" opens back in the example at that line.
- [x] The note on how to read a résumé shows on any of the user's résumés,
  after restarts too, until they select a line or close it. It's saved in
  `settings`, so it syncs with the rest of the user's data.
- [ ] Once the relay ships, "Start free" goes first on the AI step and the own
  key moves under Advanced.

**Done when:** a fresh install reaches the example in under 30 seconds, and its
own first résumé without visiting Settings.

## Phase 2: AI without a key (Track A)

- [ ] **The Kron relay and free credits.**
  - An open-source relay that forwards redacted text and stores only token
    counts.
  - An anonymous device token on first launch, rate-limited for each token and
    each IP address, with the per-request token caps.
  - An allowance sized from phase 0, and a clear message when it runs out.
  - An optional email sign-up for more credits, buying credits, or restoring
    them on a new device. If reinstalling for fresh credits becomes abuse, add
    a small proof-of-work step or a lower limit for each IP address.
  - A short privacy page that says exactly what passes through.
- [ ] **Sign in with ChatGPT.**
  - An OAuth sign-in through the browser, with tokens kept in the keyring and
    refreshed quietly.
  - Requests use OpenAI's Responses API with `store: false` and streaming.
  - Redaction is unchanged.
  - Needs a Go, Plus, Pro or Business plan.
- [ ] **The setup screen order:** Start free, then Sign in with ChatGPT, then
  the user's own key (Advanced), then AI on this computer (Advanced).
- [ ] **Hide the per-task model pickers** under Advanced until a second service
  is added.
- [ ] **Say what paid for a run:** "used your free credits" or "used your
  ChatGPT plan", next to the cost from phase 1.

## Phase 3: Catch scope inflation (Track B)

Test set first. Each rule only adds rejections, and each is measured on
`cases.txt` and `generated.txt` before it's trusted.

- [ ] **Label new cases:** job-description echo, facts combined across jobs,
  verb inflation, negation, and invented terms.
- [ ] **Combined facts must share a job or project.** A line may combine facts
  from one entry only. It's the cheapest rule, since citations already carry
  their facts.
- [ ] **Job-description echo.** A word in `new_words` that also appears in the
  job description means rejection. `verify` needs the description passed in.
- [ ] **Negation and modality.** Words like not, never, without, helped to and
  planned, present in a fact and missing from the line, or the other way round.
- [ ] **The ownership-verb ladder** (assisted < built < led, owned, managed,
  architected), and unsupported "team of N". Label the scale as Kron's own.
- [ ] **A term check against ESCO and Wikidata.**
- [ ] **Three states in the inspector:** Checked, Reworded (with the new words
  highlighted), and Left out.
- [ ] **Generation returns the words of each fact it used.** Optionally, limit
  bullets to a menu of edits (keep, trim, merge within an entry, a verb no
  higher on the ladder). Compare fluency against the rejection rate.
- [ ] **The meaning check through the user's AI service,** last.

## Phase 4: Wording and polish (any time, low risk)

- [ ] **Wording:**
  - AI service; AI on this computer.
  - Write again.
  - Kept off your résumé.
  - Check these.
  - Your facts changed since this was written.
  - A one-line explanation of Career facts on the first visit.
  - Move the stand-in name under Privacy.
- [ ] **A reassuring heading** above the left-out list.
- [ ] **The "Set up AI" toast** is hidden on the AI settings page, and goes away
  once a service is added.
- [ ] **Keyboard hints** appear only after the first arrow key. "Keyboard
  shortcuts" moves to a Help menu and Ctrl+/.
- [ ] **The new job form:**
  - The description box comes first.
  - The placeholder becomes "e.g. …", in lighter text.
  - Role and company are filled in from the description, for the user to
    confirm.
- [ ] **Preview zoom:** Ctrl + / − / 0, and "fit width".
- [ ] **Re-join broken lines** in "Check the text": join a line that doesn't end
  in `.`, `:` or `;` to the next one when it starts in lowercase.
- [ ] **Make Moss the default dark palette** (`src/appearance/palette.rs`).

## Phase 5: Release readiness

- [ ] macOS Developer ID signing and notarization, Windows code signing, and
  Flatpak or AppImage on Linux.
- [ ] Signed automatic updates.
- [ ] A LICENSE file at the root.
- [ ] A short privacy statement covering local data, redaction, and the relay.

## Phase 6: What chat apps can't copy

- [ ] **An ATS self-check.** Run Kron's own PDF reader on the exported file and
  show what an application system will read. Warn on two-column templates.
- [ ] **Interview prep from what was sent.** Show the résumé that was sent, the
  facts and quotes behind each line, and a story outline for each line. Show it
  on "Interview in 3 days".
- [ ] **What a job asks for that the user's facts don't show:** each
  requirement no fact covers, with "Add a fact if true". Not a score.
- [ ] **A record that grows:**
  - a gentle monthly check-in;
  - importing LinkedIn's data export;
  - optionally, local git history.
- [ ] **A consistency check** between the LinkedIn export and the user's facts,
  such as dates and titles.
- [ ] **An open export format** for all of a user's facts.

## Later (already decided)

- DOCX, image and other exports.
- A browser extension.
- A Claude Desktop extension, with Kron's facts and checker as tools in the
  chat app.
- Exploring career paths: a guided conversation, ESCO links, evidence and gaps.
  Kept separate from this plan.
