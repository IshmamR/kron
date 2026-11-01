"""Writes the test CVs, synthetic people, and the facts each holds
(*.facts, kind<TAB>text), from one description of each, so the two can't
drift apart. To rebuild, from this folder:

    python3 cvs.py .
    lualatex senior.tex   # then keep senior.pdf only
    for cv in junior columns linkedin; do
        google-chrome --headless=new --no-pdf-header-footer --print-to-pdf=$cv.pdf $cv.html
    done

Chrome prints the variable font Inter as Type 3 fonts, which the PDF reader
must read as words. `columns` puts contacts, skills and languages in a
sidebar beside the experience, under a photo. `linkedin` is laid out as
LinkedIn's "Save to PDF" profile is: a sidebar, roles grouped under their
company with durations, descriptions as paragraphs, and page footers."""
import base64, html, sys, pathlib, struct, zlib

OUT = pathlib.Path(sys.argv[1])

JUNIOR = {
    "name": "Tasnim Chowdhury",
    "contacts": ["tasnim.chowdhury@example.com", "+880 1812 345 678", "github.com/tasnimc", "tasnimc.dev"],
    "place": "Dhaka, Bangladesh",
    "summary": "Frontend developer who cares about the last 10%: loading states, keyboard navigation, and the error message you see when the Wi-Fi drops. One year shipping a React Native app used by 30,000 riders; before that, a lot of late nights on hackathon projects.",
    "sections": [
        ("Experience", "experience", [
            {"title": "Junior Frontend Developer", "organization": "Kotha Mobility", "location": "Dhaka", "dates": "Jun 2024 – Present", "bullets": [
                "Rebuilt the rider sign-up flow in React Native, cutting drop-off between the phone and OTP screens from 38% to 21%.",
                "Added offline support to the trip history screen with a local SQLite cache, so riders on 2G can still see their last 50 trips.",
                "Wrote the team's first accessibility checklist and fixed more than 40 issues flagged by TalkBack on Android.",
                "Paired with the backend team to move the fare estimate API from polling to WebSockets.",
                "Review about ten pull requests a week and run the Friday demo for the mobile team.",
            ]},
            {"title": "Software Engineering Intern", "organization": "Brightcart", "location": "Dhaka", "dates": "Jan 2024 – May 2024", "bullets": [
                "Built an internal dashboard in Next.js for tracking failed deliveries by area, now used daily by the operations team.",
                "Replaced a hand-written date picker with an accessible component library, removing 1,200 lines of code.",
                "Wrote Playwright tests for the checkout flow that caught two pricing bugs before release.",
            ]},
        ]),
        ("Projects", "project", [
            {"title": "Bus Koi", "skills": ["React", "Mapbox", "Firebase"], "dates": "2023", "bullets": [
                "A live map of Dhaka's city buses built from rider check-ins; won second place at the Dhaka AI Hackathon 2023 out of 64 teams.",
                "Reached 2,300 users in its first month after a post in a university Facebook group.",
            ]},
            {"title": "Bangla Typing Tutor", "skills": ["TypeScript", "Web Workers"], "dates": "2022", "bullets": [
                "A browser game that teaches the Bijoy keyboard layout; open source, with 140 GitHub stars.",
            ]},
        ]),
        ("Education", "education", [
            {"title": "B.Sc. in Computer Science and Engineering", "organization": "Eastbridge University", "location": "Dhaka", "dates": "2020 – 2024", "bullets": [
                "CGPA 3.71 / 4.00; thesis on predicting bus arrival times from crowd-sourced GPS traces.",
                "Teaching assistant for Data Structures (CSE 220) for two semesters.",
            ]},
        ]),
        ("Skills", "skills", [
            {"group": "Languages", "skills": ["TypeScript", "JavaScript", "Kotlin (basic)", "SQL"]},
            {"group": "Frontend", "skills": ["React", "React Native", "Next.js", "Tailwind CSS", "Redux Toolkit"]},
            {"group": "Testing and tools", "skills": ["Jest", "Playwright", "Git", "Figma", "Firebase"]},
        ]),
        ("Activities", "other", [
            {"bullets": [
                "Organiser, Women in Tech Dhaka (2023 – present): run monthly beginner workshops on Git and React.",
                "Speak Bangla (native), English (fluent) and Hindi (conversational).",
            ]},
        ]),
    ],
}

