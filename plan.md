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
- **Sign in with ChatGPT comes before the relay** (changed 2026-11-21). It
  needs no server, and the relay may come last. It ships only through OpenAI's
  official route, with Kron's own client identity. Don't use rig's
  `providers::chatgpt`, because it signs in as Codex.
- **The default dark palette is Moss.** Linen & Sage stays the light default.
- **Extraction worst case is about 200K tokens per run.** The relay caps each
  request: input at 100K, so Haiku's above-100K price never applies, and output
  too.

## Phase 0: Measure (days)

- [x] **Measure the cost per new user** (2026-11-21, `generation::tests::live_cost`
  on 33 real CVs of 1–3 pages, 220–940 words, on DeepSeek Flash). Finding the
  facts: 1.2–2.2K tokens in, 1.3–3.8K out (median 1.4K and 2.2K), 4–12 s.
  The first résumé: 1.3–3K in, 0.4–1.3K out, 2–5 s. A new user's two calls cost
  $0.002 on DeepSeek Flash (at most $0.0035), $0.016 on GPT-5.4 mini (at most
  $0.025) and $0.018 on Claude Haiku 4.5. Output is most of it: the facts are
  written out longer than the CV. The setup step now says "Reading a CV costs
  a cent or two, and writing a résumé less than a cent".
- [x] **What pushes an extraction toward 200K tokens:** not a CV. The largest
  real one took 2.2K in, so 200K needs a much longer document or retries.
- [x] **A Figma CV read as no text.** Figma's PDF export maps one character to
  an empty string in its ToUnicode CMap (`<65> <>`); hayro-cmap (0.1.0 and the
  latest 0.1.1) rejects the entry and with it the whole map, so every glyph
  came without text and the import failed with `NoText`. 1 of the 33 real CVs.
  Fixed in a patched copy, `vendor/hayro-cmap` (see its `KRON.md`); it now
  gives 33 facts.
- [ ] **Drop `vendor/hayro-cmap`** once a hayro-cmap release reads empty CMap
  entries; check on each hayro upgrade. Not reported upstream (decided
  2026-11-22).
- [x] **Confirm Sign in with ChatGPT access** in OpenAI's developer docs
  (checked 2026-11-21). There's a self-serve route for "open-source projects,
  personal projects that run locally": see phase 1's ChatGPT section.
- [ ] **Decide the licence.**

## Phase 1: The first two minutes (Track A)

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

### Sign in with ChatGPT (moved from phase 2; built and tried with a real account)

