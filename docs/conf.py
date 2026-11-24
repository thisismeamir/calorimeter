"""Sphinx configuration for Calorimeter documentation (Read the Docs)."""

from datetime import datetime

project = "Calorimeter"
copyright = f"{datetime.now().year}, Calorimeter Contributors"
author = "Calorimeter Contributors"
release = "0.1.0"
version = "0.1"

extensions = [
    "myst_parser",
    "sphinx_copybutton",
    "sphinx_design",
    "sphinx.ext.intersphinx",
    "sphinx.ext.autosectionlabel",
]

# MyST-Parser (Markdown) configuration
myst_enable_extensions = [
    "colon_fence",
    "deflist",
    "fieldlist",
    "substitution",
    "tasklist",
    "attrs_inline",
]
myst_heading_anchors = 3
autosectionlabel_prefix_document = True

source_suffix = {
    ".md": "markdown",
    ".rst": "restructuredtext",
}
master_doc = "index"

templates_path = ["_templates"]
exclude_patterns = ["_build", "Thumbs.db", ".DS_Store"]

# Furo theme (clean monochrome + #7017FF Calorimeter accent)
html_theme = "furo"
html_title = "Calorimeter v0.1 Documentation"
html_theme_options = {
    "light_css_variables": {
        "color-brand-primary": "#7017FF",
        "color-brand-content": "#7017FF",
        "font-stack": "'IBM Plex Sans', -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
        "font-stack--monospace": "'IBM Plex Mono', ui-monospace, SFMono-Regular, Menlo, monospace",
    },
    "dark_css_variables": {
        "color-brand-primary": "#A56EFF",
        "color-brand-content": "#A56EFF",
    },
    "navigation_with_keys": True,
    "top_of_page_buttons": ["view", "edit"],
}

# Copybutton: strip shell prompts when copying code blocks
copybutton_prompt_text = r">>> |\.\.\. |\$ |❯ "
copybutton_prompt_is_regexp = True
