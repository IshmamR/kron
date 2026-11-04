# Native functionality → Gallery

Audited November 3, 2026. Source of truth is the current Rust implementation and its workspace tests, rather than the older proposal notes. For example, **PDF export is already implemented** in `job_view.rs`; an earlier design note saying it was absent is stale.

**Interactive** means the action changes sample state in the prototype. **Simulated** means its UI and local outcome are represented, while the native integration or algorithm is not executed. **Specified** means the behavior remains an explicit native implementation requirement. These labels describe the prototype, not the implementation status of the existing app.

## Navigation and library

| Existing functionality | Gallery location / treatment | Prototype | Native source |
| --- | --- | --- | --- |
| Résumés, Facts, Import, Settings | Persistent sidebar; job editor returns to the gallery | Interactive | [workspace.rs](../../src/workspace.rs) |
| Collapsible sidebar and shortcut | Titlebar sidebar button; ⌘/Ctrl B | Interactive | `Workspace::toggle_sidebar` |
| Undo / Redo across changes | Sidebar actions, notification Undo, ⌘/Ctrl Z and Y; native text-field history takes precedence | Interactive | `Workspace::undo_or_redo` |
| Keyboard shortcut reference | Sidebar keyboard button and ⌘/Ctrl / | Interactive | [shortcuts.rs](../../src/workspace/shortcuts.rs) |
| Restore last page/job, sidebar, window bounds | Keep the native device settings; do not derive window behavior from the mockup's aspect ratio | Specified | `Workspace::restore_page`, `save_page`, `save_window` |
| All / Active / Interviewing / Offers / Closed, counts and filter shortcuts | Application groups in sidebar; ⌘/Ctrl 1–5 in Gallery | Interactive | [resume_grid.rs](../../src/workspace/resume_grid.rs) |
| Thumbnails, missing résumé, multiple pages, changed-fact and claim indicators | Existing Gallery composition with real sample status and health | Interactive; thumbnail images are static sample captures | `ResumeGrid::render_card` |
| Arrow selection and Enter open | Focus follows grid geometry; Enter opens the document | Interactive | `ResumeGrid::move_selection` |
| Add job: title, company, optional link, description, validation | New job sheet includes all four fields and length/required constraints | Interactive | [job_form.rs](../../src/workspace/job_form.rs) |
| Empty and filtered-empty states | Distinct initial-library and no-results treatments | Interactive | `ResumeGrid::render` |

Search and company sorting in Gallery are proposal additions, not claims about existing native features.

## Résumé, evidence and generation

| Existing functionality | Gallery location / treatment | Prototype | Native source |
| --- | --- | --- | --- |
| Résumé / Application tabs and shortcuts | Compact job header, ⌘/Ctrl 1 and 2 | Interactive | [job_view.rs](../../src/workspace/job_view.rs) |
| Generate / Regenerate with optional remembered instructions | Header action opens instructions sheet; previous wording remains during failure | Simulated AI, interactive sheet | `JobView::ask_to_generate`, `generate` |
| No provider / no facts prerequisite | Settings or Import guidance instead of a dead action | Interactive state | `JobView::generate` |
| Writing in progress, completion, failure, retry | Writing label, notifications, error and retained prior version | Simulated | `JobView::generate` |
| Modern, Classic, Sidebar and A4 / US Letter | Canvas controls change presentation without changing wording | Interactive HTML approximation | `UseTemplate`, `UsePaper`, [typeset.rs](../../src/typeset.rs) |
| PDF export, filename, success/failure | Export PDF sheet identifies current/sent version, template and paper | Simulated; no fake PDF is downloaded | `JobView::export`, `pdf_name` |
| Select résumé lines; arrows; Enter edits; Escape clears then leaves | Paper hit areas and Source editor; no mutually exclusive evidence tabs | Interactive | `JobView::move_selection`, `edit_line`, `back` |
| Editable wording, save on Enter/leaving, cancel, Revert to generated wording | Persistent Source editor; generated wording and contextual warning | Interactive | `sync_line_editor`, `save_line`, `Inspector::line_editor` |
| Multiple cited facts and original excerpts | Source iterates actual sample citations and all their sources | Interactive | [inspector.rs](../../src/workspace/inspector.rs) |
| Words introduced by a rewording | Distinct marked-word treatment with judgment guidance | Simulated highlighting/validation; native verifier remains authoritative | `Inspector::new_words`, [verifier.rs](../../src/model/verifier.rs) |
| Changed, hidden, needs-review or missing cited fact | Status within Source plus document-level changed-facts notice | Interactive for sample changes; missing-fact treatment specified in renderer | `ShownResume::change`, `Inspector::sources` |
| Unsupported claims remain visible alongside Source | Persistent second inspector section, independently scrollable | Interactive | `Inspector::render` |
| Add unsupported claim as a fact | Add as fact → resolved state → regeneration can use it | Interactive sample mutation | `Inspector::unsupported_claim`, `commands::add_claim_as_fact` |
| Restore hidden supporting fact | Show fact → shown-again state → regenerate | Interactive | `commands::set_hidden` |
| Exact sent résumé distinct from current | Version picker, read-only notice, View sent / Back to current | Interactive sample versioning | `sent_notice`, `JobView::show_version` |
| Sent version preserves template/paper and blocks editing/repair | Disabled layout controls and non-editable Source/claims actions; PDF export remains available | Interactive | `template_bar`, `Inspector::line_editor` |
| Resizable inspector, independent scroll, multi-page document | Divider supports pointer drag and arrow keys; paper and inspector scroll separately | Interactive; exact native Typst pagination specified | `h_resizable`, `render_page` |
| Typesetting failure and recovery | Failure state with a route to the Modern template | Simulated | `JobView::render_resume` |