SENIOR = {
    "name": "Daniel Ferreira",
    "contacts": ["daniel.ferreira@example.com", "+49 151 2345 6789", "linkedin.com/in/danielferreira-eng", "github.com/dferreira"],
    "place": "Berlin, Germany",
    "summary": "Staff engineer who likes boring infrastructure and the people who keep it running. Eleven years building payment and data systems in Go, Java and Kotlin; I have spent the last four making a 300-engineer company's deploys safe enough that nobody talks about them.",
    "sections": [
        ("Experience", "experience", [
            {"title": "Staff Software Engineer, Platform", "organization": "Lumenpay GmbH", "location": "Berlin, Germany", "dates": "Mar 2021 – Present", "bullets": [
                "Led the design and rollout of a progressive delivery system for 180 services, cutting failed production deploys by 64% in its first year.",
                "Set up the payments reliability programme: SLOs for 14 critical flows, an error-budget policy agreed with product, and a weekly review I still chair.",
                "Drove the migration of the ledger from a single PostgreSQL primary to a sharded CockroachDB cluster with no customer-visible downtime.",
                "Cut the platform group's cloud spend by €1.1M a year by rightsizing Kubernetes node pools and moving batch jobs to spot instances.",
                "Mentor six engineers across three teams; two have since been promoted to senior.",
                "Wrote Lumenpay's incident review guide and trained 40 incident commanders.",
            ]},
            {"title": "Senior Software Engineer", "organization": "Lumenpay GmbH", "location": "Berlin, Germany", "dates": "Jun 2018 – Feb 2021", "bullets": [
                "Built the card authorisation service in Go, handling 2,500 requests per second at peak with a p99 latency of 85 ms.",
                "Introduced contract testing between 22 services with Pact, replacing a shared staging environment that broke every week.",
                "Led a team of five through PCI DSS Level 1 certification for the card platform.",
            ]},
            {"title": "Software Engineer", "organization": "Tabuleiro Analytics", "location": "Lisbon, Portugal", "dates": "Sep 2015 – May 2018", "bullets": [
                "Wrote the streaming ingestion pipeline in Java on Kafka that took the product from daily to near-real-time reporting for 300 retail clients.",
                "Owned the on-call rotation for the data platform and reduced pages from 30 a week to 6.",
                "Gave the company's first internal talks on property-based testing; the practice is now part of onboarding.",
            ]},
            {"title": "Junior Developer", "organization": "Correio Digital", "location": "Porto, Portugal", "dates": "Jan 2013 – Aug 2015", "bullets": [
                "Maintained the parcel tracking website (Java, Spring MVC) used by 1.2 million customers a month.",
                "Automated the release checklist with Jenkins, taking releases from a half-day manual process to 40 minutes.",
            ]},
        ]),
        ("Talks and writing", "other", [
            {"bullets": [
                "“Deploys nobody notices”, GopherCon EU 2024, Berlin.",
                "“What our ledger migration taught us about idempotency”, Lumenpay engineering blog, 2023.",
            ]},
        ]),
        ("Education", "education", [
            {"title": "M.Sc. in Informatics Engineering", "organization": "University of Porto", "location": "Porto, Portugal", "dates": "2011 – 2013", "bullets": [
                "Thesis on consistency models in geo-replicated databases, graded 18/20.",
            ]},
            {"title": "B.Sc. in Informatics Engineering", "organization": "University of Porto", "location": "Porto, Portugal", "dates": "2008 – 2011", "bullets": []},
        ]),
        ("Certifications", "certification", [
            {"title": "Certified Kubernetes Administrator (CKA)", "organization": "The Linux Foundation", "dates": "2022", "bullets": []},
        ]),
        ("Skills", "skills", [
            {"group": "Languages", "skills": ["Go", "Java", "Kotlin", "SQL", "Python"]},
            {"group": "Infrastructure", "skills": ["Kubernetes", "Terraform", "Argo Rollouts", "CockroachDB", "PostgreSQL", "Kafka"]},
            {"group": "Practices", "skills": ["SRE", "incident management", "capacity planning", "technical writing"]},
            {"group": "Spoken", "skills": ["Portuguese (native)", "English (fluent)", "German (B2)"]},
        ]),
    ],
}


