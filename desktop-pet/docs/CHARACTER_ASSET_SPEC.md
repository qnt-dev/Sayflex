# Character asset specification

This document is the visual contract for the independent Desktop Pet product.
It is intentionally separate from SayFlex.

## Approved identity

- Small original hacker-alien creature with fox/cat ears, a hood, glossy visor,
  oversized technical jacket, short legs, rounded paws and a luminous tail.
- The personality may carry the playful, unruly energy of a compact alien
  creature, but must not copy the anatomy, face, markings, palette or other
  recognisable design language of an existing franchise character.
- Preserve the current silhouette and proportions unless a prompt explicitly
  asks for a pose change.
- Base palette: near-black and deep violet materials, restrained magenta
  construction accents and electric cyan technical light. The orb remains the
  brightest element in the scene.
- Free-floating square/pixel particles are forbidden around the creature.
  Embedded rectangular lights on clothing are allowed. Free square particles
  belong exclusively to the orb.
- The visor is a clean, empty, glossy surface. Face graphics are rendered at
  runtime and must not be baked into new rig sources.

## Palette-ready materials

Runtime colour selection must not apply a global hue rotation to a finished
PNG. Every approved rig source is processed into semantic masks:

1. `material_base` — jacket and hood material;
2. `material_secondary` — trousers, cuffs, boots and structural dark panels;
3. `accent` — restrained decorative seams;
4. `emissive` — cyan technical lights;
5. `neutral` — visor, outline, highlights and surfaces that must not be tinted.

The extractor, not OpenAI Image, produces the masks from the approved master.
This keeps lighting and volume stable across colour presets. The settings UI
will select a palette made of these semantic tokens; the orb palette is a
separate setting.

## Rig and pose contract

- Canonical canvas: 1024 x 1536 pixels, transparent PNG. If alpha generation
  fails, use one perfectly flat `#202126` background and no shadow.
- One character per file. Never use sprite sheets or multi-pose contact sheets.
- Full body is visible with generous clearance above ears, below feet and on
  both sides. Nothing touches the canvas edge.
- The foot baseline and character scale are identical between views.
- Generate the right-facing view only; left-facing runtime assets are mirrored
  by the deterministic extraction pipeline.
- Limbs must have clean silhouettes. Negative space is preferred; a flattened
  pose is acceptable for modest locomotion only when calibrated overlap masks
  keep every animated frame continuous at runtime.
- A production motion rig needs separate upper arm, forearm, paw, thigh, lower
  leg and foot layers, plus torso, head/hood, ears and tail. Hidden joint areas
  are reconstructed during extraction from approved source poses.
- Free-floating particles, floor, cast shadow, scenery, text, logo and watermark
  are forbidden in rig sources.

The currently approved front and right-profile masters are hash-pinned by the
extractors. The front pose remains an intact sprite. The profile master drives
the modest walk/run cycle; larger traversal mechanics use dedicated key poses
instead of forcing the flattened limbs beyond their safe angular range.

## First vertical slice

The first quality gate is one coherent sequence:

`idle -> notice -> anticipation -> walk -> run -> sprint -> slide stop -> recovery`

Traversal mechanics are then layered onto the same rig:

- vault;
- normal jump;
- flip jump when overhead clearance is safe;
- ledge grab and pull-up;
- orb-assisted lift for ledges above the body reach.

For Orb Lift the character calls the orb, grips it with both hands, hangs under
it while the legs and tail trail with inertia, transfers both hands to the
ledge, pulls up, plants the feet and settles. The orb never teleports the body
and never turns the character into a generic flying sprite.

## Acceptance checklist for every generated image

1. It is unmistakably the same approved character.
2. No free square particles surround the creature.
3. No orb is present unless the prompt is specifically for an orb interaction.
4. The visor has no baked face.
5. View angle, canvas, scale and foot baseline match the request.
6. Required limbs are complete, separated and not fused with the torso.
7. No crop, floor, shadow, scenery, text, watermark or extra character exists.
8. Edges are crisp enough for alpha extraction and do not contain background
   bloom.
