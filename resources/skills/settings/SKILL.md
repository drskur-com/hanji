---
name: settings
description: Explain where Hanji's settings live, what each value means, and how to change them, and change them on request. Use when the user asks about or wants to change group or project names, icons, and colors; sheet order, sorting, or gluing; book info (title, authors, language, cover); book styles (fonts, sizes, line spacing, alignment, heading numbers and placement); automatic input conversion; or app settings (theme, editor font, AI).
---

# Hanji settings

Answer in the user's language.

## Where settings live

| What | File | How to change |
|---|---|---|
| Group and project settings, book info, book styles | `.hanji.toml` in each group folder (for example `Projects/Novel/.hanji.toml`, `Notes/Inbox/.hanji.toml`) | `edit_file`. In the app: the project menu's "Book info…" (책 정보…) and "Book style…" (책 스타일…), or the group's context menu |
| Library marker | `.hanji/library.toml` | Internal app state. Do not read or change it |
| Skills | `Skills/<name>/SKILL.md` and files next to it | `edit_file`; new files with `write_file` |
| Color themes | `Themes/<name>.json` | `write_file`, `edit_file` (read the theme skill first). Built-in themes cannot be changed |
| App settings (per machine) | macOS `~/Library/Application Support/Hanji/config.toml`, Windows `%APPDATA%\Hanji\config.toml`, Linux `~/.config/Hanji/config.toml` | Outside the library, so the file tools cannot change it. The assistant's model: `set_model`. Anything else: tell the user to use the app's Settings window (설정) |

Library layout: `Notes/<group>/` (notes), `Projects/<project>/` (manuscripts; subfolders are manuscript groups), `Projects/<project>/Extras/` (extra material, left out of the book), `Projects/<project>/Extras/Attachments/` (attachments and the cover `cover.<ext>`). Sheet file names are permanent identifiers (`<ULID>.md`); the title shown in the app is the first line of the sheet.

## Values in `.hanji.toml`

Every value is optional; missing values use the default. Keys the app does not know are ignored.

Group values (top level, no table name):
- `title` (string): display name. Defaults to the folder name.
- `icon` (string): `inbox`, `notebook`, `book`, `file`, `folder`, or a Lucide icon file name (for example `book-open`, `feather`). Unknown names show a folder icon.
- `color` (string): `black`, `red`, `yellow`, `green`, `cyan`, `blue`, `magenta`, `gray`.
- `sheets` (array of strings): sheet file names in manual order. Sheets not listed follow, by name.
- `glued` (array of strings): sheets glued to the sheet right before them. The first sheet of a group cannot be glued. For example `sheets = ["a.md", "b.md"]` with `glued = ["b.md"]` makes b continue a.
- `groups` (array of strings): subgroup folder names in order.
- `sort` (string): `manual` (default for projects), `title`, `created`, `modified` (default for notes). Project manuscripts are always manual.
- `descending` (boolean): sort direction. Defaults to the sort's natural direction.
- `kind` (string): `sheets` (default, Markdown sheets) or `files` (any files).
- `files` (array of strings): file order in a `kind = "files"` group.

`[book]` (projects only, used for export):
- `title`: book title (defaults to the project name). `authors`: array of names. `language`: BCP 47 tag (defaults to `ko`). `description`: blurb.
- `cover`: cover image, relative to the project folder (for example `Extras/Attachments/cover.jpg`).
- `identifier`: book identifier (`urn:uuid:…`). The app creates it. Never change it.

`[styles.<target>]` (projects only, shared by the editor and export). Targets are `body`, `"title:part"` (`#`), `"title:chapter"` (`##`), and `"title:section"` (`###`). Title targets contain `:`, so quote them: `[styles."title:chapter"]`. Lengths are in em (relative to the body font size).
- `font` (string): font name. Defaults to the app's editor font; titles default to the body font. `embed_font` (boolean): embed the font file on export.
- `font_size` (number): scale relative to the body. Defaults: body 1.0, part 1.6, chapter 1.4, section 1.2.
- `font_weight` (integer): one of 300, 400, 500, 600, 700, 800, 900. Defaults: body 400, titles 700.
- `line_height` (number): defaults: body 1.6, titles 1.3.
- `align` (string): `left`, `center`, `right`, `justify`. Defaults: body `justify`, titles `left`.
- `first_line_indent` (number, em): defaults: body 1.0, titles 0.
- `space_before`, `space_after` (number, em): spacing above and below. For the body, `space_after` is the gap between paragraphs.
- `preserve_line_breaks` (boolean, body): keep the source's single line breaks and blank lines.
- Titles only: `placement` (`inline` at the start of the text, `page` on its own title page, `hidden` not shown but kept in the table of contents), `page_break` (start on a new page), `vertical_align` (`top`, `center`, `bottom`, position on a title page), `prefix` (text before the title; `{n}` becomes the number, for example `"Chapter {n}: "` or `"제{n}장 "`), `numbering` (`arabic` or `roman`), `restart_on_parent` (chapters and sections: restart at 1 when the parent title changes), `in_toc` (include in the table of contents, default true).

`[styles.input_transform]` (projects only, automatic conversion while typing; all off by default):
- `smart_quotes` (straight to curly quotes), `ellipsis` (`...` to `…`), `dash` (dash conversion), `apply_on_paste` (also when pasting).
- `replacements`: replacement rules. Each `[[styles.input_transform.replacements]]` table has `before`, `after`, and `enabled` (default true).

## Values in `config.toml` (app settings)

For reference only. The file tools cannot change this file; except for the model (`set_model`), point the user to the Settings window (설정).
- `theme`: theme name (a built-in theme or one from `Themes/` in the library; see the theme skill for making themes). `editor_font`, `editor_font_size` (default 16): editor font and size.
- `ai_provider` (`bedrock` or `alibaba-token-plan`), `aws_profile`, `aws_region`: assistant settings (Settings › AI). `token_plan_api_key`: the Alibaba Token Plan API key. Never show or repeat it.
- `ai_model` (Bedrock) and `token_plan_model` (Token Plan): the model the assistant uses, also for new conversations. Change it with the `set_model` tool when the user asks (call it without a model to list the available ones); it applies from the next message.
- `library_root`, `recent_libraries`: current and recent library locations. Managed by the app.

## How to change a setting

1. Confirm what the user wants to change. If they only asked a question, answer from the above and change nothing.
2. Pick the file. Project settings are in `Projects/<project>/.hanji.toml`; group settings in that group folder's `.hanji.toml`. If the project is unclear, use the open sheet's path (`<sheet path>`); if still unclear, ask.
3. Read the current content with `read_file`.
4. Change only the needed lines with `edit_file`. Add missing tables or keys in a sensible place. Keep other keys, tables, comments, and order as they are.
   - To reorder sheets, move file names within the `sheets` array; never invent names (check the real names with `list_files`).
   - Keep value types: strings in quotes, arrays in `[ ]`, booleans as `true`/`false`. If the result is not something the app can read, `edit_file` refuses to write and returns an error; fix it from the error and try again.
5. In your reply, say in a line or two what the change does. The user approves it from the diff; the app reloads the file shortly after it is applied.

Example: number chapter titles and start each on a new page.

```toml
[styles."title:chapter"]
prefix = "Chapter {n}: "
page_break = true
```