COLUMNS = {
    "name": "Chiamaka Adeyemi-Brooks",
    "headline": "Android Engineer",
    "contacts": ["chiamaka.ab@example.com", "+44 7700 900123", "linkedin.com/in/chiamaka-adeyemi-brooks", "github.com/chiamakaab"],
    "place": "London, UK",
    "summary": "Android engineer who has spent five years making money apps feel calm. I like Kotlin, small pull requests, and screens that still make sense to my mum. Moved from Lagos to London in 2022 and kept building for people who pay with their phones first.",
    "sections": [
        ("Experience", "experience", [
            {"title": "Android Engineer II", "organization": "Pennyroll", "location": "London, UK", "dates": "Aug 2022 – Present", "bullets": [
                "Led the move of the savings app from XML views to Jetpack Compose, screen by screen, over 14 months without a feature freeze.",
                "Cut cold start time on low-end Android phones from 3.1 s to 1.4 s by deferring SDK set-up and trimming the dependency graph.",
                "Built the round-ups feature end to end with the payments team; 41% of active users switched it on within three months.",
                "Run the mobile guild's monthly code review clinic for eight engineers across two squads.",
            ]},
            {"title": "Android Developer", "organization": "Kudipay", "location": "Lagos, Nigeria", "dates": "Mar 2020 – Jul 2022", "bullets": [
                "Shipped offline-first transfers that queue payments on patchy networks and retry safely, reducing failed transfers by 27%.",
                "Wrote the app's first UI test suite with Espresso, taking coverage of the payments flow from zero to 70%.",
                "Added a USSD fallback for customers without mobile data, now used by 120,000 people a month.",
            ]},
            {"title": "Junior Mobile Developer", "organization": "Hatchworks Studio", "location": "Lagos, Nigeria", "dates": "Jun 2019 – Feb 2020", "bullets": [
                "Built client apps in Kotlin and Flutter for a pharmacy chain and a school fees platform.",
            ]},
        ]),
        ("Education", "education", [
            {"title": "B.Eng. Electrical and Electronics Engineering", "organization": "University of Lagos", "location": "Lagos, Nigeria", "dates": "2014 – 2019", "bullets": [
                "Final-year project: a low-cost glucose reader that sends readings to a phone over Bluetooth Low Energy.",
            ]},
        ]),
        ("Projects", "project", [
            {"title": "naira-format", "skills": ["Kotlin", "Kotlin Multiplatform"], "dates": "2021", "bullets": [
                "A small library for formatting naira and kobo amounts on Android and iOS; 600 stars on GitHub and used in production by four Lagos fintechs.",
            ]},
        ]),
        ("Talks and community", "other", [
            {"bullets": [
                "Speaker, Droidcon London 2023: “Compose on a £60 phone”.",
                "Mentor, CodeBridge Lagos (2020 – 2022): coached 30 first-time Android developers through their first published app.",
            ]},
        ]),
    ],
    # In the sidebar.
    "side": [
        ("Skills", "skills", [
            {"group": "Android", "skills": ["Kotlin", "Jetpack Compose", "Coroutines", "Room", "Hilt"]},
            {"group": "Also", "skills": ["Flutter", "Swift (basic)", "GraphQL", "Firebase"]},
            {"group": "Practice", "skills": ["UI testing", "accessibility", "performance profiling"]},
        ]),
        ("Languages", "skills", [
            {"group": "Languages", "skills": ["Yoruba (native)", "English (fluent)", "French (basic)"]},
        ]),
    ],
}

