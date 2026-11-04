---
name: extension
description: Write, change, or fix a Hanji extension — a Lua script in Extensions/ that draws a picture, chart, or animation inside a sheet (a ```ext block), possibly with buttons and hover, or adds a command to the Extensions menu (for example inserting today's date or transforming the selected text). Use when the user wants something drawn, animated, or clickable in the editor, a custom text command or shortcut action, or when an extension block or command shows an error.
---

# Hanji extensions

An extension is one Lua (Luau) file, `Extensions/<name>.lua`, that returns a table. What the table holds decides what kind of extension it is; one file can combine kinds. A large extension can instead be a folder, `Extensions/<name>/init.lua` with modules it loads with `require` (`references/modules.md`).

| Kind | Entry point | Read before writing |
|---|---|---|
| Drawing in a sheet (picture, chart, animation, 3D) | `draw(g, t, args)` | `references/draw.md` (3D: also `references/draw-3d.md`) |
| Interactive drawing (hover, buttons, clicking to change the block) | `draw(g, t, args, ui)`, `g.region`, `click(id, args)` | `references/draw.md`, then `references/interactive.md` |
| Buttons that change other blocks or run app actions (insert text, run a command, open a sheet) | `click` returning `actions` | also `references/actions.md` |
| Command in the Extensions menu | `commands` | `references/commands.md` |
| Game or anything with running state, keyboard, dragging, or sound, in its own window | `play = { load, update, draw, keypressed, … }` | `references/draw.md`, then `references/play.md` |
| Inline text or a small picture inside a paragraph (`{{today}}`) | `value(args, ctx)`, or `draw` inline | `references/inline.md` |
| Large extension split into several files (a game with levels, shared helpers) | `Extensions/<name>/init.lua` and `require` | also `references/modules.md` |

Read only the references for the kinds you are writing; this file has what they all share. Paths are relative to this skill's folder.

Answer in the user's language. Write code, comments, and file names in English unless the user asks otherwise. The header's `description` and `args` lines are shown to the user, so write them in the user's language. Text that the extension shows to readers (labels, inserted text) follows the manuscript's language.

## Workflow

1. **Decide what to build** and which kinds it needs (the table above). If the request is ambiguous (for example "a date": insert it once, or draw it), pick the reading that fits best and say which one you chose. Ask only when the choice changes the result a lot. Then read the references for those kinds.
2. **Check what exists.** The `<extensions>` list in your instructions shows the existing extensions with their descriptions. If one already does what the user wants, use it (add its block, step 5) instead of writing a new one. If one is close, `read_file` it and change it with `edit_file`. Name a new extension with short lowercase English words joined by `-` (for example `archer`, `bar-chart`, `insert-date`).
3. **Write the file** with `write_file` at `Extensions/<name>.lua`, in one file (or, for a large extension, as a folder: `references/modules.md`). Start it with the header (below): `description`, `args` with each argument's meaning and default, and `example` values that show it well. The user picks extensions by this header in Insert → Extension…, so keep it accurate whenever you change the code.
4. **Test it with `run_extension`. Never skip this step.** Each reference says how to test its kind.
   - Your changes to extension files (`Extensions/<name>.lua`, or the files of `Extensions/<name>/`) are a draft until your reply ends; `read_file` and `run_extension` use the draft, and the user approves each extension once afterwards. So edit and test as many times as it needs.
   - If there is an error, fix the file with `edit_file` and test again. The user sees the error in the sheet otherwise. Also fix any `Note:` about the header.
5. **Place the block** if the user wants it in a sheet. Add the ```` ```ext ```` block with `edit_file`, with a blank line before and after it, where the user asked (by default at the end of the open sheet). Put arguments one per line as `key: value`. If the user did not ask for it in a sheet, tell them they can insert it with Insert → Extension…. Users change a block's arguments by double-clicking its canvas (or ⌘-clicking it), which shows the header's `args` descriptions next to the fields, so keep them clear.
6. **Reply briefly**: what the extension does, how to use it (the block, **Insert → Extension…**, or the menu item **Extensions → title**), and how to change it. Do not paste the whole script; it is in the file.

## The file

`Extensions/<name>.lua` (`<name>`: letters, digits, `-`, `_`) runs once when it loads and must `return` a table:

| Field | Type | Meaning |
|---|---|---|
| `size` | `{ width, height }` or `function(args)` returning it | Canvas size in canvas units (1–4096). Default `{ 400, 200 }`. Shown at this size, or shrunk to the page width. A function lets each block choose its size from its arguments (asked once per argument set; must finish within 0.1 s). `g.getWidth()`/`g.getHeight()` in `draw` return the size for that block. |
| `animated` | boolean | `true`: redraw while the block is visible, 30 times a second. Default `false`: draw once at `t = 0`. |
| `period` | number | Seconds after which an animation repeats exactly (0–10). The app keeps the frames of one period and stops calling `draw` after the first loop, so set it for every looping animation. |
| `draw` | `function(g, t, args, ui)` | Draws the block. `ui` is the mouse state (`references/interactive.md`). |
| `play` | table of functions | Runs in its own window with state and input (`references/play.md`). |
| `value` | `function(args, ctx)` | Text shown in place of an inline call `{{name ...}}` (`references/inline.md`). |
| `click` | `function(id, args)` | Called when a region is clicked; returns block arguments to change. Needs `draw`. |
| `export` | boolean | Default for the book preview and exports. Default `true`. Set `false` for things that make no sense on paper (games, buttons); a block's `export:` overrides it. |
| `commands` | list of `{ title = string, run = function(ctx) }` | Items in the Extensions menu, in this order. |

The table needs at least one of `draw`, `value`, `play`, or `commands`. Other fields are ignored. Code outside the returned functions (helpers, constants) runs once at load; keep it free of side effects. The canvas `g` is also a global there, so build paths and gradients once at the top of the file (`local LEAF = g.newPath(...)`) and reuse them in every `draw`; drawing at load time does nothing.

## The header

Every extension file starts with a header: a Lua block comment from `--[[---` to `---]]` holding `key: value` lines. Lua ignores it; the app reads it without running the script. The Insert → Extension… dialog lists extensions by it (with a live preview), and the assistant's prompt lists them so existing extensions get reused.

```lua
--[[---
name: archer
description: 궁수가 과녁에 화살을 쏘는 애니메이션. 2초마다 되풀이한다.
args:
  speed: 화살 속도. 기본값 1.
  label: 맞혔을 때 보이는 글. 기본값 "Hit!".
example:
  speed: 1.5
  label: 명중!
---]]
```

| Key | Meaning |
|---|---|
| `name` | The extension name. Informational: blocks always use the file name. |
| `description` | One line saying what it draws or does. **Write it in the user's language** (it is shown to the user), unlike code and comments. |
| `args` | Indented `key: description` lines, one per block argument, with its default. Also in the user's language. Leave out for command-only extensions. |
| `example` | Indented `key: value` lines: the starting values in the Insert dialog, where the user can change them and watch the preview before inserting the block. Pick values that show the drawing well. |
| `actions` | One line listing the actions `click` may return, for example `actions: set, editor.insert` (`references/actions.md`). |

The header must be the first thing in the file (blank lines before it are fine). Values are single lines.

## The block

````
```ext <name>
key: value
other: value
```
````

- `` ~~~ `` fences and longer fences work too. A fence without its closing line is not a block yet.
- Each body line `key: value` becomes `args.key`. A value that reads as a number becomes a number, `true`/`false` become booleans, and anything else stays a string (spaces at the ends removed). Lines without `:` are ignored.
- `scale`, `align`, `export`, and `id` are reserved for the app. They work for every extension and are not passed to `draw` or `size`, so never use them as argument names or implement them yourself:
  - `scale: 1.5` shows the whole canvas 1.5 times larger (0.1–4; still shrunk to the page width).
  - `align: left`, `center` (default), or `right` places the canvas within the text width, in the editor and in exports.
  - `export: false` leaves the block out of the book preview and PDF, EPUB, and DOCX exports; `export: true` keeps it even if the extension's default is `false`.
  - `id: chart` names the block so buttons in other blocks can change it (`references/actions.md`).
- The block is ordinary Markdown: other apps show it as a code block.

## The sandbox

Available: `string` (including Luau's `string.split`), `table`, `math`, `utf8`, `bit32`, `os.date`, `os.time`, `os.clock`, `os.difftime`, `print` (ignored), `pcall`, `error`, `tostring`, `tonumber`, `pairs`, `ipairs`, `select`, `type`, `setmetatable`.

Not available: files (`io`), processes (`os.execute`), the network, changing globals of the app. `require` loads only the modules of a folder extension (`references/modules.md`). Memory is limited to 64 MB.

`math.random` restarts from the same seed before every `draw`, so the same `t` always gives the same picture.

## Strings and Korean

Lua strings are bytes; Korean characters take three bytes each. `#s`, `s:sub(i, j)`, `s:reverse()`, and `s:len()` count and cut bytes. To work with characters:

```lua
local count = utf8.len(s)                    -- number of characters
local chars = {}
for _, code in utf8.codes(s) do              -- each character
  table.insert(chars, utf8.char(code))
end
local first3 = s:sub(1, (utf8.offset(s, 4) or #s + 1) - 1)   -- first 3 characters
```

`string.find`, `string.gsub`, and `string.split` with plain ASCII patterns or literal Korean text work. Lua patterns are not regular expressions (`%a`, `%w` match only ASCII letters).

## Shared rules

- **Arguments.** Every argument needs a default (`local power = args.power or 1`). Numbers, `true`, and `false` arrive as numbers and booleans; everything else as strings. Split lists with `string.split(args.values or "", ",")` and convert each item with `tonumber`.
- **Limits.** Each draw call has 0.1 s (an animation frame at most 2,000 shapes), loading the file 0.1 s, and each command 2 s; the script has no files, network, `io`, or `os.execute` (`require` only within a folder extension; `g.newImage` reads only image files in the library). If the user asks for something the API cannot do (making new image files, reading other files, network, realistic 3D with textures or shadows cast between objects, text along a path), say so and offer the closest thing it can do.

## Fixing an extension

When the user reports a problem or a block shows an error ("확장 <name>: ..."), `read_file` the extension, reproduce the problem with `run_extension` using the block's arguments from the sheet, fix it, and test again. Error messages name the function and argument (for example `g.circle: argument #3 must be a number`) or the missing function.