## Career facts

| Existing functionality | Gallery location / treatment | Prototype | Native source |
| --- | --- | --- | --- |
| Grouped facts, kind, entry, status, All / Needs review / Hidden | Facts list, filters, search and detail panel | Interactive, based on all 83 sample facts | [facts_page.rs](../../src/workspace/facts_page.rs), [fact_list.rs](../../src/workspace/fact_list.rs) |
| Search and arrow navigation; selected-result behavior | Search remains in place while list/detail update | Interactive | `FactList::set_query`, `FactsPage::focus_list` |
| Edit, save, multiline, cancel and extracted-wording Revert | Detail editor retains original extraction | Interactive | `sync_fact_editor`, `save_edit`, `revert_fact` |
| Review reason and confirm-next, including last item | Review notice and Confirm and next action | Interactive | `confirm_fact` |
| Hide / Show with Delete/Backspace | Detail action and hidden-state explanation | Interactive | `toggle_hidden` |
| Add a note, one fact per line, grouping, original source | Add facts panel; ⌘/Ctrl Return saves; Escape cancels | Interactive | `render_add`, `add_note` |
| One or multiple sources with original context | Sources list and View excerpt in context | Interactive | `source_excerpt`, `sources` |
| Résumé usage backlinks including changed-fact indication | Used in links open the exact résumé line and relevant version | Interactive | `OpenLine`, `Detail::read` |
| Possible duplicates | Duplicate wording and Merge action in detail | Interactive fixture case | `possible_duplicate`, `merge_with` |
| Multi-select merge, choose wording, preserve sources/citations | Explicit merge panel, checkboxes/⌘-click, number keys, confirm/cancel and Undo | Interactive | `render_merge`, `KeepWording`, `merge` |

## Applications

| Existing functionality | Gallery location / treatment | Prototype | Native source |
| --- | --- | --- | --- |
| Stage, upcoming interview, quiet application / follow-up nudge | Application header and gallery metadata | Interactive sample computation | [application_view.rs](../../src/workspace/application_view.rs), [application.rs](../../src/model/application.rs) |
| Log Applied, Recruiter replied, Interview, Offer, Rejected, Withdrew, No response, Note | Log sheet exposes all event kinds; inapplicable transitions are disabled | Interactive | `ApplicationView::render_status` |
| Date/time, interview kind and participants, optional note | Conditional fields in event sheet | Interactive | `render_form` |
| Applied records which résumé was sent | Résumé selector in event sheet; sent panel opens that version | Interactive | `ApplicationView::save` |
| Required note/date validation | Inline error keeps the sheet open | Interactive | `ApplicationView::save` |
| Timeline editing/deletion and Undo | Actions on each event | Interactive | `render_timeline`, `delete` |
| Mark as no response | Follow-up notice action | Interactive | `log_now` |
| Posting link, full description, sent/current distinction | Application's persistent job detail column | Interactive; opening external postings is a normal browser link | `render_details` |