LINKEDIN = {
    "name": "Sofie van der Linden",
    "headline": "Senior Product Manager at Fietsly | Marketplaces, pricing and the occasional spreadsheet argument",
    "contacts": ["sofie.vdlinden@example.com", "www.linkedin.com/in/sofievanderlinden", "sofievdl.com"],
    "contact_labels": ["", " (LinkedIn)", " (Personal)"],
    "place": "Amsterdam, North Holland, Netherlands",
    "summary": "I build two-sided marketplaces, mostly the unglamorous parts: pricing, payouts and trust. Eight years in product, two before that as a data analyst, so I still write my own SQL and occasionally regret it. Right now I lead pricing and supply at Fietsly, a bike-sharing marketplace in six Dutch cities.",
    "sections": [
        ("Experience", "experience", [
            {"title": "Senior Product Manager", "organization": "Fietsly", "location": "Amsterdam, North Holland, Netherlands", "dates": "January 2023 - Present", "duration": "(1 year 10 months)", "company_duration": "4 years 2 months", "bullets": [
                "Introduced dynamic pricing by neighbourhood and hour, raising revenue per bike by 18% without lowering ride counts.",
                "Redesigned the payout model for fleet partners, cutting payout disputes from about 60 a month to 9.",
                "Set up the experimentation review that now signs off every pricing test before launch.",
            ]},
            {"title": "Product Manager", "organization": "Fietsly", "location": "Amsterdam, North Holland, Netherlands", "dates": "September 2020 - December 2022", "duration": "(2 years 4 months)", "paragraph": True, "bullets": [
                "Owned the rider app's checkout and subscriptions.",
                "Launched the monthly pass, which grew to 23,000 subscribers in its first year.",
                "Moved card payments to a new provider with no downtime during the switch.",
            ]},
            {"title": "Product Analyst", "organization": "Boekhuis.nl", "location": "Utrecht, Netherlands", "dates": "June 2018 - August 2020", "duration": "(2 years 3 months)", "paragraph": True, "bullets": [
                "Built the weekly trading dashboards used by category managers.",
                "Ran the first A/B tests on search ranking, which lifted conversion from search by 6%.",
            ]},
            {"title": "Junior Data Analyst", "organization": "Kader Consulting", "location": "Rotterdam, South Holland, Netherlands", "dates": "September 2016 - May 2018", "duration": "(1 year 9 months)", "paragraph": True, "bullets": [
                "Cleaned and modelled retail sales data for clients in fashion and groceries.",
            ]},
        ]),
        ("Education", "education", [
            {"title": "Master of Science - MS, Business Analytics", "organization": "Universiteit van Amsterdam", "dates": "2015 - 2016", "bullets": []},
            {"title": "Bachelor of Science - BS, Econometrics and Operations Research", "organization": "Erasmus University Rotterdam", "dates": "2012 - 2015", "bullets": []},
        ]),
    ],
    "side": [
        ("Top Skills", "skills", [
            {"group": "Top Skills", "skills": ["Pricing Strategy", "Marketplace Design", "Experimentation"]},
        ]),
        ("Languages", "skills", [
            {"group": "Languages", "skills": ["Dutch (Native or Bilingual)", "English (Full Professional)", "German (Professional Working)"]},
        ]),
        ("Certifications", "certification", [
            {"title": "Certified Scrum Product Owner (CSPO)", "bullets": []},
        ]),
        ("Honors-Awards", "other", [
            {"bullets": ["Fietsly Hackweek winner 2021"]},
        ]),
    ],
}


def facts(person):
    """kind<TAB>text, one per line: what extraction should find."""
    lines = [("summary", person["summary"])]
    lines += [("contact", contact) for contact in person["contacts"]]
    for _, kind, entries in person["sections"] + person.get("side", []):
        for entry in entries:
            for key, fact in [("title", "title"), ("organization", "organization"), ("location", "location"), ("dates", "date"), ("group", "skill_group")]:
                if entry.get(key):
                    lines.append((fact, entry[key]))
            lines += [("skill", skill) for skill in entry.get("skills", [])]
            lines += [("bullet", bullet) for bullet in entry.get("bullets", [])]
    return "".join(f"{kind}\t{text}\n" for kind, text in lines)


