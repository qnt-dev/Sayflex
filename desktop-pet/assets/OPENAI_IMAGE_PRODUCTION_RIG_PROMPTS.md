# OpenAI Image prompt pack: production motion rig

Generate the remaining files **sequentially**, not in one request. For each
generation, attach the identity reference named in the step. After a step is
approved, use its output as the primary reference for the next step. This is
more reliable than asking the model for a sprite sheet.

The prompts are written specifically for OpenAI Image. Put approved outputs in
`desktop-pet/assets/source/` using the exact file names below.

## Approved front master — completed

`source/rig_master_front.png` is the approved identity and front rig master.
It is built deterministically from `source/image.png` by
`prepare_front_master.py`; no generative redraw is used for the character.
The accepted image already has visible paws, relaxed external arms and
separated legs, so the former Step 1B generation is no longer required.

`source/front_visor_donor.png` is a tightly cropped source plate used only for
the blank visor glass. It is not a second character identity reference.

The two prompts below are retained as history only. **Do not run them.**

## Archived Step 1A — clean identity plate

Reference image: the original `source/stand.png`, not a previously generated
variation.

Output: `source/rig_master_front_clean.png`

This pass changes no pose. It removes visual elements only.

```text
Edit the supplied image. Keep the character itself in exactly the same front-
facing pose, at exactly the same scale, position, proportions, silhouette,
colours, materials, lighting, outlines and level of detail. Do not redesign or
beautify the character and do not move any body part.

Make ONLY these removals:
1. Remove the floating orb completely.
2. Remove every detached square, pixel, glitch tile and particle outside the
   character silhouette, including those near the ears and tail.
3. Remove the eyes, mouth, cross, stars and every other face graphic from the
   visor. Leave the visor as one clean, blank, dark glossy surface.
4. Remove the background, background glow, bloom, floor and shadow. Everything
   outside the character silhouette must be truly transparent alpha.

Keep embedded cyan and magenta technical details that physically touch the
clothing. Keep the current arms in the pockets and keep the current leg pose in
this pass. Exactly one unchanged character on a 1024 x 1536 transparent PNG.
No orb, no detached particles, no scenery, no text, no watermark and no extra
object. Do not replace transparency with black, grey, gradient or checkerboard.
```

## Archived Step 1B — riggable front pose

Reference image: the approved `source/rig_master_front_clean.png` only.

Output: `source/rig_master_front.png`

```text
Edit the supplied clean character plate. Preserve the exact same character,
front view, canvas, scale, position, head, ears, hood, blank visor, jacket,
trousers, tail, palette, lighting, outlines and transparent background. Change
ONLY the arms, the jacket areas beneath them and the spacing of the legs.

Remove both paws from the jacket pockets. Show two complete relaxed arms beside
the torso, each slightly away from the jacket, with gently bent elbows and both
rounded paws fully visible. Leave a continuous transparent gap around each arm
from upper sleeve to paw, except for the natural shoulder connection. The arms
must be easy to cut into independent animation layers. Reconstruct both empty
pockets and the complete jacket body beneath the removed arms.

Separate the legs slightly while keeping both feet on the same baseline. Both
complete legs and feet must have distinct silhouettes with a narrow transparent
gap between them. Do not change their length or the character's height.

Do not change anything else. Do not add an orb, face, free particles, glow,
floor, shadow, scenery, text or watermark. Everything outside the character
silhouette remains truly transparent alpha. Exactly one character on the same
1024 x 1536 canvas.
```

## Approved right-profile master — completed

`source/rig_master_profile_right.png` is the approved right-facing locomotion
master. `extract_profile_motion.py` validates its exact SHA-256, removes only
detached low-alpha backdrop residue, and prepares mirrored runtime layers.

The Step 2 prompt below is retained as history only. **Do not run it.** The next
OpenAI Image request starts at Step 3 and uses the approved profile master.

## Archived Step 2 — clean right-profile rig master

References, in priority order:

1. approved `source/rig_master_front.png` — character identity;
2. `source/profile_arms_right.png` — strict side-view pose only.

Output: `source/rig_master_profile_right.png`

