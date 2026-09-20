---
name: image-generation
description: Use when the deliverable is a raster image — an illustration, sprite, texture, photo-like scene, product or UI mockup, hero image, logo exploration, infographic — or when an existing image needs editing (background swap, object removal, text localization, cutout, style transfer, compositing). Covers generating from scratch, generating from reference images, and revising a previous generation. Do NOT use when extending an existing SVG/vector icon or logo system in the repo, or when a simple shape, diagram or wireframe is better written directly as SVG/HTML/CSS.
---

# Image generation

Fleet generates images through `fleet__image` (new image) and `fleet__image_edit`
(revise one you already made). Both call the OpenAI Images API directly.

## Before you call

Answer three questions:

1. **Should this be a bitmap at all?** Extending a repo's existing icon set, a
   logo system, or anything that wants crisp scaling belongs in SVG, written by
   hand. A generated bitmap cannot be edited by the next person. Use this skill
   when the deliverable is genuinely a picture.
2. **Generate or edit?** If the user wants to change parts of an existing image
   while keeping the rest, that is an edit. If they hand you images only as
   style/composition/subject references, that is still a generate.
3. **One asset or several?** Distinct assets need distinct calls with distinct
   prompts. `n` produces variants of *one* prompt — it is not a way to batch
   different pictures.

## What the controls actually do

`model`, `quality` and `size` are **only honoured when an `OPENAI_API_KEY` is
set**. On the ChatGPT plan quota the backend accepts them and then ignores all
three: every quality tier comes back as `low`, sizes are replaced with
dimensions of its own choosing, and even a nonsense model name still returns a
picture (measured 2026-09-20).

The result line always reports what the backend actually used and names any
control it dropped. **Read it.** Do not tell the user you rendered at `max`
quality because you asked for `max`.

When the controls do apply:

- `gpt-image-2.5-flare` (default) is the fast tier; `gpt-image-2.5-sunburst`
  costs the same and renders slower and sharper. Use sunburst for final assets,
  dense text, diagrams and identity-sensitive edits.
- `quality`: `low` for drafts and thumbnails, `high` and up for finals.
  `xhigh` and `max` exist only on the 2.5 models.
- `size`: `auto`, or `WIDTHxHEIGHT` with edges that are multiples of 16px, at
  most 3840px, aspect ratio within 3:1, total pixels 655,360–8,294,400.
  Square renders fastest. 4K is `3840x2160` / `2160x3840`.
- `background: transparent` needs `output_format` `png` or `webp`.

## Writing the prompt

Order the prompt scene → subject → details → constraints. Add detail in
proportion to how specific the request already was: normalise a detailed
request into a clear spec rather than embroidering it, and only enrich a vague
one where it materially helps.

Never invent brand names, slogans, extra characters or narrative the user did
not ask for.

A spec worth reaching for on anything non-trivial:

```
Use case: <product-mockup | ui-mockup | infographic | illustration | logo | photoreal | concept art | …>
Asset type: <where it will be used>
Subject: <the main thing>
Scene/backdrop: <environment>
Style/medium: <photo / flat vector / 3D render / watercolour / …>
Composition: <wide|close|top-down; where the subject sits; negative space for copy>
Lighting/mood: <lighting and feel>
Palette: <colour notes>
Text (verbatim): "<exact string>"
Constraints: <must keep>
Avoid: <no logos, no watermark, no text, …>
```

### Text inside the image

Quote the exact string verbatim and say where it goes and roughly how it should
be set. For unusual words or names, spell them out letter by letter and demand
exact rendering — models mangle text they have to guess at.

### Reference images

`images` takes absolute paths. Models cannot tell what role a picture plays, so
**say it in the prompt**: reference them by position ("image 1 is the style
reference; image 2 is the product that must appear unchanged") and state what
to take from each. Unlabelled references produce unpredictable blends.

## Editing

`fleet__image_edit` takes the `thread_id` from a previous call and resends that
picture as the edit target, so the change builds on it.

**State invariants explicitly, every single round.** "Everything I did not
mention stays the same" is what you *want*, not what the model guarantees —
multi-round edits drift. Write `change only the background to a warm sunset;
keep the product, its edges and the lighting on it unchanged`. Repeat that
clause each round, even when it feels redundant.

Make one targeted change per call and look at the result before the next one.
Several changes at once are hard to attribute when the output goes wrong.

To revise a plain local file Fleet did not generate, attach it via `images` on
`fleet__image` instead — `fleet__image_edit` only knows about handles it issued.

## After the image comes back

Files land under `~/.fleet/generated_images/<handle>/` and stay there. Inspect
the result before reporting: check subject, composition, text accuracy and every
constraint you listed.

- To show the user: pass the path to `fleet__ask`'s `images` field.
- To hand it over as a deliverable: `fleet__artifact`.
- If the project consumes it: copy it into the repo yourself and update
  whatever references it. Never leave a project-referenced asset living only in
  `~/.fleet`.

Report the final path, and say which model and quality the backend actually
used — not the ones you requested.