def junior_html(person):
    e = html.escape
    out = ["""<!doctype html><html><head><meta charset="utf-8"><style>
@page { size: A4; margin: 14mm 15mm; }
body { font-family: 'Inter', 'Helvetica Neue', Arial, sans-serif; font-size: 9.6pt; color: #1d2530; line-height: 1.38; }
h1 { font-size: 22pt; margin: 0; letter-spacing: -0.3px; color: #0f3d5e; }
.contacts { margin: 3px 0 10px; color: #4a5562; }
.summary { margin: 0 0 8px; }
h2 { font-size: 10.5pt; text-transform: uppercase; letter-spacing: 1.2px; color: #0f3d5e; border-bottom: 1.2px solid #0f3d5e; padding-bottom: 2px; margin: 12px 0 6px; }
.row { display: flex; justify-content: space-between; margin-top: 6px; }
.what { font-weight: 600; }
.where { color: #4a5562; }
.when { color: #4a5562; white-space: nowrap; }
ul { margin: 2px 0 0; padding-left: 16px; }
li { margin: 1.5px 0; }
.skills div { margin: 2px 0; }
</style></head><body>"""]
    out.append(f"<h1>{e(person['name'])}</h1>")
    out.append(f"<div class='contacts'>{' · '.join(e(c) for c in person['contacts'] + [person['place']])}</div>")
    out.append(f"<p class='summary'>{e(person['summary'])}</p>")
    for heading, kind, entries in person["sections"]:
        out.append(f"<h2>{e(heading)}</h2>")
        if kind == "skills":
            out.append("<div class='skills'>")
            for entry in entries:
                out.append(f"<div><b>{e(entry['group'])}:</b> {e(', '.join(entry['skills']))}</div>")
            out.append("</div>")
            continue
        for entry in entries:
            if entry.get("title"):
                what = f"<span class='what'>{e(entry['title'])}</span>"
                if entry.get("organization"):
                    what += f" <span class='where'>· {e(entry['organization'])}, {e(entry['location'])}</span>"
                if entry.get("skills"):
                    what += f" <span class='where'>· {e(', '.join(entry['skills']))}</span>"
                out.append(f"<div class='row'><div>{what}</div><div class='when'>{e(entry['dates'])}</div></div>")
            if entry["bullets"]:
                out.append("<ul>" + "".join(f"<li>{e(b)}</li>" for b in entry["bullets"]) + "</ul>")
    out.append("</body></html>")
    return "\n".join(out)


def tex(text):
    for a, b in [("\\", r"\textbackslash{}"), ("&", r"\&"), ("%", r"\%"), ("$", r"\$"), ("#", r"\#"), ("_", r"\_"), ("{", r"\{"), ("}", r"\}")]:
        text = text.replace(a, b)
    return text.replace("–", "--")


def senior_tex(person):
    out = [r"""\documentclass[10pt,a4paper]{article}
\usepackage[margin=16mm,top=14mm,bottom=14mm]{geometry}
\usepackage{fontspec}
\usepackage{enumitem}
\usepackage{titlesec}
\usepackage{xcolor}
\definecolor{ink}{HTML}{7A2E1F}
\titleformat{\section}{\large\scshape\color{ink}}{}{0pt}{}[\vspace{-4pt}\rule{\textwidth}{0.6pt}]
\titlespacing*{\section}{0pt}{9pt}{4pt}
\setlist[itemize]{leftmargin=14pt,itemsep=1pt,topsep=2pt,parsep=0pt}
\pagestyle{empty}
\setlength{\parindent}{0pt}
\begin{document}
"""]
    out.append(r"{\fontsize{24}{28}\selectfont\color{ink} " + tex(person["name"]) + r"}\\[3pt]")
    out.append(" \\textbar{} ".join(tex(c) for c in person["contacts"] + [person["place"]]) + r"\\[6pt]")
    out.append(tex(person["summary"]))
    for heading, kind, entries in person["sections"]:
        out.append(r"\section*{" + tex(heading) + "}")
        if kind == "skills":
            for entry in entries:
                out.append(r"\textbf{" + tex(entry["group"]) + ":} " + tex(", ".join(entry["skills"])) + r"\\")
            continue
        if kind == "other":
            out.append(r"\begin{itemize}" + "".join(r"\item " + tex(b) for b in entries[0]["bullets"]) + r"\end{itemize}")
            continue
        for entry in entries:
            first = r"\textbf{" + tex(entry["title"]) + "}"
            if entry.get("organization"):
                first += ", " + tex(entry["organization"])
            if entry.get("location"):
                first += r" \textit{(" + tex(entry["location"]) + ")}"
            out.append(first + r" \hfill " + tex(entry["dates"]) + r"\\[-2pt]")
            if entry["bullets"]:
                out.append(r"\begin{itemize}" + "".join("\n\\item " + tex(b) for b in entry["bullets"]) + "\n\\end{itemize}")
            else:
                out.append(r"\vspace{3pt}")
    out.append(r"\end{document}")
    return "\n".join(out)


