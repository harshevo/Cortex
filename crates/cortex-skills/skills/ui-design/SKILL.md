---
name: ui-design
description: Design or restyle a user interface (web page, component, app screen) so it looks deliberate and works well; use for any task whose result people will look at.
argument-hint: "[what to design or improve]"
---

Build interface work that looks designed, not defaulted, and that holds up in use.

Task: $ARGUMENTS

## 1. Find the system that is already there

Before writing any style, look for what the project uses: a theme or tokens file, CSS variables, a Tailwind config, a component library, existing screens next to the one you are changing. Reuse its colours, type sizes, spacing steps, radii and components. A new screen should be indistinguishable in style from its neighbours. Only when there is nothing to follow do you choose a direction yourself.

## 2. Decide before building

In two or three sentences for yourself: who uses this and what is the one thing they come to do; what the eye should land on first, second, third; what the feel is (dense and precise, calm and spacious, playful). Every later choice follows from this. If you are choosing the direction yourself, commit to one and carry it through rather than averaging several.

## 3. The parts that make it look designed

- Hierarchy: one clear primary element per view. Create contrast with size, weight and colour together, and remove emphasis from the rest. If everything is bold, nothing is.
- Type: one family, two at most. A real scale (for example 12, 14, 16, 20, 28, 40), not arbitrary sizes. Body text 15 to 17 px with line height about 1.5; headings tighter (1.1 to 1.25). Lines of prose 45 to 75 characters. Tabular figures for numbers that line up.
- Spacing: one scale (4, 8, 12, 16, 24, 32, 48, 64). More space between groups than within them; that is what makes grouping visible without boxes. Be generous: cramped is the commonest fault.
- Colour: neutrals carry most of the interface; one accent, used for the primary action and little else. Semantic colours (danger, success, warning) only for their meaning. Tint the neutrals slightly rather than using pure grey, and avoid pure black on pure white. Define colours once, as variables.
- Depth and edges: one consistent radius family; borders or shadows, rarely both; shadows soft and few.
- Alignment: everything sits on a shared edge or grid. Stray one-off offsets read as mistakes.

Avoid the generic look: a purple-blue gradient on white, three identical cards with an icon on top, centred everything, emoji as icons, a shadow on every box. If the result could be any product's page, it is not finished.

## 4. The parts that make it work

- Every interactive element has hover, focus (a visible ring), active, disabled and, where it applies, loading states.
- Every view that shows data has empty, loading and error states with real text.
- Text contrast at least 4.5:1 (3:1 for large text). Targets at least 44 px on touch screens. Keyboard reachable in a sensible order; real `button`, `a`, `label` and heading elements; images with `alt`.
- Layout holds from 360 px wide to a wide desktop: let content wrap, stack columns, and never scroll sideways. Long names, long numbers and missing values must not break it.
- Light and dark, if the project has both: check each.
- Motion is short (150 to 250 ms), eases, serves a purpose, and respects `prefers-reduced-motion`.
- Words: buttons say what they do ("Save changes", not "Submit"); errors say what happened and what to do next.

## 5. Check it by looking

Run the interface if the project can (dev server, storybook, the app itself) and look at the result. With the `browser` tools, open it, read the snapshot, take a screenshot, and go through the widths and states above. If you cannot view it, re-read your markup and styles as a user at a narrow and a wide width would meet them, and say in your report that it was not viewed. Compare against the neighbours from step 1.

Report what you built, the choices you made and why, and what you could not check.
