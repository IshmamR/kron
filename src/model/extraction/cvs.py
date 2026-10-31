"""Writes the junior and senior test CVs, synthetic people, and the facts
each holds (*.facts, kind<TAB>text), from one description of each, so the two
can't drift apart. To rebuild, from this folder:

    python3 cvs.py .
    lualatex senior.tex   # then keep senior.pdf only
    google-chrome --headless=new --no-pdf-header-footer --print-to-pdf=junior.pdf junior.html

Chrome prints the variable font Inter as Type 3 fonts, which the PDF reader
must read as words."""
import html, sys, pathlib

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


def facts(person):
    """kind<TAB>text, one per line: what extraction should find."""
    lines = [("summary", person["summary"])]
    lines += [("contact", contact) for contact in person["contacts"]]
    for _, kind, entries in person["sections"]:
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


(OUT / "junior.html").write_text(junior_html(JUNIOR))
(OUT / "junior.facts").write_text(facts(JUNIOR))
(OUT / "senior.tex").write_text(senior_tex(SENIOR))
(OUT / "senior.facts").write_text(facts(SENIOR))