def photo():
    """A drawn head and shoulders, as a PNG data URI: a raster image, as a
    photo is, of no one."""
    size = 240
    rows = []
    for y in range(size):
        row = bytearray([0])
        for x in range(size):
            head = (x - 120) ** 2 + (y - 98) ** 2 < 46 ** 2
            shoulders = ((x - 120) / 88) ** 2 + ((y - 236) / 82) ** 2 < 1
            if head or shoulders:
                shade = 90 + (x + y) // 12
                row += bytes([shade // 2, shade // 3 + 20, shade])
            else:
                row += bytes([214 - y // 8, 200 - y // 10, 186 - y // 12])
        rows.append(bytes(row))

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(b"".join(rows), 9)) + chunk(b"IEND", b"")
    return "data:image/png;base64," + base64.b64encode(png).decode()


def columns_html(person):
    e = html.escape
    out = ["""<!doctype html><html><head><meta charset="utf-8"><style>
@page { size: A4; margin: 0; }
body { margin: 0; font-family: 'Lato', sans-serif; font-size: 9.8pt; color: #222831; line-height: 1.4; }
.page { display: flex; min-height: 297mm; }
.side { width: 64mm; background: #1f3b4d; color: #e9eef2; padding: 12mm 7mm 10mm 9mm; box-sizing: border-box; }
.side img { width: 34mm; height: 34mm; border-radius: 50%; display: block; margin: 0 auto 7mm; }
.side h3 { font-size: 9pt; letter-spacing: 1.4px; text-transform: uppercase; color: #9fd3c7; margin: 6mm 0 2mm; }
.side .item { margin: 1.2mm 0; word-break: break-all; }
.side .group { font-weight: 700; margin-top: 2mm; }
.main { flex: 1; padding: 12mm 11mm 10mm 9mm; }
h1 { font-size: 23pt; margin: 0; color: #1f3b4d; font-weight: 900; }
.headline { font-size: 11.5pt; color: #3a8f7f; margin: 1mm 0 4mm; }
h2 { font-size: 10.5pt; letter-spacing: 1.2px; text-transform: uppercase; color: #1f3b4d; border-bottom: 1px solid #c9d3da; padding-bottom: 1mm; margin: 5mm 0 2mm; }
.what { font-weight: 700; margin-top: 3mm; }
.meta { color: #5b6670; font-size: 8.8pt; }
ul { margin: 1mm 0 0; padding-left: 4mm; }
li { margin: 0.8mm 0; }
</style></head><body><div class="page">"""]
    out.append(f"<div class='side'><img src='{photo()}' alt=''>")
    out.append("<h3>Contact</h3>" + "".join(f"<div class='item'>{e(c)}</div>" for c in person["contacts"] + [person["place"]]))
    for heading, _, entries in person["side"]:
        out.append(f"<h3>{e(heading)}</h3>")
        for entry in entries:
            if entry["group"] != heading:
                out.append(f"<div class='group'>{e(entry['group'])}</div>")
            out.append("".join(f"<div class='item'>{e(skill)}</div>" for skill in entry["skills"]))
    out.append("</div><div class='main'>")
    out.append(f"<h1>{e(person['name'])}</h1><div class='headline'>{e(person['headline'])}</div>")
    out.append(f"<p>{e(person['summary'])}</p>")
    for heading, _, entries in person["sections"]:
        out.append(f"<h2>{e(heading)}</h2>")
        for entry in entries:
            if entry.get("title"):
                out.append(f"<div class='what'>{e(entry['title'])}</div>")
                meta = [entry.get("organization"), entry.get("location"), ", ".join(entry.get("skills", [])), entry["dates"]]
                out.append(f"<div class='meta'>{e(' · '.join(part for part in meta if part))}</div>")
            out.append("<ul>" + "".join(f"<li>{e(b)}</li>" for b in entry["bullets"]) + "</ul>")
    out.append("</div></div></body></html>")
    return "\n".join(out)


def linkedin_html(person):
    """Two pages, split by hand, so each can carry its footer."""
    e = html.escape
    out = ["""<!doctype html><html><head><meta charset="utf-8"><style>
@page { size: A4; margin: 0; }
body { margin: 0; font-family: 'Liberation Sans', Arial, sans-serif; color: #181818; font-size: 10pt; line-height: 1.35; }
.page { position: relative; height: 297mm; display: flex; page-break-after: always; overflow: hidden; }
.side { width: 62mm; background: #e9eef3; padding: 14mm 6mm 0 9mm; box-sizing: border-box; }
.side h3 { font-size: 12pt; margin: 0 0 3mm; font-weight: 400; color: #000; }
.side .block { margin-bottom: 8mm; }
.side div.line { margin: 1mm 0; color: #333; overflow-wrap: anywhere; }
.side .label { color: #737373; }
.main { flex: 1; padding: 14mm 12mm 0 9mm; }
h1 { font-size: 24pt; font-weight: 400; margin: 0 0 2mm; }
.headline { font-size: 11pt; margin-bottom: 1mm; }
.place { color: #737373; font-size: 10pt; margin-bottom: 7mm; }
h2 { font-size: 14pt; font-weight: 400; margin: 6mm 0 3mm; }
.company { font-size: 11pt; font-weight: 700; margin-top: 4mm; }
.total { color: #737373; }
.role { font-size: 10.5pt; margin-top: 2mm; }
.when, .where { color: #737373; }
.text { margin: 1.5mm 0 0; }
.footer { position: absolute; bottom: 9mm; right: 12mm; color: #9a9a9a; font-size: 8.5pt; }
</style></head><body>"""]
    contacts = "".join(f"<div class='line'>{e(c)}<span class='label'>{e(label)}</span></div>"
                       for c, label in zip(person["contacts"], person["contact_labels"]))
    side = [f"<div class='block'><h3>Contact</h3>{contacts}</div>"]
    for heading, kind, entries in person["side"]:
        if kind == "skills":
            lines = entries[0]["skills"]
        elif kind == "certification":
            lines = [entry["title"] for entry in entries]
        else:
            lines = entries[0]["bullets"]
        side.append(f"<div class='block'><h3>{e(heading)}</h3>" + "".join(f"<div class='line'>{e(line)}</div>" for line in lines) + "</div>")

    def role(entry):
        text = (f"<div class='text'>{e(' '.join(entry['bullets']))}</div>" if entry.get("paragraph")
                else "".join(f"<div class='text'>- {e(b)}</div>" for b in entry["bullets"]))
        return (f"<div class='role'>{e(entry['title'])}</div><div class='when'>{e(entry['dates'])} {e(entry['duration'])}</div>"
                f"<div class='where'>{e(entry['location'])}</div>{text}")

    experience = person["sections"][0][2]
    education = person["sections"][1][2]
    first = [f"<h1>{e(person['name'])}</h1><div class='headline'>{e(person['headline'])}</div>",
             f"<div class='place'>{e(person['place'])}</div>",
             f"<h2>Summary</h2><div class='text'>{e(person['summary'])}</div>",
             "<h2>Experience</h2>",
             f"<div class='company'>{e(experience[0]['organization'])}</div><div class='total'>{e(experience[0]['company_duration'])}</div>",
             role(experience[0]), role(experience[1])]
    second = []
    for page, entry in [(first, experience[2]), *[(second, entry) for entry in experience[3:]]]:
        page.append(f"<div class='company'>{e(entry['organization'])}</div>" + role(entry))
    second.append("<h2>Education</h2>")
    for entry in education:
        second.append(f"<div class='company'>{e(entry['organization'])}</div>"
                      f"<div class='text'>{e(entry['title'])} · ({e(entry['dates'])})</div>")
    for number, (side_html, main) in enumerate([("".join(side), first), ("", second)], 1):
        out.append(f"<div class='page'><div class='side'>{side_html}</div><div class='main'>{''.join(main)}</div>"
                   f"<div class='footer'>Page {number} of 2</div></div>")
    out.append("</body></html>")
    return "\n".join(out)


(OUT / "junior.html").write_text(junior_html(JUNIOR))
(OUT / "junior.facts").write_text(facts(JUNIOR))
(OUT / "senior.tex").write_text(senior_tex(SENIOR))
(OUT / "senior.facts").write_text(facts(SENIOR))
(OUT / "columns.html").write_text(columns_html(COLUMNS))
(OUT / "columns.facts").write_text(facts(COLUMNS))
(OUT / "linkedin.html").write_text(linkedin_html(LINKEDIN))
(OUT / "linkedin.facts").write_text(facts(LINKEDIN))