```text
Using the approved front rig master as the primary identity reference, create
the exact same character in a strict orthographic 90-degree side profile facing
right. Use the secondary side image only to understand the viewing angle; do
not copy its background or any identity drift.

Preserve the approved character's exact proportions, silhouette language,
large cybernetic ears, hood, blank glossy visor, oversized jacket, trousers,
rounded paws, luminous tail, line weight, cel shading, deep-violet and near-
black materials, restrained magenta accents and electric-cyan technical lights.

This is a source for a segmented premium 2D motion rig. Show both complete arms:
the far arm behind the torso and slightly forward, the near arm in front and
slightly backward. Give both elbows a gentle natural bend. Leave an unbroken
background gap around every forearm and paw. Show both complete legs with the
far leg slightly backward and the near leg slightly forward. Leave background
gaps between thighs, lower legs and feet. Keep shoulder, elbow, hip, knee and
ankle locations anatomically clear. Reconstruct a complete torso beneath the
limbs. The tail must be fully visible and separated from jacket and legs.

No orb. No free-floating square particles, pixel blocks, glitch tiles or halo
around the creature. Embedded clothing lights are allowed. The visor contains
no face or symbols.

Exactly one full-body character, 1024 x 1536 transparent canvas, same character
height and foot baseline as the front master. No floor, shadow, scenery, text,
logo, watermark or extra object. Do not create a three-quarter view, sprite
sheet, T-pose, action pose or multiple variations.
```

## Step 3 — ledge-grab key pose

Primary reference: approved `source/rig_master_profile_right.png`

Output: `source/pose_ledge_grab_profile_right.png`

```text
Edit the supplied approved right-profile rig master into one clean ledge-grab
key pose. Preserve the exact character, strict right-facing profile, scale,
palette, materials, outline and rendering style. Change only the pose.

The character is hanging from an invisible horizontal ledge above its head.
Both complete arms reach upward, both elbows slightly bent under load, and both
paws grip the same horizontal contact level. The shoulders are raised naturally
without changing the jacket design. The torso hangs below the hands with a
subtle stretch. Both legs trail downward with soft knee bends and clear negative
space between them. Feet are relaxed. The tail hangs lower and lags behind the
body. The pose must show believable weight, not a floating standing pose.

Do not draw the ledge or any scenery; the real desktop window edge supplies the
contact surface at runtime. Keep all arms, paws, legs and feet complete and
separable. Blank glossy visor. No orb. No free-floating square particles around
the creature. Exactly one character on a 1024 x 1536 transparent canvas. No
floor, shadow, text, logo, watermark or additional objects.
```

## Step 4 — Orb Lift hanging key pose

Primary reference: approved `source/rig_master_profile_right.png`

Secondary reference: current `orb.png` for the orb identity.

Output: `source/pose_orb_lift_profile_right.png`

```text
Edit the supplied approved right-profile character rig into one Orb Lift key
pose. Preserve the exact same original character, strict right-facing profile,
scale, palette, materials, outline and rendering style. Preserve the supplied
orb design exactly.

Place the orb above and slightly in front of the character's head. The character
hangs beneath it and grips the lower sides of the orb securely with both paws.
Both complete arms reach upward with slightly bent elbows and visibly carry the
body weight. The torso stretches subtly below the shoulders. Both knees bend
backward from upward acceleration, the feet trail below, and the tail streams
downward with delayed inertia. The pose communicates that the orb is actively
lifting the character, not that the character can fly.

Only the orb may have a small cluster of free-floating square/pixel particles.
There must be no square particles around the creature, ears, jacket, legs or
tail. Keep embedded clothing lights. The visor remains blank and glossy.

Exactly one character and one orb on a 1024 x 1536 transparent canvas. Keep all
limbs complete and visually separable. No floor, ledge, scenery, cast shadow,
text, logo, watermark, motion blur or extra character.
```

## Step 5 — pull-up compression key pose

Primary reference: approved `source/rig_master_profile_right.png`

Output: `source/pose_pull_up_profile_right.png`

```text
Edit the approved strict right-profile character into the compression phase of
a ledge pull-up. Preserve the exact character identity, profile direction,
scale, palette, material detail, line weight and cel shading.

Both paws remain fixed at one invisible horizontal contact line near chest
height. The elbows bend deeply as the character pulls the chest above that
line. The shoulders and torso compress naturally, one knee rises toward the
ledge, the other leg trails below, and the tail counterbalances behind. The
silhouette must clearly communicate effort, weight transfer and preparation to
plant a foot. Keep both arms, paws, legs and feet complete and separable.

Do not draw the ledge. Blank glossy visor. No orb. No free-floating square
particles around the creature. Exactly one full character on a 1024 x 1536
transparent canvas. No floor, shadow, scenery, text, logo or watermark.
```

## Generation order and stop rule

Steps 1 and 2 are approved and must not be regenerated. Generate Step 3 next.
Reject and regenerate any later pose if identity changes, if free particles
remain around the creature, if the visor contains a face, or if any required
limb is cropped. Use `source/rig_master_profile_right.png` as the profile
identity source for all later profile poses.