## Import, onboarding and settings

| Existing functionality | Gallery location / treatment | Prototype | Native source |
| --- | --- | --- | --- |
| File picker, drop, PDF/Markdown/text, multiple-file queue | Import landing page and per-file Read / Skip / Skip rest | Text files interactive; PDF parsing simulated | [import_page.rs](../../src/workspace/import_page.rs) |
| Duplicate import prompt | Per-file confirmation before text review | Simulated filename case; native uses file hashes | `ImportPage::next`, `render_ask` |
| Correct extracted text before saving | Check the text editor; empty text is rejected | Interactive | `render_confirm`, `save` |
| Paste / write text | Text editor with ordinary system paste; no automatic clipboard read | Interactive editor; native clipboard integration specified | `Paste`, `Write` |
| Privacy preview, hidden identifiers, restore individual detail, additional phrase | Hide your details step with toggleable marks and Also hide field | Interactive presentation; exact native redaction specified | [redaction_check.rs](../../src/workspace/redaction_check.rs) |
| Not now / saved-without-extraction and later Find facts | Saved-document reminder and route back to privacy review | Interactive sample state | `Workspace::render_unread`, `ImportPage::find_facts` |
| Extraction progress, review results and retry | Finding facts notification and Show facts route | Simulated sample extraction; no API call | `ImportPage::extract` |
| First import, skip, confirmed name | First launch and name-confirmation states | Interactive preview state | [onboarding.rs](../../src/workspace/onboarding.rs) |
| System / Light / Dark and motion preferences | Settings → General; preset gallery is the selected design addition | Interactive local preference | `settings_page`, [appearance.rs](../../src/appearance.rs) |
| AI provider/model, key save/remove/status | Settings → AI; key sheet explicitly accepts placeholder data only | Simulated keyring; no credential persistence | [ai_settings.rs](../../src/workspace/ai_settings.rs) |
| Stand-in name save and rejection when it appears in facts | Settings → AI field with validation | Interactive sample validation | [stand_in_name.rs](../../src/workspace/stand_in_name.rs) |

## Native-only requirements kept open

- Exact Typst rendering, PDF bytes and system save dialogs, including disk errors and cancellation.
- Real extraction/generation, redaction/verifier algorithms, provider timeouts and token/cost recording.
- Keyring missing/locked/save/remove failures; never store real keys in this prototype.
- Window restoration, OS titlebars/menus, native drag/drop, motion preferences, focus semantics and actual macOS validation.
- Native transaction/history semantics and simultaneous updates from other windows.

These are integration/verification boundaries. They are not permission to remove the corresponding UI actions or simplify the native app's data model.

## Audit evidence

`cargo test --offline workspace:: -- --nocapture`: **110 passed**, including claim repair, multiple source/usage queries, review/merge/hide, exact-line navigation, PDF naming, sent versions, import correction/privacy, application history and keyboard behavior.

The browser pass exercised the assembled preview inside its sandboxed iframe and CSP. It verified:

- A new job, its first generated sample résumé, application logging and an older read-only sent version after regeneration.
- Line editing with an invented figure warning, Revert, keyboard Undo after saving, and simultaneous Source / Unsupported claims.
- A hidden supporting fact restored, a claim added as a fact, regeneration instructions, and the repaired claim appearing on the new sample résumé.
- Fact edits marking résumé usages as changed; exact-line backlinks; duplicate merging retaining both original sources; Undo restoring the separate facts.
- Fact search selecting a matching detail, review confirmation decrementing the remaining count, and the shortcut sheet making background controls inert.
- Corrected text → saved source → hidden-name/contact review → an additional hidden phrase → sample extraction → review results.
- Linen selection, motion preference, the explicitly simulated provider-key sheet, and unchanged wording after switching template and paper.

Observed desktop content widths included roughly 1,004, 1,407 and 1,726 CSS pixels with no horizontal overflow in the inspected views. Both inspector headings remained present in the editor. The initial compact Gallery view was also inspected. Full small-width, assistive-technology and native macOS verification remain open. Browser error logs were empty at the end of these walkthroughs.

The browser check caught and fixed an issue the earlier preview tests missed: sandboxed frames block native form transport. The preview now validates fields and dispatches local sheet actions in JavaScript; the sandbox and CSP remain intact. Native integration boundaries listed above remain explicit.
