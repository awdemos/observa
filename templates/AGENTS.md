# observa/templates

## OVERVIEW

Tera HTML templates and HTMX partials for the dashboard. These templates define the dark, terminal-inspired UI and are rendered by `observa-server` at runtime.

## WHERE TO LOOK

| Task | Location | Notes |
|---|---|---|
| Layout | `/var/home/a/code/observa/templates/base.html` | Top-level page wrapper and shared includes |
| Dashboard home | `/var/home/a/code/observa/templates/index.html` | Main dashboard view |
| Chat page | `/var/home/a/code/observa/templates/chat.html` | Full chat interface |
| Metrics page | `/var/home/a/code/observa/templates/metrics.html` | Metric tables and charts |
| Logs page | `/var/home/a/code/observa/templates/logs.html` | Log tail view |
| Security page | `/var/home/a/code/observa/templates/security.html` | Security alert table |
| AI servers page | `/var/home/a/code/observa/templates/ai_servers.html` | Discovered AI endpoint list |
| HTMX partials | `/var/home/a/code/observa/templates/partials/` | Fragments swapped in by HTMX requests |

## COMMANDS

There are no crate-specific commands here. Validate changes through the workspace server:

```bash
cargo test -p observa-server
cargo clippy -p observa-server -- -D warnings
```

## NOTES

- Templates extend `base.html` and reuse its blocks.
- HTMX partials should be small and self-contained; they are swapped into existing DOM elements.
- Keep CSS classes aligned with the tokens defined in `/var/home/a/code/observa/assets/css/observa.css`.
