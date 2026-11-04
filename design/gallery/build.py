"""Assemble the inline design reference without a dependency install."""
from pathlib import Path

directory = Path(__file__).resolve().parent
shell = (directory / 'shell.html').read_text()
styles = (directory / 'workflows.css').read_text()
behavior = (directory / 'workflows.js').read_text()
fixture = (directory / 'sample-data.json').read_text().replace('</', '<\\/')
fragment = shell.replace('</style>', styles + '\n  </style>', 1)
fragment += '\n<script type="application/json" id="gw-sample">' + fixture + '</script>\n'
fragment += '<script>\n' + behavior + '\n</script>\n</div>\n'
output = directory / 'gallery-workflows.html'
output.write_text(fragment)
print(output)