Lets a user with a paid ChatGPT plan use Kron with no API key. It runs inside
Kron, with no server of ours. From OpenAI's docs, read 2026-11-21:
[open-source overview](https://developers.openai.com/siwc/token-sharing-open-source),
[cookbook](https://developers.openai.com/cookbook/articles/sign-in-with-chatgpt),
[models and inference](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference),
[errors](https://developers.openai.com/siwc/token-sharing-open-source/errors-and-recovery),
[preview limitations](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations),
[help article](https://help.openai.com/en/articles/20001542-using-your-chatgpt-plan-in-other-apps-and-sites).

What the docs say:
- **Eligibility:** self-serve for "open-source projects, personal projects that
  run locally, and selected private apps". "Paid or remotely hosted" apps join
  a waitlist instead. Kron fits; whether paid relay credits later would change
  that isn't stated.
- **No registration step:** the first sign-in uses
  `client_id=dynamic_agent_client` with an app name and a host ID Kron makes
  and keeps; OpenAI issues a client ID (`oaiapp_…`) to save and reuse. No
  client secret, no API key.
- **The flow:** OAuth with PKCE (S256), state and nonce, and a redirect to
  `http://127.0.0.1:<port>/…` (not `localhost`; only the port may vary). Code
  exchange at `auth.openai.com`, with `resource=https://api.openai.com/v1`.
- **Scopes:** `openid profile email offline_access resource.invoke
  chatgpt.tokens.use.direct`. Signing in alone doesn't grant plan use: check
  the granted scopes, and offer an API key if the user declined.
- **Tokens:** about an hour, with a rotating refresh token, kept in the
  keyring.
- **Requests:** `POST /v1/responses` with the token, `store: false` and
  `stream: true`. Models come from `GET /v1/models`, not a hardcoded name.
  `temperature`, `max_output_tokens` and a system-role message are rejected:
  Kron sets the first two today, so this provider leaves them out, and the
  preamble goes in `instructions`.
- **Plans:** Go, Plus, Pro and Business (the cookbook says Plus and Pro). The
  user sets a weekly cap for each app in ChatGPT's settings.
- **Errors to handle:** `subscription_sharing_usage_limit_exceeded` (429: say
  so, and link to ChatGPT settings → Usage), `…_user_not_eligible` (403),
  `…_usage_unavailable` (503: retry later), `…_unsupported_capability` (400),
  and refresh errors that mean signing in again.
- **The UI:** a "Continue with ChatGPT" button following OpenAI's branding
  guidelines, and "Using ChatGPT plan" with a "Manage usage" link while it's
  in use.
- **Not stated:** whether request content is kept or used for training, and
  any preview or end date.

To do:
- [x] Sign-in: browser, loopback listener, PKCE, client ID and host ID kept,
  tokens in the keyring, quiet refresh. (`src/llm/chatgpt.rs`)
- [x] A ChatGPT provider: Responses API with streaming, models from the
  account, without the rejected fields. rig's OpenAI client does it, on its
  Responses route.
- [x] "Continue with ChatGPT" first on the AI setup step, with the key guide
  second; in Settings too, with Manage usage and Sign out.
- [x] The errors above, in words a user can act on; "Using your ChatGPT
  plan" on its row, and no cost shown.
- [x] Try it with a real ChatGPT account: sign in, read a CV, write a résumé,
  sign out. (2026-11-21: facts in 11.6 s, a résumé in 7.7 s, on GPT-6.2-Sol,
  the account's first model. The plan's models refuse reasoning effort
  `none`, and its streamed replies name no content type; both fixed.)
- [x] Compare models (2026-11-21, the four test CVs and one résumé each).
  Every ChatGPT model took 33–47 s per CV and 19–36 s per résumé; DeepSeek
  Flash took 4–5 s and 3 s. On the plan the speed is the route's, not the
  model's: the fastest (GPT-6.1 Terra, GPT-6-Luna, GPT-6-Sol) save about 5 s on
  GPT-6.2-Sol, so the account's first model stays the default. Recall was
  95–100% on every ChatGPT model and 98–100% on DeepSeek Flash and V4 Flash;
  DeepSeek V4 Pro was slower and missed more (89–100%). No résumé made a claim
  its facts didn't back. GPT-5.6-Terra once answered in JSON Kron couldn't
  read. Re-run with `KRON_LIVE_VENDOR=… KRON_LIVE_MODEL=… cargo test
  live_extraction -- --ignored --nocapture`.
- [x] Read OpenAI's guidelines (2026-11-21): the SIWC UI/UX guidelines, the
  brand guidelines, and the help article on using a plan in other apps. There
  are no terms for this route beyond OpenAI's general ones. Done from them: the
  approved button (black with the white logo, or white with the black logo on
  dark), "You're using your ChatGPT plan" once after the first sign-in, "Using
  ChatGPT plan" with Manage usage, and Manage usage as the action on a
  usage-limit error. Not applicable yet: plan pages (Kron has no paid plans)
  and the banner for users signed in another way (Kron has no accounts).
- [ ] Before adding paid relay credits: OpenAI's docs say paid or remotely
  hosted apps use the interest form, not this open-source route.

**Done when:** a fresh install reaches the example in under 30 seconds, and its
own first résumé without visiting Settings.

Checked 2026-11-21 on a release build with an empty data folder: the example
was on screen 0.75 s after launch. With the CV pasted, a ChatGPT sign-in, the
privacy review, the name and a pasted job, the first résumé came 2 min 20 s
later without visiting Settings: 40 s of it signing in, 36 s finding 67 facts
(filled in by the job step meanwhile) and 20 s writing. Every line was backed.
The first attempt failed as the computer's DNS dropped for a moment, so
requests that never reached OpenAI are now retried.

## Phase 2: Free credits without a key (Track A; may move last)

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
- [ ] **Check the relay doesn't cost Kron its ChatGPT route.** OpenAI's
  self-serve route is for open-source and local apps; "paid or remotely hosted"
  ones join a waitlist. Ask OpenAI before selling credits.
- [ ] **The setup screen order:** Start free, then Continue with ChatGPT, then
  the user's own key (Advanced), then AI on this computer (Advanced).
- [ ] **Hide the per-task model pickers** under Advanced until a second service
  is added.
- [ ] **Say what paid for a run:** "used your free credits" or "used your
  ChatGPT plan", next to the cost from phase 1.

## Phase 3: Catch scope inflation (Track B)

Test set first. Each rule only adds rejections, and each is measured on
`cases.txt` and `generated.txt` before it's trusted.

- [x] **Label new cases:** job-description echo, facts combined across jobs,
  verb inflation, negation, and invented terms. `cases.txt` now names each
  fact's entry (`fact E1:`) and the job (`job:`), with honest counterparts
  for each new label. Before any rule (2026-11-22), the verifier let
  through all of them: 6 of 6 cross_entry, 6 of 6 job_echo, 7 of 7
  dropped_qualifier, 12 of 12 scope_inflation, 6 of 6 invented_term, with
  no false flags on the 16 honest counterparts.
- [x] **Combined facts must share a job or project.** A line may combine facts
  from one entry only. It's the cheapest rule, since citations already carry
  their facts. Before, a bullet's citations of other entries were dropped and
  what it took from them stayed; now such a bullet is left out as "Mixes in
  work from another entry" (`other_entry`). A cited fact the bullet takes no
  words from is still only dropped. cross_entry: 6 of 6 caught, no false
  flags on combined (2026-11-22).
- [x] **Job-description echo.** A word in `new_words` that also appears in the
  job description means rejection. `verify` needs the description passed in.
  `verifier::echo` checks every line against the job's title, company and
  ad; a few plain verbs (improved, reduced, developed…) and "years" may be
  taken. job_echo: 6 of 6 caught, no false flags on job_wording
  (2026-11-22). `generated.txt` has no jobs, so real output isn't measured
  yet: re-run `write_test_set` to see how often real lines echo.
- [x] **Negation and modality.** Words like not, never, without, helped to and
  planned, present in a fact and missing from the line, or the other way round.
  A line must keep one of its facts' hedges (proposed, planned, started,
  tried, prototyped, working towards…), matched as whole words, and may not
  add a not, no, never, without, zero or none. A fact's own negation may go.
  "Helped to" is left to the verb ladder. dropped_qualifier: 7 of 7 caught,
  no false flags on kept_qualifier or the 93 real lines (2026-11-22).
- [x] **The ownership-verb ladder** (assisted < built < led, owned, managed,
  architected), and unsupported "team of N". Label the scale as Kron's own.
  A line keeps its facts' assisted, helped, contributed or shadowed (with
  the hedges), and says led, owned, managed, ran… only where a fact does.
  An unsupported "team of N" is already an invented figure. scope_inflation:
  9 of 12 caught, no false flags on same_scope or the real lines
  (2026-11-22). Left: found → fixed, a guest lecture → a course, wrote a
  guide → set the standards; no word list catches those.
- [ ] **A term check against ESCO and Wikidata.** Left for later (2026-11-22):
  neither gives every skill its synonyms (ESCO's are formal phrasings,
  Wikidata's crowd-added), Kron would have to bundle an extract, and all it
  would fix is a check mark on an alias like "Kubernetes" for "k8s".
- [x] **Three states in the inspector:** Checked, Reworded (with the new words
  highlighted), and Left out. On the page, a reworded line's new words have
  a highlight underlined darker, as Grammarly marks; it's never exported.
  Checked has no mark and is said only in the inspector, to keep the page
  clear (2026-11-22).
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
- [x] **A new default dark palette:** Graphite, in place of Orchid's violet
  (2026-11-22; migration `0006_kron.sql` moves installs still on Orchid).

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
- [ ] **A fit check for a job:** the job's requirements, each shown as covered
  (with the facts behind it) or not shown (with "Add a fact if true"), and a
  coarse estimate from them on the job page before writing: Strong fit,
  Partial fit or Stretch. An estimate, never a percentage.
- [ ] **Test résumés for poor-fit jobs** with the fit check: an engineer's
  facts for a bartender's ad and the other way round. They put the most
  pressure on the job-echo rule, which a matching job hardly tests.
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
