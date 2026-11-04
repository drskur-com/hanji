---
name: theme
description: Create or change a color theme for the Hanji app (window, panels, editor, tables, code highlighting). Use when the user wants a new theme, a theme based on a palette or another app's theme (Nord, Dracula, Solarized, Catppuccin, and so on), or wants to adjust the colors of a theme they made.
---

# Hanji color themes

Answer in the user's language.

## Where themes live

- Built-in themes: Hanji Light, Hanji Dark, Gruvbox Light, Gruvbox Dark. They are part of the app; you cannot read or change them, and no library theme may use their names.
- Library themes: `Themes/<name>.json` in the library. Create one with `write_file` (the `Themes/` folder is made on approval) and change it with `edit_file`. The app reloads the folder whenever a file changes, so an approved theme appears right away.
- The user picks a theme in Settings → General → Theme, or in the View → Theme menu. You cannot switch the theme yourself; tell the user where to pick it. Library themes are deleted from Settings (the trash button next to the theme).

## File format

One file holds one theme family, usually a light and a dark theme. Name them `<Family> Light` and `<Family> Dark`: the Toggle Light/Dark command switches between the two themes of the same family.

```json
{
  "name": "Nord",
  "themes": [
    {
      "name": "Nord Dark",
      "mode": "dark",
      "colors": {
        "background": "#2e3440",
        "foreground": "#eceff4",
        "border": "#3b4252",
        "primary.background": "#5e81ac",
        "primary.foreground": "#eceff4"
      },
      "highlight": {
        "syntax": {
          "keyword": { "color": "#81a1c1" },
          "string": { "color": "#a3be8c" },
          "comment": { "color": "#616e88", "font_style": "italic" }
        }
      }
    }
  ]
}
```

- `mode` is `light` or `dark`. Every theme needs a unique `name`.
- Colors are `#rrggbb` or `#rrggbbaa` (alpha last). Missing keys fall back to the app's default light or dark colors, which may not match your palette, so set every key below.
- Font, size, and corner values (`font_family`, `font_size`, `radius`, and so on) are ignored: only colors are used.
- `write_file` checks the file. If it reports an error, fix the JSON and write it again.

## Color keys

| Key | Used for |
|---|---|
| `background`, `foreground` | Editor and main window background, body text |
| `border` | Thin lines and dividers |
| `muted.background`, `muted.foreground` | Quiet surfaces; secondary text (word counts, excerpts, hints) |
| `accent.background`, `accent.foreground` | Hovered menu items and controls |
| `secondary.background`, `secondary.foreground` | Light emphasis buttons (for example "New project…") |
| `primary.background`, `primary.foreground`, `primary.hover.background`, `primary.active.background` | Main buttons and the accent color |
| `caret`, `ring` | Text cursor; focus ring |
| `selection.background` | Selected text. Alpha is capped at 30% (`…4d`) |
| `link`, `link.hover`, `link.active` | Links |
| `popover.background`, `popover.foreground`, `overlay` | Menus and dialogs; the dimmed layer behind dialogs (use alpha) |
| `sidebar.background`, `sidebar.foreground`, `sidebar.border` | Library panel |
| `sidebar.accent.background`, `sidebar.accent.foreground` | Selected group in the library panel |
| `title_bar.background`, `title_bar.border`, `status_bar.background`, `status_bar.border` | Toolbar and status bar |
| `list.background`, `list.hover.background` | Lists (sheet list, projects); hovered row (a 5% tint of the text color works well) |
| `list.active.background`, `list.active.border` | Selected sheet and other selected rows. Alpha is capped at 20% (`…33`); 10% of the text color (`…1a`) reads well |
| `scrollbar.thumb.background`, `scrollbar.thumb.hover.background` | Scrollbars (use alpha) |
| `table.background`, `table.even.background`, `table.head.background`, `table.row.border` | Tables in the editor: row background, striped rows, header, grid lines |
| `danger.background`, `success.background`, `warning.background`, `info.background` | Errors, removed lines, success, warnings |
| `base.red`, `base.green`, `base.yellow`, `base.blue`, `base.magenta`, `base.cyan` | Group and project colors chosen in the library panel |

## Code highlighting

`highlight.syntax` colors code blocks in sheets. Each entry is `{ "color": "#…" }` and may add `"font_style": "italic"` or `"font_weight": 700`. Set these names:

`attribute`, `boolean`, `comment`, `comment.doc`, `constant`, `constructor`, `embedded`, `emphasis`, `emphasis.strong`, `function`, `keyword`, `link_text`, `link_uri`, `number`, `operator`, `punctuation`, `property`, `string`, `string.escape`, `string.regex`, `string.special`, `string.special.symbol`, `tag`, `text.literal`, `text.code.span`, `title`, `type`, `variable`, `variable.special`

## Making a good theme

- Check contrast: body text against `background` should be at least 7:1, muted text at least 4.5:1. Grid lines and dividers must stay visible against the background (dark themes often fail here).
- In a dark theme, panels (`sidebar`, `title_bar`, `status_bar`) are usually a little darker than `background`; in a light theme a little darker or tinted.
- Keep one accent hue for `primary`, `caret`, `ring`, and `list.active.border`.
- Code colors must be readable on `background` in that theme's mode: do not reuse light-theme syntax colors in a dark theme.
- When basing a theme on a known palette, use its published colors and say which palette you used.
