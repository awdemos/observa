# Observa UI Design System

## Direction

Mission Control: a dense, dark system-operations dashboard. Information is organized by priority and frequency of use, not by visual wow. The atmosphere is near-black, typography is tight and legible, and a single cyan accent marks live state and primary actions. Decorative 3D visuals are removed; motion is reserved for HTMX swaps and hover feedback.

## Tokens

### Palette

- `--bg`: #0a0a0a (page background)
- `--bg-elev`: #111111 (cards, panels)
- `--bg-elev-2`: #181818 (hover states, nested panels)
- `--fg`: #f7f8f8 (primary text)
- `--fg-muted`: #a0a0a0 (secondary text)
- `--fg-dim`: #555555 (tertiary / placeholders)
- `--border`: rgba(255, 255, 255, 0.08) (shadow-as-border)
- `--border-strong`: rgba(255, 255, 255, 0.14)
- `--accent`: #00d9e4 (cyan)
- `--accent-2`: #ff2a8b (reserved for critical/error highlights only)
- `--success`: #00e676
- `--warn`: #ffab00
- `--error`: #ff5252
- `--glow-accent`: 0 0 20px rgba(0, 217, 228, 0.25)

### Typography

- Primary: `Inter`, system-ui, sans-serif.
- Mono: `JetBrains Mono`, monospace for data/codes.
- Headings: 600 weight, tight letter-spacing (-0.02em).
- Display: 2rem, weight 600, letter-spacing -0.03em.
- Body: 0.875rem / 1.5.
- Data/mono: 0.8125rem / 1.4.
- Labels: 0.6875rem, uppercase, letter-spacing 0.06em, `--fg-dim`.

### Spacing

- Base 4px grid.
- Page padding: 16px (mobile) / 24px (desktop).
- Card padding: 16px (compact) / 24px (standard).
- Gap scale: 8, 12, 16, 24, 32.
- Max content width: 1400px, centered.

### Radii

- Cards/panels: 12px.
- Buttons/inputs: 8px.
- Pills/badges: 9999px.

### Elevation

- Panel: `box-shadow: 0 0 0 1px var(--border);`
- Hover: `background: var(--bg-elev-2);` (no transform lift on dense dashboards).
- Active accent: 1px accent border or left accent border.

## Primitives

### Panel

- `background: var(--bg-elev)`
- `border: 1px solid var(--border)`
- `border-radius: 12px`
- `padding: 16px`
- Optional `panel-compact` variant with `padding: 12px`.

### Section header

- `display: flex; justify-content: space-between; align-items: baseline;`
- Title: 1rem, weight 600, `--fg`.
- Action link: 0.8125rem, `--accent`, hover underline.

### Metric tile

- Used in KPI strip.
- Label: uppercase mono, `--fg-dim`.
- Value: 1.5rem, weight 600, `--fg` (accent tint for live metrics).
- Sparkline placeholder area optional.

### Button

- Primary: bg `--accent`, text #0a0a0a, radius 8px, weight 600.
- Secondary: bg transparent, border `--border`, text `--fg-muted`, hover `--fg`.

### Inputs

- bg `--bg-elev-2`, border `--border`, radius 8px, focus ring in `--accent`.

### Data table

- Header: uppercase mono label, `--fg-dim`, no border-bottom on header row.
- Rows: border-top `var(--border)`, hover `--bg-elev-2`.
- Truncate long names; monospace for numeric columns.

### Status badge

- pill shape, small mono text.
- Debug/Info: subtle gray/cyan.
- Warn: amber.
- Error/Critical: red/magenta.

## Layout

### Global

- Sticky top nav: brand, links, status pill.
- `max-width: 1400px; margin: 0 auto;` content container.
- Mobile: single column, nav collapses to hamburger.

### Dashboard (index)

- Top KPI strip: CPU %, memory %, network RX/TX rates, disk usage summary, active process count.
- Below: 2-column grid on desktop, 1 column on mobile.
  - Left/main column (~60%): live trend chart + top processes table.
  - Right/secondary column (~40%): security alerts preview + recent logs preview + chat assistant widget area.

### Metrics page

- Top KPI strip (smaller variant).
- Two-column grid:
  - Left: CPU core usage (compact horizontal bars, 2 columns on desktop).
  - Right: disks + networks as compact lists.
- Below: top processes table (full width).

### Network page

- Remove 3D galaxy.
- Two-column grid of interface cards (name, RX/TX totals, current rates, mini sparkline placeholder).
- Full-width recent traffic table if data exists.

### Logs page

- Filters inline above table.
- Log rows: timestamp, severity badge, unit/message.

### Security page

- Severity summary row + filterable table.

### Chat page

- Centered conversation column (max 720px), messages scroll, sticky input.

## Motion

- Only transform/opacity animations.
- HTMX swaps: fade-in 200ms.
- Button/link hover: 150ms ease-out color/background.
- No decorative motion, no parallax, no ambient rotation.

## Notes

- Decorative Three.js panels are removed from all pages. The vendored library remains available but is not initialized by default.
- Keep JavaScript optional; core navigation works without it.
- Format display strings in Rust context builders.
- Use CSS Grid for page layout, Flexbox inside components.
