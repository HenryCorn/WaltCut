# Keyboard shortcuts

WaltCut has one built-in **WaltCut Default** shortcut map for its English-only interface and
single layout. Users can still customize shortcut assignments; there are no alternate built-in
profiles. The map is applied to WaltCut command ids; command registrations are
the source of truth for labels, shortcuts, parameters, enabled state, and execution.

| Workflow | Shortcut | Action |
|---|---|---|
| Project | `Cmd+O`, `Cmd+S`, `Cmd+Shift+S` | Open, save, save as. |
| References | `Cmd+I` | Import footage, images or audio. |
| Editing | `Cmd+Z`, `Cmd+Shift+Z` | Undo, redo. |
| Editing | `Cmd+K` | Split at the playhead. |
| Editing | `Backspace`, `Shift+Delete` | Clear, ripple delete. |
| Drawing | `Shift+P` | Select the Brush tool in the Program monitor. |
| Preview | `Space`, `Left`, `Right` | Play/stop, previous frame, next frame. |
| Preview | `Shift+Left`, `Shift+Right` | Step several frames. |
| Timeline | `=`, `-`, `\` | Zoom in, zoom out, fit sequence. |
| Layout | `Alt+Shift+1`, `Alt+Shift+0` | Show WaltCut layout, reset to saved layout. |
| Settings | `Cmd+,`, `Cmd+Alt+K` | Preferences, shortcut editor. |

The exact active bindings are discoverable through `filmcraft-cli commands` and command
descriptions. UI commands are also listed in the control channel and MCP command catalog. Keep
shortcut assignments unambiguous within their scope, and update this page when a default used by
the retained drawing workflow changes.

Commands remain available by id even when they have no keyboard shortcut. See
[`agents.md`](agents.md) for the agent interfaces and [`animation.md`](animation.md) for the
retained workflow.
