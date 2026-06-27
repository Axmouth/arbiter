# Calm Infrastructure: design language

The adopted visual language for arbiter's UI (and any sibling tooling). Distilled to the
actionable rules. The guiding feeling is **serious engineering, approachable ergonomics**:
composed, quiet, dense, trustworthy, like a good terminal or editor, never a marketing site.
Confidence comes from restraint.

## Tokens live in CSS variables

All color comes from the variables in `web-ui/src/index.css`. Dark is the default (`:root`);
the warm light palette is under `[data-theme='light']`. Components reference variables only
(`bg-(--bg-...)`, `text-(--text-...)`), never raw hex, so a retune is one file.

## Color

- **Dark (default): graphite, not black.** Steel, slate, machinery. Surfaces step up from the
  app background in small increments. Accent is restrained and cool (dusty indigo / steel
  blue / muted teal).
- **Light: warm materials, not SaaS white.** Paper, linen, sandstone, parchment, clay. Accent
  is a warm family (copper / terracotta / muted amber / olive). The warmth is the identity.
- Accents never dominate. Status colors (success, error, warning, running) are muted, not neon.

## Typography

Slightly smaller than average, crisp, dense, readable. Body ~15px, navigation / buttons /
code ~13-14px. Large headlines are rare. Reward technical readers.

## Layout and density

Moderately dense. Content close together with clear rhythm for fast scanning. Avoid huge
whitespace, hero sections, and oversized marketing blocks. Prefer information over empty space,
but never hide useful information just to look clean.

## Shapes, borders, shadows

- Radius 8-10px. Machined, not inflated. No giant pills, no perfectly square controls.
- **Borders define structure** (1px subtle contrast). Prefer borders over shadows.
- Shadows very sparingly, for depth only, never decoration.

## Buttons

Small, contained, quiet, dependable. Never dominate the page. Hover is subtle brightness or a
border change. No gradients, scaling, bouncing, or large motion.

## Motion

Motion explains, it does not decorate. Good motion shows system behavior (a state changing, a
run progressing, loading, a retry, a topology change). Fast, smooth, purposeful. Avoid floating
cards, big reveals, parallax.

## Dashboard components

A small internal set, everything composes from it: Button, Card, Badge / status chip, Code
block, Metric, Table, Sidebar, Topbar, Dialog, Inspector, Timeline, Node graph. Compact
navigation, dense tables, contained cards, small controls, clear metrics. No oversized charts
(charts only when useful).

## Honesty

Never present unfinished work as complete. Mark surfaces Stable / Experimental / Planned / Not
planned. Honesty builds trust (see IMPLEMENTED_SURFACE.md and FOLLOWUPS.md).

## The feeling

After six hours in the interface the user should feel calm, focused, and confident, never
visually exhausted or distracted. A precision tool.
