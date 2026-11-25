# Website and docs

- The site in this directory is published to `getdux.app`; when a change affects anything it describes or shows, update it in the same change.
- Copy is playful, headings never count features, keybindings are never enumerated, and every value quoted matches what the app does.
- No half-width prose unless something sits in the other half: shorten the copy or add a real second column.
- In `.astro` templates write the space between a closing inline tag and the next word as `{" "}`; Astro removes the line break there and the build does not warn.
- The docs at `getdux.app/docs` are generated from the Markdown in `docs/`; a new page is a `.md` file with `title`, `description`, `group` and `order` frontmatter.
- The docs describe what a user sees, configures and does: every setting, default, limit, command, refusal, platform caveat and data-loss warning.
- HTTP routes, sockets, wire frames, engine and worker mechanics, storage layout beyond "your state lives here", and design rationale stay out of the docs.
- Admonitions box only what a skimmer must not miss: WARNING or CAUTION for security and loss, IMPORTANT for must-know behavior, TIP for a better way, NOTE for asides.
- Docs and screenshots show the TUI's default glyphs (`theme::Glyphs::PLAIN`: `※`, `✷`, `↳`), never Nerd Font ones; a page that names an icon shows a screenshot of it in context nearby.
- When the default glyph set changes, the docs text and every affected screenshot change with it.
- A stale screenshot under `public/screens/` is regenerated with `tools/preview-env/screens/reshoot.sh`, with no argument for all or the screenshot's name for one.
