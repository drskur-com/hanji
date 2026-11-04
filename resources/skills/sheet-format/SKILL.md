---
name: sheet-format
description: The exact Markdown format of Hanji sheet files and the places where it differs from standard Markdown. Read this before creating or changing any sheet (.md) with create_sheet, edit_file, or write_file, so the edit does not break paragraphs, headings, images, line groups, footnotes, or tables.
---

# Hanji sheet format

Sheets are Markdown (CommonMark + GFM, with frontmatter and `$` math), but Hanji reads and saves them line by line with its own rules. Write files the way the app itself would save them; otherwise the app reshapes them on its next save, or the book comes out wrong.

## Paragraphs and line breaks
- One paragraph is one line. Separate paragraphs with exactly one blank line. Never hard-wrap a paragraph across lines: a line break inside a paragraph is joined with a space when the app opens the file.
- Extra blank lines are meaningful: each additional blank line shows up as an empty line in the editor (and as vertical space when the project keeps line breaks). Do not add or remove blank lines you were not asked to change.
- These lines stay together with a single line break (no blank line between): consecutive list items and their indented continuation lines, consecutive `>` quote lines, consecutive `|` table rows, a line ending in a hard break (two trailing spaces or `\`) and the line after it.
- For a hard line break inside a paragraph, end the line with `\` (or two spaces). Prefer a line group (below) for verse or letters.
- For a scene break, use `***` on its own line, with blank lines around it. Never use a line of `---` or `===` after text: the app glues it to the previous line on save, which turns that line into a heading.
- Keep the file's existing leading and trailing blank lines and final newline as they are. Use LF line endings.

## Headings
- `#` is a part, `##` a chapter, `###` a section. Styles, numbering, and prefixes such as "Chapter 3" come from the project settings, so never type numbers into heading text.
- `####` to `######` are plain bold paragraphs in the book; avoid them for structure.
- Use ATX headings (`#`) only, not setext (`===`/`---` underlines).
- To leave a heading unnumbered, add the literal suffix `{unnumbered}`, for example `## Prologue {unnumbered}`. It works only on the sheet's first heading. Pandoc forms such as `{-}` or `{.unnumbered}` are not supported and show as text.
- The sheet's title in the app is its first non-empty line, heading or not. Start a new sheet with its title line, for example `# Chapter title`.

## Line groups `:::`
- A line group keeps line breaks exactly as written; in the book the whole group is one paragraph where every line break is a hard break. Use it for poems, lyrics, letters, and addresses.
- Open with three or more colons at the very start of a line (no indent), optionally followed by a label that is ignored: `::: poem`. Close with a line of only colons, at least as many as the opener.
- Groups do not nest. Inside a group, block syntax (headings, lists, quotes, tables, code fences) is shown as literal text; inline formatting and footnotes still work.
- Always close a group you open; an unclosed group runs to the end of the file.

## Images
- An image must be alone on its own line, with blank lines around it. Inside a sentence, list, or quote only its caption is shown.
- Syntax: `![caption](path "optional title"){attributes}`. Put the attributes right after `)` with no space, and nothing after them on the line.
- Attributes: `width=N` and `height=N` in pixels (integers, `px` optional; no percentages or other units), `.align-left` or `.align-right` (centered is the default and is not written), or `.full-page` alone. Omit `{}` when everything is default. Order: `width height .align-*`.
- Wrap paths that contain spaces or parentheses in `<…>`. Escape `[`, `]`, and `\` in the caption.
- Paths: web URLs as-is; otherwise relative to the sheet folder, then the project folder, then `<project>/Extras/Attachments/`. Project images are written project-relative, for example `Extras/Attachments/<ULID>.png`. Do not move or rename attachment files.

## Tables
- GFM pipe tables. Every row, including the alignment row, starts with `|` and rows are on consecutive lines with no blank lines between them. The alignment row (`| --- | :-: | --: |`) is required.

## Lists and tasks
- Tasks are `- [ ] text` and `- [x] text`. To check or uncheck, change only the character between the brackets.
- In `Notes/`, tasks may carry Obsidian Tasks markers on the same line: `⏳ YYYY-MM-DD` (scheduled), `🛫` (start), `📅` (due), `✅` (done), `➕` (created), `🆔 id`, and `#tag`. Keep them on the task line and keep the dates in `YYYY-MM-DD` form.
- Lines indented under a task belong to it. Keep nested items directly under their parent with no blank lines in between.

## Footnotes
- Reference `[^n]`, definition `[^n]: text`. Definitions go at the end of the sheet, each as its own paragraph (a blank line between them).
- For a new footnote, use the next number after the largest existing one. The book numbers footnotes by the order they are first referenced, and drops definitions with no reference, so never delete a definition that is still referenced or leave a reference without a definition.

## Links, math, code, frontmatter
- Links: `[text](<url>)`. Links between sheets are relative paths to the other sheet's `<ULID>.md` file; `#heading` anchors are not supported. Exported books keep only the link text, so do not put information only in a URL.
- Math: `$…$` inline, and a `$$` line before and after for a block. Math is shown as monospace text, not typeset.
- Code: fenced with ```` ``` ```` or `~~~`, kept verbatim.
- Frontmatter: only if the file's first line is exactly `---`, closed by another `---`. It is ignored in exports. Do not add frontmatter unless asked.
- HTML is dropped from the book, except `<br>`, which forces a line break (also inside headings).

## Characters to keep
- Never "normalize" typography: keep curly quotes (“ ” ‘ ’), `…`, `—`, `–`, CJK brackets (「」『』), and invisible characters such as non-breaking space (U+00A0), thin space (U+2009), zero-width space (U+200B), word joiner (U+2060), and soft hyphen (U+00AD). When writing new text, follow the quote and dash style the sheet already uses.

## Files
- Change the open sheet by default. If you propose a change to another sheet, say so in your reply.
- Sheet file names are permanent identifiers (`<ULID>.md`), not titles. Never rename, move, or delete sheet files; the group's `.hanji.toml` refers to them by name for order and gluing. Gluing lives only in `.hanji.toml` (`glued`), never in the sheet text.
- Create new sheets with create_sheet, not write_file.
- If the sheet is open in the app with unsaved changes, the app's next save overwrites your change. Changes to a sheet the user is not typing in are safe.

## Before you finish an edit
- Each paragraph is one line, with one blank line between paragraphs.
- Images are alone on their line with blank lines around them.
- Every `:::` group you touched is closed.
- Every footnote reference has a definition, and the reverse.
- You did not add numbers to headings, `---` scene breaks, or hard wraps.
