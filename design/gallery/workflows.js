(() => {
  const root = document.getElementById('kron-gallery-workflows');
  const app = root.querySelector('.kr-gallery');
  const main = app.querySelector('[data-main]');
  const overlays = app.querySelector('[data-overlay-slot]');
  const palettes = JSON.parse(root.querySelector('#kr-palettes').textContent);
  const fixture = JSON.parse(root.querySelector('#gw-sample').textContent);
  let data = structuredClone(fixture);
  const past = [], future = [];
  const NOW = Date.UTC(2026, 10, 3, 6);
  const templateKeys = { '01a16db87ec272ed8e791686548e0043':'modern', '01a16db87ec473b0a60a381ca9e50c47':'classic', '01a16db87ec6739284524a71f048a231':'sidebar' };
  const templateIds = Object.fromEntries(Object.entries(templateKeys).map(([id,key])=>[key,id]));
  const filters = [['all','All applications'],['active','Active'],['interviewing','Interviewing'],['offers','Offers'],['closed','Closed']];
  const labels = {wishlisted:'Added to library',applied:'Applied',recruiter_replied:'Recruiter replied',interview:'Interview',offer:'Offer',rejected:'Rejected',withdrawn:'Withdrew',no_response:'No response',note:'Note'};
  const saved = window.openai?.widgetState?.privateContent || {};
  const ui = {
    page:'resumes', editor:false, job:null, version:null, line:null, filter:'all', search:'', sort:'activity',
    fact:null, factFilter:'all', factSearch:'', marked:[], keep:null, addFacts:false,
    theme:palettes.some(p=>p.id===saved.theme)?saved.theme:'system', appearance:'system', motion:'system', collapsed:false,
    inspectorWidth:320, settingsTab:'general', provider:true, model:'deepseek-flash', standIn:'Jordan Ellis',
    modal:null, lineDraft:null, factDraft:null, queue:[], queueTotal:0, importing:{step:'choose',text:'',name:null,document:null,hidden:[],kept:[]},
    busy:null, problem:null, scenario:'Library', onboarding:null, name:'Alex Rahman', toast:null
  };
  const uid = () => 'preview-' + Date.now().toString(36) + '-' + Math.random().toString(36).slice(2,7);
  const escape = value => String(value ?? '').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
  const icon = name => `<i data-lucide="${name}" aria-hidden="true"></i>`;
  const button = (label,action,value='',kind='kr-control',extra='') => `<button type="button" class="${kind} cursor-interaction" data-action="${action}" data-value="${escape(value)}" ${extra}>${label}</button>`;
  const options = (items,current) => items.map(([value,label])=>`<option value="${value}" ${value===current?'selected':''}>${escape(label)}</option>`).join('');
  const date = (time,withTime=false) => new Intl.DateTimeFormat('en-US',{month:'short',day:'numeric',...(withTime?{hour:'numeric',minute:'2-digit'}:{}),timeZone:'Asia/Dhaka'}).format(new Date(time));
  const title = value => value.charAt(0).toUpperCase()+value.slice(1);
  function icons() { if(globalThis.lucide) lucide.createIcons({attrs:{width:16,height:16}}); }
  function live(rows) { return rows.filter(row=>!row.deleted_at); }
  function facts() { return live(data.facts).filter(f=>!f.merged_into); }
  function resolve(id) { let f=data.facts.find(f=>f.id===id);for(let i=0;f?.merged_into&&i<64;i++)f=data.facts.find(next=>next.id===f.merged_into);return f; }
  function job() { return data.jobs.find(j=>j.id===ui.job); }
  function versions(jobId=ui.job) { return live(data.generations).filter(r=>r.job_id===jobId).sort((a,b)=>b.generated_at-a.generated_at); }
  function current(jobId=ui.job) { return versions(jobId)[0]; }
  function shown() { return data.generations.find(r=>r.id===ui.version)||current(); }
  function readOnly() { return !!ui.version && ui.version!==current()?.id; }
  function events(jobId=ui.job) { return live(data.events).filter(e=>e.job_id===jobId).sort((a,b)=>b.at-a.at); }
  function sent(jobId=ui.job) { return events(jobId).find(e=>e.kind==='applied')?.resume_id; }
  function lines(resumeId=shown()?.id) {
    const sections=live(data.sections).filter(s=>s.generation_id===resumeId).sort((a,b)=>a.sort_key.localeCompare(b.sort_key));
    return sections.flatMap(section=>live(data.blocks).filter(b=>b.section_id===section.id).sort((a,b)=>a.sort_key.localeCompare(b.sort_key)).flatMap(block=>live(data.lines).filter(l=>l.block_id===block.id).sort((a,b)=>a.sort_key.localeCompare(b.sort_key))));
  }
  function cited(lineId) { return data.line_citations.filter(c=>c.line_id===lineId).sort((a,b)=>a.sort_key.localeCompare(b.sort_key)).map(c=>resolve(c.fact_id)).filter(Boolean); }
  function claims(resumeId=shown()?.id) { return live(data.unsupported_claims).filter(c=>c.generation_id===resumeId); }
  function changed(resumeId) { return data.fact_versions.filter(v=>v.generation_id===resumeId).filter(v=>{const f=resolve(v.fact_id);return !f||f.text!==v.text||!!f.hidden!==!!v.hidden;}); }
  function sources(factId) { return live(data.fact_sources).filter(s=>s.fact_id===factId); }
  function entryLabel(fact) {
    if(!fact.entry_id)return 'Other facts';
    const peers=facts().filter(f=>f.entry_id===fact.entry_id);
    return [peers.find(f=>f.kind==='title')?.text,peers.find(f=>f.kind==='organization')?.text].filter(Boolean).join(' · ')||title(data.entries.find(e=>e.id===fact.entry_id)?.kind||'Facts');
  }
  function usedIn(factId) {
    const ids=data.line_citations.filter(c=>resolve(c.fact_id)?.id===factId).map(c=>c.line_id);
    return live(data.generations).flatMap(r=>lines(r.id).filter(l=>ids.includes(l.id)).map(line=>({line,resume:r,job:data.jobs.find(j=>j.id===r.job_id)})));
  }
  function stage(jobId) {
    const list=events(jobId), latest=list.find(e=>e.kind!=='note'&&e.at<=NOW);
    const closed=['rejected','withdrawn','no_response'].includes(latest?.kind);
    const interview=!closed&&list.find(e=>e.kind==='interview');
    if(!closed&&latest?.kind==='offer')return {kind:'offers',label:'Offer'};
    if(closed)return {kind:'closed',label:labels[latest.kind]};
    if(interview)return {kind:'interviewing',label:(interview.at>NOW?'Interview · ':'Interviewed · ')+date(interview.at)};
    return {kind:'active',label:latest?.kind==='wishlisted'||!latest?'Not applied':labels[latest.kind]};
  }
  function nudge(jobId) {
    const s=stage(jobId),list=events(jobId),interview=list.find(e=>e.kind==='interview');
    if(s.kind==='interviewing'&&interview?.at>NOW)return `Interview in ${Math.max(1,Math.floor((interview.at-NOW)/86400000))} days`;
    if(s.kind==='closed'||s.kind==='offers')return null;
    const last=list.find(e=>e.at<=NOW&&e.kind!=='note'&&e.kind!=='wishlisted');
    const days=last?Math.floor((NOW-last.at)/86400000):0;
    return days>=7?`${last.kind==='interview'?'No news':'No reply'} in ${days} days`:null;
  }
  function filteredJobs(filter=ui.filter) {
    return live(data.jobs).filter(j=>{const s=stage(j.id);return (filter==='all'||(filter==='active'?s.kind!=='closed':s.kind===filter))&&(!ui.search||(j.title+' '+j.company).toLowerCase().includes(ui.search.toLowerCase()));}).sort((a,b)=>ui.sort==='company'?a.company.localeCompare(b.company):(events(b.id)[0]?.at||0)-(events(a.id)[0]?.at||0));
  }
  function toast(message) { ui.toast=message;renderToast(); }
  function renderToast() {
    app.querySelector('.gw-toast')?.remove();
    if(ui.toast)app.insertAdjacentHTML('beforeend',`<div class="gw-toast" role="status"><span>${escape(ui.toast)}</span>${past.length?button('Undo','undo','','kr-link'):''}${button(icon('x'),'dismiss-toast','','kr-tool','aria-label="Dismiss notification"')}</div>`);
    icons();
  }
  function mutate(label,change) {
    past.push({snapshot:JSON.stringify(data),label});if(past.length>16)past.shift();future.length=0;
    change();render();toast(label);persist();
  }
  function undo(redo=false) {
    const from=redo?future:past,to=redo?past:future,item=from.pop();if(!item)return;
    to.push({snapshot:JSON.stringify(data),label:item.label});data=JSON.parse(item.snapshot);ui.marked=[];ui.lineDraft=null;ui.factDraft=null;render();toast((redo?'Redid: ':'Undid: ')+item.label);
  }
  function persist() {
    window.openai?.setWidgetState?.({modelContent:{design:'Gallery workflows',page:ui.page,company:job()?.company||null,theme:ui.theme},privateContent:{theme:ui.theme}})?.catch(()=>{});
  }
  function applyTheme() {
    const dark=ui.appearance==='dark'||(ui.appearance==='system'&&matchMedia('(prefers-color-scheme:dark)').matches);
    const p=palettes.find(p=>p.id===ui.theme)||palettes.find(p=>p.id===(dark?'orchid':'linen')),c=p.colors;
    const roles={bg:'bg',side:'sidebar',surface:'surface',soft:'surface2',line:'border',hair:'border-soft',ink:'ink',secondary:'secondary',accent:'accent',selection:'accent2','on-accent':'accent-text',warning:'warm',error:'red',shadow:'shadow'};
    Object.entries(roles).forEach(([role,key])=>app.style.setProperty('--kr-'+role,c[key]));
    if(p.appearance==='Light')app.style.setProperty('--kr-secondary',`color-mix(in srgb, ${c.secondary} 80%, ${c.ink})`);
    app.style.colorScheme=p.appearance.toLowerCase();app.dataset.theme=p.id;app.classList.toggle('gw-collapsed',ui.collapsed);
  }
  function renderNav() {
    app.querySelector('.kr-side-titlebar').innerHTML=`<div class="kr-traffic" aria-hidden="true"><span></span><span></span><span></span></div><div class="kr-spacer"></div>${button(icon('panel-left'),'sidebar','','kr-tool','aria-label="Toggle sidebar"')}`;
    app.querySelector('nav').innerHTML=[['resumes','files','Résumés',live(data.jobs).length],['facts','book-open','Career facts',facts().filter(f=>f.review).length||''],['import','square-arrow-down','Import','']].map(([page,glyph,label,count])=>`<button class="kr-nav cursor-interaction" data-action="page" data-value="${page}" aria-label="${label}" ${ui.page===page?'aria-current="page"':''}>${icon(glyph)}<span class="gw-nav-label">${label}</span><span class="kr-count">${count}</span></button>`).join('');
    const filterHost=app.querySelector('[data-filters]');filterHost.hidden=ui.page!=='resumes'||ui.editor;
    filterHost.innerHTML=filters.map(([filter,label])=>`<button class="kr-nav kr-filter-nav cursor-interaction" data-action="filter" data-value="${filter}" aria-pressed="${ui.filter===filter}">${label}<span class="kr-count">${live(data.jobs).filter(j=>{const s=stage(j.id);return filter==='all'||(filter==='active'?s.kind!=='closed':s.kind===filter);}).length}</span></button>`).join('');
    app.querySelectorAll('.kr-side-heading')[1].hidden=filterHost.hidden;
    app.querySelector('.kr-side-bottom').innerHTML=`${button(icon('settings-2')+'<span class="gw-nav-label">Settings</span>','page','settings','kr-nav','aria-label="Settings"')}<div class="gw-history">${button(icon('undo-2'),'undo','','kr-tool',`aria-label="Undo" ${past.length?'':'disabled'}`)}${button(icon('redo-2'),'redo','','kr-tool',`aria-label="Redo" ${future.length?'':'disabled'}`)}<span class="kr-spacer"></span>${button(icon('keyboard'),'shortcuts','','kr-tool','aria-label="Keyboard shortcuts"')}</div><div class="kr-person"><span class="kr-avatar">AR</span><div>${escape(ui.name)}<small>Personal library</small></div></div>`;
  }
  function render() {
    applyTheme();renderNav();
    app.classList.toggle('gw-onboarding',!!ui.onboarding);
    if(ui.onboarding)renderOnboarding();
    else if(ui.page==='facts')renderFacts();
    else if(ui.page==='import')renderImport();
    else if(ui.page==='settings')renderSettings();
    else if(ui.editor)renderEditor();
    else renderGallery();
    renderToast();icons();
  }
  function renderGallery() {
    main.innerHTML=`<header class="kr-library-bar"><h1>${ui.filter==='all'?'Résumés':filters.find(([id])=>id===ui.filter)[1]}</h1><span class="kr-heading-count">${filteredJobs().length}</span><span class="kr-spacer"></span><label class="kr-search">${icon('search')}<input data-input="job-search" aria-label="Search applications" placeholder="Search" value="${escape(ui.search)}"></label><select class="kr-sort" data-setting="job-sort" aria-label="Sort applications">${options([['activity','Last activity'],['company','Company']],ui.sort)}</select>${button(icon('plus')+'New job','new-job','','kr-tool')}</header>${unreadBanner()}<section class="kr-scroll" style="flex:1" aria-label="Résumés"><div class="kr-document-grid" data-grid></div></section><footer class="kr-library-status"><span>${filteredJobs().length} applications</span><span class="kr-spacer"></span><span class="kr-secondary">↑ ↓ ← → select · Return opens</span></footer>`;
    renderTiles();
  }
  function renderTiles() {
    const crops={'Ledgerly':[259,108],'Cinder Labs':[492,108],'Atlas Freight':[958,108],'Brightwave':[1191,108],'Quillpay':[1424,108],'Northstar Metrics':[1657,108],'Kestrel Labs':[259,499],'Tidewater Systems':[492,499]};
    const host=main.querySelector('[data-grid]');if(!host)return;
    main.querySelector('.kr-heading-count').textContent=filteredJobs().length;
    main.querySelector('.kr-library-status > span').textContent=filteredJobs().length+' applications';
    host.innerHTML=filteredJobs().map(j=>{const r=current(j.id),s=stage(j.id),crop=crops[j.company]||[259,108],count=claims(r?.id).length,n=nudge(j.id);
      return `<button class="kr-document cursor-interaction" data-action="open-job" data-value="${j.id}" aria-label="Open ${escape(j.company)}: ${escape(j.title)}"><div class="kr-preview">${r?`<div class="kr-mini-paper" aria-hidden="true" style="--kr-sx:${crop[0]};--kr-sy:${crop[1]}"></div>${j.company==='Atlas Freight'?'<span class="kr-page-stack">2 pages</span>':''}`:`<div class="kr-blank-paper">${icon('file-text')}<span>No résumé yet</span></div>`}</div><h2 class="kr-tile-title">${escape(j.title)}</h2><p class="kr-tile-company">${escape(j.company)}</p><div class="kr-tile-status"><span class="kr-status-dot kr-${s.kind}"></span>${escape(s.label)}</div><div class="kr-tile-health">${count?`<span class="kr-claim-color">${icon('circle-alert')}${count} unsupported</span>`:''}${changed(r?.id).length?'<span class="kr-warning">Facts changed</span>':''}${n?`<span class="kr-secondary">${escape(n)}</span>`:''}</div></button>`;
    }).join('')||`<div class="gw-empty" style="grid-column:1/-1"><h2>${data.jobs.length?'No matching applications':'Your library starts here'}</h2><p>${data.jobs.length?'Choose another filter or search.':'Add a job to create its résumé from your career facts.'}</p>${button('New job','new-job')}</div>`;
    icons();
  }
  function openJob(id,line=null,resume=null) {
    saveFactDraft();saveLineDraft();ui.job=id;ui.page='resumes';ui.editor=true;ui.version=resume;ui.line=line;ui.mode='resume';ui.problem=null;render();persist();
    if(line)main.querySelector(`[data-line="${line}"]`)?.scrollIntoView({block:'center'});
  }
  function renderEditor() {
    if(!job()){ui.editor=false;renderGallery();return;}
    const j=job(),r=shown(),ro=readOnly(),s=stage(j.id);
    main.innerHTML=`<div class="kr-editor"><header class="gw-toolbar">${button(icon('chevron-left'),'back','','kr-tool','aria-label="Back to gallery"')}<div class="kr-document-name"><h1>${escape(j.company)}</h1><p>${escape(j.title)} · ${escape(s.label)}</p></div><span class="kr-spacer"></span><div class="kr-segment" role="group" aria-label="Job view">${button('Résumé','mode','resume','',`aria-pressed="${ui.mode!=='application'}"`)}${button('Application','mode','application','',`aria-pressed="${ui.mode==='application'}"`)}</div>${button(ui.busy==='generation'?'Writing…':r?'Regenerate':'Generate','generate','','kr-control',ro||ui.busy==='generation'?'disabled':'')}${button(icon('download')+'Export PDF','export','','kr-control',r?'':'disabled')}</header><div data-editor-body style="display:flex;flex:1;min-height:0;flex-direction:column"></div></div>`;
    if(ui.mode==='application'){renderApplication();return;}
    const host=main.querySelector('[data-editor-body]');
    host.innerHTML=`<div class="kr-editor-layout" style="grid-template-columns:minmax(0,1fr) min(${ui.inspectorWidth}px,38%)"><section class="kr-canvas">${r?`<div class="kr-canvas-tools"><select data-setting="template" aria-label="Résumé template" ${ro?'disabled':''}>${options([['modern','Modern'],['classic','Classic'],['sidebar','Sidebar']],templateKeys[r.template_version_id]||'modern')}</select><span class="kr-divider"></span><select data-setting="paper" aria-label="Paper size" ${ro?'disabled':''}>${options([['a4','A4'],['letter','US Letter']],r.paper)}</select><span class="kr-spacer"></span><select data-setting="version" aria-label="Résumé version">${options([['','Current version'],...(sent()?[ [sent(),'Sent · '+date(data.generations.find(r=>r.id===sent())?.generated_at||NOW)] ]:[])],ui.version||'')}</select></div>`:''}<div class="kr-paper-stage kr-scroll" data-paper-stage>${ro?`<div class="gw-notice"><p>Sent version · Read only. The wording, template and paper are kept as sent.</p>${button('Back to current','version-current','','kr-link')}</div>`:r&&sent()&&sent()!==r.id?`<div class="gw-notice"><p>The résumé you sent is older than this version.</p>${button('View sent','version-sent','','kr-link')}</div>`:''}${!ro&&changed(r?.id).length?`<div class="gw-notice"><p>${changed(r.id).length} facts changed after this résumé was generated. Your wording stays until you regenerate.</p></div>`:''}${ui.problem?`<div class="gw-error" role="alert">${escape(ui.problem)} ${button('Try again','generate','','kr-link')}</div>`:''}${!r?`<div class="gw-empty">${icon('file-text')}<h2>No résumé for this job yet</h2><p>Write a draft from your career facts, tailored to this job.</p>${button('Generate a résumé','generate')}</div>`:ui.scenario==='Typesetting error'?`<div class="gw-empty"><h2>Couldn’t typeset the résumé</h2><p>The template could not render this document.</p>${button('Use Modern template','fix-typeset')}</div>`:paperMarkup(r)}</div></section><aside class="kr-inspector" aria-label="Evidence and unsupported claims"><section class="kr-source-section kr-scroll" aria-label="Source"><div class="kr-row"><h2>Source</h2><span class="kr-spacer"></span><span class="kr-caption">${ui.line?cited(ui.line).length+(cited(ui.line).length===1?' fact':' facts'):''}</span></div><div data-source-content></div></section><section class="kr-claims-section kr-scroll" aria-label="Unsupported claims"><h2>Unsupported claims <span class="kr-badge">${claims(r?.id).length}</span></h2><div data-claims-content></div></section><button class="gw-resizer" data-resizer aria-label="Resize evidence panel"></button></aside></div>`;
    renderSource();renderClaims();
    // Narrow conversation previews stack evidence below paper; desktop keeps both panes.
    if(app.getBoundingClientRect().width<=800)host.querySelector('.kr-editor-layout').style.gridTemplateColumns='minmax(0,1fr)';
  }
  function paperMarkup(resume) {
    const sections=live(data.sections).filter(s=>s.generation_id===resume.id).sort((a,b)=>a.sort_key.localeCompare(b.sort_key));
    const fragments=sections.flatMap(section=>{
      const blocks=live(data.blocks).filter(b=>b.section_id===section.id).sort((a,b)=>a.sort_key.localeCompare(b.sort_key));
      return blocks.map((block,index)=>`${index===0&&section.title?`<h3>${escape(section.title)}</h3>`:''}<div class="gw-block">${live(data.lines).filter(l=>l.block_id===block.id).sort((a,b)=>a.sort_key.localeCompare(b.sort_key)).map(line=>`<button class="kr-resume-line gw-role-${line.role} cursor-interaction" data-line="${line.id}" data-action="select-line" data-value="${line.id}" aria-pressed="${ui.line===line.id}" data-edited="${line.text!==line.generated}" data-outdated="${cited(line.id).some(f=>changed(resume.id).some(v=>resolve(v.fact_id)?.id===f.id))}">${escape(line.text)}</button>`).join('')}</div>`);
    });
    const count=job()?.company==='Atlas Freight'?2:1,cut=Math.ceil(fragments.length/count);
    return Array.from({length:count},(_,page)=>`<article class="kr-paper gw-paper" data-template="${templateKeys[resume.template_version_id]||'modern'}" aria-label="Résumé page ${page+1}" style="margin-top:${page?'26px':'0'};min-height:${resume.paper==='a4'?'760':'735'}px">${fragments.slice(page*cut,(page+1)*cut).join('')}</article><div class="kr-page-caption">Page ${page+1} of ${count} · ${title(templateKeys[resume.template_version_id]||'modern')} · ${resume.paper==='a4'?'A4':'US Letter'}</div>`).join('');
  }
  function lineProblem(line) {
    const evidence=cited(line.id).map(f=>f.text).join(' '),figures=line.text.match(/\b\d+(?:[.,]\d+)*(?:%|\s?ms)?/g)||[];
    return figures.find(figure=>!evidence.includes(figure));
  }
  function renderSource() {
    const host=main.querySelector('[data-source-content]');if(!host)return;
    const line=data.lines.find(l=>l.id===ui.line);
    if(!line){host.innerHTML='<p class="kr-secondary" style="font-size:12px;line-height:1.8;margin-top:15px">Select a line to see its cited facts and original excerpts. ↑ ↓ moves between lines.</p>';return;}
    const ro=readOnly(),problem=lineProblem(line),evidence=cited(line.id);
    const words=new Set(evidence.flatMap(f=>f.text.toLowerCase().match(/[\p{L}\p{N}]+/gu)||[]));
    const marked=line.text.split(/([\p{L}\p{N}]+)/u).map(word=>words.has(word.toLowerCase())||!/[\p{L}\p{N}]/u.test(word)?escape(word):`<mark class="gw-mark">${escape(word)}</mark>`).join('');
    host.innerHTML=`${ro?`<p class="kr-selected-wording">${escape(line.text)}</p>`:`<label class="kr-caption" style="display:block;margin-top:13px">Résumé wording<textarea class="kr-edit-area" data-input="line-wording" aria-label="Résumé wording" maxlength="4096">${escape(ui.lineDraft??line.text)}</textarea></label><div class="kr-row">${button('Save','save-line','','kr-control')}${button('Cancel','cancel-line','','kr-tool')}<span class="kr-caption">Return saves · Esc cancels</span></div>`}${line.text!==line.generated?`<div class="gw-generated">Generated as “${escape(line.generated)}” ${!ro?button('Revert','revert-line','','kr-link'):''}</div><div class="gw-notice"><p>${problem?`The cited facts don’t contain “${escape(problem)}”. Your edit stays as written; check it or revert.`:'You edited this line. Check that it still says only what its facts support.'}</p></div>`:''}<p class="kr-overline">Words the cited facts don’t contain</p><p style="font-size:12px;line-height:1.7">${marked}</p><p class="kr-caption" style="margin-top:7px">Marked wording needs your judgment; a rewording can be valid.</p>${evidence.length?evidence.map(f=>`<div class="gw-evidence-fact"><div class="kr-evidence"><p>${escape(f.text)}</p></div>${changed(shown()?.id).some(v=>resolve(v.fact_id)?.id===f.id)?'<p class="kr-warning kr-caption" style="margin-top:8px">Changed since this résumé was generated</p>':''}${f.hidden?'<p class="kr-warning kr-caption">This fact is hidden</p>':''}${f.review?'<p class="kr-warning kr-caption">Needs review</p>':''}${sources(f.id).map(s=>`<blockquote>“${escape(s.original_text)}”</blockquote><p class="kr-caption">${escape(data.documents.find(d=>d.id===s.document_id)?.file_name||'Your note')}</p>`).join('')}${button('Show in Career facts '+icon('arrow-up-right'),'show-fact',f.id,'kr-link')}</div>`).join(''):'<div class="gw-error">This line cites a fact that no longer exists.</div>'}`;
    icons();
  }
  function claimReason(claim) {
    if(claim.added_as)return 'Added to your facts. Regenerate to use it.';
    if(claim.hidden_fact_id&&!resolve(claim.hidden_fact_id)?.hidden)return 'The fact is shown again. Regenerate to use it.';
    return {invented_figure:`The cited facts don’t contain ${claim.figure}.`,invented_year:`The cited facts don’t support the year ${claim.year}.`,uses_hidden_fact:'This line needs a fact you have hidden.',no_supporting_fact:'No cited fact supports this claim.'}[claim.reason]||'Check the cited facts.';
  }
  function renderClaims() {
    const host=main.querySelector('[data-claims-content]');if(!host)return;
    host.innerHTML=claims().map(c=>`<div class="kr-claim"><p>${escape(c.text)}</p><p class="kr-reason">${escape(claimReason(c))}</p>${readOnly()?'':c.added_as?button('Show in Career facts','show-fact',c.added_as,'kr-link'):c.hidden_fact_id?button(resolve(c.hidden_fact_id)?.hidden?'Show fact':'Show in Career facts',resolve(c.hidden_fact_id)?.hidden?'unhide-claim':'show-fact',resolve(c.hidden_fact_id)?.hidden?c.id:c.hidden_fact_id,'kr-link'):button('Add as fact','claim-add',c.id,'kr-link')}</div>`).join('')||'<p class="kr-secondary" style="font-size:12px;line-height:1.8;margin-top:15px">No unsupported claims. Every generated line cites a fact.</p>';
  }
  function saveLineDraft() {
    if(ui.lineDraft===null)return;
    const line=data.lines.find(l=>l.id===ui.line),value=ui.lineDraft.trim();ui.lineDraft=null;
    if(!line||!value||value===line.text||readOnly())return;
    mutate('Résumé wording saved',()=>{line.text=value;});
  }
  function filteredFacts() {
    return facts().filter(f=>(ui.factFilter==='all'||(ui.factFilter==='review'?f.review:f.hidden))&&(!ui.factSearch||(f.text+' '+entryLabel(f)).toLowerCase().includes(ui.factSearch.toLowerCase())));
  }
  function renderFacts() {
    if(!resolve(ui.fact)||!filteredFacts().some(f=>f.id===resolve(ui.fact)?.id))ui.fact=filteredFacts()[0]?.id||null;
    main.innerHTML=`<div class="kr-facts"><header class="gw-toolbar"><h1>Career facts</h1><span class="kr-caption">${facts().length} facts</span><span class="kr-spacer"></span>${ui.marked.length>1?button('Merge '+ui.marked.length+' facts','merge-view','','kr-control'):''}${button(icon('plus')+'Add facts','add-facts','','kr-control')}</header><div class="kr-facts-body"><section style="border-right:1px solid var(--kr-hair);min-height:0;display:flex;flex-direction:column"><div class="gw-fact-toolbar"><div class="kr-segment" role="group" aria-label="Fact filters">${[['all','All',facts().length],['review','Needs review',facts().filter(f=>f.review).length],['hidden','Hidden',facts().filter(f=>f.hidden).length]].map(([value,label,count])=>button(label+' '+count,'fact-filter',value,'',`aria-pressed="${ui.factFilter===value}"`)).join('')}</div><label class="kr-search">${icon('search')}<input aria-label="Search facts" data-input="fact-search" value="${escape(ui.factSearch)}" placeholder="Search facts"></label></div><div class="kr-fact-list gw-fact-list-region" data-fact-list style="border:0;padding-left:14px"></div></section><section class="kr-fact-detail kr-scroll" data-fact-detail aria-label="Fact detail"></section></div></div>`;
    renderFactRows();renderFactDetail();
  }
  function renderFactRows() {
    const host=main.querySelector('[data-fact-list]');if(!host)return;
    const groups=new Map();filteredFacts().forEach(f=>{const label=entryLabel(f);if(!groups.has(label))groups.set(label,[]);groups.get(label).push(f);});
    host.innerHTML=[...groups].map(([label,rows])=>`<h2 class="kr-fact-group">${escape(label)}</h2>${rows.map(f=>`<div class="gw-fact-list-row"><input type="checkbox" data-mark="${f.id}" ${ui.marked.includes(f.id)?'checked':''} aria-label="Mark fact to merge: ${escape(f.text)}"><button class="kr-fact-row cursor-interaction" data-action="select-fact" data-value="${f.id}" aria-pressed="${ui.fact===f.id}">${escape(f.text)}<small>${f.review?'Needs review · ':''}${f.hidden?'Hidden · ':''}${title(f.kind)} · ${sources(f.id).length} source${sources(f.id).length===1?'':'s'}</small></button></div>`).join('')}`).join('')||'<p class="kr-secondary" style="padding:30px 10px">No facts match this view.</p>';
  }
  function renderFactDetail() {
    const host=main.querySelector('[data-fact-detail]');if(!host)return;
    if(ui.addFacts){host.innerHTML=`<h2 style="font:650 22px Arial,sans-serif">Add facts</h2><p class="kr-secondary" style="margin-top:10px;font-size:13px;line-height:1.8">Each line becomes a fact. Lines written together are grouped, and your note is kept as their source.</p><textarea class="gw-fact-editor" data-input="new-facts" aria-label="New facts" placeholder="One fact per line"></textarea><div class="kr-row">${button('Add','save-facts','','kr-control kr-primary')}${button('Cancel','cancel-facts','','kr-tool')}</div>`;return;}
    if(ui.merging&&ui.marked.length>1){renderMerge(host);return;}
    const f=resolve(ui.fact);if(!f){host.innerHTML='<p class="kr-secondary">Select a fact to review it.</p>';return;}
    const uses=usedIn(f.id),duplicates=facts().filter(other=>other.id!==f.id&&other.entry_id===f.entry_id&&other.kind===f.kind&&other.text.toLowerCase().includes('12 backend')&&f.text.toLowerCase().includes('12 backend'));
    host.innerHTML=`<div class="kr-caption">${escape(title(f.kind))} · ${escape(entryLabel(f))}</div><label class="kr-field">Fact text<textarea class="gw-fact-editor" data-input="fact-wording" aria-label="Fact text" maxlength="4096">${escape(ui.factDraft??f.text)}</textarea></label><div class="kr-row">${button('Save','save-fact','','kr-control')}${button('Cancel','cancel-fact','','kr-tool')}<span class="kr-caption">Return saves · Shift Return adds a line</span></div>${f.text!==f.extracted?`<div class="gw-generated">Extracted as “${escape(f.extracted)}” ${button('Revert','revert-fact',f.id,'kr-link')}</div>`:''}${f.review?`<div class="gw-notice"><p>Needs review: ${escape(f.review_note||'Check the original excerpt, correct the fact if needed, then confirm.')}</p></div>`:''}${f.hidden?'<div class="gw-notice"><p>Hidden. Future résumés will not use this fact.</p></div>':''}<div class="gw-fact-detail-actions">${f.review?button('Confirm and next','confirm-fact',f.id,'kr-control kr-primary'):''}${button(icon(f.hidden?'eye':'eye-off')+(f.hidden?'Show':'Hide'),'toggle-fact',f.id,'kr-control')}${button('Mark for merge','mark-fact',f.id,'kr-tool')}</div><h3>Sources · ${sources(f.id).length}</h3>${sources(f.id).map(s=>`<div class="gw-source"><p>“${escape(s.original_text)}”</p><small>${escape(data.documents.find(d=>d.id===s.document_id)?.file_name||'Your note')}</small>${button('View excerpt in context','source-context',s.id,'kr-link')}</div>`).join('')}<h3>Used in · ${uses.length} lines</h3>${uses.length?uses.map(use=>`<button class="kr-backlink cursor-interaction" data-action="backlink" data-value="${use.line.id}" style="width:100%;text-align:left;align-items:flex-start"><div>${escape(use.job?.company||'Résumé')}<p class="kr-secondary" style="font-size:11px;margin-top:5px">${escape(use.line.text)}</p>${changed(use.resume.id).some(v=>resolve(v.fact_id)?.id===f.id)?'<p class="kr-warning" style="font-size:11px;margin-top:5px">Generated before this change</p>':''}</div>${icon('arrow-up-right')}</button>`).join(''):'<p class="kr-secondary" style="font-size:12px">No résumé uses this fact yet.</p>'}${duplicates.length?`<h3>Possible duplicates</h3>${duplicates.map(other=>`<div class="gw-source"><p>${escape(other.text)}</p>${button('Merge…','merge-with',other.id,'kr-link')}</div>`).join('')}`:''}`;
  }
  function saveFactDraft() {
    if(ui.factDraft===null)return;
    const f=resolve(ui.fact),value=ui.factDraft.trim();ui.factDraft=null;
    if(f&&value&&value!==f.text)mutate('Fact saved',()=>{f.text=value;});
  }
  function renderMerge(host) {
    const marked=ui.marked.map(resolve).filter(Boolean);if(!marked.some(f=>f.id===ui.keep))ui.keep=marked.find(f=>!f.review)?.id||marked[0]?.id;
    host.innerHTML=`<h2 style="font:650 22px Arial,sans-serif">Merge ${marked.length} facts</h2><p class="kr-secondary" style="font-size:13px;margin-top:12px;line-height:1.8">Pick the wording to keep. All original sources and résumé citations stay attached to the kept fact.</p>${marked.map((f,index)=>`<label class="gw-merge-option"><input type="radio" name="merge-wording" value="${f.id}" data-keep ${ui.keep===f.id?'checked':''}><span><strong>${index+1}.</strong> ${escape(f.text)}<small class="kr-caption" style="display:block">${sources(f.id).length} sources · ${usedIn(f.id).length} résumé lines${f.review?' · Needs review':''}</small></span></label>`).join('')}<div class="gw-notice"><p>Existing résumé wording stays as generated. Undo can restore the separate facts.</p></div><div class="kr-row">${button('Merge','merge-confirm','','kr-control kr-primary')}${button('Cancel','merge-cancel','','kr-tool')}</div>`;
  }
  function renderApplication() {
    const j=job(),s=stage(j.id),r=shown(),posting=data.job_descriptions.find(d=>d.job_id===j.id)?.text||'',sentResume=data.generations.find(r=>r.id===sent()),n=nudge(j.id);
    main.querySelector('[data-editor-body]').innerHTML=`<div class="gw-split"><section class="kr-scroll"><div class="kr-application"><div class="kr-row"><h2>${escape(s.label)}</h2><span class="kr-spacer"></span>${button(icon('plus')+'Log','log-event','','kr-control')}</div><p>Keep the history of this application and the résumé you sent.</p>${n?`<div class="gw-notice"><p>${escape(n)}</p>${!n.startsWith('Interview')?button('Mark as no response','no-response','','kr-link'):''}</div>`:''}<div class="kr-timeline">${events().map(e=>`<div class="gw-event"><div class="kr-row"><h3>${e.kind==='interview'?title(e.interview_kind||'Other')+' interview':labels[e.kind]||title(e.kind)}${e.at>NOW?(s.kind==='closed'?' · Called off':' · Upcoming'):''}</h3><span class="kr-spacer"></span>${button('Edit','edit-event',e.id,'kr-tool')}${button('Delete','delete-event',e.id,'kr-tool')}</div><p>${date(e.at,e.kind==='interview')}</p>${e.interview_with?`<p>With ${escape(e.interview_with)}</p>`:''}${e.note?`<p>${escape(e.note)}</p>`:''}${e.resume_id?button('View résumé sent','event-resume',e.resume_id,'kr-link'):''}</div>`).join('')||'<p class="kr-secondary">Nothing logged yet.</p>'}</div></div></section><aside class="gw-job-details kr-scroll"><h2>Job</h2><p style="font-weight:600;color:var(--kr-ink)">${escape(j.title)}</p><p>${escape(j.company)}</p>${j.url?`<p style="margin:12px 0"><a href="${escape(j.url)}" target="_blank" rel="noopener noreferrer" style="color:var(--kr-accent)">Open the posting ↗</a></p>`:''}<p style="margin-top:15px">${escape(posting)}</p><h3>Sent résumé</h3>${sentResume?`<p>Generated ${date(sentResume.generated_at,true)}</p><p>${sentResume.id!==current()?.id?'Older than the current version.':'Same as the current version.'}</p>${button('View sent résumé','version-sent','','kr-link')}`:'<p>Nothing sent yet. Logging “Applied” records which résumé you sent.</p>'}</aside></div>`;
  }
  function renderSettings() {
    main.innerHTML=`<header class="gw-toolbar"><h1>Settings</h1><span class="kr-spacer"></span><div class="kr-segment" role="group" aria-label="Settings pages">${button('General','settings-tab','general','',`aria-pressed="${ui.settingsTab==='general'}"`)}${button('AI','settings-tab','ai','',`aria-pressed="${ui.settingsTab==='ai'}"`)}</div></header><section class="kr-scroll" style="flex:1"><div class="gw-settings">${ui.settingsTab==='general'?`<h2>Appearance</h2><div class="gw-setting"><div><h3>Appearance</h3><p>Follow your desktop, or choose light or dark.</p></div><select data-setting="appearance" aria-label="Appearance">${options([['system','Match system'],['light','Light'],['dark','Dark']],ui.appearance)}</select></div><div class="gw-setting"><div><h3>Motion</h3><p>Reduced motion stops transitions such as the sidebar sliding.</p></div><select data-setting="motion" aria-label="Motion">${options([['system','Match system'],['reduced','Reduced'],['full','Full']],ui.motion)}</select></div><h3 style="font-size:13px;margin-top:28px">Theme preset</h3><div class="kr-palette-grid">${palettes.map(p=>`<button class="kr-palette cursor-interaction" data-action="palette" data-value="${p.id}" aria-pressed="${ui.theme===p.id}"><span class="kr-palette-swatches">${['sidebar','surface','accent'].map(k=>`<span style="background:${p.colors[k]}"></span>`).join('')}</span>${escape(p.name)}</button>`).join('')}</div>${button('Use system theme','palette','system','kr-link','style="margin-top:16px"')}`:`<h2>AI</h2><div class="gw-setting"><div><h3>Provider</h3><p>Used to find facts and write résumés. Your name and contacts are hidden first.</p></div><select aria-label="Provider"><option>DeepSeek</option></select></div><div class="gw-setting"><div><h3>Model</h3><p>Choose the model used for the request.</p></div><input data-input="model" aria-label="Model" value="${escape(ui.model)}"></div><div class="gw-setting"><div><h3>API key</h3><p>${ui.provider?'A key is configured.':'No key is configured.'} The native app keeps it in the system keyring.</p></div><div class="kr-row">${button(ui.provider?'Replace key…':'Add key…','key-setup','','kr-control')}${ui.provider?button('Remove','key-remove','','kr-tool'):''}</div></div><div class="gw-setting"><div><h3>Your name is sent as</h3><p>A made-up name replaces yours in AI requests. Your name is put back in the response.</p></div><form data-form="stand-in"><input name="standIn" aria-label="Stand-in name" value="${escape(ui.standIn)}" required maxlength="100"><button class="kr-link cursor-interaction" type="submit">Save</button></form></div>`}</div></section>`;
  }
  function unreadBanner() {
    const unread=live(data.documents).filter(d=>d.preview_unread);if(!unread.length)return '';
    return `<div class="gw-notice" style="margin:0 25px 12px"><p>${unread.length} saved document${unread.length>1?'s':''} still need facts found.</p>${button(ui.provider?'Find facts':'Set up AI',ui.provider?'find-unread':'setup-ai',unread[0].id,'kr-link')}</div>`;
  }
  function renderImport() {
    const im=ui.importing;
    let content='';
    const steps=`<div class="gw-import-steps"><span class="${im.step==='choose'||im.step==='ask'?'gw-current':''}">1 · Choose</span><span class="${im.step==='edit'?'gw-current':''}">2 · Check text</span><span class="${im.step==='privacy'?'gw-current':''}">3 · Hide details</span><span class="${im.step==='done'?'gw-current':''}">4 · Review facts</span></div>`;
    if(im.step==='choose')content=`<h1>Import your experience</h1><p>Bring in a CV, project notes or anything else about your work.</p><div class="gw-drop" data-drop>${icon('file-up')}<h2>Drop your CV here</h2><p>PDF, Markdown or plain text · Files are read on your device</p>${button('Choose files…','browse-files')}<input type="file" data-files multiple accept=".pdf,.md,.markdown,.txt" hidden></div><div class="kr-row" style="margin-top:22px">${button('Write text','write-import','','kr-control')}${button('Paste text','paste-import','','kr-control')}<span class="kr-spacer"></span></div>${unreadBanner()}`;
    else if(im.step==='ask')content=`<p class="kr-caption">File ${ui.queueTotal-ui.queue.length} of ${ui.queueTotal}</p><h1>${escape(im.name)}</h1><p>${im.duplicate?'You have imported a file with this name before. Check that this is the version you want.':'Read this file and check its extracted text before saving.'}</p><div class="kr-row" style="margin-top:25px">${button('Read it','read-import','','kr-control kr-primary')}${button('Skip','skip-import','','kr-control')}${ui.queue.length?button('Skip the rest','skip-rest','','kr-tool'):''}</div>`;
    else if(im.step==='pdf')content=`<h1>${escape(im.name)}</h1><div class="gw-notice"><p>This design preview does not extract PDF text. The native app does. Use the sample CV to inspect the text-review and privacy steps.</p></div><div class="kr-row">${button('Use sample CV text','sample-import','','kr-control')}${button('Choose another file','skip-import','','kr-tool')}</div>`;
    else if(im.step==='edit')content=`<h1>Check the text</h1><p>${im.name?'Text read from '+escape(im.name)+'. ':''}Correct anything that’s wrong. Facts are taken from this corrected text; nothing has been sent.</p><textarea data-input="import-text" aria-label="Source text">${escape(im.text)}</textarea><div class="kr-row">${button('Cancel','skip-rest','','kr-tool')}${ui.queue.length?button('Skip file','skip-import','','kr-tool'):''}<span class="kr-spacer"></span>${button('Save text','save-import','','kr-control kr-primary')}</div>`;
    else if(im.step==='privacy')content=`<h1>Hide your details</h1><p>Highlighted details will be replaced before the AI request. Click a detail to send it as written; click again to hide it.</p><p class="kr-caption">Name sent as ${escape(ui.standIn)} · Company names and job descriptions are sent as written.</p><div class="gw-private-text">${privacyMarkup(im)}</div><form data-form="hide-phrase" class="kr-row"><label class="kr-field" style="flex:1;margin:0">Also hide<input name="phrase" placeholder="A person, username or phrase" required></label><button type="submit" class="kr-control cursor-interaction" style="align-self:end">Hide</button></form><div class="kr-row" style="margin-top:24px">${button('Not now','privacy-later','','kr-tool')}<span class="kr-spacer"></span>${button('Confirm and find facts','extract','','kr-control kr-primary')}</div>`;
    else content=`<div class="gw-empty">${icon('check')}<h2>Ready to review</h2><p>${im.found||0} facts were added. Check anything marked for review against its original source.</p><div class="kr-row">${button('Show facts','import-facts','','kr-control kr-primary')}${ui.queue.length?button('Next file','skip-import','','kr-control'):button('Import more','skip-rest','','kr-tool')}</div></div>`;
    main.innerHTML=`<section class="kr-scroll" style="flex:1"><div class="gw-import">${steps}${ui.problem?`<p class="gw-error" role="alert">${escape(ui.problem)}</p>`:''}${content}</div></section>`;
  }
  function privacyMarkup(im) {
    const builtins=[[ui.name||'Alex Rahman',ui.standIn],...((im.text.match(/[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}/gi)||[]).map(t=>[t,'[email]'])),...((im.text.match(/(?:https?:\/\/|github\.com\/|linkedin\.com\/)[^\s·]+/g)||[]).map(t=>[t,'[link]'])),...((im.text.match(/\+\d[\d ()-]{7,}\d/g)||[]).map(t=>[t,'[phone]'])),...im.hidden.map(t=>[t,'[hidden]'])];
    im.tokens=[...new Map(builtins.map(t=>[t[0],t])).values()].filter(([word])=>im.text.includes(word));
    if(!im.tokens.length)return escape(im.text);
    const re=new RegExp('('+im.tokens.map(([t])=>t.replace(/[.*+?^${}()|[\]\\]/g,'\\$&')).sort((a,b)=>b.length-a.length).join('|')+')','g');
    return im.text.split(re).map(part=>{const token=im.tokens.find(([t])=>t===part);return token?`<button data-action="privacy-token" data-value="${escape(part)}" aria-pressed="${!im.kept.includes(part)}" data-tooltip="${im.kept.includes(part)?'Sent as written':'Sent as '+escape(token[1])}">${escape(part)}</button>`:escape(part);}).join('');
  }
  function nextImport() {
    const file=ui.queue.shift();ui.problem=null;
    ui.importing=file?{step:'ask',text:file.text||'',file,name:file.name,document:null,hidden:[],kept:[],duplicate:data.documents.some(d=>d.file_name===file.name)}:{step:'choose',text:'',name:null,document:null,hidden:[],kept:[]};render();
  }
  function renderOnboarding() {
    main.innerHTML=`<div class="gw-welcome"><h1>${ui.onboarding==='name'?'What should we call you?':'Welcome to Kron'}</h1><p>${ui.onboarding==='name'?'Confirm the name found in your first import. It is hidden from AI requests and restored on your résumé.':'Start with your experience. Import a CV or write a few notes, then build résumés from facts you can trace.'}</p>${ui.onboarding==='name'?`<form data-form="name"><label class="kr-field">Your name<input name="name" value="${escape(ui.name)}" placeholder="Your name" required></label><div class="kr-row" style="margin-top:23px"><button class="kr-control kr-primary cursor-interaction" type="submit">Confirm</button>${button('Skip','onboarding-skip','','kr-tool')}</div></form>`:`<div class="kr-row">${button('Import your CV','onboarding-import','','kr-control kr-primary')}${button('Skip for now','onboarding-skip','','kr-tool')}</div>`}</div>`;
  }
  function modal(titleText,body,footer='',form='') {
    ui.focusReturn=document.activeElement;ui.modal=form||titleText;
    main.inert=true;app.querySelector('.kr-sidebar').inert=true;
    overlays.innerHTML=`<div class="kr-overlay"><${form?'form':'section'} class="kr-dialog gw-dialog-wide" role="dialog" aria-modal="true" aria-label="${escape(titleText)}" ${form?`data-form="${form}"`:''}><div class="kr-row"><h2>${escape(titleText)}</h2><span class="kr-spacer"></span>${button(icon('x'),'close-modal','','kr-tool','aria-label="Close dialog"')}</div>${body}<div class="kr-dialog-footer">${footer||button('Done','close-modal')}</div></${form?'form':'section'}></div>`;
    icons();overlays.querySelector('input,textarea,select,button')?.focus();
  }
  function closeModal() { overlays.innerHTML='';ui.modal=null;main.inert=false;app.querySelector('.kr-sidebar').inert=false;if(ui.focusReturn?.isConnected)ui.focusReturn.focus(); }
  // The preview iframe intentionally disallows navigation/form transport.
  // Keep validation and local form actions without enabling form submission.
  function localSubmit(form) {
    if(form&&form.reportValidity())form.dispatchEvent(new Event('submit',{bubbles:true,cancelable:true}));
  }
  function newJobForm() {
    modal('New job',`<div class="gw-form-grid"><label class="kr-field">Title<input name="title" required maxlength="256" placeholder="Senior Backend Engineer"></label><label class="kr-field">Company<input name="company" required maxlength="256" placeholder="Company"></label></div><label class="kr-field">Link (optional)<input type="url" name="url" maxlength="2048" placeholder="https://…"></label><label class="kr-field">Description<textarea name="description" required maxlength="20480" placeholder="Paste the job description"></textarea></label>`,button('Cancel','close-modal')+'<button class="kr-control kr-primary cursor-interaction" type="submit">Add job</button>','new-job');
  }
  function generateForm() {
    saveLineDraft();
    if(!ui.provider){ui.settingsTab='ai';ui.page='settings';render();toast('Add an AI provider before generating.');return;}
    if(!facts().some(f=>!f.hidden)){toast('Import your CV or add some facts first.');return;}
    modal(current()?'Regenerate the résumé':'Generate a résumé',`<p class="kr-intro">Steer what it leads with or leaves out. Instructions cannot add facts.</p><label class="kr-field">Instructions (optional)<textarea name="instructions" maxlength="2048" placeholder="Lead with the open-source work. Keep it to one page.">${escape(current()?.instructions||'')}</textarea></label>`,button('Cancel','close-modal')+'<button class="kr-control kr-primary cursor-interaction" type="submit">Generate</button>','generate');
  }
  function eventForm(event=null,kind='note') {
    const selected=event?.kind||kind;ui.editingEvent=event?.id||null;
    const local=new Date((event?.at||NOW)+6*3600000).toISOString().slice(0,16);
    const applied=events().some(e=>e.kind==='applied'),closed=stage(ui.job).kind==='closed';
    modal(event?'Edit event':'Log application event',`<label class="kr-field">Event<select name="kind" data-event-kind aria-label="Event">${Object.entries(labels).filter(([k])=>k!=='wishlisted').map(([key,label])=>`<option value="${key}" ${key===selected?'selected':''} ${!event&&((key==='applied'&&applied)||(['rejected','withdrawn','no_response'].includes(key)&&closed))?'disabled':''}>${label}</option>`).join('')}</select></label><label class="kr-field">When<input type="datetime-local" name="at" value="${local}" required></label><div data-interview-fields ${selected==='interview'?'':'hidden'}><div class="gw-form-grid"><label class="kr-field">Kind<select name="interviewKind" aria-label="Interview kind">${options([['phone','Phone'],['technical','Technical'],['onsite','Onsite'],['other','Other']],event?.interview_kind||'technical')}</select></label><label class="kr-field">With<input name="with" value="${escape(event?.interview_with||'')}"></label></div></div><label class="kr-field" data-resume-field ${selected==='applied'?'':'hidden'}>Résumé sent<select name="resume" aria-label="Résumé sent"><option value="">No résumé</option>${versions().map(r=>`<option value="${r.id}" ${(event?.resume_id||current()?.id)===r.id?'selected':''}>Generated ${date(r.generated_at,true)}${r.id===current()?.id?' · Current':''}</option>`).join('')}</select></label><label class="kr-field">Note${selected==='note'?'':' (optional)'}<textarea name="note">${escape(event?.note||'')}</textarea></label><p class="gw-error" data-form-error role="alert"></p>`,button('Cancel','close-modal')+'<button class="kr-control kr-primary cursor-interaction" type="submit">Save event</button>','event');
  }
  function showShortcuts() {
    const groups={General:[['Toggle sidebar','⌘ B'],['Undo / Redo','⌘ Z / ⌘ Y'],['Keyboard shortcuts','⌘ /']],Gallery:[['Move between résumés','↑ ↓ ← →'],['Open selection','Return'],['Switch application filter','⌘ 1–5']],Résumé:[['Move between lines','↑ ↓'],['Edit selected line','Return'],['Save wording / cancel','Return / Esc'],['Résumé / Application','⌘ 1 / ⌘ 2'],['Clear selection, then back','Esc']],Facts:[['Search','⌘ F'],['Confirm and next','Return'],['Hide / show','Delete'],['Mark for merge','⌘ click or checkbox'],['Choose wording / merge','1–9 / Return'],['Save note','⌘ Return']],Import:[['Write text','⌘ N'],['Save text / confirm privacy','⌘ Return'],['Cancel','Esc']]};
    modal('Keyboard shortcuts',Object.entries(groups).map(([name,rows])=>`<h3 style="font-size:13px;margin:20px 0 9px">${name}</h3>${rows.map(([label,key])=>`<div class="kr-row" style="font-size:12px;padding:5px 0"><span>${label}</span><span class="kr-spacer"></span><span class="kr-secondary">${key}</span></div>`).join('')}`).join(''));
  }
  function setScenario(value) {
    closeModal();data=structuredClone(fixture);past.length=0;future.length=0;ui.scenario=value;ui.page='resumes';ui.editor=false;ui.onboarding=null;ui.problem=null;ui.toast=null;ui.line=null;ui.version=null;ui.marked=[];ui.provider=value!=='No AI';
    if(value==='First launch'){Object.keys(data).forEach(key=>{data[key]=[];});ui.onboarding='welcome';ui.firstNameConfirmed=false;ui.name='';ui.provider=false;}
    if(value==='Empty library')data.jobs=[];
    if(value==='Typesetting error'||value==='Generation error'){ui.job=data.jobs.find(j=>j.company==='Cinder Labs').id;ui.editor=true;ui.mode='resume';if(value==='Generation error')ui.problem='The model’s response could not be read. Your previous résumé is still here.';}
    if(value==='Import duplicate'){ui.page='import';ui.queue=[{name:'alex_rahman_cv.pdf',text:data.document_texts[0].text}];ui.queueTotal=1;nextImport();return;}
    render();
  }
  function simulateDraft(jobId,instructions) {
    const previous=current(jobId),next={...(previous||{}),id:uid(),job_id:jobId,kind:'resume',generated_at:NOW+past.length,template_version_id:previous?.template_version_id||templateIds.modern,paper:previous?.paper||'a4',instructions:instructions||null};
    data.generations.push(next);
    function addLine(block,role,text,factIds) {
      const id=uid();data.lines.push({id,block_id:block,role,sort_key:'a'+data.lines.length,generated:text,text});
      factIds.forEach((fact,index)=>data.line_citations.push({line_id:id,fact_id:fact,sort_key:'a'+index}));
    }
    function addSection(kind,heading) {
      const section={id:uid(),generation_id:next.id,sort_key:'a'+data.sections.length,kind,title:heading};data.sections.push(section);return section;
    }
    function addBlock(section,kind='entry') {
      const block={id:uid(),section_id:section.id,sort_key:'a'+data.blocks.length,kind};data.blocks.push(block);return block;
    }
    if(previous) {
      for(const section of live(data.sections).filter(s=>s.generation_id===previous.id)) {
        const copy={...section,id:uid(),generation_id:next.id};data.sections.push(copy);
        for(const block of live(data.blocks).filter(b=>b.section_id===section.id)) {
          const cloned={...block,id:uid(),section_id:copy.id};data.blocks.push(cloned);
          for(const line of live(data.lines).filter(l=>l.block_id===block.id)) {
            const evidence=cited(line.id);if(evidence.some(f=>f.hidden))continue;
            const text=evidence.length===1&&['bullet','name'].includes(line.role)?evidence[0].text:line.generated;
            addLine(cloned.id,line.role,text,evidence.map(f=>f.id));
          }
        }
      }
      const resolved=claims(previous.id).filter(c=>c.added_as||(c.hidden_fact_id&&!resolve(c.hidden_fact_id)?.hidden));
      if(resolved.length) {
        const section=addSection('custom','Additional experience'),block=addBlock(section,'item');
        resolved.forEach(c=>addLine(block.id,'item',c.text,[resolve(c.added_as||c.hidden_fact_id).id]));
      }
      claims(previous.id).filter(c=>!resolved.includes(c)).forEach(c=>data.unsupported_claims.push({...c,id:uid(),generation_id:next.id}));
    } else {
      let name=facts().find(f=>f.kind==='name');
      if(!name) {
        const document=uid();name={id:uid(),kind:'name',text:ui.name,extracted:ui.name,hidden:0,review:null,entry_id:null};
        data.documents.push({id:document,file_name:null});data.document_texts.push({document_id:document,text:ui.name});data.facts.push(name);data.fact_sources.push({id:uid(),fact_id:name.id,document_id:document,original_text:ui.name});
      }
      const header=addBlock(addSection('header',''),'header');addLine(header.id,'name',name.text,[name.id]);
      const block=addBlock(addSection('experience','Experience'));
      facts().filter(f=>!f.hidden&&f.kind==='bullet').slice(0,12).forEach(f=>addLine(block.id,'bullet',f.text,[f.id]));
    }
    data.fact_versions.push(...facts().map(f=>({generation_id:next.id,fact_id:f.id,text:f.text,hidden:f.hidden})));
    ui.version=null;ui.line=null;ui.problem=null;
  }
  const actions = {
    page(value){saveFactDraft();saveLineDraft();ui.page=value;ui.editor=false;ui.problem=null;render();},
    filter(value){ui.filter=value;renderGallery();renderNav();icons();},
    'open-job'(value){openJob(value);},
    back(){saveLineDraft();ui.editor=false;ui.version=null;ui.line=null;render();},
    mode(value){saveLineDraft();ui.mode=value;renderEditor();icons();},
    sidebar(){ui.collapsed=!ui.collapsed;applyTheme();},
    undo(){undo();},redo(){undo(true);},shortcuts:showShortcuts,
    'dismiss-toast'(){ui.toast=null;renderToast();},'new-job':newJobForm,
    'select-line'(value){saveLineDraft();ui.line=value;ui.lineDraft=null;main.querySelectorAll('[data-line]').forEach(b=>b.setAttribute('aria-pressed',String(b.dataset.line===value)));renderSource();},
    'save-line':saveLineDraft,
    'cancel-line'(){ui.lineDraft=null;renderSource();},
    'revert-line'(){const line=data.lines.find(l=>l.id===ui.line);if(line)mutate('Generated wording restored',()=>{line.text=line.generated;ui.lineDraft=null;});},
    'show-fact'(value){saveLineDraft();ui.fact=resolve(value)?.id;ui.factFilter='all';ui.factSearch='';ui.page='facts';ui.addFacts=false;ui.merging=false;render();},
    'claim-add'(value){const c=data.unsupported_claims.find(c=>c.id===value);if(!c)return;mutate('Claim added as a fact. Regenerate to use it.',()=>{const id=uid(),document=uid();data.documents.push({id:document,file_name:null});data.document_texts.push({document_id:document,text:c.text});data.facts.push({id,kind:'bullet',text:c.text,extracted:c.text,hidden:0,entry_id:null,review:null});data.fact_sources.push({id:uid(),fact_id:id,document_id:document,original_text:c.text});c.added_as=id;});},
    'unhide-claim'(value){const c=data.unsupported_claims.find(c=>c.id===value),f=resolve(c?.hidden_fact_id);if(f)mutate('Fact shown again. Regenerate to use it.',()=>{f.hidden=0;});},
    'fact-filter'(value){saveFactDraft();ui.factFilter=value;ui.fact=null;ui.addFacts=false;ui.merging=false;renderFacts();icons();},
    'select-fact'(value,event){saveFactDraft();if(event?.metaKey||event?.ctrlKey){actions['mark-fact'](value);return;}ui.fact=value;ui.addFacts=false;ui.merging=false;renderFactRows();renderFactDetail();icons();},
    'save-fact':saveFactDraft,'cancel-fact'(){ui.factDraft=null;renderFactDetail();icons();},
    'revert-fact'(value){const f=resolve(value);if(f)mutate('Extracted wording restored',()=>{f.text=f.extracted;ui.factDraft=null;});},
    'toggle-fact'(value){saveFactDraft();const f=resolve(value);if(f)mutate(f.hidden?'Fact shown':'Fact hidden',()=>{f.hidden=f.hidden?0:1;});},
    'confirm-fact'(value){saveFactDraft();const f=resolve(value);if(f)mutate('Fact confirmed',()=>{f.review=null;f.review_note=null;ui.fact=facts().find(other=>other.review)?.id||f.id;});},
    'mark-fact'(value){ui.marked=ui.marked.includes(value)?ui.marked.filter(id=>id!==value):[...ui.marked,value];renderFacts();icons();},
    'merge-with'(value){ui.marked=[ui.fact,value];ui.merging=true;renderFacts();icons();},
    'merge-view'(){saveFactDraft();ui.merging=true;renderFactDetail();icons();},
    'merge-cancel'(){ui.merging=false;ui.marked=[];renderFacts();icons();},
    'merge-confirm'(){const kept=resolve(ui.keep);if(!kept||ui.marked.length<2)return;mutate('Facts merged; sources and citations preserved',()=>{for(const id of ui.marked.filter(id=>id!==kept.id)){const other=resolve(id);if(!other)continue;other.merged_into=kept.id;data.fact_sources.filter(s=>s.fact_id===id).forEach(s=>s.fact_id=kept.id);data.line_citations.filter(c=>c.fact_id===id).forEach(c=>c.fact_id=kept.id);}data.line_citations=data.line_citations.filter((c,i,rows)=>rows.findIndex(other=>other.line_id===c.line_id&&other.fact_id===c.fact_id)===i);ui.fact=kept.id;ui.marked=[];ui.merging=false;});},
    'add-facts'(){saveFactDraft();ui.addFacts=true;ui.merging=false;renderFactDetail();main.querySelector('[data-input="new-facts"]')?.focus();},
    'cancel-facts'(){ui.addFacts=false;renderFactDetail();icons();},
    'save-facts'(){const text=main.querySelector('[data-input="new-facts"]')?.value.trim();if(!text)return;mutate('Facts added from your note',()=>{const doc=uid(),entry=uid();data.documents.push({id:doc,file_name:null});data.document_texts.push({document_id:doc,text});data.entries.push({id:entry,kind:'custom'});for(const line of text.split('\n').map(t=>t.trim()).filter(Boolean)){const id=uid();data.facts.push({id,kind:'bullet',text:line,extracted:line,entry_id:entry,hidden:0,review:null});data.fact_sources.push({id:uid(),fact_id:id,document_id:doc,original_text:line});ui.fact=id;}ui.addFacts=false;ui.factFilter='all';ui.factSearch='';});},
    backlink(value){saveFactDraft();const section=data.sections.find(s=>s.id===data.blocks.find(b=>b.id===data.lines.find(l=>l.id===value)?.block_id)?.section_id),resume=data.generations.find(r=>r.id===section?.generation_id);if(resume)openJob(resume.job_id,value,resume.id===current(resume.job_id)?.id?null:resume.id);},
    'source-context'(value){const s=data.fact_sources.find(s=>s.id===value),text=data.document_texts.find(d=>d.document_id===s?.document_id)?.text||s?.original_text||'';modal('Original source',`<p class="kr-intro">${escape(data.documents.find(d=>d.id===s?.document_id)?.file_name||'Your note')}</p><div class="gw-private-text" style="max-height:400px;overflow:auto">${escape(text).replace(escape(s?.original_text||''),'<mark class="gw-mark">'+escape(s?.original_text||'')+'</mark>')}</div>`);},
    generate:generateForm,
    'fix-typeset'(){ui.scenario='Library';const r=shown();if(r)mutate('Modern template selected',()=>{r.template_version_id=templateIds.modern;});},
    'version-current'(){ui.version=null;ui.mode='resume';renderEditor();icons();},
    'version-sent'(){saveLineDraft();ui.version=sent();ui.mode='resume';ui.line=null;renderEditor();icons();},
    'event-resume'(value){ui.version=value;ui.mode='resume';ui.line=null;renderEditor();icons();},
    export(){const r=shown();if(!r)return;modal('Export PDF',`<label class="kr-field">File name<input name="filename" value="${escape(ui.name.replaceAll(' ','_')+'_'+job().company.replaceAll(' ','_')+'.pdf')}"></label><p class="kr-intro">${readOnly()?'Sent version':'Current résumé'} · ${title(templateKeys[r.template_version_id])} · ${r.paper==='a4'?'A4':'US Letter'}</p>`,button('Cancel','close-modal')+button('Save PDF','export-save','','kr-control kr-primary'));},
    'export-save'(){closeModal();toast('Export preview complete. PDF writing runs in the native app.');},
    'log-event'(){eventForm();},'edit-event'(value){eventForm(data.events.find(e=>e.id===value));},
    'delete-event'(value){mutate('Event deleted',()=>{const e=data.events.find(e=>e.id===value);if(e)e.deleted_at=NOW;});},
    'no-response'(){mutate('Application marked as no response',()=>{data.events.push({id:uid(),job_id:ui.job,at:NOW,kind:'no_response'});});},
    'settings-tab'(value){ui.settingsTab=value;renderSettings();icons();},
    palette(value){ui.theme=value;applyTheme();renderSettings();icons();},
    'setup-ai'(){ui.page='settings';ui.settingsTab='ai';render();},
    'key-setup'(){modal('AI provider key','<p class="kr-intro">Design preview: use placeholder text here. This preview never writes to the keyring or calls an AI provider.</p><label class="kr-field">API key<input name="key" type="password" placeholder="Example key" required autocomplete="off"></label>',button('Cancel','close-modal')+'<button class="kr-control kr-primary cursor-interaction" type="submit">Save</button>','key');},
    'key-remove'(){ui.provider=false;renderSettings();icons();toast('Key removal previewed. The real keyring is unchanged.');},
    'close-modal':closeModal,
    'browse-files'(){main.querySelector('[data-files]').click();},
    'write-import'(){ui.importing={step:'edit',text:'',name:null,document:null,hidden:[],kept:[]};ui.problem=null;renderImport();},
    'paste-import'(){ui.importing={step:'edit',text:'',name:null,document:null,hidden:[],kept:[]};ui.problem=null;renderImport();main.querySelector('[data-input="import-text"]')?.focus();},
    'read-import'(){ui.importing.step=ui.importing.file?.pdf?'pdf':'edit';renderImport();icons();},
    'sample-import'(){ui.importing.text=data.document_texts[0]?.text||fixture.document_texts[0].text;ui.importing.step='edit';renderImport();},
    'skip-import':nextImport,
    'skip-rest'(){ui.queue=[];ui.importing={step:'choose',text:'',name:null,document:null,hidden:[],kept:[]};ui.problem=null;renderImport();icons();},
    'save-import'(){const im=ui.importing;if(!im.text.trim()){ui.problem='Add some text before saving.';renderImport();return;}mutate('Corrected source text saved',()=>{const id=uid();data.documents.push({id,file_name:im.name,preview_unread:true});data.document_texts.push({document_id:id,text:im.text});im.document=id;im.step='privacy';ui.problem=null;if(ui.scenario==='First launch'&&!ui.firstNameConfirmed){ui.name=im.text.trim().split('\n')[0].slice(0,100);ui.onboarding='name';}});},
    'privacy-token'(value){const im=ui.importing;im.kept=im.kept.includes(value)?im.kept.filter(t=>t!==value):[...im.kept,value];renderImport();icons();},
    'privacy-later'(){ui.importing.step='choose';renderImport();icons();toast('Document saved. You can find its facts later.');},
    'find-unread'(value){const doc=data.documents.find(d=>d.id===value);ui.page='import';ui.importing={step:'privacy',text:data.document_texts.find(d=>d.document_id===value)?.text||'',name:doc?.file_name,document:value,hidden:[],kept:[]};render();},
    extract(){if(!ui.provider){actions['setup-ai']();toast('Source text is saved. Set up AI to find its facts.');return;}const im=ui.importing;ui.busy='extraction';toast('Finding facts…');setTimeout(()=>{mutate('Facts found in the sample preview',()=>{const candidates=im.text.split('\n').map(t=>t.trim().replace(/^[-•]\s*/,'' )).filter(t=>t.length>25&&!t.includes('@')).slice(0,12);for(const text of candidates){const id=uid();data.facts.push({id,kind:'bullet',text,extracted:text,entry_id:null,hidden:0,review:'unsure',review_note:'Review this preview extraction against the original text.'});data.fact_sources.push({id:uid(),fact_id:id,document_id:im.document,original_text:text});}const doc=data.documents.find(d=>d.id===im.document);if(doc)doc.preview_unread=false;im.found=candidates.length;im.step='done';ui.busy=null;});},450);},
    'import-facts'(){ui.page='facts';ui.factFilter='review';ui.fact=null;ui.factSearch='';render();},
    'onboarding-import'(){ui.onboarding=null;ui.page='import';render();},
    'onboarding-skip'(){if(ui.onboarding==='name')ui.firstNameConfirmed=true;ui.onboarding=null;render();}
  };
  app.addEventListener('click',event=>{
    const submit=event.target.closest('button[type="submit"]');
    if(submit?.closest('form[data-form]')){event.preventDefault();localSubmit(submit.closest('form'));return;}
    const b=event.target.closest('button[data-action]');if(!b||b.disabled)return;const handler=actions[b.dataset.action];if(handler){handler(b.dataset.value,event);persist();}
  });
  app.addEventListener('input',event=>{
    const e=event.target;
    switch(e.dataset.input){
      case 'job-search':ui.search=e.value;renderTiles();break;
      case 'fact-search':ui.factSearch=e.value;if(!filteredFacts().some(f=>f.id===ui.fact)){ui.fact=filteredFacts()[0]?.id||null;ui.factDraft=null;}renderFactRows();renderFactDetail();icons();break;
      case 'line-wording':ui.lineDraft=e.value;break;
      case 'fact-wording':ui.factDraft=e.value;break;
      case 'import-text':ui.importing.text=e.value;break;
      case 'model':ui.model=e.value;break;
      case 'inspector-width':ui.inspectorWidth=Number(e.value);main.querySelector('.kr-editor-layout').style.gridTemplateColumns=`minmax(0,1fr) min(${ui.inspectorWidth}px,38%)`;break;
    }
  });
  app.addEventListener('change',async event=>{
    const e=event.target;
    if(e.dataset.mark){actions['mark-fact'](e.dataset.mark);return;}
    if(e.hasAttribute('data-keep')){ui.keep=e.value;return;}
    if(e.hasAttribute('data-event-kind')){overlays.querySelector('[data-interview-fields]').hidden=e.value!=='interview';overlays.querySelector('[data-resume-field]').hidden=e.value!=='applied';return;}
    if(e.hasAttribute('data-files')){ui.queue=await Promise.all([...e.files].map(async file=>({name:file.name,pdf:file.name.toLowerCase().endsWith('.pdf'),text:file.name.toLowerCase().endsWith('.pdf')?'':await file.text()})));ui.queueTotal=ui.queue.length;nextImport();return;}
    switch(e.dataset.setting){
      case 'job-sort':ui.sort=e.value;renderTiles();break;
      case 'version':saveLineDraft();ui.version=e.value||null;ui.line=null;renderEditor();icons();break;
      case 'template':{const r=shown(),key=e.value;mutate('Template changed; wording preserved',()=>{r.template_version_id=templateIds[key];});break;}
      case 'paper':{const r=shown(),value=e.value;mutate('Paper changed; wording preserved',()=>{r.paper=value;});break;}
      case 'appearance':ui.appearance=e.value;ui.theme='system';applyTheme();break;
      case 'motion':ui.motion=e.value;break;
    }
    persist();
  });
  app.addEventListener('submit',event=>{
    const form=event.target,kind=form.dataset.form;if(!kind)return;event.preventDefault();const f=new FormData(form);
    if(kind==='new-job'){const id=uid(),titleText=String(f.get('title')).trim(),company=String(f.get('company')).trim(),description=String(f.get('description')).trim();if(!titleText||!company||!description)return;closeModal();mutate('Job added',()=>{data.jobs.push({id,title:titleText,company,url:String(f.get('url')).trim()||null});data.job_descriptions.push({job_id:id,text:description});data.events.push({id:uid(),job_id:id,at:NOW,kind:'wishlisted'});ui.filter='all';ui.search='';});openJob(id);}
    else if(kind==='generate'){const instructions=String(f.get('instructions')).trim(),jobId=ui.job;closeModal();ui.busy='generation';render();setTimeout(()=>{ui.busy=null;if(ui.scenario==='Generation error'){ui.problem='The model response could not be read. Your previous résumé is still here.';render();return;}mutate('Résumé regenerated from the sample facts',()=>simulateDraft(jobId,instructions));},500);}
    else if(kind==='event'){const eventKind=String(f.get('kind')),note=String(f.get('note')).trim(),at=Date.parse(String(f.get('at'))+'+06:00');if(eventKind==='note'&&!note){form.querySelector('[data-form-error]').textContent='Write a note before saving.';return;}if(!Number.isFinite(at)){form.querySelector('[data-form-error]').textContent='Choose a valid date and time.';return;}const id=ui.editingEvent,record={id:id||uid(),job_id:ui.job,kind:eventKind,at,note:note||null,interview_kind:eventKind==='interview'?String(f.get('interviewKind')):null,interview_with:eventKind==='interview'?String(f.get('with')).trim():null,resume_id:eventKind==='applied'?String(f.get('resume'))||null:null};closeModal();mutate(id?'Event updated':'Event logged',()=>{if(id)Object.assign(data.events.find(e=>e.id===id),record);else data.events.push(record);});}
    else if(kind==='key'){ui.provider=true;form.reset();closeModal();renderSettings();icons();toast('Provider setup previewed. No key was stored.');}
    else if(kind==='stand-in'){const name=String(f.get('standIn')).trim();if(facts().some(f=>f.text.toLowerCase().includes(name.toLowerCase()))){toast('Choose a made-up name that is not in your career facts.');return;}ui.standIn=name;toast('Stand-in name saved in this preview.');}
    else if(kind==='hide-phrase'){const phrase=String(f.get('phrase')).trim();if(phrase)ui.importing.hidden.push(phrase);renderImport();icons();}
    else if(kind==='name'){ui.name=String(f.get('name')).trim();ui.firstNameConfirmed=true;ui.onboarding=null;render();}
    persist();
  });
  // A rerender can return DOM focus to the iframe body. App shortcuts still
  // need to work, while focused text inputs retain their own undo history.
  document.addEventListener('keydown',event=>{
    const modifier=event.metaKey||event.ctrlKey,e=event.target,input=e.matches('input,textarea,select');
    if(event.key==='Enter'&&e.form?.dataset.form&&(!e.matches('textarea,select')||modifier)){event.preventDefault();localSubmit(e.form);return;}
    if(e.hasAttribute('data-resizer')&&['ArrowLeft','ArrowRight'].includes(event.key)){event.preventDefault();ui.inspectorWidth=Math.max(280,Math.min(420,ui.inspectorWidth+(event.key==='ArrowLeft'?20:-20)));main.querySelector('.kr-editor-layout').style.gridTemplateColumns=`minmax(0,1fr) min(${ui.inspectorWidth}px,45%)`;return;}
    if(ui.modal){if(event.key==='Escape'){event.preventDefault();closeModal();return;}if(event.key==='Tab'){const controls=[...overlays.querySelectorAll('button:not(:disabled),input,textarea,select')].filter(el=>el.getClientRects().length);const first=controls[0],last=controls.at(-1);if(event.shiftKey&&document.activeElement===first){event.preventDefault();last?.focus();}else if(!event.shiftKey&&document.activeElement===last){event.preventDefault();first?.focus();}}if(modifier&&event.key==='Enter'){localSubmit(overlays.querySelector('form'));event.preventDefault();}return;}
    if(e.dataset.input==='line-wording'){if(event.key==='Escape'){event.preventDefault();ui.lineDraft=null;renderSource();}else if(event.key==='Enter'&&!event.shiftKey){event.preventDefault();saveLineDraft();}return;}
    if(e.dataset.input==='fact-wording'){if(event.key==='Escape'){event.preventDefault();ui.factDraft=null;renderFactDetail();}else if(event.key==='Enter'&&!event.shiftKey){event.preventDefault();saveFactDraft();}return;}
    if(input){
      if(e.dataset.input==='fact-search'&&['ArrowDown','Escape'].includes(event.key)){event.preventDefault();const first=filteredFacts()[0];if(first){actions['select-fact'](first.id);main.querySelector(`[data-action="select-fact"][data-value="${first.id}"]`)?.focus();}return;}
      if(event.key==='Escape'&&ui.addFacts){event.preventDefault();actions['cancel-facts']();return;}
      if(event.key==='Escape'&&ui.page==='import'){event.preventDefault();actions['skip-rest']();return;}
      if(modifier&&event.key==='Enter'){event.preventDefault();if(ui.addFacts)actions['save-facts']();else if(ui.page==='import')actions[ui.importing.step==='privacy'?'extract':'save-import']();}return;
    }
    if(modifier&&event.key==='Enter'&&ui.page==='facts'){event.preventDefault();actions['mark-fact'](ui.fact);return;}
    if(modifier&&event.key.toLowerCase()==='n'&&ui.page==='import'){event.preventDefault();actions['write-import']();return;}
    if(modifier&&event.key.toLowerCase()==='b'){event.preventDefault();actions.sidebar();return;}
    if(modifier&&event.key==='/'){event.preventDefault();showShortcuts();return;}
    if(modifier&&event.key.toLowerCase()==='z'){event.preventDefault();undo(event.shiftKey);return;}
    if(modifier&&event.key.toLowerCase()==='y'){event.preventDefault();undo(true);return;}
    if(modifier&&ui.page==='facts'&&event.key.toLowerCase()==='f'){event.preventDefault();main.querySelector('[data-input="fact-search"]')?.focus();return;}
    if(modifier&&/^[1-5]$/.test(event.key)&&ui.page==='resumes'){event.preventDefault();if(ui.editor){if(event.key==='1'||event.key==='2')actions.mode(event.key==='1'?'resume':'application');}else actions.filter(filters[Number(event.key)-1][0]);return;}
    if(ui.page==='resumes'&&ui.editor&&ui.mode!=='application'){
      if(['ArrowUp','ArrowDown'].includes(event.key)){event.preventDefault();const all=lines(),index=all.findIndex(l=>l.id===ui.line),next=all[Math.max(0,Math.min(all.length-1,index+(event.key==='ArrowDown'?1:-1)))];if(next){actions['select-line'](next.id);main.querySelector(`[data-line="${next.id}"]`)?.scrollIntoView({block:'nearest'});}}
      if(event.key==='Enter'&&ui.line&&!readOnly()){event.preventDefault();main.querySelector('[data-input="line-wording"]')?.focus();}
      if(event.key==='Escape'){event.preventDefault();if(ui.line){ui.line=null;renderEditor();icons();}else actions.back();}
    }else if(ui.page==='resumes'&&!ui.editor&&['ArrowUp','ArrowDown','ArrowLeft','ArrowRight'].includes(event.key)){event.preventDefault();const list=[...main.querySelectorAll('[data-action="open-job"]')],index=list.indexOf(document.activeElement),columns=getComputedStyle(main.querySelector('[data-grid]')).gridTemplateColumns.split(' ').length,step=(event.key==='ArrowUp'||event.key==='ArrowDown'?columns:1)*(event.key==='ArrowUp'||event.key==='ArrowLeft'?-1:1);list[Math.max(0,Math.min(list.length-1,index<0?0:index+step))]?.focus();}
    else if(ui.page==='facts'){
      if(ui.merging&&/^[1-9]$/.test(event.key)){ui.keep=ui.marked[Number(event.key)-1]||ui.keep;renderFactDetail();icons();}
      else if(event.key==='Enter'){event.preventDefault();if(ui.merging)actions['merge-confirm']();else if(resolve(ui.fact)?.review)actions['confirm-fact'](ui.fact);}
      else if(event.key==='Delete'||event.key==='Backspace'){event.preventDefault();actions['toggle-fact'](ui.fact);}
      else if(event.key==='Escape'){ui.marked=[];ui.merging=false;ui.addFacts=false;renderFacts();icons();}
      else if(['ArrowUp','ArrowDown'].includes(event.key)){event.preventDefault();const all=filteredFacts(),index=all.findIndex(f=>f.id===ui.fact),next=all[Math.max(0,Math.min(all.length-1,index+(event.key==='ArrowDown'?1:-1)))];if(next){actions['select-fact'](next.id);main.querySelector(`[data-action="select-fact"][data-value="${next.id}"]`)?.focus();}}
    }else if(ui.page==='import'&&event.key==='Escape')actions['skip-rest']();
  });
  app.addEventListener('dragover',event=>{if(event.target.closest('[data-drop]'))event.preventDefault();});
  let resizing=null;
  app.addEventListener('pointerdown',event=>{if(!event.target.hasAttribute('data-resizer'))return;resizing={x:event.clientX,width:ui.inspectorWidth};event.target.setPointerCapture(event.pointerId);});
  app.addEventListener('pointermove',event=>{if(!resizing)return;ui.inspectorWidth=Math.max(280,Math.min(420,resizing.width+resizing.x-event.clientX));const layout=main.querySelector('.kr-editor-layout');if(layout)layout.style.gridTemplateColumns=`minmax(0,1fr) min(${ui.inspectorWidth}px,45%)`;});
  app.addEventListener('pointerup',()=>{resizing=null;});
  app.addEventListener('drop',async event=>{if(!event.target.closest('[data-drop]'))return;event.preventDefault();ui.queue=await Promise.all([...event.dataTransfer.files].map(async file=>({name:file.name,pdf:file.name.toLowerCase().endsWith('.pdf'),text:file.name.toLowerCase().endsWith('.pdf')?'':await file.text()})));ui.queueTotal=ui.queue.length;nextImport();});
  render();
  if(globalThis.Tweak){const controls={theme:ui.theme,scenario:'Library'};const tweak=new Tweak({container:app,onChange:()=>{if(controls.scenario!==ui.scenario)setScenario(controls.scenario);ui.theme=controls.theme;applyTheme();persist();}});tweak.addSelect(controls,'theme',{label:'Theme',options:[{label:'Follow system',value:'system'},...palettes.map(p=>({label:p.name,value:p.id}))]});tweak.addSelect(controls,'scenario',{label:'Preview state',options:['Library','First launch','Empty library','No AI','Generation error','Typesetting error','Import duplicate']});}
  matchMedia('(prefers-color-scheme:dark)').addEventListener('change',()=>{if(ui.theme==='system')applyTheme();});
  window.addEventListener('openai:set_globals',event=>{const state=event.detail?.globals?.widgetState?.privateContent;if(state?.theme&&(state.theme==='system'||palettes.some(p=>p.id===state.theme))){ui.theme=state.theme;applyTheme();}});
})();
